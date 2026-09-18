//! The persistent signal / source / emitter catalog (Phase 10.2, D4/D7):
//! engine-free, file-based (manifest + snapshot + fsynced WAL), single
//! writer, with merges that leave redirect tombstones, explicit cascades,
//! pinning, session undo, and schema migration.

pub mod exchange;
pub mod migrate;
pub mod model;
pub mod op;
pub mod state;
pub mod store;
pub mod wal;

pub use model::{
    Alias, BandCoverage, BandPlanEntry, Emitter, Entity, Id, ObsRecord, Observation, ProvKind,
    Provenance, Signal, Source, Survey, Transmission,
};
pub use op::{Op, edit_from_text};
pub use state::State;
pub use store::Catalog;

/// Manifest `format`.
pub const FORMAT: &str = "neowon-catalog";
/// Current schema; manifests without one are v0.
pub const SCHEMA: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0} not found")]
    NotFound(Id),
    #[error("{0} is pinned; unpin it first")]
    Pinned(Id),
    #[error("{0} is referenced by {1} entities; use cascade")]
    Referenced(Id, usize),
    #[error("redirect loop or chain too long at {0}")]
    RedirectLoop(Id),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("corrupt catalog: {0}")]
    Corrupt(String),
    #[error("catalog schema {0} is newer than this build ({SCHEMA})")]
    TooNew(u32),
    #[error("{0} is open in another process")]
    Locked(std::path::PathBuf),
    #[error("a write failed; reopen the catalog")]
    Poisoned,
}

/// Now, as RFC 3339 UTC with seconds (`2026-09-18T21:35:07Z`).
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

/// RFC 3339 UTC for seconds since the Unix epoch (Hinnant's
/// days-to-civil algorithm).
pub fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_known_instants() {
        assert_eq!(super::rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(super::rfc3339(1_789_766_107), "2026-09-18T21:15:07Z");
    }
}
