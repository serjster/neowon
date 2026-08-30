//! neowon-mcp: an MCP stdio server that drives a running neowon app
//! through its control socket (`NEOWON_CONTROL`). All scope logic lives in
//! the app; this crate is a curated, schema-described façade over the
//! script grammar plus the `get …` queries.
//!
//! Usage:
//!   neowon-mcp --connect 127.0.0.1:7777   # attach to a running app
//!   neowon-mcp --spawn-sim                # spawn `neowon-app --sim` itself

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;

/// Line client for the app's control socket.
struct ScopeClient {
    out: TcpStream,
    lines: std::io::Lines<BufReader<TcpStream>>,
    /// Keeps a spawned `neowon-app --sim` alive (killed on drop).
    child: Option<std::process::Child>,
}

impl ScopeClient {
    fn connect(addr: &str, deadline: Duration) -> std::io::Result<TcpStream> {
        let until = Instant::now() + deadline;
        loop {
            match TcpStream::connect(addr) {
                Ok(s) => return Ok(s),
                Err(e) if Instant::now() >= until => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    }

    fn attach(addr: &str, child: Option<std::process::Child>) -> std::io::Result<Self> {
        let stream = Self::connect(addr, Duration::from_secs(20))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        Ok(Self {
            out: stream.try_clone()?,
            lines: BufReader::new(stream).lines(),
            child,
        })
    }

    fn request(&mut self, line: &str) -> std::io::Result<String> {
        writeln!(self.out, "{line}")?;
        self.lines
            .next()
            .ok_or_else(|| std::io::Error::other("control connection closed"))?
    }
}

impl Drop for ScopeClient {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Find the neowon-app binary next to our own executable (same target
/// dir), or take `NEOWON_APP_BIN`.
fn app_binary() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("NEOWON_APP_BIN") {
        return p.into();
    }
    let mut p = std::env::current_exe().unwrap_or_default();
    p.set_file_name("neowon-app");
    p
}

fn spawn_sim() -> std::io::Result<ScopeClient> {
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let child = std::process::Command::new(app_binary())
        .arg("--sim")
        .env("NEOWON_CONTROL", port.to_string())
        .env_remove("NEOWON_SCRIPT")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    ScopeClient::attach(&format!("127.0.0.1:{port}"), Some(child))
}

// ---------------------------------------------------------------- tools

#[derive(Deserialize, JsonSchema)]
struct ChannelParams {
    /// Channel index: 0 = CH1, 1 = CH2.
    ch: usize,
    /// Enable or disable the channel.
    enabled: Option<bool>,
    /// Volts per division at the input (e.g. 0.2). Hardware ladders:
    /// 5 mV…5 V in 1-2-5 steps.
    volts_div: Option<f64>,
    /// Coupling: "dc", "ac", or "gnd".
    coupling: Option<String>,
    /// Probe attenuation factor (1, 10, 20, 50, 100, 500, 1000).
    probe: Option<f64>,
    /// Vertical offset as a fraction of full scale, -0.5..=0.5.
    offset: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
struct TriggerParams {
    /// Trigger source channel (0 or 1).
    source: usize,
    /// Edge slope: "rising" or "falling".
    slope: String,
    /// Trigger level in volts at the probe tip.
    level: f64,
    /// Sweep mode: "auto", "normal", or "single".
    sweep: String,
}

#[derive(Deserialize, JsonSchema)]
struct HorizontalParams {
    /// Sample rate in S/s (hardware ladder 2.5 S/s … 100 MS/s in
    /// 1-2.5-5 steps; e.g. 250000).
    sample_rate: Option<f64>,
    /// Horizontal trigger position as a fraction of the record
    /// (0.5 = centered).
    trigger_position: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
struct RunParams {
    /// true = run continuous acquisition, false = stop.
    on: bool,
}

#[derive(Deserialize, JsonSchema)]
struct StimulusParams {
    /// Stimulus preset name (simulated backend only), e.g. "probe-comp",
    /// "xy-circle", "sweep", "am", "quake".
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct ScriptParams {
    /// neowon automation script text: one action per line, the same
    /// grammar NEOWON_SCRIPT files and session files use. Covers every
    /// control (channels, triggers incl. pulse/slope/video, math, FFT,
    /// pass/fail, recording, history, sessions, screenshots …).
    script: String,
}

#[derive(Deserialize, JsonSchema)]
struct ScreenshotParams {
    /// Optional region of interest in plot-texture pixels (1000×500):
    /// [x, y, width, height].
    roi: Option<[u32; 4]>,
}

#[derive(Clone)]
struct Scope {
    client: std::sync::Arc<Mutex<ScopeClient>>,
    tool_router: ToolRouter<Self>,
}

impl Scope {
    fn req(&self, line: &str) -> Result<String, ErrorData> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| ErrorData::internal_error("client poisoned", None))?;
        let reply = client
            .request(line)
            .map_err(|e| ErrorData::internal_error(format!("control socket: {e}"), None))?;
        if reply.contains(r#""ok":false"#) {
            return Err(ErrorData::invalid_params(reply, None));
        }
        Ok(reply)
    }

    fn exec_lines(&self, script: &str) -> Result<String, ErrorData> {
        let mut n = 0;
        for line in script.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            self.req(line)?;
            n += 1;
        }
        Ok(format!(r#"{{"ok":true,"applied":{n}}}"#))
    }
}

#[tool_router(router = tool_router)]
impl Scope {
    #[tool(description = "Scope status: running state, backend, frame count, \
        recorder/history state, active stimulus.")]
    async fn scope_status(&self) -> Result<String, ErrorData> {
        self.req("get status")
    }

    #[tool(description = "Full instrument configuration: sample rate, trigger \
        (kind, level, sweep), per-channel settings, display, math/FFT/pass-fail.")]
    async fn scope_config(&self) -> Result<String, ErrorData> {
        self.req("get config")
    }

    #[tool(description = "Automatic measurements for CH1, CH2, and the math \
        trace: 18 metrics (Freq, Period, Vpp, Vmax/min/top/base/amp/avg/rms, \
        Rise, Fall, ±Width, ±Duty, Over/Preshoot) with running statistics.")]
    async fn measurements(&self) -> Result<String, ErrorData> {
        self.req("get measure")
    }

    #[tool(description = "Configure a channel: enable, volts/div, coupling, \
        probe factor, vertical offset. Only the fields you pass are changed.")]
    async fn configure_channel(&self, p: Parameters<ChannelParams>) -> Result<String, ErrorData> {
        let p = p.0;
        let mut script = String::new();
        if let Some(on) = p.enabled {
            script.push_str(&format!("enable {} {}\n", p.ch, on as u8));
        }
        if let Some(v) = p.volts_div {
            script.push_str(&format!("vdiv {} {v}\n", p.ch));
        }
        if let Some(c) = &p.coupling {
            script.push_str(&format!("coupling {} {c}\n", p.ch));
        }
        if let Some(f) = p.probe {
            script.push_str(&format!("probe {} {f}\n", p.ch));
        }
        if let Some(o) = p.offset {
            script.push_str(&format!("offset {} {o}\n", p.ch));
        }
        self.exec_lines(&script)
    }

    #[tool(description = "Configure an edge trigger (source, slope, level, \
        sweep). For pulse/slope/video triggers use exec_script with the \
        trigpulse/trigslope/trigvideo actions.")]
    async fn configure_trigger(&self, p: Parameters<TriggerParams>) -> Result<String, ErrorData> {
        let p = p.0;
        self.req(&format!(
            "trigger {} {} {} {}",
            p.source, p.slope, p.level, p.sweep
        ))
    }

    #[tool(description = "Configure the horizontal system: sample rate and/or \
        horizontal trigger position.")]
    async fn configure_horizontal(
        &self,
        p: Parameters<HorizontalParams>,
    ) -> Result<String, ErrorData> {
        let p = p.0;
        let mut script = String::new();
        if let Some(r) = p.sample_rate {
            script.push_str(&format!("rate {r}\n"));
        }
        if let Some(t) = p.trigger_position {
            script.push_str(&format!("trigpos {t}\n"));
        }
        self.exec_lines(&script)
    }

    #[tool(description = "Start or stop continuous acquisition.")]
    async fn run(&self, p: Parameters<RunParams>) -> Result<String, ErrorData> {
        self.req(&format!("run {}", p.0.on as u8))
    }

    #[tool(description = "Auto-set: measure the signal and pick sensible \
        vertical/horizontal/trigger settings automatically.")]
    async fn autoset(&self) -> Result<String, ErrorData> {
        self.req("autoset")
    }

    #[tool(description = "Select a stimulus preset (simulated backend only): \
        probe-comp, sine variants, sweeps, AM/FM, XY figures (xy-circle, \
        xy-lissajous, xy-heart …), quake. See scope_status for the active one.")]
    async fn set_stimulus(&self, p: Parameters<StimulusParams>) -> Result<String, ErrorData> {
        self.req(&format!("stimulus {}", p.0.name))
    }

    #[tool(description = "Run raw neowon automation script text (the same \
        grammar as NEOWON_SCRIPT and session files) — the escape hatch that \
        reaches every control: advanced triggers, math, FFT, pass/fail, \
        recording, capture save/load, history scrubbing, sessions.")]
    async fn exec_script(&self, p: Parameters<ScriptParams>) -> Result<String, ErrorData> {
        self.exec_lines(&p.0.script)
    }

    #[tool(description = "Capture the scope display (the waveform plot) as a \
        PNG image, optionally cropped to a region of interest.")]
    async fn screenshot(
        &self,
        p: Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let path = std::env::temp_dir().join(format!("neowon-mcp-shot-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let roi =
            p.0.roi
                .map(|[x, y, w, h]| format!(" {x} {y} {w} {h}"))
                .unwrap_or_default();
        self.req(&format!("shot {}{roi}", path.display()))?;
        // The shot is written after a GPU readback; wait for the file.
        let deadline = Instant::now() + Duration::from_secs(5);
        let bytes = loop {
            match std::fs::read(&path) {
                Ok(b) if !b.is_empty() => break b,
                _ if Instant::now() >= deadline => {
                    return Err(ErrorData::internal_error("screenshot timed out", None));
                }
                _ => std::thread::sleep(Duration::from_millis(100)),
            }
        };
        let _ = std::fs::remove_file(&path);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(CallToolResult::success(vec![ContentBlock::image(
            b64,
            "image/png",
        )]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Scope {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Remote control of a neowon oscilloscope (OWON VDS1022 or its \
                 deterministic simulator). Typical flow: scope_status → \
                 configure_channel/configure_trigger → measurements → \
                 screenshot (returns an image of the display). exec_script \
                 reaches every remaining control with the documented neowon \
                 script grammar.",
        )
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let client = if let Some(i) = args.iter().position(|a| a == "--connect") {
        let addr = args
            .get(i + 1)
            .ok_or("--connect needs an ADDR:PORT argument")?;
        ScopeClient::attach(addr, None)?
    } else if args.iter().any(|a| a == "--spawn-sim") {
        spawn_sim()?
    } else {
        eprintln!("usage: neowon-mcp --connect ADDR:PORT | --spawn-sim");
        std::process::exit(2);
    };

    let scope = Scope {
        client: std::sync::Arc::new(Mutex::new(client)),
        tool_router: Scope::tool_router(),
    };
    let service = scope.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
