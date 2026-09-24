//! The merged, frequency-sorted station set and the queries the stations
//! window and overlays run against it.

use crate::geo::{LatLon, haversine_km};
use crate::station::{Source, Station};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Frequency,
    Distance,
}

/// What the stations window and `get stations` ask for. An empty `sources`
/// means every source; `near`/`radius_km` only filter when both are set;
/// `on_air_at` only hides scheduled stations that are off air — stations
/// without a schedule have no reason to be hidden.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// Case-insensitive substring of name, callsign, id, country or notes.
    pub text: String,
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub near: Option<LatLon>,
    pub radius_km: Option<f64>,
    /// `(weekday, UTC minutes)`, weekday 0 = Monday.
    pub on_air_at: Option<(u8, u16)>,
    pub sources: Vec<Source>,
    pub sort: SortBy,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            text: String::new(),
            lo_hz: 0.0,
            hi_hz: f64::INFINITY,
            near: None,
            radius_km: None,
            on_air_at: None,
            sources: Vec::new(),
            sort: SortBy::Frequency,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Index {
    /// Sorted by `(freq_hz, id)`.
    stations: Vec<Station>,
    /// Distinct for every index built: a cache of positions into this
    /// index keys on it, so a reload — the only way the set changes —
    /// invalidates the cache. A clone shares it: same stations, same order.
    generation: u64,
}

impl Default for Index {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl Index {
    pub fn new(mut stations: Vec<Station>) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        stations.sort_by(|a, b| {
            a.freq_hz
                .total_cmp(&b.freq_hz)
                .then_with(|| a.id.cmp(&b.id))
        });
        Self {
            stations,
            generation: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Which build of the set this is; never the same for two builds.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The station at position `i` of the frequency-sorted set — what a
    /// cached [`query_positions`](Self::query_positions) result reads.
    pub fn get(&self, i: usize) -> Option<&Station> {
        self.stations.get(i)
    }

    pub fn len(&self) -> usize {
        self.stations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stations.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Station> {
        self.stations.iter()
    }

    /// Every station in `lo..=hi`, binary search over the sorted set
    /// (empty when `lo > hi`).
    pub fn within(&self, lo: f64, hi: f64) -> &[Station] {
        &self.stations[self.range(lo, hi)]
    }

    fn range(&self, lo: f64, hi: f64) -> std::ops::Range<usize> {
        let start = self.stations.partition_point(|s| s.freq_hz < lo);
        let end = self.stations.partition_point(|s| s.freq_hz <= hi);
        start..end.max(start)
    }

    /// The filtered, sorted result; the distance is reported whenever a
    /// centre was given, whether or not a radius narrowed the set.
    ///
    /// Only the stations in `lo_hz..=hi_hz` are looked at: the
    /// stations window runs this every frame, and in `view` scope the
    /// frequency range is the IQ window's, a sliver of the set.
    pub fn query(&self, q: &Query) -> Vec<(&Station, Option<f64>)> {
        self.query_positions(q)
            .into_iter()
            .map(|(i, km)| (&self.stations[i], km))
            .collect()
    }

    /// [`query`](Self::query) as positions into this index ([`get`](Self::get)),
    /// so a caller can keep the result across frames, keyed on
    /// [`generation`](Self::generation), without borrowing the index.
    pub fn query_positions(&self, q: &Query) -> Vec<(usize, Option<f64>)> {
        self.query_visiting(q).0
    }

    /// `query_positions`, plus how many stations it looked at — the
    /// structural cost the tests hold to the range's size, not the set's.
    fn query_visiting(&self, q: &Query) -> (Vec<(usize, Option<f64>)>, usize) {
        let text = q.text.trim().to_lowercase();
        let mut out = Vec::new();
        let candidates = self.range(q.lo_hz, q.hi_hz);
        let visited = candidates.len();
        for i in candidates {
            let s = &self.stations[i];
            if !q.sources.is_empty() && !q.sources.contains(&s.source) {
                continue;
            }
            if !text.is_empty() && !s.matches_text(&text) {
                continue;
            }
            if let Some((weekday, minute)) = q.on_air_at
                && let Some(schedule) = &s.schedule
                && !schedule.on_air(weekday, minute)
            {
                continue;
            }
            let km = q
                .near
                .and_then(|near| s.at().map(|p| haversine_km(near, p)));
            if let (Some(_), Some(radius)) = (q.near, q.radius_km) {
                // Without coordinates a distance cannot be checked, so it
                // does not pass a radius filter.
                match km {
                    Some(d) if d <= radius => {}
                    _ => continue,
                }
            }
            out.push((i, km));
        }
        if q.sort == SortBy::Distance {
            let st = &self.stations;
            out.sort_by(|(a, ka), (b, kb)| {
                let d = |k: &Option<f64>| k.unwrap_or(f64::INFINITY);
                let (a, b) = (&st[*a], &st[*b]);
                d(ka)
                    .total_cmp(&d(kb))
                    .then_with(|| a.freq_hz.total_cmp(&b.freq_hz))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        (out, visited)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::{Modulation, Schedule, Service};

    /// A seeded splitmix64, as everywhere else in the workspace: the test
    /// data must be identical on every platform.
    fn splitmix64(x: &mut u64) -> u64 {
        *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn seeded(n: usize) -> (Vec<Station>, Index) {
        let mut seed = 0x5EED_1022_u64;
        let mut raw = Vec::new();
        for i in 0..n {
            let r = splitmix64(&mut seed);
            let mut s = Station::new(
                Source::ALL[(r % 5) as usize],
                format!("id{i}"),
                format!("Station {i}"),
                (r % 1_000_000_000) as f64,
            );
            if r & 1 == 0 {
                s.lat = Some((splitmix64(&mut seed) % 180) as f64 - 90.0);
                s.lon = Some((splitmix64(&mut seed) % 360) as f64 - 180.0);
            }
            if r & 2 == 0 {
                s.schedule = Some(Schedule {
                    start_min: (splitmix64(&mut seed) % 1440) as u16,
                    stop_min: (splitmix64(&mut seed) % 1440) as u16,
                    days: splitmix64(&mut seed) as u8,
                });
            }
            s.modulation = Modulation::Unknown;
            s.service = Service::Other;
            raw.push(s);
        }
        let index = Index::new(raw.clone());
        (raw, index)
    }

    #[test]
    fn range_queries_match_a_linear_scan() {
        let (raw, index) = seeded(400);
        let mut seed = 7_u64;
        for _ in 0..40 {
            let a = (splitmix64(&mut seed) % 1_000_000_000) as f64;
            let b = (splitmix64(&mut seed) % 1_000_000_000) as f64;
            let (lo, hi) = (a.min(b), a.max(b));
            let mut want: Vec<&Station> = raw
                .iter()
                .filter(|s| s.freq_hz >= lo && s.freq_hz <= hi)
                .collect();
            want.sort_by(|x, y| {
                x.freq_hz
                    .total_cmp(&y.freq_hz)
                    .then_with(|| x.id.cmp(&y.id))
            });
            let got: Vec<&str> = index.within(lo, hi).iter().map(|s| s.id.as_str()).collect();
            let want: Vec<&str> = want.iter().map(|s| s.id.as_str()).collect();
            assert_eq!(got, want, "{lo}..{hi}");
        }
        assert_eq!(index.len(), 400);
        assert!(!index.is_empty());
    }

    #[test]
    fn positions_read_back_the_query_and_generations_differ() {
        let (raw, index) = seeded(300);
        let q = Query {
            lo_hz: 1e8,
            hi_hz: 6e8,
            sort: SortBy::Distance,
            near: Some(LatLon { lat: 0.0, lon: 0.0 }),
            ..Query::default()
        };
        let by_ref: Vec<(&str, Option<f64>)> = index
            .query(&q)
            .iter()
            .map(|(s, k)| (s.id.as_str(), *k))
            .collect();
        let by_pos: Vec<(&str, Option<f64>)> = index
            .query_positions(&q)
            .iter()
            .map(|&(i, k)| (index.get(i).unwrap().id.as_str(), k))
            .collect();
        assert_eq!(by_ref, by_pos);
        assert!(index.get(index.len()).is_none());
        // A rebuild of the same stations is a new generation; a clone is not.
        let again = Index::new(raw);
        assert_ne!(again.generation(), index.generation());
        assert_eq!(index.clone().generation(), index.generation());
        assert_ne!(Index::default().generation(), Index::default().generation());
    }

    /// The naive query: every station, filtered by range in the loop.
    fn linear<'a>(index: &'a Index, q: &Query) -> Vec<(&'a Station, Option<f64>)> {
        let all = Index::new(
            index
                .iter()
                .filter(|s| s.freq_hz >= q.lo_hz && s.freq_hz <= q.hi_hz)
                .cloned()
                .collect(),
        );
        let ids: Vec<(String, Option<f64>)> = all
            .query(&Query {
                lo_hz: f64::NEG_INFINITY,
                hi_hz: f64::INFINITY,
                ..q.clone()
            })
            .into_iter()
            .map(|(s, k)| (s.id.clone(), k))
            .collect();
        ids.iter()
            .map(|(id, k)| (index.iter().find(|s| &s.id == id).unwrap(), *k))
            .collect()
    }

    #[test]
    fn a_view_query_visits_its_range_not_the_set() {
        // Five stations inside a 10 kHz view; the rest of the set grows
        // 1000-fold outside it. The stations window runs this per frame.
        let q = Query {
            lo_hz: 1_000_000_000.0,
            hi_hz: 1_000_010_000.0,
            ..Query::default()
        };
        for n in [100, 10_000, 100_000] {
            let (mut raw, _) = seeded(n); // all below 1 GHz
            for i in 0..5 {
                raw.push(Station::new(
                    Source::Fcc,
                    format!("in{i}"),
                    format!("In {i}"),
                    1_000_000_000.0 + 2_000.0 * i as f64,
                ));
            }
            let index = Index::new(raw);
            let (rows, visited) = index.query_visiting(&q);
            println!("set {:>6}: view query visited {visited}", index.len());
            assert_eq!(visited, 5, "query cost grew with the set ({n})");
            assert_eq!(rows.len(), 5);
        }
    }

    #[test]
    fn the_range_bound_query_matches_the_linear_one() {
        let (_, index) = seeded(2_000);
        let mut seed = 11_u64;
        let near = LatLon {
            lat: 38.72,
            lon: -9.14,
        };
        for i in 0..60 {
            let a = (splitmix64(&mut seed) % 1_000_000_000) as f64;
            let b = (splitmix64(&mut seed) % 1_000_000_000) as f64;
            // Every fifth range is reversed (lo > hi): empty, not a panic.
            let (lo, hi) = if i % 5 == 0 {
                (a.max(b), a.min(b))
            } else {
                (a.min(b), a.max(b))
            };
            let r = splitmix64(&mut seed);
            let q = Query {
                text: if r & 1 == 0 {
                    "1".into()
                } else {
                    String::new()
                },
                lo_hz: lo,
                hi_hz: hi,
                near: (r & 2 == 0).then_some(near),
                radius_km: (r & 4 == 0).then_some(3_000.0),
                on_air_at: (r & 8 == 0).then_some(((r >> 8) as u8 % 7, (r >> 16) as u16 % 1440)),
                sources: if r & 16 == 0 {
                    vec![Source::Fcc, Source::Eibi]
                } else {
                    vec![]
                },
                sort: if r & 32 == 0 {
                    SortBy::Distance
                } else {
                    SortBy::Frequency
                },
            };
            let got: Vec<(&str, Option<f64>)> = index
                .query(&q)
                .iter()
                .map(|(s, k)| (s.id.as_str(), *k))
                .collect();
            let want: Vec<(&str, Option<f64>)> = linear(&index, &q)
                .iter()
                .map(|(s, k)| (s.id.as_str(), *k))
                .collect();
            assert_eq!(got, want, "{q:?}");
        }
    }

    #[test]
    fn query_filters_compose() {
        let mut a = Station::new(Source::Wikidata, "Q1".into(), "Antena 1".into(), 100.3e6);
        a.service = Service::Broadcast;
        a.modulation = Modulation::Wfm;
        a.lat = Some(38.72);
        a.lon = Some(-9.14);
        let b = Station::new(Source::Fcc, "2".into(), "KBZZ".into(), 118.1e6);
        let mut c = Station::new(Source::Eibi, "3".into(), "RNZ".into(), 9.75e6);
        c.schedule = Some(Schedule {
            start_min: 0,
            stop_min: 1440,
            days: Schedule::DAILY,
        });
        // Monday nights only.
        let mut d = Station::new(Source::Eibi, "4".into(), "night".into(), 5e6);
        d.schedule = Some(Schedule {
            start_min: 22 * 60,
            stop_min: 2 * 60,
            days: 1,
        });
        let index = Index::new(vec![a, b, c, d]);

        let q = Query {
            text: "antena".into(),
            lo_hz: 88e6,
            hi_hz: 108e6,
            near: Some(LatLon {
                lat: 38.72,
                lon: -9.14,
            }),
            radius_km: Some(150.0),
            ..Query::default()
        };
        let got = index.query(&q);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.id, "Q1");
        assert!(got[0].1.unwrap() < 0.1);

        // Sources and sort by distance (no near ⇒ frequency order stays).
        let q = Query {
            sources: vec![Source::Wikidata, Source::Eibi],
            sort: SortBy::Distance,
            ..Query::default()
        };
        let got = index.query(&q);
        let ids: Vec<&str> = got.iter().map(|(s, _)| s.id.as_str()).collect();
        assert_eq!(ids, ["4", "3", "Q1"]);

        // On air at Wednesday noon: the Monday-night EiBi row is hidden,
        // the 24/7 row and the unscheduled rows stay.
        let q = Query {
            on_air_at: Some((2, 12 * 60)),
            ..Query::default()
        };
        let ids: Vec<&str> = index.query(&q).iter().map(|(s, _)| s.id.as_str()).collect();
        assert_eq!(ids, ["3", "Q1", "2"]);
        // At Monday 23:00 the night row is on air.
        let q = Query {
            on_air_at: Some((0, 23 * 60)),
            ..Query::default()
        };
        let ids: Vec<&str> = index.query(&q).iter().map(|(s, _)| s.id.as_str()).collect();
        assert!(ids.contains(&"4"), "{ids:?}");
        let q = Query {
            text: "kbzz".into(),
            ..Query::default()
        };
        assert_eq!(index.query(&q).len(), 1);
        let q = Query {
            lo_hz: 100.3e6,
            hi_hz: 100.3e6,
            ..Query::default()
        };
        assert_eq!(index.query(&q)[0].0.freq_hz, 100.3e6); // inclusive
        let q = Query {
            lo_hz: 100.3e6 + 1.0,
            hi_hz: 100.3e6 + 1.0,
            ..Query::default()
        };
        assert!(index.query(&q).is_empty());
    }

    #[test]
    fn radius_drops_stations_without_coordinates() {
        let mut a = Station::new(Source::Fcc, "a".into(), "near".into(), 1e6);
        a.lat = Some(38.72);
        a.lon = Some(-9.14);
        let b = Station::new(Source::Fcc, "b".into(), "nowhere".into(), 2e6);
        let index = Index::new(vec![a, b]);
        let q = Query {
            near: Some(LatLon {
                lat: 38.72,
                lon: -9.14,
            }),
            radius_km: Some(50.0),
            ..Query::default()
        };
        let got = index.query(&q);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.id, "a");
    }
}
