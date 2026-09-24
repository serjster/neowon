//! `neowon sdr smoke`: the SDR hardware smoke:
//! tune to `--freq`, detect, classify and decode, write a JSON readout, and
//! FAIL if there is no peak within ±2 RBW, the class is `unknown`, the
//! confidence is below 0.70, or the decode is empty.
//!
//! By default the source is the RTL dongle; `--sim <scene>` runs the same
//! pipeline against a `neowon-sim` RF scene and never touches USB. The one
//! place that can open the dongle is [`open`]; unit tests build their
//! backend from a scene directly and cannot reach it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use neowon_backend::{Backend, SdrGain};
use neowon_sim::sdr::{RfScene, SimSdrBackend};

pub mod pipeline;
#[cfg(test)]
mod tests;

use pipeline::{Params, Readout};

#[derive(Subcommand)]
pub enum SdrCmd {
    /// Hardware smoke: tune, detect, classify, decode; exits non-zero on any
    /// FAIL rule. Opens the RTL dongle unless `--sim` is given.
    Smoke(SmokeArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct SmokeArgs {
    /// Frequency under test, Hz (e.g. 100.1e6)
    #[arg(long)]
    pub freq: f64,
    /// Run against this simulated RF scene instead of the dongle
    /// (rf-reference, rf-fm-band, rf-am, rf-fm, rf-digital, …)
    #[arg(long, value_name = "SCENE")]
    pub sim: Option<String>,
    /// The simulator's seed (with --sim)
    #[arg(long, default_value_t = 1, requires = "sim")]
    pub seed: u64,
    /// The dongle's serial, when more than one is attached
    #[arg(long, conflicts_with = "sim")]
    pub serial: Option<String>,
    /// Tuner gain in dB (default: automatic)
    #[arg(long)]
    pub gain: Option<f64>,
    /// The hardware centre sits this far below --freq, Hz, keeping the
    /// signal clear of the zero-IF DC spike
    #[arg(long, default_value_t = 250e3)]
    pub lo_offset: f64,
    /// Write the JSON readout here (atomically)
    #[arg(long)]
    pub json_out: Option<PathBuf>,
    /// Append the readout, dated, to this markdown file
    #[arg(long)]
    pub doc: Option<PathBuf>,
}

/// Where the samples come from. Parsing the arguments builds one of these
/// and nothing more; only [`open`] acts on it.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Sim { scene: String, seed: u64 },
    Rtl { serial: Option<String> },
}

impl SmokeArgs {
    pub fn source(&self) -> Source {
        match &self.sim {
            Some(scene) => Source::Sim {
                scene: scene.clone(),
                seed: self.seed,
            },
            None => Source::Rtl {
                serial: self.serial.clone(),
            },
        }
    }

    pub fn params(&self) -> Params {
        Params {
            lo_offset_hz: self.lo_offset,
            gain: self.gain.map_or(SdrGain::Auto, SdrGain::Manual),
            ..Params::new(self.freq)
        }
    }
}

/// A simulator backend on `scene`, seeded — never USB.
pub fn sim_backend(scene: &str, seed: u64) -> Result<SimSdrBackend> {
    let Some(s) = RfScene::preset(scene) else {
        bail!(
            "unknown sim scene {scene:?}; known: {}",
            RfScene::PRESETS.join(", ")
        );
    };
    let mut b = SimSdrBackend::with_scene(s);
    b.set_seed(seed)
        .map_err(|e| anyhow::anyhow!("seeding the sim: {e}"))?;
    Ok(b)
}

/// Open the source. The RTL arm is the only USB access in this command.
fn open(source: &Source) -> Result<(Box<dyn Backend>, &'static str)> {
    match source {
        Source::Sim { scene, seed } => Ok((Box::new(sim_backend(scene, *seed)?), "sim")),
        Source::Rtl { serial } => {
            // Unit tests must never reach the dongle (AGENTS.md, hardware
            // safety): they build sim backends directly, and this refuses
            // if one ever routes here.
            if cfg!(test) {
                bail!("unit tests never open the RTL dongle");
            }
            let b = neowon_sdr::RtlBackend::open(serial.as_deref())
                .context("opening the RTL-SDR dongle")?;
            Ok((Box::new(b), "rtl"))
        }
    }
}

pub fn run(cmd: &SdrCmd) -> Result<()> {
    match cmd {
        SdrCmd::Smoke(args) => smoke(args),
    }
}

fn smoke(args: &SmokeArgs) -> Result<()> {
    let (mut backend, source) = open(&args.source())?;
    let readout = pipeline::run(backend.as_mut(), source, &args.params())?;
    drop(backend);
    let json = to_json(&readout);
    println!("{json}");
    if let Some(path) = &args.json_out {
        // The contract's path is `audit/…`, a directory the tree lacks.
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        neowon_core::atomic_file::write(path, format!("{json}\n"))
            .with_context(|| format!("writing {}", path.display()))?;
    }
    if let Some(path) = &args.doc {
        append_doc(path, &readout, &json, &today())?;
    }
    if readout.failures.is_empty() {
        println!("SDR SMOKE OK ({source})");
        Ok(())
    } else {
        let why: Vec<String> = readout
            .failures
            .iter()
            .map(|f| format!("{} ({})", f.rule.name(), f.detail))
            .collect();
        bail!("SDR SMOKE FAILED ({source}): {}", why.join("; "));
    }
}

pub fn to_json(r: &Readout) -> String {
    let num = |x: f64| {
        if x.is_finite() {
            format!("{x}")
        } else {
            "null".into()
        }
    };
    let opt = |x: Option<f64>| x.map_or("null".into(), num);
    let failures: Vec<String> = r
        .failures
        .iter()
        .map(|f| {
            format!(
                r#"{{"rule":{},"detail":{}}}"#,
                quote(f.rule.name()),
                quote(&f.detail)
            )
        })
        .collect();
    format!(
        concat!(
            r#"{{"source":{},"dongle_serial":{},"tune_hz":{},"centre_hz":{},"peak_hz":{},"#,
            r#""peak_tol_hz":{},"class":{},"confidence":{},"decode":{},"snr_db":{},"#,
            r#""pass":{},"failures":[{}]}}"#
        ),
        quote(r.source),
        quote(&r.dongle_serial),
        num(r.tune_hz),
        num(r.centre_hz),
        opt(r.peak_hz),
        num(r.peak_tol_hz),
        quote(&r.class),
        num(r.confidence),
        quote(&r.decode),
        opt(r.snr_db),
        r.failures.is_empty(),
        failures.join(",")
    )
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Append the readout to `path` under a dated heading, atomically (the
/// file is rewritten whole with the section added).
pub fn append_doc(path: &Path, r: &Readout, json: &str, date: &str) -> Result<()> {
    let mut text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let verdict = if r.failures.is_empty() {
        "PASS"
    } else {
        "FAIL"
    };
    text.push_str(&format!(
        "\n## SDR smoke readout ({date}) — source `{}`, {verdict}\n\n\
         `neowon sdr smoke --freq {}` (the `neowon-cli` binary){}.\n\n```json\n{json}\n```\n",
        r.source,
        r.tune_hz,
        if r.source == "sim" {
            ", on the simulator — not a hardware readout"
        } else {
            ", on the dongle"
        },
    ));
    neowon_core::atomic_file::write(path, text)
        .with_context(|| format!("writing {}", path.display()))
}

/// Today's UTC date, `YYYY-MM-DD` (the doc heading only; never the signal).
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_date(secs / 86_400)
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Hinnant's
/// `civil_from_days`).
pub fn civil_date(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}
