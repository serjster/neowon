//! Screenshot capture: the whole window (what the operator sees, egui
//! included) through Bevy's `Screenshot`, and the raw plot-texture readback
//! the pixel tests assert on. Split out of `mod.rs` (hard size budget) —
//! capture, wait-for-callback and the retry guard are a job of their own.

use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::image::TextureFormatPixelInfo;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

use crate::Link;
use crate::gpu::{PLOT_H, PLOT_W};

/// Shots in flight (observers decrement; `quit` waits on this). A logical
/// shot increments exactly once, whether it is a plot readback or a window
/// capture, and decrements exactly once on its result (or on giving up).
static PENDING_SHOTS: AtomicUsize = AtomicUsize::new(0);

/// Outstanding shots, polled by `Action::Quit`.
pub(crate) fn pending() -> usize {
    PENDING_SHOTS.load(Ordering::SeqCst)
}

/// Frames a whole-window capture may be in flight before the renderer is
/// assumed to have skipped its target (no surface yet, or a frame where the
/// window could not deliver a texture). ~1 s at 60 fps.
const RETRY_FRAMES: u32 = 60;
/// Retries before a window capture is reported failed. Bounded so a `quit`
/// waiting on pending shots can never re-arm forever.
const MAX_ATTEMPTS: u32 = 3;

/// A request for a whole-window capture. The observer runs when the frame's
/// image comes back; the entry leaves this list when its entity does.
pub(crate) struct Inflight {
    entity: Entity,
    path: String,
    /// Crop in captured-image (physical window) pixels.
    roi: Option<(u32, u32, u32, u32)>,
    frames: u32,
    attempt: u32,
}

#[derive(Resource, Default)]
pub(crate) struct WindowShots {
    inflight: Vec<Inflight>,
}

/// `shot <path> [x y w h]`: request the whole window, optionally cropped to
/// an ROI in captured-image pixels (physical window pixels).
pub(crate) fn window(
    shots: &mut WindowShots,
    commands: &mut Commands,
    path: &str,
    roi: Option<(u32, u32, u32, u32)>,
) {
    PENDING_SHOTS.fetch_add(1, Ordering::SeqCst);
    let entity = spawn_window(commands, path.to_string(), roi);
    shots.inflight.push(Inflight {
        entity,
        path: path.to_string(),
        roi,
        frames: 0,
        attempt: 0,
    });
}

/// `shotplot <path> [x y w h]`: read the plot texture back and write it.
/// `roi` is in plot-texture pixels (1000x500).
pub(crate) fn plot(
    commands: &mut Commands,
    source: Handle<Image>,
    path: String,
    roi: Option<(u32, u32, u32, u32)>,
) {
    PENDING_SHOTS.fetch_add(1, Ordering::SeqCst);
    commands.spawn(Readback::texture(source)).observe(
        move |event: On<ReadbackComplete>, mut cmd: Commands| {
            write_plot(&event.data, &path, roi);
            PENDING_SHOTS.fetch_sub(1, Ordering::SeqCst);
            cmd.entity(event.entity).despawn();
        },
    );
}

/// Spawn one window-capture entity with its result observer. The caller owns
/// the pending count: a retry replaces the entity, it does not add a shot.
///
/// The observer refuses an all-zero image: Bevy 0.19's screenshot path copies
/// the capture texture unconditionally and reports success even when the
/// window's view did not render into it that frame (then the read-back is a
/// zeroed buffer — the all-black captures this module exists to prevent).
/// A blank frame is retried; after [`MAX_ATTEMPTS`] the shot fails loudly
/// instead of writing a black PNG.
fn spawn_window(
    commands: &mut Commands,
    path: String,
    roi: Option<(u32, u32, u32, u32)>,
) -> Entity {
    let mut spawned = commands.spawn(Screenshot::primary_window());
    let entity = spawned.id();
    spawned.observe(
        move |event: On<ScreenshotCaptured>,
              mut cmd: Commands,
              mut link: ResMut<Link>,
              mut shots: ResMut<WindowShots>| {
            let Some(i) = shots.inflight.iter().position(|s| s.entity == event.entity) else {
                return; // a timed-out shot's late image; nothing to report it to
            };
            if is_blank(&event.image) {
                if shots.inflight[i].attempt >= MAX_ATTEMPTS {
                    error!("script: {} got only blank frames, giving up", path);
                    link.last_shot = Some(format!("{path} (failed: blank capture)"));
                    PENDING_SHOTS.fetch_sub(1, Ordering::SeqCst);
                    shots.inflight.swap_remove(i);
                    return;
                }
                shots.inflight[i].attempt += 1;
                shots.inflight[i].frames = 0;
                warn!("script: {path} blank capture, retry");
                let (path, roi) = (path.clone(), roi);
                shots.inflight[i].entity = spawn_window(&mut cmd, path, roi);
                return;
            }
            match write_image(&event.image, &path, roi) {
                Ok(desc) => {
                    info!("script: wrote {path} ({desc})");
                    link.last_shot = Some(path.clone());
                }
                Err(e) => {
                    error!("script: cannot write {path}: {e}");
                    link.last_shot = Some(format!("{path} (failed: {e})"));
                }
            }
            PENDING_SHOTS.fetch_sub(1, Ordering::SeqCst);
            cmd.entity(event.entity).despawn();
        },
    );
    entity
}

/// Watch in-flight window captures: drop finished ones, retry a frame the
/// renderer skipped, and give up (reporting, not hanging) after
/// [`MAX_ATTEMPTS`]. A capture whose entity is gone either delivered its
/// image (the observer despawned it) or never got one and was dropped.
pub(crate) fn tick(
    mut commands: Commands,
    alive: Query<(), With<Screenshot>>,
    mut shots: ResMut<WindowShots>,
    mut link: ResMut<Link>,
) {
    let mut i = 0;
    while i < shots.inflight.len() {
        if !alive.contains(shots.inflight[i].entity) {
            shots.inflight.swap_remove(i);
            continue;
        }
        let s = &mut shots.inflight[i];
        s.frames += 1;
        if s.frames > RETRY_FRAMES {
            if s.attempt >= MAX_ATTEMPTS {
                error!("script: {} got no capture, giving up", s.path);
                link.last_shot = Some(format!("{} (failed: no capture)", s.path));
                PENDING_SHOTS.fetch_sub(1, Ordering::SeqCst);
                commands.entity(s.entity).try_despawn();
                shots.inflight.swap_remove(i);
                continue;
            }
            s.attempt += 1;
            s.frames = 0;
            warn!("script: {} retry {}", s.path, s.attempt);
            let entity = spawn_window(&mut commands, s.path.clone(), s.roi);
            s.entity = entity;
        }
        i += 1;
    }
}

/// True when the capture has no content at all. A rendered frame always holds
/// at least the camera's clear colour; Bevy's screenshot path, however, can
/// come back with a zeroed buffer (successfully "captured") when the window's
/// view was not rendered that frame. Writing that would be a silent black PNG.
pub(crate) fn is_blank(image: &Image) -> bool {
    image
        .data
        .as_deref()
        .is_none_or(|d| d.iter().all(|&b| b == 0))
}

/// Write a captured window image as PNG (`.png`) or binary PPM (anything
/// else), optionally cropped to `roi` in image pixels. Returns `"WxH"` of
/// what was written, for the status line.
pub(crate) fn write_image(
    image: &Image,
    path: &str,
    roi: Option<(u32, u32, u32, u32)>,
) -> Result<String, String> {
    let (w, h) = (image.width(), image.height());
    let data = image
        .data
        .as_deref()
        .ok_or_else(|| "image has no pixel data".to_string())?;
    let channels = image.texture_descriptor.format.pixel_size().unwrap_or(0);
    let stride = image.width() as usize * channels;
    if channels == 0 || data.len() < stride * image.height() as usize {
        return Err(format!(
            "short pixel buffer for {:?}: {}",
            image.texture_descriptor.format,
            data.len()
        ));
    }
    let (x0, y0, w, h) = crop(w, h, roi);
    let px = (w as usize)
        .checked_mul(h as usize)
        .ok_or_else(|| "image size overflow".to_string())?;
    let mut rgb = Vec::with_capacity(px * 3);
    for row in y0..y0 + h {
        let base = row as usize * stride + x0 as usize * channels;
        let line = &data[base..base + w as usize * channels];
        match image.texture_descriptor.format {
            TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb => {
                for p in line.as_chunks::<4>().0 {
                    rgb.extend_from_slice(&[p[2], p[1], p[0]]);
                }
            }
            TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb => {
                for p in line.as_chunks::<4>().0 {
                    rgb.extend_from_slice(&p[..3]);
                }
            }
            other => return Err(format!("unsupported screenshot format {other:?}")),
        }
    }
    write_rgb(path, w, h, &rgb).map_err(|e| e.to_string())?;
    Ok(format!("{w}x{h}"))
}

/// Clamp `roi` to the image; the default is the whole image.
fn crop(w: u32, h: u32, roi: Option<(u32, u32, u32, u32)>) -> (u32, u32, u32, u32) {
    let (x0, y0, cw, ch) = roi.unwrap_or((0, 0, w, h));
    let x0 = x0.min(w.saturating_sub(1));
    let y0 = y0.min(h.saturating_sub(1));
    (x0, y0, cw.min(w - x0), ch.min(h - y0))
}

/// Write a (possibly cropped) region of the plot texture — PNG when the
/// path ends `.png`, binary PPM otherwise. Readback rows are 256-byte
/// aligned; the stride strips that.
fn write_plot(rgba: &[u8], path: &str, roi: Option<(u32, u32, u32, u32)>) {
    let stride = rgba.len() / PLOT_H as usize;
    let (x0, y0, w, h) = roi.unwrap_or((0, 0, PLOT_W, PLOT_H));
    let (x0, y0) = (x0.min(PLOT_W - 1), y0.min(PLOT_H - 1));
    let w = w.min(PLOT_W - x0);
    let h = h.min(PLOT_H - y0);
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for row in y0..y0 + h {
        let base = row as usize * stride + x0 as usize * 4;
        for px in rgba[base..base + w as usize * 4].as_chunks::<4>().0 {
            rgb.extend_from_slice(&px[..3]);
        }
    }
    match write_rgb(path, w, h, &rgb) {
        Ok(()) => info!("script: wrote {path} ({w}x{h})"),
        Err(e) => error!("script: cannot write {path}: {e}"),
    }
}

/// PNG when the path ends `.png`, binary PPM otherwise.
fn write_rgb(path: &str, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    if path.ends_with(".png") {
        write_png(path, w, h, rgb)
    } else {
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        ppm.extend_from_slice(rgb);
        std::fs::write(path, &ppm)
    }
}

fn write_png(path: &str, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(std::io::Error::other)?;
    writer
        .write_image_data(rgb)
        .map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension};

    fn image(format: TextureFormat, data: Vec<u8>) -> Image {
        Image::new(
            Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            format,
            RenderAssetUsages::MAIN_WORLD,
        )
    }

    #[test]
    fn bgra_window_pixels_become_rgb() {
        let img = image(
            TextureFormat::Bgra8UnormSrgb,
            vec![30, 20, 10, 255, 60, 50, 40, 255],
        );
        let dir = std::env::temp_dir().join("neowon-shot-unit-bgra.ppm");
        let path = dir.display().to_string();
        assert_eq!(write_image(&img, &path, None).unwrap(), "2x1");
        let bytes = std::fs::read(&dir).unwrap();
        let body = &bytes[bytes.len() - 6..];
        assert_eq!(body, &[10, 20, 30, 40, 50, 60]);
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn rgba_window_pixels_keep_their_order() {
        let img = image(
            TextureFormat::Rgba8UnormSrgb,
            vec![1, 2, 3, 255, 4, 5, 6, 255],
        );
        let dir = std::env::temp_dir().join("neowon-shot-unit-rgba.ppm");
        let path = dir.display().to_string();
        assert_eq!(write_image(&img, &path, None).unwrap(), "2x1");
        let bytes = std::fs::read(&dir).unwrap();
        assert_eq!(&bytes[bytes.len() - 6..], &[1, 2, 3, 4, 5, 6]);
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn a_crop_selects_the_requested_pixels() {
        // 2x1: left red, right blue (BGRA).
        let img = image(
            TextureFormat::Bgra8UnormSrgb,
            vec![30, 20, 10, 255, 60, 50, 40, 255],
        );
        let dir = std::env::temp_dir().join("neowon-shot-unit-crop.ppm");
        let path = dir.display().to_string();
        assert_eq!(write_image(&img, &path, Some((1, 0, 1, 1))).unwrap(), "1x1");
        let bytes = std::fs::read(&dir).unwrap();
        assert_eq!(&bytes[bytes.len() - 3..], &[40, 50, 60]);
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn unsupported_formats_are_refused_not_written() {
        let img = image(TextureFormat::R32Float, vec![0; 8]);
        let dir = std::env::temp_dir().join("neowon-shot-unit-bad.ppm");
        let path = dir.display().to_string();
        assert!(write_image(&img, &path, None).is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn an_all_zero_capture_is_blank() {
        let blank = image(TextureFormat::Bgra8UnormSrgb, vec![0; 8]);
        assert!(is_blank(&blank));
        let clear = image(
            TextureFormat::Bgra8UnormSrgb,
            vec![43, 44, 47, 255, 43, 44, 47, 255],
        );
        assert!(!is_blank(&clear));
    }
}
