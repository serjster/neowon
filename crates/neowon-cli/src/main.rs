//! Headless bring-up and debugging tool for the VDS1022 driver.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use neowon_core::{Coupling, Slope, Sweep};
use neowon_dsp::{basic_stats, estimate_frequency};
use neowon_vds1022::{ChannelSetup, Vds1022};

/// Default FPGA bitstream location: the community OWON-VDS1022 checkout.
const DEFAULT_FPGA_DIR: &str = "/Users/zenx/projects/owon/OWON-VDS1022/fwr";

#[derive(Parser)]
#[command(name = "neowon", about = "VDS1022 oscilloscope CLI")]
struct Cli {
    /// Directory containing VDS1022_FPGAV*.bin bitstreams
    #[arg(long, global = true, env = "NEOWON_FPGA_DIR", default_value = DEFAULT_FPGA_DIR)]
    fpga_dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Connect and print device identity + calibration
    Probe,
    /// Capture frames and print measurements (optionally CSV)
    Dump {
        #[arg(long, default_value_t = 5)]
        frames: u32,
        #[command(flatten)]
        acq: AcqArgs,
        /// Write samples to a CSV file
        #[arg(long)]
        csv: Option<PathBuf>,
    },
    /// Stream continuously, printing measurements once per second
    Stream {
        #[arg(long, default_value_t = 10.0)]
        secs: f64,
        #[command(flatten)]
        acq: AcqArgs,
    },
    /// Hardware smoke test: expects the 1 kHz probe-comp signal on CH1
    Smoke {
        #[command(flatten)]
        acq: AcqArgs,
    },
    /// Run the backend auto-set against the live signal and print the result
    Autoset,
}

#[derive(clap::Args, Clone, Copy)]
struct AcqArgs {
    /// Sample rate in S/s (snapped to the hardware ladder)
    #[arg(long, default_value_t = 250e3)]
    rate: f64,
    /// Volts per division (5 mV..5 V, snapped)
    #[arg(long, default_value_t = 1.0)]
    volts_div: f64,
    /// Probe attenuation factor
    #[arg(long, default_value_t = 1.0)]
    probe: f64,
    /// Enable CH2 as well
    #[arg(long)]
    ch2: bool,
    /// Trigger level in volts (edge, rising, CH1)
    #[arg(long, default_value_t = 2.5)]
    trig_level: f64,
    /// Sweep mode
    #[arg(long, value_enum, default_value_t = SweepArg::Auto)]
    sweep: SweepArg,
    /// Hardware peak-detect mode
    #[arg(long)]
    peak: bool,
    /// Trigger holdoff in seconds
    #[arg(long, default_value_t = 100e-9)]
    holdoff: f64,
}

#[derive(clap::ValueEnum, Clone, Copy, PartialEq)]
enum SweepArg {
    Auto,
    Normal,
    Single,
}

impl From<SweepArg> for Sweep {
    fn from(s: SweepArg) -> Self {
        match s {
            SweepArg::Auto => Sweep::Auto,
            SweepArg::Normal => Sweep::Normal,
            SweepArg::Single => Sweep::Single,
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Probe => probe(&cli),
        Cmd::Dump {
            frames,
            acq,
            ref csv,
        } => dump(&cli, frames, acq, csv.as_deref()),
        Cmd::Stream { secs, acq } => stream(&cli, secs, acq),
        Cmd::Smoke { acq } => smoke(&cli, acq),
        Cmd::Autoset => autoset(&cli),
    }
}

fn autoset(cli: &Cli) -> Result<()> {
    use neowon_backend::Backend;
    let mut be = neowon_vds1022::backend::Vds1022Backend::open(Some(&cli.fpga_dir))
        .context("opening VDS1022")?;
    match be.autoset().map_err(|e| anyhow::anyhow!(e.to_string()))? {
        None => bail!("autoset found no signal"),
        Some(cfg) => {
            let ch = &cfg.channels[cfg.trigger.source];
            println!(
                "autoset: {} V/div, {} S/s, trigger {:.3} V",
                ch.volts_div, cfg.sample_rate, cfg.trigger.level
            );
            // Show what it looks like with the chosen settings.
            let frame = be
                .poll_frame(Duration::from_secs(2))
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .context("no frame after autoset")?;
            println!("{}", report(&frame));
            Ok(())
        }
    }
}

fn open(cli: &Cli) -> Result<Vds1022> {
    Vds1022::open(Some(&cli.fpga_dir)).context("opening VDS1022")
}

fn probe(cli: &Cli) -> Result<()> {
    let mut dev = open(cli)?;
    let cal = &dev.cal;
    println!("serial      : {}", cal.serial);
    println!("hw version  : {}", cal.hw_version);
    println!("oem         : {}", cal.oem);
    println!("phasefine   : {}", cal.phasefine);
    println!(
        "cold start  : {} (FPGA {})",
        dev.cold_start,
        if dev.cold_start {
            "uploaded by us"
        } else {
            "was already loaded"
        }
    );
    println!("cal (per voltbase 5mV..5V/div):");
    for (name, arr) in [
        ("gain", &cal.gain),
        ("ampl", &cal.ampl),
        ("comp", &cal.comp),
    ] {
        for (ch, row) in arr.iter().enumerate() {
            println!("  ch{} {:4}: {:?}", ch + 1, name, row);
        }
    }
    dev.stop()?;
    Ok(())
}

/// Shared acquisition setup: CH1 (and optionally CH2), edge trigger, run.
fn setup(dev: &mut Vds1022, acq: AcqArgs) -> Result<f64> {
    let vb = nearest_vb(acq.volts_div);
    let ch = ChannelSetup {
        enabled: true,
        vb,
        coupling: Coupling::Dc,
        probe: acq.probe,
        offset: 0.0,
    };
    dev.configure_channel(0, ch)?;
    dev.configure_channel(
        1,
        ChannelSetup {
            enabled: acq.ch2,
            ..ch
        },
    )?;
    let actual = dev.set_sample_rate(acq.rate)?;
    dev.set_peak_mode(acq.peak)?;
    dev.set_edge_trigger(0, Slope::Rising, acq.trig_level, acq.sweep.into())?;
    dev.set_holdoff(0, acq.holdoff)?;
    dev.run()?;
    eprintln!(
        "configured: {} V/div, {} S/s, trigger {} V rising CH1, {:?} sweep{}",
        neowon_vds1022::consts::VOLTBASE_MV[vb] as f64 / 1000.0,
        actual,
        acq.trig_level,
        Sweep::from(acq.sweep),
        if acq.peak { ", peak-detect" } else { "" },
    );
    Ok(actual)
}

fn nearest_vb(volts_div: f64) -> usize {
    neowon_vds1022::consts::nearest_voltbase(volts_div)
}

fn report(frame: &neowon_core::CaptureFrame) -> String {
    let mut out = String::new();
    for cap in &frame.channels {
        let stats = basic_stats(cap);
        let freq = estimate_frequency(&cap.raw, frame.sample_rate);
        out.push_str(&format!(
            "seq {:>5} ch{}  vpp {:>8}  vavg {:>8}  freq(sw) {:>10}  freq(hw) {:>10}{}",
            frame.seq,
            cap.ch + 1,
            stats.map_or("-".into(), |s| format!("{:.3} V", s.vpp)),
            stats.map_or("-".into(), |s| format!("{:.3} V", s.vavg)),
            freq.map_or("-".into(), format_hz),
            cap.freq_meter.map_or("-".into(), format_hz),
            if cap.clipped { "  CLIPPED" } else { "" },
        ));
        out.push('\n');
    }
    out.pop();
    out
}

fn format_hz(f: f64) -> String {
    if f >= 1e6 {
        format!("{:.4} MHz", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.4} kHz", f / 1e3)
    } else {
        format!("{f:.2} Hz")
    }
}

fn dump(cli: &Cli, frames: u32, acq: AcqArgs, csv: Option<&std::path::Path>) -> Result<()> {
    let mut dev = open(cli)?;
    setup(&mut dev, acq)?;
    let mut csv_file = csv.map(std::fs::File::create).transpose()?;
    for _ in 0..frames {
        let frame = dev.capture(Duration::from_secs(5))?;
        println!("{}", report(&frame));
        if let Some(f) = csv_file.as_mut() {
            for cap in &frame.channels {
                for (i, v) in cap.iter_volts().enumerate() {
                    writeln!(
                        f,
                        "{},{},{},{:.6}",
                        frame.seq,
                        cap.ch + 1,
                        i as f64 / frame.sample_rate,
                        v
                    )?;
                }
            }
        }
    }
    dev.stop()?;
    Ok(())
}

fn stream(cli: &Cli, secs: f64, acq: AcqArgs) -> Result<()> {
    let mut dev = open(cli)?;
    setup(&mut dev, acq)?;
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut last_print = Instant::now() - Duration::from_secs(1);
    let mut count = 0u64;
    while Instant::now() < end {
        match dev.capture(Duration::from_millis(500)) {
            Ok(frame) => {
                count += 1;
                if last_print.elapsed() >= Duration::from_secs(1) {
                    println!("[{count} frames] {}", report(&frame));
                    last_print = Instant::now();
                }
            }
            Err(neowon_vds1022::Error::NotReady) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    eprintln!("{count} frames in {secs} s ({:.1}/s)", count as f64 / secs);
    dev.stop()?;
    Ok(())
}

fn smoke(cli: &Cli, mut acq: AcqArgs) -> Result<()> {
    let mut dev = open(cli)?;

    // Pass 1: 1 V/div. A x1 probe shows ~5 Vpp, a x10 probe ~0.5 Vpp.
    acq.volts_div = 1.0;
    acq.probe = 1.0;
    setup(&mut dev, acq)?;
    let _ = dev.capture(Duration::from_secs(5))?; // discard pre-config record
    let frame = dev.capture(Duration::from_secs(5))?;
    let cap = &frame.channels[0];
    let stats = basic_stats(cap).context("no samples")?;
    println!("{}", report(&frame));

    let (vpp, probe_factor) = if (0.3..=0.9).contains(&stats.vpp) {
        // Looks like a x10 probe: re-range for resolution and confirm.
        println!("(~0.5 Vpp at the BNC: probe switch is at x10, re-ranging)");
        acq.volts_div = 0.2;
        setup(&mut dev, acq)?;
        let _ = dev.capture(Duration::from_secs(5))?;
        let frame = dev.capture(Duration::from_secs(5))?;
        println!("{}", report(&frame));
        let s = basic_stats(&frame.channels[0]).context("no samples")?;
        (s.vpp * 10.0, 10.0)
    } else {
        (stats.vpp, 1.0)
    };
    let freq = estimate_frequency(&cap.raw, frame.sample_rate).context("no frequency")?;
    dev.stop()?;

    let mut failures = vec![];
    // Probe-comp squares overshoot on an untrimmed probe; allow up to 7 V.
    if !(4.0..=7.0).contains(&vpp) {
        failures.push(format!("vpp {vpp:.3} V (at probe tip) outside 4..7 V"));
    }
    if !(980.0..=1020.0).contains(&freq) {
        failures.push(format!("freq {freq:.1} Hz outside 980..1020 Hz"));
    }
    if failures.is_empty() {
        println!(
            "SMOKE OK: 1 kHz probe-comp signal verified ({vpp:.2} Vpp via x{probe_factor} probe)"
        );
        Ok(())
    } else {
        bail!("SMOKE FAILED: {}", failures.join("; "));
    }
}
