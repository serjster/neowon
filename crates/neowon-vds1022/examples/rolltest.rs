//! Can the VDS1022 stream gaplessly, so the host can hold a long record at
//! full sample rate? (Phase 7.8 follow-up, docs/tasks/phase78-lab-semantics
//! -spec.md G2.)
//!
//! Roll mode is the device's streaming mode: it fills the 5000-sample buffer
//! progressively and reports the write position in each frame's `cursor`.
//! `set_sample_rate` only engages it below 2.5 kS/s, but the register is a
//! plain flag and the vendor's Python exposes it as an override at any rate.
//! This harness forces roll across a range of rates and reports, per rate:
//!
//!   * whether frames keep flowing,
//!   * how the cursor advances between reads,
//!   * the read rate versus the buffer's wrap time — the gapless condition
//!     is `read_interval < 5000 / sample_rate`.
//!
//! Requires the scope with the 1 kHz probe-comp signal on CH1.
//!
//!   cargo run -p neowon-vds1022 --example rolltest

use std::time::{Duration, Instant};

use neowon_core::Coupling;
use neowon_vds1022::device::{ChannelSetup, Vds1022};

/// Samples per record.
const SAMPLES: f64 = 5000.0;

fn main() {
    let fpga = neowon_vds1022::backend::default_fpga_dir();
    let mut dev = Vds1022::open(Some(&fpga)).expect("open");
    dev.configure_channel(
        0,
        ChannelSetup {
            enabled: true,
            vb: 5,
            coupling: Coupling::Dc,
            probe: 1.0,
            offset: 0.0,
        },
    )
    .unwrap();

    println!(
        "{:>10}  {:>6}  {:>7}  {:>9}  {:>9}  {:>8}  verdict",
        "rate", "roll", "frames", "read int.", "wrap", "cursors"
    );

    for rate in [2.5e3, 25e3, 250e3, 2.5e6] {
        for roll in [false, true] {
            let actual = dev.set_sample_rate(rate).unwrap();
            dev.set_roll(roll).unwrap();
            dev.run().unwrap();

            // Drain whatever was in flight from the previous setting.
            let settle = Instant::now() + Duration::from_millis(300);
            while Instant::now() < settle {
                let _ = dev.get_frames();
            }

            let mut cursors: Vec<u16> = Vec::new();
            let mut reads = 0u32;
            let start = Instant::now();
            let deadline = start + Duration::from_secs(3);
            while Instant::now() < deadline {
                match dev.get_frames() {
                    Ok(frames) => {
                        reads += 1;
                        if let Some(f) = frames.first() {
                            cursors.push(f.cursor);
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
                let _ = dev.keep_alive();
            }
            let elapsed = start.elapsed().as_secs_f64();

            let read_interval = if reads > 0 {
                elapsed / reads as f64
            } else {
                f64::NAN
            };
            let wrap = SAMPLES / actual;
            // Distinct cursor values tell us whether the device is reporting
            // a moving write position (streaming) or a constant one.
            let mut uniq: Vec<u16> = cursors.clone();
            uniq.sort_unstable();
            uniq.dedup();
            let verdict = if reads == 0 {
                "no frames".to_string()
            } else if uniq.len() == 1 {
                format!("static cursor {}", uniq[0])
            } else {
                format!(
                    "cursor moves ({}..{}, {} distinct){}",
                    uniq.first().unwrap(),
                    uniq.last().unwrap(),
                    uniq.len(),
                    if read_interval < wrap {
                        " — gapless possible"
                    } else {
                        " — reads too slow"
                    }
                )
            };
            println!(
                "{:>10}  {:>6}  {:>7}  {:>8.1}ms  {:>8.1}ms  {:>8}  {}",
                fmt_rate(actual),
                roll,
                reads,
                read_interval * 1e3,
                wrap * 1e3,
                cursors.len(),
                verdict,
            );
        }
    }

    dev.stop().ok();
    println!("\ndone — device stopped");
}

fn fmt_rate(r: f64) -> String {
    if r >= 1e6 {
        format!("{} MS/s", r / 1e6)
    } else if r >= 1e3 {
        format!("{} kS/s", r / 1e3)
    } else {
        format!("{r} S/s")
    }
}
