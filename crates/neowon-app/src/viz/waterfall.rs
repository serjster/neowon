//! Realtime spectrogram: every FFT spectrum becomes one palettized row of
//! a scrolling texture (newest at the bottom). Rows are built on the CPU
//! from the same `neowon-dsp` spectra the spectrum window shows, so the
//! two views can never disagree.

use bevy::prelude::*;
use neowon_dsp::Spectrum;

use crate::derived::FftState;

/// Texture geometry: frequency bins × visible history rows.
pub const WF_W: usize = 1024;
pub const WF_H: usize = 512;

#[derive(Resource, Default)]
pub struct WaterfallState {
    pub on: bool,
    pub image: Handle<Image>,
    /// Sequence number of the last spectrum folded in.
    last_seq: u64,
    /// Rows drawn so far (rendering scrolls only once the ring is full).
    pub rows: usize,
}

/// Log-magnitude → thermal palette (black → blue → red → yellow → white),
/// `t` in [0, 1].
pub fn thermal(t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    let seg = |a: f32, b: f32| ((t - a) / (b - a)).clamp(0.0, 1.0);
    let (r, g, b) = if t < 0.25 {
        (0.0, 0.0, seg(0.0, 0.25) * 0.7)
    } else if t < 0.55 {
        (seg(0.25, 0.55), 0.0, 0.7 * (1.0 - seg(0.25, 0.55)))
    } else if t < 0.8 {
        (1.0, seg(0.55, 0.8), 0.0)
    } else {
        (1.0, 1.0, seg(0.8, 1.0))
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]
}

/// One palettized row from a spectrum: `WF_W` columns, max-pooled over the
/// spectrum's bins, dB-mapped over `(db_lo, db_hi)`.
pub fn build_row(s: &Spectrum, db_lo: f32, db_hi: f32) -> Vec<u8> {
    let n = s.amplitude.len().max(1);
    let mut row = Vec::with_capacity(WF_W * 4);
    for col in 0..WF_W {
        let from = col * n / WF_W;
        let to = (((col + 1) * n / WF_W).max(from + 1)).min(n);
        let mut db = f32::NEG_INFINITY;
        for i in from..to {
            db = db.max(s.dbv(i) as f32);
        }
        let t = (db - db_lo) / (db_hi - db_lo).max(1e-6);
        row.extend_from_slice(&thermal(t));
    }
    row
}

/// Create the (initially black) waterfall texture.
pub fn setup(mut wf: ResMut<WaterfallState>, mut images: ResMut<Assets<Image>>) {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let image = Image::new(
        Extent3d {
            width: WF_W as u32,
            height: WF_H as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0; WF_W * WF_H * 4],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    wf.image = images.add(image);
}

/// Fold the newest spectrum in: scroll the image up one row and write the
/// new row at the bottom.
pub fn update(
    mut wf: ResMut<WaterfallState>,
    fft: Res<FftState>,
    meas: Res<crate::derived::MeasureState>,
    mut images: ResMut<Assets<Image>>,
) {
    if !wf.on {
        return;
    }
    let Some(s) = &fft.spectrum else { return };
    if meas.last_seq == wf.last_seq {
        return;
    }
    wf.last_seq = meas.last_seq;
    let (db_lo, db_hi) = fft.db;
    let row = build_row(s, db_lo, db_hi);
    let Some(mut image) = images.get_mut(&wf.image) else {
        return;
    };
    let data = image.data.as_mut().expect("waterfall image has CPU data");
    let stride = WF_W * 4;
    data.copy_within(stride.., 0);
    let last = (WF_H - 1) * stride;
    data[last..last + stride].copy_from_slice(&row);
    wf.rows = (wf.rows + 1).min(WF_H);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spectrum(amps: Vec<f64>) -> Spectrum {
        Spectrum {
            amplitude: amps,
            bin_hz: 1.0,
        }
    }

    #[test]
    fn row_maps_db_range_to_palette_ends() {
        // 0 dBV bins at full scale, tiny bins at the floor.
        let s = spectrum(vec![1.0; 8]);
        let row = build_row(&s, -100.0, 0.0);
        assert_eq!(row.len(), WF_W * 4);
        // 1.0 V = 0 dBV = top of the range → white.
        assert_eq!(&row[0..4], &[255, 255, 255, 255]);

        let s = spectrum(vec![1e-9; 8]);
        let row = build_row(&s, -100.0, 0.0);
        // Far below the floor → black (blue channel 0 at t=0).
        assert_eq!(&row[0..3], &[0, 0, 0]);
    }

    #[test]
    fn row_max_pools_bins() {
        // One hot bin among many quiet ones must survive pooling.
        let mut amps = vec![1e-9; 4096];
        amps[2048] = 1.0;
        let row = build_row(&spectrum(amps), -100.0, 0.0);
        let hot_col = 2048 * WF_W / 4096;
        assert_eq!(&row[hot_col * 4..hot_col * 4 + 3], &[255, 255, 255]);
    }
}
