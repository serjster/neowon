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

#[derive(Debug, Default, Clone)]
pub struct Index {
    /// Sorted by `(freq_hz, id)`.
    stations: Vec<Station>,
}

impl Index {
    pub fn new(mut stations: Vec<Station>) -> Self {
        stations.sort_by(|a, b| {
            a.freq_hz
                .total_cmp(&b.freq_hz)
                .then_with(|| a.id.cmp(&b.id))
        });
        Self { stations }
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

    /// Every station in `lo..=hi`, binary search over the sorted set.
    pub fn within(&self, lo: f64, hi: f64) -> &[Station] {
        let start = self.stations.partition_point(|s| s.freq_hz < lo);
        let end = self.stations.partition_point(|s| s.freq_hz <= hi);
        &self.stations[start..end]
    }

    /// The filtered, sorted result; the distance is reported whenever a
    /// centre was given, whether or not a radius narrowed the set.
    pub fn query(&self, q: &Query) -> Vec<(&Station, Option<f64>)> {
        let text = q.text.trim().to_lowercase();
        let mut out = Vec::new();
        for s in &self.stations {
            if s.freq_hz < q.lo_hz || s.freq_hz > q.hi_hz {
                continue;
            }
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
            out.push((s, km));
        }
        if q.sort == SortBy::Distance {
            out.sort_by(|(a, ka), (b, kb)| {
                let d = |k: &Option<f64>| k.unwrap_or(f64::INFINITY);
                d(ka)
                    .total_cmp(&d(kb))
                    .then_with(|| a.freq_hz.total_cmp(&b.freq_hz))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        out
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
