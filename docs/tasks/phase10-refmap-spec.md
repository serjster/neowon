# Phase 10.14 — RF reference: band map, band overlay, station database

Operator request (2026-09-19): an RF spectrum map of the whole tunable range,
laid out like the classic band charts, where a click on a region tunes there;
an SDR++-style overlay naming the band the SDR is in; and a downloadable
database of known stations by geography — name, frequency, modulation —
overlaid on the spectrum, browsable, click-to-tune, **stored separately from
the user catalog**. Location only with the operator's permission.

Read `PLAN.md` §4 (Phase 10 status) and `docs/tasks/phase10-sdr-spec.md`
(D10 centre vs tuned, D12 workspaces, D13 persistence) first. This spec
extends Phase 10; it does not change the backend abstraction.

## Decisions (user-approved 2026-09-19)

- **D16 — band plans use the SDR++ JSON schema, as-is.** `{name,
  country_name, country_code, author_name, author_url, bands:[{name, type,
  start, end}]}`, Hz, `type` ∈ SDR++'s vocabulary (`broadcast`, `amateur`,
  `aviation`, `marine`, `military`, `utility`, …; unknown types render grey,
  never rejected). Shipped: SDR++'s `general.json` (worldwide) plus its
  country plans, copied from `tmp-inspiration/SDRPlusPlus/root/res/bandplans/`
  into `assets/bandplans/` with SDR++'s GPL-3 notice in
  `assets/bandplans/README.md` (licence is not a criterion — operator,
  10.0). Operator plans drop into `~/.neowon/bandplans/*.json` and shadow a
  shipped plan of the same file name. User-catalog `BandPlanEntry` entities
  (neowon-catalog) draw **over** the active plan, marked as the operator's.
  No Portugal plan ships; `general` is the fallback and the operator may
  author one in the same format.
- **D17 — station sources: Wikidata, EiBi, OurAirports, FMLIST, FCC.** One
  importer per source, each a pure `parse(&[u8]) -> Vec<Station>` plus an
  optional fetch URL. Wikidata (CC0; broadcast FM/AM/TV; fetched by radius
  around the location via SPARQL), EiBi (shortwave schedule CSV, season
  A/B chosen from the date), OurAirports (public domain; airport
  frequencies joined to airport coordinates), FCC (US public domain; FM/AM
  `fmq`/`amq` pipe-delimited query output, fetched by radius), FMLIST
  (**import only** — the operator exports it with their own account; the
  importer maps columns by header name; format confirmed from the
  operator's first file, recorded under Deviations).
- **D18 — location: manual, or IP lookup on explicit consent.** Lat/lon or
  a Maidenhead locator, typed (no network). `Locate me` performs **one**
  HTTPS request to `https://ipapi.co/json/` only after a dialog naming the
  service and what it learns (your public IP → approximate city); the script
  line `location ip` is itself the explicit consent. Stored in
  `~/.neowon/location.json` `{lat, lon, country_code?, source:
  manual|locator|ip, set_at}`. No location ⇒ stations are shown unranked and
  unfiltered by distance, and the band plan stays on the last chosen one.
  Never a background or startup lookup.
- **D19 — HTTP: `ureq` 3 (rustls), confined to `neowon-refdb`.** Blocking,
  no tokio, runs on a worker thread the app spawns per fetch. Only
  `neowon-refdb::fetch` imports it. Network happens **only** on an explicit
  `refdb fetch` / `location ip` action — never at startup, never in tests.
  CSV is parsed by hand (no `csv` crate); JSON via the existing
  `serde_json`.
- **D20 — the reference DB is not the catalog.** It lives in
  `~/.neowon/refdb/` (override `NEOWON_REFDB`), one snapshot per source,
  replaced wholesale on each fetch/import; the app never writes it
  anywhere else and neowon-catalog never reads it. The only bridge is an
  explicit **Add to catalog** on a station, which files a `Signal` with
  provenance `reference` and `input_ref = refdb:<source>:<id>` — a copy,
  not a link.
- **D21 — tuning from the map.** A click on a band (map window or strip)
  sets Tuned to the clicked frequency; the D10 amendment (2026-09-19,
  out-of-band targets recentre the hardware) moves the window when needed.
  A click on a station tunes to it **and** selects a demodulator when one
  fits (`WFM` broadcast → `wfm`, `AM`/aviation → `am`, `NFM` → `nfm`;
  others leave demod unchanged — SSB/CW are deferred per 10.10), emitted as
  the equivalent script lines so the parity rule holds.

## Scope fence

- New crate `crates/neowon-refdb/` (engine-free: serde, serde_json,
  thiserror, ureq). No Bevy.
- `crates/neowon-app/`: new `src/refmap/` (state, actions, readout) and new
  UI files `src/ui/sdr_bandstrip.rs`, `src/ui/bandmap_window.rs`,
  `src/ui/stations_window.rs`. Touch `sdr_view.rs` only to reserve the strip
  rect and call the overlays; `ui.rs` **may not grow** (over-budget debt).
- `crates/neowon-mcp/`: one new tool file `src/refmap_tools.rs`.
- `assets/bandplans/` (JSON + README).
- Off limits: `neowon-catalog` internals (use its public API for Add to
  catalog), `neowon-sdr`, `neowon-dsp`, every backend crate.

## Hard rules

- AGENTS.md hardware rule: all app runs `--sim`/`--sdr-sim`; no USB.
- **No network in any test.** Fetch is tested against a
  `std::net::TcpListener` stub on 127.0.0.1 serving fixture bytes; the fetch
  API takes the base URL as a parameter so tests can point it there.
- Fixtures are small text excerpts under `crates/neowon-refdb/tests/fixtures/`
  (≤ 50 rows each, source + licence noted in a README there). No binary
  blobs.
- Deterministic: no wall-clock in parsing; EiBi "on air now" takes the time
  as a parameter; the app passes the clock, tests pass a fixed instant.
- File budgets per AGENTS.md (≤ 500 soft / 700 hard).
- Every UI control has a script action; every action round-trips through
  `Display`/`parse` (extend `every_action_round_trips`).

## Existing code

- `SdrState` (`neowon-app/src/sdr/mod.rs`): `config.centre_hz`, `tuned_hz`,
  `span()`, `view_centre()`, `set_tuned()` (recentres beyond ±45% of the
  rate — `TUNE_REACH`), `demod`.
- `ui/sdr_view.rs::show` splits `plot ∪ descriptors` into spectrum (45%)
  and waterfall; `freq_at`/`x_at` map x ↔ Hz; `draw_channel` paints the
  tuned cursor. `ui/catalog_window.rs` is the model for a browsable,
  click-to-tune window (it injects `SdrAction::Tune`).
- Script: `crate::script` grammar; SDR verbs in `sdr/actions.rs`; control
  socket `get …` readouts in `sdr/readout.rs` (hand-emitted JSON).
- `neowon-catalog::BandPlanEntry {lo_hz, hi_hz, service, designator, …}`.
- SDR++ band plan widget for reference behaviour:
  `tmp-inspiration/SDRPlusPlus/core/src/gui/widgets/bandplan.cpp` and
  `…/gui/menus/bandplan.cpp` (colours per type, label fitting).

## Work items

### 10.14.1 — `neowon-refdb` core (no network)

```rust
pub struct BandPlan { pub name: String, pub country_code: String, pub bands: Vec<Band> }
pub struct Band { pub name: String, pub kind: String, pub lo_hz: f64, pub hi_hz: f64 }
impl BandPlan {
    pub fn from_sdrpp_json(bytes: &[u8]) -> Result<Self, Error>;
    pub fn at(&self, hz: f64) -> impl Iterator<Item = &Band>;          // overlapping bands allowed
    pub fn within(&self, lo: f64, hi: f64) -> impl Iterator<Item = &Band>;
}
pub fn load_plans(shipped: &Path, user: &Path) -> Vec<(String, BandPlan)>; // user shadows by file name

pub enum Modulation { Am, Fm, Wfm, Nfm, Usb, Lsb, Cw, Dab, Dvbt, Atsc, Digital, Unknown }
pub enum Service { Broadcast, Aviation, Marine, Amateur, Utility, Other }
pub struct Station {
    pub source: Source, pub id: String, pub name: String, pub freq_hz: f64,
    pub bandwidth_hz: Option<f64>, pub modulation: Modulation, pub service: Service,
    pub lat: Option<f64>, pub lon: Option<f64>, pub country: Option<String>,
    pub callsign: Option<String>, pub power_kw: Option<f64>,
    pub schedule: Option<Schedule>,       // EiBi: UTC start/stop minutes, weekday mask
    pub notes: String,
}
pub enum Source { Wikidata, Eibi, OurAirports, Fcc, Fmlist }

pub mod geo {
    pub struct LatLon { pub lat: f64, pub lon: f64 }
    pub fn haversine_km(a: LatLon, b: LatLon) -> f64;
    pub fn from_locator(s: &str) -> Result<LatLon, Error>;   // 4/6/8-char Maidenhead, square centre
    pub fn to_locator(p: LatLon, chars: usize) -> String;
}

pub struct Index { /* stations sorted by freq_hz */ }
impl Index {
    pub fn within(&self, lo: f64, hi: f64) -> &[Station];     // binary search
    pub fn query(&self, q: &Query) -> Vec<(&Station, Option<f64>)>; // (station, km)
}
pub struct Query { pub text: String, pub lo_hz: f64, pub hi_hz: f64, pub near: Option<LatLon>,
                   pub radius_km: Option<f64>, pub on_air_at: Option<(u8 /*weekday*/, u16 /*utc min*/)>,
                   pub sources: Vec<Source>, pub sort: SortBy }

pub struct Store { dir: PathBuf }   // ~/.neowon/refdb or NEOWON_REFDB
impl Store {
    pub fn open(dir: &Path) -> Result<Self, Error>;
    pub fn replace(&self, src: Source, stations: &[Station], meta: &Meta) -> Result<(), Error>; // atomic .tmp+rename
    pub fn load_all(&self) -> Result<(Index, Vec<Meta>), Error>;
    pub fn clear(&self, src: Source) -> Result<(), Error>;
}
pub struct Meta { pub source: Source, pub count: usize, pub fetched_at: String, pub origin: String, pub licence: String }
```

Files: `<dir>/<source>.json` (stations) + `<dir>/meta.json`. Location lives
next to it in `~/.neowon/location.json` (`geo::Location` load/save).

**Done when:** `cargo test -p neowon-refdb` — band plan: every shipped
`assets/bandplans/*.json` parses, `general.at(99.4e6)` names the FM
broadcast band, overlapping bands both returned (exact); locator:
`IN58` ↔ lat/lon round trip and a known reference point within 1 km
(tolerance); haversine Lisbon↔Porto 274 ± 2 km; index range query equals a
linear-scan oracle on a seeded random set (exact); store replace/load round
trip is field-exact and a crashed `.tmp` leaves the previous snapshot
readable.

### 10.14.2 — Importers (`neowon-refdb::sources`, no network)

One module per source, pure `parse`:

- `wikidata` — SPARQL JSON results (`application/sparql-results+json`).
  Query (built by `wikidata::query(center, radius_km)`): items that are
  instances of (subclasses of) radio or TV station with `P2144` frequency
  (normalised through its unit: Hz/kHz/MHz/GHz item), `P625` inside
  `wikibase:around` the location, optional `P17`, `P1920`/callsign `P2317`,
  labels in the operator's language then English. Modulation inferred:
  88–108 MHz → `Wfm`, 148.5 kHz–1.7 MHz → `Am`, TV bands → `Dvbt`, else
  `Unknown`.
- `eibi` — `sked-<a|b><yy>.csv`, `;`-separated, Latin-1: `kHz;Time(UTC);
  Days;ITU;Station;Lng;Target;Remarks;P;Start;Stop`. Modulation `Am`
  (remarks may mark `USB`/`DRM` → `Usb`/`Digital`); `Schedule` from
  Time/Days. `eibi::season_file(date)` picks A (last Sun Mar → last Sun Oct)
  or B.
- `ourairports` — `airport-frequencies.csv` joined on `airport_ref` with
  `airports.csv` for name/lat/lon/country; modulation `Am`, service
  `Aviation`, name `"<ICAO> <type> — <airport>"`. Parse takes both files.
- `fcc` — `fmq`/`amq` pipe-delimited output (`list=4`); lat/lon from the
  DMS fields; ERP kW; `Wfm`/`Am`.
- `fmlist` — header-mapped CSV; unknown columns ignored; missing
  frequency ⇒ row skipped and counted in the import report.

Each importer returns `(Vec<Station>, Report { rows, kept, skipped:
Vec<(line, reason)> })`; the report reaches the UI status line and `get refdb`.

**Done when:** a fixture per source parses to a golden count and to
field-exact golden stations for 3 hand-checked rows each; malformed rows are
skipped with a reason, never a panic (fuzz-ish: every fixture truncated at
every line boundary parses without panicking).

### 10.14.3 — Fetch + location (`neowon-refdb::fetch`, the only network code)

```rust
pub fn fetch(src: Source, at: &FetchTarget, base: &BaseUrls) -> Result<(Vec<Station>, Report), Error>;
pub fn locate_ip(base: &str) -> Result<Location, Error>;   // ipapi.co/json
```

`FetchTarget { center: Option<LatLon>, radius_km: f64, date: (i32, u8, u8) }`.
Wikidata/FCC require a centre (error names `location` otherwise); EiBi and
OurAirports fetch whole files (~3 MB / ~10 MB) and filter nothing at fetch
time. Timeouts 30 s, one retry, User-Agent `neowon/<version>`. Wikidata
requests respect its User-Agent policy.

**Done when:** each fetch path, pointed at a local `TcpListener` stub,
returns the same stations as `parse` on the served fixture (exact); an HTTP
500 and a truncated body produce errors with the source named; `locate_ip`
parses the stub's JSON into `Location{source: Ip}`.

### 10.14.4 — Band strip + band overlay (app)

- `refmap::RefMap` resource: loaded plans, active plan name, location,
  `Index`, fetch job (worker thread + `crossbeam_channel` result), toggles.
  Loaded at startup from disk only.
- **Band strip**: an 18 px strip between the spectrum's top and the axis,
  published as ROI `sdr_bandstrip`. Segments of `plan.within(view)` + user
  catalog band entries, coloured by `kind` (SDR++ palette), labels drawn
  only when they fit (else truncated with …, else omitted), hover tooltip
  `name · lo–hi · kind`, click = tune to the pointer's frequency (D21).
- **Band overlay** (SDR++ style): a translucent badge at the spectrum's
  top-left naming every band containing Tuned — `2m Ham Band · 144–146
  MHz`, or `— no allocation in <plan>` — anchored so it never covers the
  tuned cursor label.
- Script: `bandplan <name>` / `bandplan list` (to the status line),
  `sdr bandstrip on|off`, `sdr bandlabel on|off`. `get bands` →
  `{plan, at_tuned:[…], in_view:[…]}`.

**Done when:** `cargo test -p neowon-app --test sdr_refmap -- --ignored`
(`--sdr-sim`, `NEOWON_REFDB` pointed at a temp dir; once 10.12 lands,
`NEOWON_NO_STATE` too):
`sdr tune 99.4M` → `get bands` names the FM broadcast band (exact);
`bandplan usa` switches plans (exact); `sdr bandstrip off` removes the ROI
from the layout dump; `ui_geometry` extended — the strip never overlaps the
dock or spectrum trace area at every size × scale.

### 10.14.5 — RF map window

- `bandmap` window (View menu + script `bandmap window on|off`, the
  `catalog window` pattern — `window` alone is the WxH resize): the whole
  device range (`caps.freq_range_hz`, else 0.1 MHz–2 GHz), one row per
  decade (0.1–1, 1–10, 10–100, 100–1000, 1000–10000 MHz, clipped), log-x
  within each row, like the reference charts. Segments by kind with fitted
  labels; out-of-range greyed; the current hardware window drawn as a
  bracket and Tuned as a red tick.
- Click: tune to the clicked frequency (D21). Double-click on a band:
  tune to its centre and set `sdr span` to fit it when it fits the sample
  rate, else leave the span. Scroll zooms a row (display only).
- Legend of kinds with counts.

**Done when:** in `sdr_refmap`: `bandmap goto "80m Ham Band"` (script
equivalent of the double-click) sets Tuned to the band centre and the
hardware centre follows (exact); window ROI painted and non-overlapping in
`ui_geometry`; a pixel check finds the red Tuned tick in the right decade
row at the right x (± 2 px).

### 10.14.6 — Station overlay, stations window, location UI

- **Overlay** on the spectrum: stations in view, filtered by location
  radius (per service defaults: broadcast FM/TV 150 km, aviation 100 km,
  shortwave/EiBi no radius but "on air now"), a tick at the frequency and a
  label `name · WFM` in up to 3 staggered rows; when labels collide the
  closer/stronger wins and the rest become ticks. Hover: full record +
  distance. Click: tune + demod (D21). Toggle `sdr stations on|off`,
  `sdr stationradius <km>`.
- **Stations window** (`stations window on|off`): search box, filters
  (source, service, modulation, in view, near me, on air now), columns
  `freq · name · mod · service · km · source`, sort by freq or distance;
  row click = tune + demod; `Add to catalog` per row (D20). A **Sources**
  tab: per source count / fetched date / origin / licence, `Fetch` (disabled
  with a reason when the source needs a location), `Import file…`, `Clear`,
  and a progress/status line for the running fetch.
- **Location** in the same window (and Utility): lat/lon or locator
  fields, `Locate me` with the D18 consent dialog, `Clear`. Shows the
  locator and the source of the fix.
- Script: `location <lat> <lon>` | `location <locator>` | `location ip` |
  `location clear`; `refdb fetch <source> [radius_km]`; `refdb import
  <source> <path>`; `refdb clear <source>`; `stations tune <source>:<id>`;
  `stations catalog <source>:<id>`. Readouts: `get location`, `get refdb`
  (sources + meta + running job), `get stations [view|near|all] [n]`.

**Done when:** in `sdr_refmap`, with a fixture store seeded at a station
on 100.3 MHz (`Wfm`, 20 km away) and one at 118.1 MHz (`Am`, aviation):
`location 38.72 -9.14` → `get location` exact; `get stations view` lists
the 100.3 MHz station with its km (± 0.5); `stations tune wikidata:Q…`
sets Tuned 100.3 MHz and demod `wfm` (exact); tuning to 118.1 MHz via the
same path recentres the hardware and sets `am` (exact); `refdb import
eibi <fixture>` then `get refdb` shows the golden count; `stations catalog
…` files one Signal with provenance `reference` and the catalog file is
the only catalog change (the refdb dir is untouched by it, and vice versa);
`location ip` against a stub is covered by 10.14.3, never run in the app
test.

### 10.14.7 — MCP

`refmap_tools.rs`: `rf_bands {freq_hz?}` (bands at a frequency / in view),
`stations {query, near?, in_view?, limit}`, `station_tune {id}`,
`refdb {action: status|fetch|import|clear, source?, path?}`,
`location {lat?, lon?, locator?}` — `location ip` is **not** exposed over
MCP (consent belongs to the operator at the screen). All through the
script grammar.

**Done when:** `--test mcp_e2e` lists the new tools and `stations` returns
the fixture station through `--spawn-sim`.

## Verification contract

`cargo build`; `cargo test` (incl. `-p neowon-refdb`); `cargo test -p
neowon-app --test sdr_refmap --test sdr_integration --test ui_geometry --
--ignored`; `cargo test -p neowon-app --test shaders`; `cargo fmt --all`;
`cargo clippy --workspace --all-targets` clean. PLAN.md §4 status updated
per sub-item; §7 gains rows for the station sources and band plans.

## Order

10.14.1 → 10.14.4 → 10.14.5 (band map usable, no network) → 10.14.2 →
10.14.6 → 10.14.3 → 10.14.7. The operator sees the map before any network
code exists.

## Open questions / risks

- Wikidata coverage of broadcast frequencies is uneven per country; if it
  proves thin near the operator, FMLIST import is the fallback (D17).
- Wikidata `P2144` sometimes carries several frequencies (multi-site
  networks); each becomes its own station row, keyed `Q…#n`.
- EiBi "on air now" depends on the app clock; the season boundary is
  computed, not hard-coded.
- FMLIST export format unknown until the operator supplies one.
- Label density at full span (2.048 MHz of FM holds ~10 stations) is fine;
  a future wider scan view (survey) would need clustering.

## Deviations (recorded per AGENTS.md)

- **10.14.4/.5 placement (2026-09-19, after a UX critic's audit the operator
  asked for).**
  - The band strip sits **between the spectrum and the waterfall**, not
    across the spectrum's top. That is where the eye already crosses from
    trace to waterfall, and there it does not fight the detection labels.
  - The **band name** goes in the app bar after the tuned frequency, not in
    a canvas badge: the app bar is always in view.
  - A whole-range **minimap** (`bandmap mini`) was added across the top of
    the canvas.
  - The RF map window is as specified.
- **Verbs.** `bandmap strip|mini|window on|off` and `bandmap goto <band>`
  replace `sdr bandstrip` / `sdr bandlabel` / `bandmap window`, because
  `sdr/actions.rs` is 22 lines under the 700-line hard budget. There is no
  label toggle, since the app-bar band name is always shown.
- **Clicks.** A click in the strip tunes to the band's **centre**; a
  double-click fits the span (`bandmap goto`). A click in the minimap or map
  window tunes to the rounded pointer frequency.
- **Catalog bands.** User-catalog `BandPlanEntry` overlays are not drawn
  yet; they come with the station overlay (10.14.6).
- **UI tree.** The acceptance checks assert from the UI element tree
  (`get uitree`, added at the operator's request), not from pixels.
- **P1920 is not a callsign (checked on Wikidata 2026-09-19).** P1920 is
  "CWGC burial ground ID". The Wikidata query binds the callsign from
  **P2317 only** (plus P17 for the country, as specced); using P1920 would
  have labelled stations with war-graves identifiers.
- **Wikidata entities pinned (checked 2026-09-19):** radio station Q14350,
  television station Q1616075; units hertz Q39369, kilohertz Q2143992,
  megahertz Q732707, gigahertz Q3276763. The unit is read from the entity,
  never inferred from the number.
- **FMLIST (10.14.2).** The importer is header-mapped and delimiter-
  detected (`,` `;` tab) because no operator file exists yet; frequencies
  come from a `MHz`/`kHz` header when named, magnitude otherwise. The
  real header shape gets recorded here when the operator supplies a file.
- **Import order (10.14.3 before 10.14.6).** The spec's Order section put
  the stations window before `fetch`; the Sources tab's Fetch button is a
  call into `neowon-refdb::fetch`, so the fetch API was built first. The
  operator-visible order is unchanged (both land before 10.14.7).
- **FCC `list=4` is headerless (confirmed live 2026-09-19).** The
  pipe-delimited output has no header row; its columns are
  `1 call sign | 2 "89.3 MHz" | 3 service | … | 10 city | 11 state |
  12 country | … | 18 facility id | 19 N | 20–22 lat D M S | 23 W |
  24–26 lon D M S | 27 licensee | …`. The importer anchors on the
  frequency field and the hemisphere/DMS runs, not on fixed offsets. The
  radius search parameters (also from the live form) are `serv`, `list=4`,
  `dist` km, `dlat2`/`mlat2`/`slat2`+`NS`, `dlon2`/`mlon2`/`slon2`+`EW`.
  `crates/neowon-refdb/tests/fixtures/fcc.txt` holds three real rows.
- **`locate_ip` leaves `set_at` empty.** The spec's determinism rule and
  the signature (no clock) win: the app stamps `set_at` with the catalog's
  RFC 3339 clock (neowon-refdb stays wall-clock-free).
- **D20 provenance maps to `ProvKind::Db` (2026-09-19).** `neowon-catalog`
  is off limits and its `ProvKind` has no `Reference` variant, so the
  catalog copy is filed as `Db` with `tool = "neowon-refdb"` and
  `input_ref = "refdb:<source>:<id>"`; the row is `+ cat` in the Stations
  window. `get catalog` now reports each row's provenance (additive).
- **`NEOWON_LOCATION` (2026-09-19).** `geo::location_path()` honors it so
  the app tests never touch the operator's `~/.neowon/location.json`, the
  analogue of `NEOWON_REFDB`/`NEOWON_STATE`. The station tests set both.
- **Verbs kept out of `sdr/actions.rs`.** `stations window|overlay|radius|
  find|filter|onair|scope|sort|tune|catalog`, `location …` and
  `refdb …` live in `refmap/actions.rs` (the file is 22 lines under the
  700 hard budget); the spec's `sdr stations` / `sdr stationradius` names
  are not used. Every verb round-trips in `every_action_round_trips`.
- **`rf_bands` landed with the UI round** in `neowon-mcp/src/ui_tools.rs`
  (plan/goto options); `refmap_tools.rs` adds `stations`,
  `station_tune`, `refdb` and `location`. `location ip` is not exposed
  over MCP, per the spec.
- **Import paths.** One `refdb import <source> <path>`: OurAirports takes
  a directory holding `airport-frequencies.csv` and `airports.csv`;
  FMLIST stays import-only; `refdb fetch fmlist` refuses with that
  sentence.
- **10.14.4 geometry check** is asserted from the UI element tree in
  `--test sdr_refmap` (band strip and minimap inside `SDR canvas`, never
  overlapping `SDR dock`) rather than from `ui_geometry` pixels, following
  the UI-tree deviation above.
- **`Import file…`** in the Sources tab is a path field plus an Import
  button, source-chosen by a combo — a native file picker would add an
  `rfd` dependency, which is not approved.
