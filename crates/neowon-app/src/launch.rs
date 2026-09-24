//! Command-line choice of instrument, and the supervisor that drives it.
//!
//! `--sim`: the simulated scope; `--demo [slow]`: oscilloscope-music XY
//! playback on it (assets/demo, from lofibucket.com's Oscilloscope Quake);
//! `--audio`: the sound
//! card as a streaming scope; `--sdr-sim`: the simulated SDR; `--rtl`: an
//! RTL-SDR dongle; nothing: the VDS1022 (hardware).
//!
//! `instrument scope|sdr` switches instrument at run time within the launch's
//! family: the simulators swap for each other, the hardware for the other
//! hardware (VDS1022 ↔ RTL-SDR), and `--audio` pairs with the simulated SDR.

use neowon_backend::{Backend, Command, ScopeConfig, SdrConfig, Supervisor};

#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub demo: bool,
    demo_slow: bool,
    sim: bool,
    audio: bool,
    sdr_sim: bool,
    rtl: bool,
}

impl Launch {
    pub fn from_args() -> Self {
        let has = |flag: &str| std::env::args().any(|a| a == flag);
        let demo = has("--demo");
        Self {
            demo,
            demo_slow: has("slow"),
            sim: demo || has("--sim"),
            audio: has("--audio"),
            sdr_sim: has("--sdr-sim"),
            rtl: has("--rtl"),
        }
    }

    pub fn sdr(&self) -> bool {
        self.sdr_sim || self.rtl
    }

    /// The simulated family (`--sim` + `--sdr-sim`), for tests that switch
    /// instrument. Test-only on purpose: the default `Launch` is the
    /// hardware family, and a unit test must never open a real device.
    #[cfg(test)]
    pub(crate) fn sim() -> Self {
        Self {
            sim: true,
            sdr_sim: true,
            ..Default::default()
        }
    }

    /// A supervisor for this launch's scope (`sdr` false) or SDR (`sdr`
    /// true) instrument. It connects on its own thread; nothing is sent.
    pub fn supervisor(&self, sdr: bool) -> Supervisor {
        let hardware = !(self.sim || self.audio || self.sdr_sim);
        if sdr && (self.rtl || hardware) {
            neowon_backend::spawn(|| {
                neowon_sdr::RtlBackend::open(None)
                    .map(|b| Box::new(b) as Box<dyn Backend>)
                    .map_err(|e| e.to_string())
            })
        } else if sdr {
            neowon_backend::spawn(|| {
                Ok(Box::new(neowon_sim::SimSdrBackend::new()) as Box<dyn Backend>)
            })
        } else if self.audio {
            neowon_backend::spawn(|| {
                neowon_audio::AudioBackend::open().map(|b| Box::new(b) as Box<dyn Backend>)
            })
        } else if self.sim || self.sdr_sim {
            neowon_backend::spawn(
                || Ok(Box::new(neowon_sim::SimBackend::new()) as Box<dyn Backend>),
            )
        } else {
            neowon_backend::spawn(neowon_vds1022::backend::factory(None))
        }
    }

    /// Spawn the supervisor and send the first config. That config decides
    /// what the supervisor replays on every (re)connect, so it must match
    /// the instrument: an SDR refuses a scope config. Returns the scope
    /// config the UI starts from either way.
    pub fn start(&self) -> (Supervisor, ScopeConfig) {
        let sup = self.supervisor(self.sdr());

        // Defaults matched to the 1 kHz probe-comp signal through a x10 probe.
        let mut config = crate::view::startup_config();
        if self.demo {
            for ch in config.channels.iter_mut().take(2) {
                ch.enabled = true;
                ch.volts_div = 0.5; // wav full scale (+-2 V) fills the +-4-div window
            }
        }
        if self.sdr() {
            sup.apply(SdrConfig::default());
        } else {
            sup.apply(config.clone());
        }
        if self.demo {
            let name = if self.demo_slow {
                "quake-slow"
            } else {
                "quake"
            };
            let _ = sup.commands.send(Command::Stimulus(name.into()));
        }
        (sup, config)
    }
}
