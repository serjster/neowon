//! `Backend` implementation wrapping the blocking driver, plus a reconnecting
//! factory for the supervisor.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use neowon_backend::{Backend, BackendError, Capabilities, ScopeConfig};
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
            self.dev
                .set_edge_trigger(t.source.min(1), t.slope, t.level, t.sweep)
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
    let t = &c.trigger;
    vec![
        t.source as u64,
        t.level.to_bits(),
        matches!(t.slope, neowon_core::Slope::Falling) as u64,
        match t.sweep {
            neowon_core::Sweep::Auto => 0,
            neowon_core::Sweep::Normal => 1,
            neowon_core::Sweep::Single => 2,
        },
    ]
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
