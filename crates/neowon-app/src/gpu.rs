//! Digital-phosphor GPU pipeline (Bevy 0.19), following the compute-plugin
//! pattern from GoL's physarum tool: pipelines at `RenderStartup`, buffers in
//! `PrepareResources`, bind groups in `PrepareBindGroups`, dispatch as a
//! system in the `RenderGraph` schedule before the camera driver.

use bevy::{
    core_pipeline::schedule::camera_driver,
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{
                storage_buffer_read_only_sized, storage_buffer_sized, texture_storage_2d,
                uniform_buffer,
            },
            *,
        },
        renderer::{RenderContext, RenderDevice, RenderQueue},
        texture::GpuImage,
    },
};
use neowon_core::SharedFrame;
use std::borrow::Cow;

/// Plot area in pixels: 20 x 10 divisions at 50 px/div.
pub const PLOT_W: u32 = 1000;
pub const PLOT_H: u32 = 500;
const MAX_SAMPLES: usize = 5000;
/// Trace layers: CH1, CH2, math.
pub const CHANNELS: usize = 3;

/// Trace display style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceMode {
    Vectors,
    Dots,
    Xy,
}

/// Display colormap for the composed trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    /// Per-channel phosphor colors (default).
    Phosphor,
    /// Intensity-graded thermal map (DPO-style, all channels combined).
    Thermal,
    /// Monochrome green CRT.
    Green,
}

/// Phosphor persistence setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Persistence {
    Off,
    Seconds(f32),
    Infinite,
}

impl Persistence {
    pub const LADDER: [Persistence; 5] = [
        Persistence::Off,
        Persistence::Seconds(0.2),
        Persistence::Seconds(1.0),
        Persistence::Seconds(5.0),
        Persistence::Infinite,
    ];

    pub fn label(&self) -> String {
        match self {
            Persistence::Off => "off".into(),
            Persistence::Seconds(s) => format!("{s}s"),
            Persistence::Infinite => "inf".into(),
        }
    }
}

/// Everything the render world needs, extracted once per render frame.
#[derive(Resource, Clone, ExtractResource)]
pub struct Phosphor {
    pub display_image: Handle<Image>,
    pub frame: Option<SharedFrame>,
    pub mode: TraceMode,
    pub persistence: Persistence,
    /// Decay factor for this render frame (computed main-side from dt).
    pub decay: f32,
    pub gain: f32,
    /// CRT styling (phosphor halo, scanlines, vignette) in the compose pass.
    pub crt: bool,
    pub palette: Palette,
    /// One-shot: a new record arrived since the last render frame.
    pub new_frame: bool,
    /// Horizontal zoom window into the record: (center, span) as fractions
    /// of the record. (0.5, 1.0) shows the whole record — the home view.
    pub hview: (f64, f64),
    /// When present, the display shows this reduced timeline instead of a
    /// single record. Arc'd because the whole resource is cloned into the
    /// render world every frame.
    pub deep: Option<std::sync::Arc<DeepFrame>>,
}

/// A timeline reduced for display: min/max pairs per column per channel,
/// plus which columns nothing was acquired in.
#[derive(Debug, Clone, Default)]
pub struct DeepFrame {
    /// Bumped whenever the content changes — the uploader dedupes on it,
    /// since a rebuilt trace has no record sequence number of its own.
    pub rev: u64,
    pub columns: usize,
    /// Per channel, `2 * columns` interleaved (min, max) values.
    pub pairs: [Vec<i8>; CHANNELS],
    pub enabled: [bool; CHANNELS],
    /// 1 = this column had no acquisition, for the gap markers.
    pub gaps: Vec<u32>,
}

impl Default for Phosphor {
    fn default() -> Self {
        Self {
            display_image: Handle::default(),
            frame: None,
            mode: TraceMode::Vectors,
            persistence: Persistence::Seconds(0.2),
            decay: 1.0,
            gain: 0.8,
            crt: true,
            palette: Palette::Phosphor,
            new_frame: false,
            hview: (0.5, 1.0),
            deep: None,
        }
    }
}

pub struct PhosphorPlugin;

impl Plugin for PhosphorPlugin {
    fn build(&self, app: &mut App) {
        // Embedded so the binary is self-contained (no assets/ dir needed).
        bevy::asset::embedded_asset!(app, "shaders/waveform.wgsl");
        app.add_plugins(ExtractResourcePlugin::<Phosphor>::default());
        let render_app = app.sub_app_mut(RenderApp);
        render_app
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                prepare_buffers.in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Render,
                prepare_bind_groups.in_set(RenderSystems::PrepareBindGroups),
            )
            .add_systems(RenderGraph, dispatch.before(camera_driver));
    }
}

/// Field order must match the WGSL `Params` struct.
#[derive(Clone, Copy, Default, ShaderType)]
struct Params {
    width: u32,
    height: u32,
    samples: u32,
    mode: u32,
    decay: f32,
    gain: f32,
    en0: u32,
    en1: u32,
    en2: u32,
    crt: u32,
    palette: u32,
    /// Horizontal zoom window: first visible sample fraction, window width
    /// as a fraction of the record (1.0 = whole record).
    view_start: f32,
    view_span: f32,
    /// 1 = `wave` holds a reduced timeline: min/max pairs indexed two per
    /// column, with a gap mask packed after the channel blocks.
    deep: u32,
    _pad: u32,
    col0: Vec4,
    col1: Vec4,
    col2: Vec4,
}

#[derive(Resource)]
pub(crate) struct Pipelines {
    layout: BindGroupLayoutDescriptor,
    decay: CachedComputePipelineId,
    raster: CachedComputePipelineId,
    compose: CachedComputePipelineId,
}

#[derive(Resource)]
pub(crate) struct Buffers {
    pub(crate) wave: Buffer,
    accum: Buffer,
    params: UniformBuffer<Params>,
    /// Sequence number of the record currently in `wave`.
    uploaded_seq: u64,
    /// Revision of the deep trace currently in `wave` (0 = none).
    uploaded_deep: u64,
    /// Sample count of that record.
    pub(crate) n_samples: u32,
    /// Raster requested for this render frame.
    do_raster: bool,
    /// Last applied zoom window — a change clears the phosphor.
    hview: (f64, f64),
}

#[derive(Resource)]
pub(crate) struct PhosphorBindGroup(BindGroup);

fn init_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pipeline_cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "phosphor",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<Params>(false),
                storage_buffer_read_only_sized(false, None), // wave samples
                storage_buffer_sized(false, None),           // accum (atomic)
                texture_storage_2d(TextureFormat::Rgba8Unorm, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    let shader = bevy::asset::load_embedded_asset!(&*asset_server, "shaders/waveform.wgsl");
    let pipeline = |entry: &'static str| {
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            layout: vec![layout.clone()],
            shader: shader.clone(),
            entry_point: Some(Cow::from(entry)),
            ..default()
        })
    };
    let decay = pipeline("decay");
    let raster = pipeline("raster");
    let compose = pipeline("compose");
    commands.insert_resource(Pipelines {
        layout,
        decay,
        raster,
        compose,
    });
}

fn prepare_buffers(
    mut commands: Commands,
    buffers: Option<ResMut<Buffers>>,
    phosphor: Res<Phosphor>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let mut b = match buffers {
        Some(b) => b,
        None => {
            let wave = device.create_buffer(&BufferDescriptor {
                label: Some("phosphor wave"),
                size: (CHANNELS * MAX_SAMPLES * 4) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let accum = device.create_buffer(&BufferDescriptor {
                label: Some("phosphor accum"),
                size: (CHANNELS as u32 * PLOT_W * PLOT_H * 4) as u64,
                usage: BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            commands.insert_resource(Buffers {
                wave,
                accum,
                params: UniformBuffer::default(),
                uploaded_seq: 0,
                uploaded_deep: 0,
                n_samples: 0,
                do_raster: false,
                hview: phosphor.hview,
            });
            return; // bind next frame
        }
    };

    let mut en = [0u32; CHANNELS];
    b.do_raster = false;
    let deep = phosphor.deep.as_ref();
    if let Some(d) = deep {
        // A reduced timeline replaces the single record: channel blocks of
        // min/max pairs, then a per-column gap mask packed after them (the
        // wave buffer has room — deep uses 3 x 2000 of 15000 i32).
        for (ch, on) in d.enabled.iter().enumerate() {
            en[ch] = *on as u32;
        }
        if d.rev != b.uploaded_deep {
            let n = (d.columns * 2).min(MAX_SAMPLES);
            let mut data = vec![0i32; CHANNELS * n + d.columns];
            for ch in 0..CHANNELS {
                for (i, &v) in d.pairs[ch].iter().take(n).enumerate() {
                    data[ch * n + i] = v as i32;
                }
            }
            for (c, slot) in data[CHANNELS * n..].iter_mut().enumerate() {
                *slot = d.gaps.contains(&(c as u32)) as i32;
            }
            queue.write_buffer(&b.wave, 0, bytemuck::cast_slice(&data));
            b.uploaded_deep = d.rev;
            b.n_samples = n as u32;
            b.do_raster = true;
        }
    } else if let Some(frame) = &phosphor.frame {
        for cap in &frame.channels {
            if cap.ch < CHANNELS {
                en[cap.ch] = 1;
            }
        }
        if phosphor.new_frame && frame.seq != b.uploaded_seq {
            // Records can be shorter than MAX_SAMPLES (WAV playback emits
            // exactly the new audio each tick); pack channels contiguously
            // at the actual length and tell the shader.
            let n = frame
                .channels
                .iter()
                .filter(|c| c.ch < CHANNELS)
                .map(|c| c.raw.len().min(MAX_SAMPLES))
                .max()
                .unwrap_or(0);
            let mut data = vec![0i32; CHANNELS * n.max(1)];
            for cap in &frame.channels {
                if cap.ch >= CHANNELS {
                    continue;
                }
                for (i, &r) in cap.raw.iter().take(n).enumerate() {
                    data[cap.ch * n + i] = r as i32;
                }
            }
            queue.write_buffer(&b.wave, 0, bytemuck::cast_slice(&data));
            b.uploaded_seq = frame.seq;
            b.n_samples = n as u32;
            b.do_raster = true;
        }
    }

    // "Off" persistence: a fresh record replaces the screen outright. A
    // zoom-window change does the same — old phosphor at the wrong zoom
    // would ghost into the new view.
    let mut decay = if b.do_raster && phosphor.persistence == Persistence::Off {
        0.0
    } else {
        phosphor.decay
    };
    // A rebuilt timeline replaces the whole screen; old phosphor from the
    // previous window would smear across it.
    if deep.is_some() {
        decay = 0.0;
    }
    if (phosphor.hview.0 - b.hview.0).abs() > 1e-9 || (phosphor.hview.1 - b.hview.1).abs() > 1e-9 {
        decay = 0.0;
        b.hview = phosphor.hview;
    }
    let (center, span) = phosphor.hview;
    let span = span.clamp(0.01, 1.0);
    let center = center.clamp(span / 2.0, 1.0 - span / 2.0);
    let n_samples = b.n_samples.max(2);
    b.params.set(Params {
        width: PLOT_W,
        height: PLOT_H,
        samples: n_samples,
        mode: match phosphor.mode {
            TraceMode::Vectors => 0,
            TraceMode::Dots => 1,
            TraceMode::Xy => 2,
        },
        decay,
        gain: phosphor.gain,
        en0: en[0],
        en1: en[1],
        en2: en[2],
        crt: phosphor.crt as u32,
        palette: match phosphor.palette {
            Palette::Phosphor => 0,
            Palette::Thermal => 1,
            Palette::Green => 2,
        },
        view_start: (center - span / 2.0) as f32,
        view_span: span as f32,
        deep: deep.is_some() as u32,
        _pad: 0,
        col0: Vec4::new(1.0, 0.85, 0.1, 1.0),
        col1: Vec4::new(0.2, 0.75, 1.0, 1.0),
        col2: Vec4::new(1.0, 0.35, 0.85, 1.0),
    });
    b.params.write_buffer(&device, &queue);
}

#[allow(clippy::too_many_arguments)]
fn prepare_bind_groups(
    mut commands: Commands,
    pipelines: Res<Pipelines>,
    buffers: Option<Res<Buffers>>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    phosphor: Res<Phosphor>,
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    mut logged: Local<bool>,
) {
    let Some(b) = buffers else { return };
    let Some(params) = b.params.binding() else {
        if !*logged {
            tracing::warn!("phosphor: params buffer not written yet");
            *logged = true;
        }
        return;
    };
    let Some(display) = gpu_images.get(&phosphor.display_image) else {
        if !*logged {
            tracing::warn!("phosphor: display GpuImage not available yet");
            *logged = true;
        }
        return; // image not extracted yet; retry next frame
    };
    let layout = pipeline_cache.get_bind_group_layout(&pipelines.layout);
    let bind = device.create_bind_group(
        None,
        &layout,
        &BindGroupEntries::sequential((
            params,
            b.wave.as_entire_binding(),
            b.accum.as_entire_binding(),
            &display.texture_view,
        )),
    );
    commands.insert_resource(PhosphorBindGroup(bind));
}

pub(crate) fn dispatch(
    mut render_context: RenderContext,
    pipelines: Res<Pipelines>,
    pipeline_cache: Res<PipelineCache>,
    bind_group: Option<Res<PhosphorBindGroup>>,
    buffers: Option<Res<Buffers>>,
    mut logged: (Local<bool>, Local<bool>, Local<bool>),
) {
    let (Some(bg), Some(b)) = (bind_group, buffers) else {
        if !*logged.0 {
            tracing::warn!("phosphor: no bind group yet");
            *logged.0 = true;
        }
        return;
    };
    let get = |id| pipeline_cache.get_compute_pipeline(id);
    let (Some(decay), Some(raster), Some(compose)) = (
        get(pipelines.decay),
        get(pipelines.raster),
        get(pipelines.compose),
    ) else {
        if !*logged.1 {
            tracing::warn!("phosphor: pipelines still compiling");
            *logged.1 = true;
        }
        return; // shaders still compiling
    };
    if !*logged.2 {
        tracing::info!("phosphor: dispatching");
        *logged.2 = true;
    }

    let encoder = render_context.command_encoder();
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
    pass.set_bind_group(0, &bg.0, &[]);
    let field = (PLOT_W.div_ceil(16), PLOT_H.div_ceil(16));
    pass.set_pipeline(decay);
    pass.dispatch_workgroups(field.0, field.1, 1);
    if b.do_raster && b.n_samples > 1 {
        pass.set_pipeline(raster);
        pass.dispatch_workgroups((MAX_SAMPLES as u32).div_ceil(256), CHANNELS as u32, 1);
    }
    pass.set_pipeline(compose);
    pass.dispatch_workgroups(field.0, field.1, 1);
}
