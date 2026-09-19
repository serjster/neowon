//! RF reference data (Phase 10.14, D16–D20): band plans in the SDR++
//! schema, known stations imported from public databases, the operator's
//! location, and the store the snapshots live in. Engine-free and
//! read-mostly: it is reference material, never the operator's catalog —
//! `neowon-catalog` does not read it and nothing here writes there.

use std::path::{Path, PathBuf};

pub mod bandplan;
pub mod fetch;
pub mod geo;
pub mod index;
pub mod sources;
pub mod station;
pub mod store;

pub use bandplan::{Band, BandPlan, LoadError, NamedPlan, load_plans};
pub use geo::{LatLon, Location, LocationSource};
pub use index::{Index, Query, SortBy};
pub use sources::Report;
pub use station::{Modulation, Schedule, Service, Source, Station};
pub use store::{Meta, Store};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
    /// A fetch failed, with the source named: "Wikidata fetch: …".
    #[error("{src} fetch: {what}")]
    Fetch {
        src: crate::station::Source,
        what: String,
    },
    /// A source searches by radius and no location is set (D18).
    #[error("{0} needs a location — set one with `location <lat> <lon>`")]
    NeedsLocation(crate::station::Source),
    /// `locate_ip`'s HTTP transport, which belongs to no source.
    #[error("ip lookup: {0}")]
    Http(String),
}

/// `~/.neowon`, the home of everything the app stores for the operator.
pub fn neowon_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(Path::new(&home).join(".neowon"))
}

/// Write `bytes` so a reader sees either the old file or the complete new
/// one — never a half-written snapshot. A leftover `.tmp` is inert.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}
