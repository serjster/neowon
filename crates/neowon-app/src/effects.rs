//! User display shaders: a final full-screen compute pass over the
//! composed display texture. WGSL files live in `assets/shaders/user/`
//! (or `$NEOWON_SHADER_DIR`) and are loaded at runtime — drop a file in,
//! pick it in the Display section (or `effect <name>`), hit Reload to
//! iterate. A broken shader logs a pipeline error and the effect simply
//! never activates; the app keeps running.
//!
//! Shader contract (see the shipped examples for living documentation):
//!
//! ```wgsl
//! struct EffectParams { width: u32, height: u32, time: f32, samples: u32 }
//! @group(0) @binding(0) var<uniform> params: EffectParams;
//! @group(0) @binding(1) var<storage, read> wave: array<i32>; // 3ch × samples, ±125
//! @group(0) @binding(2) var src: texture_storage_2d<rgba8unorm, read>;
//! @group(0) @binding(3) var dst: texture_storage_2d<rgba8unorm, write>;
//! @compute @workgroup_size(16, 16)
//! fn effect(@builtin(global_invocation_id) gid: vec3<u32>) { … }
//! ```

use bevy::{
    core_pipeline::schedule::camera_driver,
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{storage_buffer_read_only_sized, texture_storage_2d, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice, RenderQueue},
        texture::GpuImage,
    },
};
use std::borrow::Cow;

use crate::gpu::{PLOT_H, PLOT_W, Phosphor};

/// Main-world state, extracted to the render world every frame.
#[derive(Resource, Clone, ExtractResource, Default)]
pub struct Effects {
    /// Active effect name (a file stem from the shader dir).
    pub active: Option<String>,
    /// Shader asset for the active effect.
    pub shader: Option<Handle<Shader>>,
    /// The effect writes here; the plot sprite (and `shot`) show it while
    /// an effect is active.
    pub output: Handle<Image>,
    /// Discovered effect names.
    pub available: Vec<String>,
    /// Seconds since startup (uniform for animated effects).
    pub time: f32,
    /// Bumped on every (re)load so the render world re-queues the pipeline.
    pub epoch: u32,
}

/// Shader search path: `$NEOWON_SHADER_DIR`, else `assets/shaders/user`
/// relative to the working directory, else beside the executable.
///
/// The last step is what makes a released build work: the archive puts the
/// shaders next to the binary, and a binary launched from a file manager or by
/// absolute path has a working directory somewhere else entirely. Without it
/// the effect list is silently empty — `scan` cannot tell "no shaders here"
/// from "wrong directory".
pub fn shader_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("NEOWON_SHADER_DIR") {
        return dir.into();
    }
    let relative = std::path::PathBuf::from("assets/shaders/user");
    if relative.is_dir() {
        return relative;
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let beside = dir.join("assets/shaders/user");
        if beside.is_dir() {
            return beside;
        }
    }
    relative
}

/// Scan the shader dir for `*.wgsl` files.
pub fn scan(fx: &mut Effects) {
    fx.available.clear();
    let dir = shader_dir();
    match std::fs::read_dir(&dir) {
        // An empty effect list otherwise looks like a deliberate build rather
        // than a directory we failed to find.
        Err(e) => warn_once!("effects: no shaders in {}: {e}", dir.display()),
        Ok(entries) => {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "wgsl")
                    && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
                {
                    fx.available.push(stem.to_string());
                }
            }
        }
    }
    fx.available.sort();
}

/// Load (or reload) an effect by name; `None` deactivates.
pub fn activate(fx: &mut Effects, shaders: &mut Assets<Shader>, name: Option<&str>) {
    match name {
        None => {
            fx.active = None;
            fx.shader = None;
        }
        Some(name) => {
            let path = shader_dir().join(format!("{name}.wgsl"));
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    fx.shader =
                        Some(shaders.add(Shader::from_wgsl(source, path.display().to_string())));
                    fx.active = Some(name.to_string());
                    fx.epoch += 1;
                    info!("effects: loaded {}", path.display());
                }
                Err(e) => {
                    error!("effects: cannot read {}: {e}", path.display());
                }
            }
        }
    }
}

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractResourcePlugin::<Effects>::default());
        app.add_systems(Startup, setup);
        app.add_systems(Update, tick);
        let render_app = app.sub_app_mut(RenderApp);
        render_app
            .add_systems(RenderStartup, init_layout)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
            .add_systems(
                Render,
                prepare_bind_group.in_set(RenderSystems::PrepareBindGroups),
            )
            .add_systems(
                RenderGraph,
                dispatch.after(crate::gpu::dispatch).before(camera_driver),
            );
    }
}

fn setup(mut fx: ResMut<Effects>, mut images: ResMut<Assets<Image>>) {
    use bevy::asset::RenderAssetUsages;
    let mut image = Image::new(
        Extent3d {
            width: PLOT_W,
            height: PLOT_H,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0; (PLOT_W * PLOT_H * 4) as usize],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
        | TextureUsages::STORAGE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::COPY_SRC;
    fx.output = images.add(image);
    scan(&mut fx);
}

/// Advance the time uniform and swap the plot sprite between the display
/// texture and the effect output.
fn tick(
    time: Res<Time>,
    mut fx: ResMut<Effects>,
    phosphor: Res<Phosphor>,
    mut sprites: Query<&mut Sprite, With<crate::PlotSprite>>,
) {
    fx.time = time.elapsed_secs();
    for mut sprite in &mut sprites {
        let want = if fx.active.is_some() {
            &fx.output
        } else {
            &phosphor.display_image
        };
        if &sprite.image != want {
            let size = sprite.custom_size;
            sprite.image = want.clone();
            sprite.custom_size = size;
        }
    }
}

// ------------------------------------------------------------- render world

#[derive(Clone, Copy, Default, ShaderType)]
struct EffectParams {
    width: u32,
    height: u32,
    time: f32,
    samples: u32,
}

#[derive(Resource)]
struct EffectPipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: Option<CachedComputePipelineId>,
    epoch: u32,
    params: UniformBuffer<EffectParams>,
}

#[derive(Resource)]
struct EffectBindGroup(BindGroup);

fn init_layout(mut commands: Commands) {
    let layout = BindGroupLayoutDescriptor::new(
        "user-effect",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<EffectParams>(false),
                storage_buffer_read_only_sized(false, None), // wave samples
                texture_storage_2d(TextureFormat::Rgba8Unorm, StorageTextureAccess::ReadOnly),
                texture_storage_2d(TextureFormat::Rgba8Unorm, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    commands.insert_resource(EffectPipeline {
        layout,
        pipeline: None,
        epoch: 0,
        params: UniformBuffer::default(),
    });
}

fn prepare(
    mut pipe: ResMut<EffectPipeline>,
    fx: Res<Effects>,
    buffers: Option<Res<crate::gpu::Buffers>>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    // (Re)queue the compute pipeline when the shader changed.
    if fx.epoch != pipe.epoch {
        pipe.epoch = fx.epoch;
        pipe.pipeline = fx.shader.clone().map(|shader| {
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                layout: vec![pipe.layout.clone()],
                shader,
                entry_point: Some(Cow::from("effect")),
                ..default()
            })
        });
    }
    let samples = buffers.map_or(2, |b| b.n_samples.max(2));
    pipe.params.set(EffectParams {
        width: PLOT_W,
        height: PLOT_H,
        time: fx.time,
        samples,
    });
    pipe.params.write_buffer(&device, &queue);
}

#[allow(clippy::too_many_arguments)]
fn prepare_bind_group(
    mut commands: Commands,
    pipe: Res<EffectPipeline>,
    fx: Res<Effects>,
    phosphor: Res<Phosphor>,
    buffers: Option<Res<crate::gpu::Buffers>>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
) {
    if fx.active.is_none() {
        return;
    }
    let (Some(params), Some(b)) = (pipe.params.binding(), buffers) else {
        return;
    };
    let (Some(src), Some(dst)) = (
        gpu_images.get(&phosphor.display_image),
        gpu_images.get(&fx.output),
    ) else {
        return;
    };
    let layout = pipeline_cache.get_bind_group_layout(&pipe.layout);
    let bind = device.create_bind_group(
        None,
        &layout,
        &BindGroupEntries::sequential((
            params,
            b.wave.as_entire_binding(),
            &src.texture_view,
            &dst.texture_view,
        )),
    );
    commands.insert_resource(EffectBindGroup(bind));
}

fn dispatch(
    mut render_context: RenderContext,
    pipe: Res<EffectPipeline>,
    fx: Res<Effects>,
    pipeline_cache: Res<PipelineCache>,
    bind_group: Option<Res<EffectBindGroup>>,
) {
    if fx.active.is_none() {
        return;
    }
    let (Some(id), Some(bg)) = (pipe.pipeline, bind_group) else {
        return;
    };
    let Some(pipeline) = pipeline_cache.get_compute_pipeline(id) else {
        return; // still compiling (or failed — the cache logs the error)
    };
    let encoder = render_context.command_encoder();
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
    pass.set_bind_group(0, &bg.0, &[]);
    pass.set_pipeline(pipeline);
    pass.dispatch_workgroups(PLOT_W.div_ceil(16), PLOT_H.div_ceil(16), 1);
}
