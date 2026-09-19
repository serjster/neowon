//! MCP tools for SDR mode and the signal catalog: thin wrappers over the
//! control socket's `get sdr|detections|modmeas|catalog|history` queries
//! and `sdr …` / `catalog …` commands, so the MCP surface mirrors every
//! script action.

use rmcp::{ErrorData, handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::Scope;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TuneParams {
    /// Tuned frequency, Hz: the channel to monitor (0.5 MHz – 1.766 GHz;
    /// below 24 MHz uses the RTL-SDR V3's HF direct-sampling input). The
    /// hardware window does not move unless Follow is on; move it with
    /// `sdr centre <hz>`, and set the channel width with `sdr width`.
    tuned_hz: f64,
    /// IQ sample rate, pairs/s (one of the instrument's offered rates).
    sample_rate: Option<f64>,
    /// Manual tuner gain in dB; omit to leave it, or use `gain_auto`.
    gain_db: Option<f64>,
    /// Hand gain to the tuner's AGC.
    gain_auto: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistoryParams {
    /// Signal id (`#12` or `12`); merged-away ids resolve to the
    /// surviving signal.
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CatalogListParams {
    /// Case-insensitive name/tag/alias filter; empty lists everything.
    filter: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CatalogCommandParams {
    /// One catalog verb and its arguments, as in scripts: `add [hz name]`,
    /// `observe`, `rename <id> <name>`, `alias <id> <name>`, `tag|untag
    /// <id> <tag>`, `pin|unpin <id>`, `edit <id> <field> <value>`,
    /// `merge <from> <to>`, `delete <id> [cascade]`, `purge <ids>
    /// [cascade]`, `bulk tag|untag|pin|unpin|delete <ids> [arg]`, `undo`,
    /// `export <path>`, `import <path>`.
    command: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SurveyParams {
    /// Range to sweep, Hz.
    start_hz: f64,
    stop_hz: f64,
    /// Most peaks kept per tuning step (default 32); beyond it the step
    /// is marked truncated and weaker signals there read `unknown`.
    peak_cap: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InstrumentParams {
    /// `scope` or `sdr`. Switches within the launch's family: simulator ↔
    /// simulator, VDS1022 ↔ RTL-SDR. Each instrument keeps its settings.
    instrument: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DemodParams {
    /// `off`, `am`, `nfm` or `wfm`.
    demod: String,
    /// Audio volume 0..1.
    volume: Option<f32>,
    /// Mute without changing the demodulator.
    mute: Option<bool>,
    /// Squelch threshold in dBFS; `null` leaves the gate open.
    squelch_db: Option<f64>,
}

#[tool_router(router = sdr_router, vis = "pub(crate)")]
impl Scope {
    #[tool(description = "SDR mode status and settings: backend, tuner, the \
        tuned frequency and the hardware window centre, rate, gain, AGC, ppm, \
        span, channel width, the strongest displayed peak and the noise floor.")]
    async fn sdr_status(&self) -> Result<String, ErrorData> {
        self.req("get sdr")
    }

    #[tool(description = "Switch the app between its two instruments, the \
        oscilloscope (`scope`) and the SDR (`sdr`).")]
    async fn instrument(&self, p: Parameters<InstrumentParams>) -> Result<String, ErrorData> {
        self.req(&format!("instrument {}", p.0.instrument.trim()))
    }

    #[tool(description = "Tune the SDR's tuned frequency (the channel you \
        monitor) and optionally set rate and gain. The hardware window stays \
        put; use `sdr centre <hz>` to move it and `sdr follow on` to keep it \
        centred on the tuned frequency.")]
    async fn sdr_tune(&self, p: Parameters<TuneParams>) -> Result<String, ErrorData> {
        let p = p.0;
        if let Some(r) = p.sample_rate {
            self.req(&format!("sdr rate {r}"))?;
        }
        if p.gain_auto == Some(true) {
            self.req("sdr gain auto")?;
        } else if let Some(g) = p.gain_db {
            self.req(&format!("sdr gain {g}"))?;
        }
        self.req(&format!("sdr tune {}", p.tuned_hz))
    }

    #[tool(description = "Signals the detector is tracking now (debounced), \
        strongest first: centre, 99% occupied bandwidth, power, SNR, first/last seen.")]
    async fn sdr_detections(&self) -> Result<String, ErrorData> {
        self.req("get detections")
    }

    #[tool(
        description = "Measurements of the strongest tracked signal (occupied \
        bandwidth, channel power, SNR, spectral flatness) and, once `sdr analyse \
        on` has run, the modulation lab's results for the signal nearest the \
        tuned frequency: modulation (auto or set with `sdr modulation`), symbol \
        rate, EVM, MER and cumulants C20–C63."
    )]
    async fn sdr_modmeas(&self) -> Result<String, ErrorData> {
        self.req("get modmeas")
    }

    #[tool(
        description = "Start a survey: sweep start_hz..stop_hz in tuning steps, \
        keeping the strongest peaks per step. Poll sdr_survey_result until \
        `running` is false."
    )]
    async fn sdr_survey(&self, p: Parameters<SurveyParams>) -> Result<String, ErrorData> {
        let p = p.0;
        self.req(&format!(
            "sdr survey {} {} cap {}",
            p.start_hz,
            p.stop_hz,
            p.peak_cap.unwrap_or(32)
        ))
    }

    #[tool(
        description = "The latest survey (coverage per step: scanned, truncated, \
        kept-power floor; peaks) and, when two have completed, their diff: each \
        signal new / gone / stronger / weaker / same, or unknown where a survey \
        could not have seen it."
    )]
    async fn sdr_survey_result(&self) -> Result<String, ErrorData> {
        let survey = self.req("get survey")?;
        let diff = self.req("get surveydiff").unwrap_or_else(|_| "null".into());
        Ok(format!(r#"{{"survey":{survey},"diff":{diff}}}"#))
    }

    #[tool(
        description = "The DSP classifier's verdict on the signal nearest the \
        tuned frequency (noise, cw, am, fm, bpsk, qpsk, 8psk, 16qam, 64qam): \
        label, confidence, trust (unproven until an over-the-air evaluation), \
        unknown, top-2 margin. Needs `sdr analyse on`."
    )]
    async fn sdr_classify(&self) -> Result<String, ErrorData> {
        self.req("get classify")
    }

    #[tool(description = "Audio: the demodulator (off/am/nfm/wfm), the output \
        device, and the one-line state — playing, muted, squelched, no device, \
        starting or off — with volume, squelch, channel power and RMS.")]
    async fn sdr_audio(&self) -> Result<String, ErrorData> {
        self.req("get audio")
    }

    #[tool(description = "Set the audio demodulator and optionally volume, mute \
        and squelch. `off` stops demodulation. The channel is D10's tuned \
        frequency with its Width.")]
    async fn sdr_demod(&self, p: Parameters<DemodParams>) -> Result<String, ErrorData> {
        let p = p.0;
        self.req(&format!("sdr demod {}", p.demod.trim()))?;
        if let Some(v) = p.volume {
            self.req(&format!("sdr volume {v}"))?;
        }
        if let Some(m) = p.mute {
            self.req(&format!("sdr mute {}", if m { "on" } else { "off" }))?;
        }
        if let Some(db) = p.squelch_db {
            self.req(&format!("sdr squelch {db}"))?;
        }
        self.req("get audio")
    }

    #[tool(description = "List catalogued signals (id, name, frequency, \
        bandwidth, tags, aliases, pinned, observation count) and catalog health.")]
    async fn catalog_list(&self, p: Parameters<CatalogListParams>) -> Result<String, ErrorData> {
        self.req(&format!("catalog list {}", p.0.filter.unwrap_or_default()))?;
        self.req("get catalog")
    }

    #[tool(description = "A catalogued signal's observation history, in time \
        order, following merges to the surviving signal.")]
    async fn catalog_history(&self, p: Parameters<HistoryParams>) -> Result<String, ErrorData> {
        self.req(&format!("get history {}", p.0.id))
    }

    #[tool(description = "Run one catalog command (add, observe, rename, alias, \
        tag, pin, edit, merge, delete, purge, bulk, undo, export, import). \
        Refusals (pinned, referenced, unknown id) come back on the status line \
        and via catalog_list.")]
    async fn catalog(&self, p: Parameters<CatalogCommandParams>) -> Result<String, ErrorData> {
        self.req(&format!("catalog {}", p.0.command.trim()))
    }
}
