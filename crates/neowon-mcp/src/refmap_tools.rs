//! MCP tools for the RF reference: known stations from the
//! reference store, their sources, and the operator's location. `location
//! ip` is deliberately **not** exposed: the consent belongs to the
//! operator at the screen. Every call goes through the script
//! grammar, so the MCP surface mirrors the verbs exactly.

use rmcp::{ErrorData, handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::Scope;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StationsParams {
    /// Case-insensitive text search over name, callsign, id, country and
    /// notes. Omit to keep the current filter.
    query: Option<String>,
    /// Restrict to the operator's location radius (sets the window's
    /// `near me` scope). Requires a location to have been set.
    near: Option<bool>,
    /// Restrict to the currently displayed IQ span.
    in_view: Option<bool>,
    /// One source only: wikidata, eibi, ourairports, fcc or fmlist.
    /// Omit (or `-`) for all.
    source: Option<String>,
    /// Most rows returned (default 25).
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StationTuneParams {
    /// Station key `source:id`, e.g. `wikidata:Q1001` (from `stations`).
    /// Tunes there and selects the fitting demodulator (WFM/AM/NFM).
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RefdbParams {
    /// `status` (read-only), `fetch`, `import` or `clear`.
    action: String,
    /// Source for fetch/import/clear.
    source: Option<String>,
    /// File for `import` (an OurAirports import takes a directory).
    path: Option<String>,
    /// Fetch radius in km; defaults to the operator's setting (150).
    radius_km: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LocationParams {
    /// Latitude with `lon` (WGS 84).
    lat: Option<f64>,
    /// Longitude with `lat`.
    lon: Option<f64>,
    /// A Maidenhead locator, e.g. `IN58` (instead of lat/lon).
    locator: Option<String>,
}

#[tool_router(router = refmap_router, vis = "pub(crate)")]
impl Scope {
    #[tool(description = "Known stations from the reference store (Wikidata, \
        EiBi, OurAirports, FCC, FMLIST — no network), filtered by text, \
        source, the location radius (`near`) or the displayed span \
        (`in_view`). Rows carry key (`source:id`), frequency, modulation, \
        service, distance km when a location is set, and whether a \
        scheduled station is on air now. These are reference rows, not the \
        operator's catalog.")]
    async fn stations(&self, p: Parameters<StationsParams>) -> Result<String, ErrorData> {
        if let Some(q) = p.0.query {
            self.req(&format!("stations find {}", q.trim()))?;
        }
        if let Some(s) = p.0.source {
            let s = s.trim();
            let value = if s.is_empty() || s == "-" { "-" } else { s };
            self.req(&format!("stations filter source {value}"))?;
        }
        let scope = if p.0.near.unwrap_or(false) {
            "near"
        } else if p.0.in_view.unwrap_or(false) {
            "view"
        } else {
            "all"
        };
        let limit = p.0.limit.unwrap_or(25).min(500);
        self.req(&format!("stations scope {scope}"))?;
        self.req(&format!("get stations {scope} {limit}"))
    }

    #[tool(description = "Tune the SDR to a known station by key \
        (`source:id` from `stations`, e.g. `wikidata:Q1001`) and select the \
        demodulator its modulation calls for (WFM broadcast, AM, NFM). An \
        out-of-band target recentres the hardware window. Files nothing \
        into the catalog.")]
    async fn station_tune(&self, p: Parameters<StationTuneParams>) -> Result<String, ErrorData> {
        self.req(&format!("stations tune {}", p.0.id.trim()))?;
        let sdr = self.req("get sdr")?;
        let audio = self.req("get audio")?;
        Ok(format!("{sdr}\n{audio}"))
    }

    #[tool(description = "The reference database: `status` lists each \
        source's snapshot (count, fetched date, origin, licence) plus the \
        running job; `fetch` downloads a source (needs a location for \
        Wikidata/FCC, waits until the job finishes); `import` reads a local \
        export (FMLIST is import-only; OurAirports takes a directory with \
        airport-frequencies.csv and airports.csv); `clear` drops one \
        source's snapshot. Reference data never touches the catalog.")]
    async fn refdb(&self, p: Parameters<RefdbParams>) -> Result<String, ErrorData> {
        let src = p.0.source.as_deref().unwrap_or("").trim();
        match p.0.action.trim().to_lowercase().as_str() {
            "status" => self.req("get refdb"),
            "fetch" => {
                let radius = p.0.radius_km.map(|r| format!(" {r}")).unwrap_or_default();
                self.req(&format!("refdb fetch {src}{radius}"))?;
                self.wait_idle(90)
            }
            "import" => {
                let path = p.0.path.as_deref().unwrap_or("").trim();
                if path.is_empty() {
                    return Err(ErrorData::invalid_params("import needs `path`", None));
                }
                self.req(&format!("refdb import {src} {path}"))?;
                self.wait_idle(60)
            }
            "clear" => {
                self.req(&format!("refdb clear {src}"))?;
                self.req("get refdb")
            }
            other => Err(ErrorData::invalid_params(
                format!("unknown action {other:?} (status|fetch|import|clear)"),
                None,
            )),
        }
    }

    #[tool(description = "The operator's location (D18), used to rank and \
        radius-filter stations: set it with `lat` + `lon`, or a Maidenhead \
        `locator` (e.g. `IN58`), or call with no arguments to read it back \
        (locator and how the fix was made). The IP lookup is not exposed \
        here — that consent belongs to the operator at the screen; ask \
        them to press `Locate me`.")]
    async fn location(&self, p: Parameters<LocationParams>) -> Result<String, ErrorData> {
        if let Some(locator) =
            p.0.locator
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        {
            self.req(&format!("location {locator}"))?;
        } else if let (Some(lat), Some(lon)) = (p.0.lat, p.0.lon) {
            self.req(&format!("location {lat} {lon}"))?;
        }
        self.req("get location")
    }
}

impl Scope {
    /// Poll `get refdb` until no job is running (or `secs` pass), so a
    /// fetch/import call returns the finished report instead of a promise.
    fn wait_idle(&self, secs: u64) -> Result<String, ErrorData> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        loop {
            let db = self.req("get refdb")?;
            let idle = serde_json::from_str::<Value>(&db)
                .map(|v| v["job"].is_null())
                .unwrap_or(false);
            if idle || std::time::Instant::now() >= deadline {
                return Ok(db);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}
