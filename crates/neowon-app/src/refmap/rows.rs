//! The Stations window's rows, built once per change and read per frame.
//!
//! In `all` and `near` scope the query's frequency range is the whole set,
//! so a range bound cannot make it cheap — run per frame, it visited every
//! station the store holds. The result depends only on the station set and
//! the window's filters, and the set changes only by a reload, which builds
//! a new [`Index`](neowon_refdb::Index) with a new generation. So the rows
//! are kept as positions into the index, keyed on `(generation, filters)`;
//! between changes a frame compares the key and reads the rows it draws.

use neowon_refdb::{Location, Query, SortBy, Source, Station};

use super::{RefMap, Scope};
use crate::sdr::SdrState;

impl RefMap {
    /// The window's filters as a range query.
    pub fn query(&self, scope: Scope, sdr: &SdrState) -> Vec<(&Station, Option<f64>)> {
        self.query_positions(scope, sdr)
            .into_iter()
            .filter_map(|(i, km)| Some((self.index.get(i)?, km)))
            .collect()
    }

    /// [`query`](Self::query) as positions into `self.index`.
    fn query_positions(&self, scope: Scope, sdr: &SdrState) -> Vec<(usize, Option<f64>)> {
        let key = RowsKey::of(self, scope, sdr);
        let (near, radius) = match scope {
            Scope::Near => (key.at, Some(self.radius_km.unwrap_or(150.0))),
            _ => (key.at, None),
        };
        let q = Query {
            text: self.find.clone(),
            lo_hz: key.lo_hz,
            hi_hz: key.hi_hz,
            near: near.map(|(lat, lon)| neowon_refdb::LatLon { lat, lon }),
            radius_km: radius,
            on_air_at: key.on_air_at,
            sources: self.filter_source.into_iter().collect(),
            sort: self.sort,
        };
        let mut rows = self.index.query_positions(&q);
        rows.retain(|&(i, _)| {
            let Some(s) = self.index.get(i) else {
                return false;
            };
            self.filter_modulation.is_none_or(|m| s.modulation == m)
                && self.filter_service.is_none_or(|v| s.service == v)
        });
        rows
    }
}

/// Everything the rows depend on. Two frames with equal keys have equal rows.
#[derive(Debug, Clone, PartialEq)]
struct RowsKey {
    generation: u64,
    scope: Scope,
    lo_hz: f64,
    hi_hz: f64,
    find: String,
    source: Option<Source>,
    service: Option<neowon_refdb::Service>,
    modulation: Option<neowon_refdb::Modulation>,
    /// The clock matters only while `on air` filters.
    on_air_at: Option<(u8, u16)>,
    at: Option<(f64, f64)>,
    radius_km: Option<f64>,
    sort: SortBy,
}

impl RowsKey {
    fn of(rm: &RefMap, scope: Scope, sdr: &SdrState) -> Self {
        let (lo_hz, hi_hz) = match scope {
            Scope::View => {
                let half = sdr.span() / 2.0;
                (sdr.view_centre() - half, sdr.view_centre() + half)
            }
            // `near` keeps the frequency range open; the radius does the work.
            Scope::All | Scope::Near => (0.0, f64::INFINITY),
        };
        Self {
            generation: rm.index.generation(),
            scope,
            lo_hz,
            hi_hz,
            find: rm.find.clone(),
            source: rm.filter_source,
            service: rm.filter_service,
            modulation: rm.filter_modulation,
            on_air_at: rm.on_air.then_some(rm.utc),
            at: rm
                .location
                .as_ref()
                .map(Location::at)
                .map(|p| (p.lat, p.lon)),
            radius_km: rm.radius_km,
            sort: rm.sort,
        }
    }
}

#[derive(Default)]
pub struct StationRows {
    key: Option<RowsKey>,
    rows: Vec<(usize, Option<f64>)>,
    /// Rebuilds so far, and stations the last frame visited (the query's
    /// scan, when it ran, plus the rows handed out): the tests' measure of
    /// per-frame cost.
    builds: u64,
    touched: usize,
}

impl StationRows {
    /// The first `limit` rows of the window's current query and how many
    /// there are in all. Rebuilds only when the key changed; otherwise the
    /// frame reads `limit` rows at most, whatever the store holds.
    pub fn page<'a>(
        &mut self,
        rm: &'a RefMap,
        sdr: &SdrState,
        limit: usize,
    ) -> (usize, Vec<(&'a Station, Option<f64>)>) {
        let key = RowsKey::of(rm, rm.scope, sdr);
        let mut scanned = 0;
        if self.key.as_ref() != Some(&key) {
            scanned = rm.index.within(key.lo_hz, key.hi_hz).len();
            self.rows = rm.query_positions(rm.scope, sdr);
            self.key = Some(key);
            self.builds += 1;
        }
        let page: Vec<(&Station, Option<f64>)> = self
            .rows
            .iter()
            .take(limit)
            .filter_map(|&(i, km)| Some((rm.index.get(i)?, km)))
            .collect();
        self.touched = scanned + page.len();
        (self.rows.len(), page)
    }
}

#[cfg(test)]
mod tests {
    use neowon_refdb::{Index, LocationSource};

    use super::*;

    fn set(n: usize) -> Index {
        Index::new(
            (0..n)
                .map(|i| {
                    let mut s = Station::new(
                        Source::ALL[i % 5],
                        format!("s{i}"),
                        format!("Station {i}"),
                        1e6 + i as f64 * 10.0,
                    );
                    s.lat = Some(38.0 + (i % 7) as f64 * 0.01);
                    s.lon = Some(-9.0);
                    s
                })
                .collect(),
        )
    }

    /// Sixty frames per set size, in every scope the window has: one build
    /// per change, and a frame reads the page it draws, not the set.
    #[test]
    fn frames_between_changes_read_a_page_not_the_set() {
        let mut rm = RefMap::shipped_only();
        rm.location = Some(Location {
            lat: 38.0,
            lon: -9.0,
            source: LocationSource::Manual,
            set_at: String::new(),
            country_code: None,
        });
        rm.radius_km = Some(10_000.0);
        let sdr = SdrState::default();
        let mut failures = Vec::new();
        for scope in [Scope::All, Scope::Near] {
            rm.scope = scope;
            let mut cache = StationRows::default();
            for (step, n) in [1_000, 10_000, 100_000].into_iter().enumerate() {
                rm.index = set(n); // a reload: a new generation
                for frame in 0..60 {
                    let (total, page) = cache.page(&rm, &sdr, 500);
                    assert_eq!((total, page.len()), (n, 500));
                    // The first frame after a reload pays for the change.
                    if frame > 0 && cache.touched != 500 {
                        failures.push(format!(
                            "{scope:?} {n}: frame {frame} read {} stations",
                            cache.touched
                        ));
                    }
                }
                if cache.builds != step as u64 + 1 {
                    failures.push(format!(
                        "{scope:?} {n} stations: {} builds for {} changes — the rows were \
                         rebuilt per frame",
                        cache.builds,
                        step + 1
                    ));
                }
            }
            // A filter change is a change; the same filter again is not.
            rm.find = "Station 9".into();
            let (total, _) = cache.page(&rm, &sdr, 500);
            cache.page(&rm, &sdr, 500);
            assert!(total < 100_000);
            if cache.builds != 4 {
                failures.push(format!(
                    "{scope:?}: {} builds after one filter change (want 4)",
                    cache.builds
                ));
            }
            rm.find.clear();
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// The cached rows are the query's rows.
    #[test]
    fn a_page_is_the_query() {
        let mut rm = RefMap::shipped_only();
        rm.index = set(2_000);
        rm.filter_source = Some(Source::Eibi);
        rm.find = "1".into();
        let sdr = SdrState::default();
        let mut cache = StationRows::default();
        let (total, page) = cache.page(&rm, &sdr, usize::MAX);
        let want: Vec<&str> = rm
            .query(Scope::All, &sdr)
            .iter()
            .map(|(s, _)| s.id.as_str())
            .collect();
        let got: Vec<&str> = page.iter().map(|(s, _)| s.id.as_str()).collect();
        assert_eq!(got, want);
        assert_eq!(total, want.len());
    }
}
