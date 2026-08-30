//! 3D signal viewport: an offscreen `Camera3d` orbiting immediate-mode
//! line art regenerated from the live signal every record — spectrogram
//! wireframe terrain, a waveform tunnel, a delay-embedding phase portrait,
//! and the XY-vs-time cube. Drawn with a dedicated gizmo group on render
//! layer 1 so it exists only in this viewport, rendered to a texture shown
//! in an egui window. (Gizmos are built for per-frame geometry; mutating
//! Mesh assets every frame trips Bevy's mesh slab allocator.)

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use std::collections::VecDeque;

use super::waterfall::thermal;
use crate::Link;
use crate::derived::FftState;

/// Offscreen render-target size.
pub const RT_W: u32 = 768;
pub const RT_H: u32 = 512;

/// Spectral history kept for the terrain (bins × rows).
const TERRAIN_W: usize = 96;
const TERRAIN_D: usize = 96;
/// Waveform history kept for tunnel / xytime (records × points).
const RINGS: usize = 48;
const RING_PTS: usize = 96;

/// Gizmo group drawn only by the viz camera (render layer 1).
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct VizGizmos;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Viz3d {
    #[default]
    Off,
    Terrain,
    Tunnel,
    Phase,
    XyTime,
}

impl Viz3d {
    pub const ALL: [Viz3d; 5] = [
        Viz3d::Off,
        Viz3d::Terrain,
        Viz3d::Tunnel,
        Viz3d::Phase,
        Viz3d::XyTime,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Viz3d::Off => "off",
            Viz3d::Terrain => "terrain",
            Viz3d::Tunnel => "tunnel",
            Viz3d::Phase => "phase",
            Viz3d::XyTime => "xytime",
        }
    }

    pub fn parse(s: &str) -> Option<Viz3d> {
        Viz3d::ALL.into_iter().find(|m| m.name() == s)
    }
}

#[derive(Resource, Default)]
pub struct Viz3dState {
    pub mode: Viz3d,
    pub image: Handle<Image>,
    /// Orbit camera: yaw/pitch in radians, distance in world units.
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    /// Spectral rows (terrain) and waveform rings (tunnel/xytime).
    heights: VecDeque<Vec<f32>>,
    rings: [VecDeque<Vec<f32>>; 2],
    last_seq: u64,
}

/// Line segments with one color each — pure data, unit-testable.
pub struct Segments {
    pub lines: Vec<([f32; 3], [f32; 3])>,
    pub colors: Vec<[f32; 4]>,
}

impl Segments {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            colors: Vec::new(),
        }
    }

    fn push(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 4]) {
        self.lines.push((a, b));
        self.colors.push(c);
    }
}

fn height_color(h: f32, alpha: f32) -> [f32; 4] {
    let c = thermal(h);
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        alpha,
    ]
}

/// Spectrogram wireframe terrain: X = frequency, Z = time (newest at the
/// front), Y = level; grid lines colored by height.
pub fn build_terrain(heights: &VecDeque<Vec<f32>>) -> Segments {
    let mut out = Segments::new();
    let d = heights.len();
    let at = |zi: usize, xi: usize| -> [f32; 3] {
        let h = heights[zi].get(xi).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        [
            xi as f32 / (TERRAIN_W - 1) as f32 * 2.0 - 1.0,
            h * 0.8,
            zi as f32 / TERRAIN_D as f32 * 2.0 - 1.0,
        ]
    };
    for zi in 0..d {
        for xi in 0..TERRAIN_W - 1 {
            let (a, b) = (at(zi, xi), at(zi, xi + 1));
            out.push(a, b, height_color(a[1] / 0.8, 1.0));
        }
    }
    for zi in 0..d.saturating_sub(1) {
        // Sparser longitudinal lines keep the wireframe readable.
        for xi in (0..TERRAIN_W).step_by(4) {
            let (a, b) = (at(zi, xi), at(zi + 1, xi));
            out.push(a, b, height_color(a[1] / 0.8, 0.6));
        }
    }
    out
}

/// Waveform tunnel: each record is a ring in the XY plane, extruded along
/// -Z into the past; radius wobbles with the sample value.
pub fn build_tunnel(rings: &VecDeque<Vec<f32>>) -> Segments {
    let mut out = Segments::new();
    for (zi, ring) in rings.iter().enumerate() {
        let z = -(zi as f32) * (3.0 / RINGS as f32) + 1.0;
        let fade = 1.0 - zi as f32 / RINGS as f32;
        let n = ring.len().max(3);
        for i in 0..n {
            let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
            let r0 = 0.6 + ring[i] * 0.35;
            let r1 = 0.6 + ring[(i + 1) % n] * 0.35;
            out.push(
                [a0.cos() * r0, a0.sin() * r0, z],
                [a1.cos() * r1, a1.sin() * r1, z],
                [0.2 * fade, 1.0 * fade, 0.5 * fade, 1.0],
            );
        }
    }
    out
}

/// Delay-embedding phase portrait: (x(t), x(t−τ), x(t−2τ)).
pub fn build_phase(samples: &[f32], tau: usize) -> Segments {
    let mut out = Segments::new();
    let n = samples.len().saturating_sub(2 * tau);
    for i in 0..n.saturating_sub(1) {
        let p = |j: usize| [samples[j], samples[j + tau], samples[j + 2 * tau]];
        let t = i as f32 / n.max(1) as f32;
        out.push(p(i), p(i + 1), [1.0 - t, 0.4 + 0.6 * t, 1.0, 1.0]);
    }
    out
}

/// CH1 vs CH2 vs time: the Lissajous figure with history as depth.
pub fn build_xytime(rings: &[VecDeque<Vec<f32>>; 2]) -> Segments {
    let mut out = Segments::new();
    let depth = rings[0].len().min(rings[1].len());
    for (zi, (xs, ys)) in rings[0].iter().zip(rings[1].iter()).take(depth).enumerate() {
        let n = xs.len().min(ys.len());
        let z = -(zi as f32) * (3.0 / RINGS as f32) + 1.0;
        let fade = 1.0 - zi as f32 / RINGS.max(1) as f32;
        for i in 0..n.saturating_sub(1) {
            out.push(
                [xs[i], ys[i], z],
                [xs[i + 1], ys[i + 1], z],
                [1.0 * fade, 0.85 * fade, 0.1 * fade, 1.0],
            );
        }
    }
    out
}

/// Create the render target and the layer-1 camera; confine the gizmo
/// group to that layer.
pub fn setup(
    mut commands: Commands,
    mut viz: ResMut<Viz3dState>,
    mut images: ResMut<Assets<Image>>,
    mut gizmo_configs: ResMut<GizmoConfigStore>,
) {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
    let mut image = Image::new(
        Extent3d {
            width: RT_W,
            height: RT_H,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0; (RT_W * RT_H * 4) as usize],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_DST;
    viz.image = images.add(image);
    viz.yaw = 0.6;
    viz.pitch = 0.45;
    viz.dist = 3.2;

    let (config, _) = gizmo_configs.config_mut::<VizGizmos>();
    config.render_layers = RenderLayers::layer(1);
    config.line.width = 1.5;

    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.02, 0.025, 0.04)),
            is_active: false,
            ..Default::default()
        },
        bevy::camera::RenderTarget::Image(viz.image.clone().into()),
        RenderLayers::layer(1),
        Transform::from_xyz(0.0, 1.5, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Decimate a record's i8 samples to `n` normalized f32 points.
fn decimate(raw: &[i8], n: usize) -> Vec<f32> {
    let len = raw.len().max(1);
    (0..n)
        .map(|i| raw[i * len / n.max(1)] as f32 / 125.0)
        .collect()
}

/// Fold new records into the histories, drive the orbit camera, and draw
/// the active mode's line art.
pub fn update(
    mut viz: ResMut<Viz3dState>,
    link: Res<Link>,
    fft: Res<FftState>,
    mut gizmos: Gizmos<VizGizmos>,
    mut cameras: Query<(&mut Camera, &mut Transform), With<Camera3d>>,
) {
    let active = viz.mode != Viz3d::Off;
    for (mut cam, mut tf) in &mut cameras {
        if cam.is_active != active {
            cam.is_active = active;
        }
        if active {
            let (y, p, d) = (viz.yaw, viz.pitch, viz.dist);
            let pos = Vec3::new(d * p.cos() * y.sin(), d * p.sin(), d * p.cos() * y.cos());
            *tf = Transform::from_translation(pos).looking_at(Vec3::ZERO, Vec3::Y);
        }
    }
    if !active {
        return;
    }

    // Ingest the newest record.
    if let Some(frame) = &link.latest
        && frame.seq != viz.last_seq
    {
        viz.last_seq = frame.seq;
        for ch in 0..2 {
            if let Some(cap) = frame.channels.iter().find(|c| c.ch == ch) {
                let pts = decimate(&cap.raw, RING_PTS);
                viz.rings[ch].push_front(pts);
                viz.rings[ch].truncate(RINGS);
            }
        }
        if let Some(s) = &fft.spectrum {
            let (lo, hi) = fft.db;
            let n = s.amplitude.len().max(1);
            let row: Vec<f32> = (0..TERRAIN_W)
                .map(|c| {
                    let i = c * n / TERRAIN_W;
                    ((s.dbv(i) as f32 - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0)
                })
                .collect();
            viz.heights.push_front(row);
            viz.heights.truncate(TERRAIN_D);
        }
    }

    let built = match viz.mode {
        Viz3d::Off => return,
        Viz3d::Terrain => build_terrain(&viz.heights),
        Viz3d::Tunnel => build_tunnel(&viz.rings[0]),
        Viz3d::Phase => {
            let samples = viz.rings[0].front().cloned().unwrap_or_default();
            let tau = (samples.len() / 16).max(1);
            build_phase(&samples, tau)
        }
        Viz3d::XyTime => build_xytime(&viz.rings),
    };
    for ((a, b), c) in built.lines.iter().zip(&built.colors) {
        gizmos.line(
            Vec3::from_array(*a),
            Vec3::from_array(*b),
            Color::srgba(c[0], c[1], c[2], c[3]),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: usize) -> VecDeque<Vec<f32>> {
        (0..n)
            .map(|i| vec![i as f32 / n as f32; TERRAIN_W])
            .collect()
    }

    #[test]
    fn terrain_wireframe_shape() {
        let m = build_terrain(&rows(4));
        // 4 rows of (W-1) cross lines + 3 strips of W/4 longitudinal lines.
        let longitudinal = TERRAIN_W.div_ceil(4);
        assert_eq!(m.lines.len(), 4 * (TERRAIN_W - 1) + 3 * longitudinal);
        assert_eq!(m.lines.len(), m.colors.len());
        // Heights normalized into [0, 0.8].
        assert!(
            m.lines
                .iter()
                .all(|(a, b)| (0.0..=0.8).contains(&a[1]) && (0.0..=0.8).contains(&b[1]))
        );
    }

    #[test]
    fn tunnel_and_xytime_segments() {
        let ring: Vec<f32> = (0..RING_PTS).map(|i| (i as f32 * 0.1).sin()).collect();
        let rings: VecDeque<Vec<f32>> = (0..5).map(|_| ring.clone()).collect();
        let t = build_tunnel(&rings);
        assert_eq!(t.lines.len(), 5 * RING_PTS);

        let pair = [rings.clone(), rings];
        let xy = build_xytime(&pair);
        assert_eq!(xy.lines.len(), 5 * (RING_PTS - 1));
    }

    #[test]
    fn phase_embedding_uses_delays() {
        let s: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let m = build_phase(&s, 10);
        // First segment starts at (s[0], s[10], s[20]).
        assert_eq!(m.lines[0].0, [0.0, 10.0, 20.0]);
        assert_eq!(m.lines.len(), 79);
    }
}
