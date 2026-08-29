//! `Backend` implementation wrapping the blocking driver, plus a reconnecting
//! factory for the supervisor.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use neowon_backend::{Backend, BackendError, Capabilities, MultiMode, ScopeConfig};
use neowon_core::SharedFrame;

use crate::consts;
use crate::device::{ChannelSetup, Vds1022};
use crate::error::Error;

pub struct Vds1022Backend {
    dev: Vds1022,
    caps: Capabilities,
    applied: Option<ScopeConfig>,
}

impl Vds1022Backend {
    pub fn open(fpga_dir: Option<&std::path::Path>) -> Result<Self, Error> {
        let dev = Vds1022::open(fpga_dir)?;
        let caps = Capabilities {
            name: "OWON VDS1022".into(),
            serial: dev.cal.serial.clone(),
            channels: 2,
            sample_rates: consts::SAMPLE_RATES.to_vec(),
            volts_div: consts::VOLTBASE_MV.iter().map(|&mv| mv as f64 / 1000.0).collect(),
            probes: vec![1.0, 10.0, 20.0, 50.0, 100.0, 500.0, 1000.0],
            record_len: consts::SAMPLES,
        };
        Ok(Self { dev, caps, applied: None })
    }
}

fn fatal(e: Error) -> BackendError {
    BackendError::Fatal(e.to_string())
}

impl Backend for Vds1022Backend {
    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn apply(&mut self, cfg: &ScopeConfig) -> Result<(), BackendError> {
        let prev = self.applied.take();
        let same = |f: fn(&ScopeConfig) -> ScopeConfigPart| {
            prev.as_ref().map(f) == Some(f(cfg))
        };

        if !same(part_channels) {
            for (i, ch) in cfg.channels.iter().take(2).enumerate() {
                self.dev
                    .configure_channel(
                        i,
                        ChannelSetup {
                            enabled: ch.enabled,
                            vb: consts::nearest_voltbase(ch.volts_div),
                            coupling: ch.coupling,
                            probe: ch.probe,
                            offset: ch.offset,
                        },
                    )
                    .map_err(fatal)?;
            }
        }
        if !same(part_rate) {
            self.dev.set_sample_rate(cfg.sample_rate).map_err(fatal)?;
            self.dev.set_trigger_position(cfg.position).map_err(fatal)?;
        } else if !same(part_position) {
            self.dev.set_trigger_position(cfg.position).map_err(fatal)?;
        }
        if !same(part_trigger) || !same(part_channels) {
            let t = &cfg.trigger;
            let src = t.source.min(1);
            self.dev
                .set_trigger(src, &t.kind, t.level, t.sweep)
                .map_err(fatal)?;
            self.dev.set_holdoff(src, t.holdoff).map_err(fatal)?;
        }
        if !same(part_acq) {
            self.dev
                .set_peak_mode(matches!(cfg.acq, neowon_core::AcqMode::Peak))
                .map_err(fatal)?;
        }
        if prev.as_ref().map(|c| c.running) != Some(cfg.running) {
            if cfg.running {
                self.dev.run().map_err(fatal)?;
            } else {
                self.dev.stop().map_err(fatal)?;
            }
        }
        self.applied = Some(cfg.clone());
        Ok(())
    }

    fn poll_frame(&mut self, budget: Duration) -> Result<Option<SharedFrame>, BackendError> {
        match self.dev.try_capture() {
            Ok(Some(frame)) => Ok(Some(Arc::new(frame))),
            Ok(None) => {
                // Not ready: the vendor apps back off ~60 ms.
                std::thread::sleep(budget.min(Duration::from_millis(60)));
                Ok(None)
            }
            Err(e) => Err(fatal(e)),
        }
    }

    fn idle(&mut self) -> Result<(), BackendError> {
        self.dev.keep_alive().map_err(fatal)
    }

    fn force_trigger(&mut self) -> Result<(), BackendError> {
        self.dev.force_trigger().map_err(fatal)
    }

    fn set_multi(&mut self, mode: MultiMode) -> Result<(), BackendError> {
        self.dev.set_multi(mode).map_err(fatal)
    }

    fn set_pass_fail_output(&mut self, level: bool) -> Result<(), BackendError> {
        self.dev.set_pf_level(level).map_err(fatal)
    }

    /// Probe the trigger-source channel and pick range, rate, and trigger
    /// level. Applies the result to the hardware and returns the new config.
    fn autoset(&mut self) -> Result<Option<ScopeConfig>, BackendError> {
        use neowon_core::{Slope, Sweep};

        let mut cfg = self.applied.clone().unwrap_or_default();
        let src = cfg.trigger.source.min(1);
        let probe = cfg.channels.get(src).map_or(1.0, |c| c.probe);
        let coupling = cfg.channels.get(src).map_or(neowon_core::Coupling::Dc, |c| c.coupling);

        let capture_one = |dev: &mut Vds1022| -> Result<neowon_core::CaptureFrame, BackendError> {
            // Discard one record (may predate the last config write).
            let _ = dev.capture(Duration::from_secs(2)).map_err(fatal)?;
            dev.capture(Duration::from_secs(2)).map_err(fatal)
        };
        let measure = |f: &neowon_core::CaptureFrame| -> Option<(i32, i32)> {
            let cap = f.channels.iter().find(|c| c.ch == src)?;
            let (min, max) = cap.raw.iter().fold((i32::MAX, i32::MIN), |(lo, hi), &r| {
                (lo.min(r as i32), hi.max(r as i32))
            });
            Some((min, max))
        };

        // Sweep down the ranges until the signal is measurable.
        self.dev
            .set_edge_trigger(src, Slope::Rising, 0.0, Sweep::Auto)
            .map_err(fatal)?;
        self.dev.set_sample_rate(250e3).map_err(fatal)?;
        self.dev.run().map_err(fatal)?;
        let mut vb = 9usize;
        let (mut min, mut max);
        loop {
            self.dev
                .configure_channel(
                    src,
                    ChannelSetup { enabled: true, vb, coupling, probe, offset: 0.0 },
                )
                .map_err(fatal)?;
            let frame = capture_one(&mut self.dev)?;
            let Some((lo, hi)) = measure(&frame) else {
                return Ok(None);
            };
            (min, max) = (lo, hi);
            if hi - lo >= 8 || vb == 0 {
                break;
            }
            vb = vb.saturating_sub(2);
        }
        if max - min < 4 {
            return Ok(None); // flat — no signal
        }

        // Choose the smallest range where the signal stays within ~85% of
        // the screen, then re-measure there for the trigger level.
        let lsb = consts::full_scale_volts(vb) * probe / consts::ADC_RANGE;
        let peak_v = (max.abs().max(min.abs()) as f64) * lsb;
        let mut best = 0;
        for (i, &mv) in consts::VOLTBASE_MV.iter().enumerate() {
            let fs = mv as f64 / 1000.0 * 10.0 * probe;
            best = i;
            if peak_v <= fs * 0.425 {
                break;
            }
        }
        vb = best;
        self.dev
            .configure_channel(
                src,
                ChannelSetup { enabled: true, vb, coupling, probe, offset: 0.0 },
            )
            .map_err(fatal)?;
        let frame = capture_one(&mut self.dev)?;
        let Some((lo, hi)) = measure(&frame) else { return Ok(None) };
        let lsb = consts::full_scale_volts(vb) * probe / consts::ADC_RANGE;
        let level = (lo + hi) as f64 / 2.0 * lsb;

        // Pick a rate that shows a handful of periods, when a frequency is
        // measurable at 250 kS/s; retry at a faster rate for fast signals.
        let mut rate = 250e3;
        let cap = frame.channels.iter().find(|c| c.ch == src);
        let mut freq = cap.and_then(|c| neowon_dsp::estimate_frequency(&c.raw, frame.sample_rate));
        if freq.is_none() {
            self.dev.set_sample_rate(25e6).map_err(fatal)?;
            let f2 = capture_one(&mut self.dev)?;
            freq = f2
                .channels
                .iter()
                .find(|c| c.ch == src)
                .and_then(|c| neowon_dsp::estimate_frequency(&c.raw, f2.sample_rate));
        }
        if let Some(f) = freq {
            // ~5 periods across the 5000-sample record.
            rate = (f * 1000.0).clamp(2.5e3, 100e6);
        }
        let actual = self.dev.set_sample_rate(rate).map_err(fatal)?;
        self.dev
            .set_edge_trigger(src, Slope::Rising, level, Sweep::Auto)
            .map_err(fatal)?;
        self.dev.run().map_err(fatal)?;

        if let Some(ch) = cfg.channels.get_mut(src) {
            ch.enabled = true;
            ch.volts_div = consts::VOLTBASE_MV[vb] as f64 / 1000.0;
            ch.offset = 0.0;
        }
        cfg.sample_rate = actual;
        cfg.trigger.level = level;
        cfg.trigger.kind = neowon_core::TriggerKind::Edge { slope: Slope::Rising };
        cfg.trigger.sweep = Sweep::Auto;
        cfg.position = 0.5;
        cfg.acq = neowon_core::AcqMode::Sample;
        cfg.running = true;
        self.applied = Some(cfg.clone());
        Ok(Some(cfg))
    }
}

// Comparable projections of ScopeConfig used for cheap diffing.
type ScopeConfigPart = Vec<u64>;

fn part_channels(c: &ScopeConfig) -> ScopeConfigPart {
    c.channels
        .iter()
        .flat_map(|ch| {
            [
                ch.enabled as u64,
                ch.volts_div.to_bits(),
                ch.probe.to_bits(),
                ch.offset.to_bits(),
                match ch.coupling {
                    neowon_core::Coupling::Ac => 0,
                    neowon_core::Coupling::Dc => 1,
                    neowon_core::Coupling::Gnd => 2,
                },
            ]
        })
        .collect()
}

fn part_rate(c: &ScopeConfig) -> ScopeConfigPart {
    vec![c.sample_rate.to_bits()]
}

fn part_position(c: &ScopeConfig) -> ScopeConfigPart {
    vec![c.position.to_bits()]
}

fn part_trigger(c: &ScopeConfig) -> ScopeConfigPart {
    use neowon_core::TriggerKind;
    let t = &c.trigger;
    let mut v = vec![
        t.source as u64,
        t.level.to_bits(),
        t.holdoff.to_bits(),
        match t.sweep {
            neowon_core::Sweep::Auto => 0,
            neowon_core::Sweep::Normal => 1,
            neowon_core::Sweep::Single => 2,
        },
    ];
    // Fold the full trigger kind into comparable u64s.
    match &t.kind {
        TriggerKind::Edge { slope } => {
            v.push(0);
            v.push(matches!(slope, neowon_core::Slope::Falling) as u64);
        }
        TriggerKind::Pulse { condition, width } => {
            v.push(1);
            v.push(condition.code() as u64);
            v.push(width.to_bits());
        }
        TriggerKind::Slope { condition, width, upper, lower } => {
            v.push(2);
            v.push(condition.code() as u64);
            v.push(width.to_bits());
            v.push(upper.to_bits());
            v.push(lower.to_bits());
        }
        TriggerKind::Video { sync, line } => {
            v.push(3);
            v.push(sync.code() as u64);
            v.push(*line as u64);
        }
    }
    v
}

fn part_acq(c: &ScopeConfig) -> ScopeConfigPart {
    vec![matches!(c.acq, neowon_core::AcqMode::Peak) as u64]
}

/// Default bitstream location, overridable with `NEOWON_FPGA_DIR`.
pub fn default_fpga_dir() -> PathBuf {
    std::env::var_os("NEOWON_FPGA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/Users/zenx/projects/owon/OWON-VDS1022/fwr"))
}

/// A supervisor factory that reconnects to whatever VDS1022 shows up.
pub fn factory(
    fpga_dir: Option<PathBuf>,
) -> impl FnMut() -> Result<Box<dyn Backend>, String> + Send + 'static {
    let dir = fpga_dir.unwrap_or_else(default_fpga_dir);
    move || {
        Vds1022Backend::open(Some(&dir))
            .map(|b| Box::new(b) as Box<dyn Backend>)
            .map_err(|e| e.to_string())
    }
}
