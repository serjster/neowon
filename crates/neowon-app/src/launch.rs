//! Command-line choice of instrument, and the supervisor that drives it.
//!
//! `--sim`: the simulated scope; `--demo [slow]`: oscilloscope-music XY
//! playback on it (assets/demo, from lofibucket.com's Oscilloscope Quake);
//! `--audio`: the sound
//! card as a streaming scope; `--sdr-sim`: the simulated SDR; `--rtl`: an
//! RTL-SDR dongle; nothing: the VDS1022 (hardware).

use neowon_backend::{Backend, Command, ScopeConfig, SdrConfig, Supervisor};

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

    /// An SDR instrument was chosen.
    pub fn sdr(&self) -> bool {
        self.sdr_sim || self.rtl
    }

    /// Spawn the supervisor and send the first config. That config decides
    /// what the supervisor replays on every (re)connect, so it must match
    /// the instrument: an SDR refuses a scope config. Returns the scope
    /// config the UI starts from either way.
    pub fn start(&self) -> (Supervisor, ScopeConfig) {
        let sup = if self.sdr_sim {
            neowon_backend::spawn(|| {
                Ok(Box::new(neowon_sim::SimSdrBackend::new()) as Box<dyn Backend>)
            })
        } else if self.rtl {
            neowon_backend::spawn(|| {
                neowon_sdr::RtlBackend::open(None)
                    .map(|b| Box::new(b) as Box<dyn Backend>)
                    .map_err(|e| e.to_string())
            })
        } else if self.audio {
            neowon_backend::spawn(|| {
                neowon_audio::AudioBackend::open().map(|b| Box::new(b) as Box<dyn Backend>)
            })
        } else if self.sim {
            neowon_backend::spawn(
                || Ok(Box::new(neowon_sim::SimBackend::new()) as Box<dyn Backend>),
            )
        } else {
            neowon_backend::spawn(neowon_vds1022::backend::factory(None))
        };

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
