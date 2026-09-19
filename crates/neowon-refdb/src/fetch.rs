//! Fetching reference data (10.14.3, D19): the only network code in the
//! workspace. Blocking `ureq` 3 over rustls, 30 s, one retry; the app runs
//! each fetch on its own worker thread. Every entry point takes its base
//! URLs as a parameter, so tests point them at a local `TcpListener` stub
//! and no test ever touches the network.

use std::time::Duration;

use crate::Error;
use crate::geo::{LatLon, Location, LocationSource};
use crate::sources::{Report, eibi, fcc, ourairports, wikidata};
use crate::station::{Source, Station};

pub const TIMEOUT: Duration = Duration::from_secs(30);
const ATTEMPTS: usize = 2;

pub fn user_agent() -> String {
    format!("neowon/{}", env!("CARGO_PKG_VERSION"))
}

/// What a fetch needs besides the source: a centre for the sources that
/// search by radius, and the date that picks an EiBi season.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchTarget {
    pub center: Option<LatLon>,
    pub radius_km: f64,
    pub date: (i32, u8, u8),
}

/// Where each source lives; tests replace these with stub addresses.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseUrls {
    pub wikidata: String,
    pub eibi: String,
    pub ourairports_frequencies: String,
    pub ourairports_airports: String,
    pub fcc_fm: String,
    pub fcc_am: String,
    pub ipapi: String,
}

impl Default for BaseUrls {
    fn default() -> Self {
        Self {
            wikidata: wikidata::ENDPOINT.into(),
            eibi: eibi::BASE_URL.into(),
            ourairports_frequencies: ourairports::URL_FREQUENCIES.into(),
            ourairports_airports: ourairports::URL_AIRPORTS.into(),
            fcc_fm: fcc::ENDPOINT_FM.into(),
            fcc_am: fcc::ENDPOINT_AM.into(),
            ipapi: "https://ipapi.co/json/".into(),
        }
    }
}

pub fn fetch(
    src: Source,
    at: &FetchTarget,
    base: &BaseUrls,
) -> Result<(Vec<Station>, Report), Error> {
    match src {
        Source::Wikidata => {
            let center = at.center.ok_or(Error::NeedsLocation(src))?;
            let query = wikidata::query(center, at.radius_km);
            let bytes = get(
                src,
                &base.wikidata,
                &[("query", &query), ("format", "json")],
            )?;
            Ok(wikidata::parse(&bytes))
        }
        Source::Eibi => {
            let url = format!("{}{}", base.eibi, eibi::season_file(at.date));
            Ok(eibi::parse(&get(src, &url, &[])?))
        }
        Source::OurAirports => {
            let frequencies = get(src, &base.ourairports_frequencies, &[])?;
            let airports = get(src, &base.ourairports_airports, &[])?;
            Ok(ourairports::parse(&frequencies, &airports))
        }
        Source::Fcc => {
            let center = at.center.ok_or(Error::NeedsLocation(src))?;
            let mut stations = Vec::new();
            let mut report = Report::default();
            for (endpoint, serv) in [(&base.fcc_fm, "FM"), (&base.fcc_am, "AM")] {
                let params = fcc::search_query(center, at.radius_km, serv);
                let pairs: Vec<(&str, &str)> = params
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                let (part_stations, part) = fcc::parse(&get(src, endpoint, &pairs)?);
                let offset = report.rows;
                report.rows += part.rows;
                report.kept += part.kept;
                report.skipped.extend(
                    part.skipped
                        .into_iter()
                        .map(|(line, why)| (line + offset, why)),
                );
                stations.extend(part_stations);
            }
            Ok((stations, report))
        }
        Source::Fmlist => Err(Error::Invalid(
            "FMLIST is import-only (D17): export the file with your own \
             account and use `refdb import fmlist <path>`"
                .into(),
        )),
    }
}

/// `location ip` (D18): **one** request, only ever from an explicit
/// operator action. `set_at` is left to the caller's clock, so parsing
/// stays deterministic.
pub fn locate_ip(base: &str) -> Result<Location, Error> {
    let bytes = get_plain(base, &[]).map_err(Error::Http)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)?;
    let lat = v["latitude"].as_f64().ok_or_else(no_coords)?;
    let lon = v["longitude"].as_f64().ok_or_else(no_coords)?;
    Ok(Location {
        lat,
        lon,
        country_code: v["country_code"].as_str().map(str::to_string),
        source: LocationSource::Ip,
        set_at: String::new(),
    })
}

fn no_coords() -> Error {
    Error::Invalid("ip lookup: no latitude/longitude in the response".into())
}

fn get(src: Source, url: &str, params: &[(&str, &str)]) -> Result<Vec<u8>, Error> {
    get_plain(url, params).map_err(|what| Error::Fetch { src, what })
}

/// One retry on any failure; the last failure is what comes back.
fn get_plain(url: &str, params: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    let mut last = String::new();
    for _ in 0..ATTEMPTS {
        match once(url, params) {
            Ok(bytes) => return Ok(bytes),
            Err(what) => last = what,
        }
    }
    Err(last)
}

fn once(url: &str, params: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();
    let mut req = agent.get(url).header("User-Agent", user_agent());
    for (k, v) in params {
        req = req.query(*k, *v);
    }
    let mut resp = req.call().map_err(|e| format!("{e}"))?;
    resp.body_mut()
        .read_to_vec()
        .map_err(|e| format!("body: {e}"))
}
