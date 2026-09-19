//! Positions and the operator's location (D18): decimal degrees, Maidenhead
//! locators, great-circle distance, and the tiny `location.json` file.
//! Nothing here performs network I/O — `location ip` is `fetch`'s job.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;

/// Mean Earth radius, IUGG; the 0.5% spread between conventions is far
/// below every use here (a 150 km radius filter, a sorted-by-distance list).
const EARTH_RADIUS_KM: f64 = 6371.0088;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl LatLon {
    pub fn dist_km(self, other: LatLon) -> f64 {
        haversine_km(self, other)
    }
}

pub fn haversine_km(a: LatLon, b: LatLon) -> f64 {
    let (p1, p2) = (a.lat.to_radians(), b.lat.to_radians());
    let dp = p2 - p1;
    let dl = (b.lon - a.lon).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * h.sqrt().min(1.0).asin()
}

/// Degrees covered by each Maidenhead pair, longitude then latitude:
/// field, square, subsquare, extended square.
const STEPS: [(f64, f64); 4] = [
    (20.0, 10.0),
    (2.0, 1.0),
    (5.0 / 60.0, 2.5 / 60.0),
    (0.5 / 60.0, 0.25 / 60.0),
];
const COUNTS: [u32; 4] = [18, 10, 24, 10];

/// The centre of a 4-, 6- or 8-character Maidenhead locator, accepted in
/// either case ("IO91wm", "io91WM").
pub fn from_locator(s: &str) -> Result<LatLon, Error> {
    let chars: Vec<char> = s.trim().chars().collect();
    if !matches!(chars.len(), 4 | 6 | 8) {
        return Err(Error::Invalid(format!(
            "locator {s:?}: need 4, 6 or 8 characters"
        )));
    }
    let mut lon = -180.0;
    let mut lat = -90.0;
    for (i, p) in chars.chunks(2).enumerate() {
        let bad = |c: char| {
            Error::Invalid(format!(
                "locator {s:?}: bad character {c:?} at {}",
                i * 2 + 1
            ))
        };
        let (x, y) = match i {
            0 => (
                alpha(p[0], b'A', 18).ok_or_else(|| bad(p[0]))?,
                alpha(p[1], b'A', 18).ok_or_else(|| bad(p[1]))?,
            ),
            1 => (
                p[0].to_digit(10).ok_or_else(|| bad(p[0]))? as u8,
                p[1].to_digit(10).ok_or_else(|| bad(p[1]))? as u8,
            ),
            2 => (
                alpha(p[0], b'a', 24).ok_or_else(|| bad(p[0]))?,
                alpha(p[1], b'a', 24).ok_or_else(|| bad(p[1]))?,
            ),
            _ => (
                p[0].to_digit(10).ok_or_else(|| bad(p[0]))? as u8,
                p[1].to_digit(10).ok_or_else(|| bad(p[1]))? as u8,
            ),
        };
        lon += f64::from(x) * STEPS[i].0;
        lat += f64::from(y) * STEPS[i].1;
    }
    let (cell_lon, cell_lat) = STEPS[chars.len() / 2 - 1];
    Ok(LatLon {
        lat: lat + cell_lat / 2.0,
        lon: lon + cell_lon / 2.0,
    })
}

fn alpha(c: char, base: u8, n: u8) -> Option<u8> {
    let u = if base.is_ascii_uppercase() {
        c.to_ascii_uppercase() as u8
    } else {
        c.to_ascii_lowercase() as u8
    };
    (u >= base && u - base < n).then_some(u - base)
}

/// The locator containing `p`, at 4, 6 or 8 characters. A point on the
/// outer edge (lon 180, lat 90) folds into the last cell rather than
/// running off the alphabet.
pub fn to_locator(p: LatLon, chars: usize) -> String {
    let chars = chars.clamp(2, 8) & !1;
    let mut lon = (p.lon + 180.0).rem_euclid(360.0);
    let mut lat = (p.lat + 90.0).clamp(0.0, 180.0);
    let mut out = String::with_capacity(chars);
    for (i, step) in STEPS.iter().enumerate().take(chars / 2) {
        let x = ((lon / step.0).floor() as i64).clamp(0, i64::from(COUNTS[i]) - 1) as u8;
        let y = ((lat / step.1).floor() as i64).clamp(0, i64::from(COUNTS[i]) - 1) as u8;
        out.push(match i {
            0 => (b'A' + x) as char,
            1 => (b'0' + x) as char,
            2 => (b'a' + x) as char,
            _ => (b'0' + x) as char,
        });
        out.push(match i {
            0 => (b'A' + y) as char,
            1 => (b'0' + y) as char,
            2 => (b'a' + y) as char,
            _ => (b'0' + y) as char,
        });
        lon -= f64::from(x) * step.0;
        lat -= f64::from(y) * step.1;
    }
    out
}

/// How the operator's position was fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocationSource {
    Manual,
    Locator,
    Ip,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    pub source: LocationSource,
    /// RFC 3339 UTC, the app's clock (tests pass a fixed string).
    pub set_at: String,
}

impl Location {
    pub fn at(&self) -> LatLon {
        LatLon {
            lat: self.lat,
            lon: self.lon,
        }
    }

    /// `None` when nothing is stored (never an error: no location is a
    /// valid state).
    pub fn load(path: &Path) -> Result<Option<Self>, Error> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), Error> {
        crate::write_atomic(path, &serde_json::to_vec(self)?)
    }
}

/// `~/.neowon/location.json` (D18), or `$NEOWON_LOCATION`. Tests point
/// the override at a temp file so they never touch the operator's fix.
pub fn location_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NEOWON_LOCATION") {
        return Some(p.into());
    }
    crate::neowon_dir().map(|d| d.join("location.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: LatLon, b: LatLon, km: f64) -> bool {
        a.dist_km(b) <= km
    }

    #[test]
    fn lisbon_to_porto_is_274_km() {
        let lisbon = LatLon {
            lat: 38.7223,
            lon: -9.1393,
        };
        let porto = LatLon {
            lat: 41.1579,
            lon: -8.6291,
        };
        assert!((haversine_km(lisbon, porto) - 274.0).abs() <= 2.0);
        assert_eq!(haversine_km(lisbon, lisbon), 0.0);
    }

    #[test]
    fn in58_names_its_square_centre() {
        // IN58 (field I=8 → 40°E of −180, field N=13 → 40°N; square 5,8)
        // spans 10–8°W by 48–49°N.
        let p = from_locator("IN58").unwrap();
        assert!(
            close(
                p,
                LatLon {
                    lat: 48.5,
                    lon: -9.0
                },
                1.0
            ),
            "{p:?}"
        );
        assert_eq!(to_locator(p, 4), "IN58");
        assert!(to_locator(p, 6).starts_with("IN58"));
        assert!(to_locator(p, 8).starts_with("IN58"));
    }

    #[test]
    fn known_locations_land_in_known_squares() {
        let london = LatLon {
            lat: 51.5074,
            lon: -0.1278,
        };
        assert_eq!(to_locator(london, 6), "IO91wm");
        let lisbon = LatLon {
            lat: 38.7223,
            lon: -9.1393,
        };
        assert_eq!(to_locator(lisbon, 6), "IM58kr");
        // A locator round trip never leaves its cell: the centre maps back.
        for loc in ["IO91wm", "IM58kr", "FN31rd", "JO22nb", "QF56od"] {
            let p = from_locator(loc).unwrap();
            assert_eq!(to_locator(p, 6), loc);
        }
    }

    #[test]
    fn bad_locators_are_refused_not_guessed() {
        for s in [
            "",
            "I",
            "IN5",
            "IN580",
            "IN58aa000",
            "ZI58",
            "IN58zz",
            "XX99xx99",
        ] {
            assert!(from_locator(s).is_err(), "{s:?} accepted");
        }
        assert!(from_locator("io91WM").is_ok()); // case-insensitive
    }

    #[test]
    fn location_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!("neowon-refdb-geo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("location.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(Location::load(&path).unwrap(), None);

        let loc = Location {
            lat: 38.7223,
            lon: -9.1393,
            country_code: Some("PT".into()),
            source: LocationSource::Locator,
            set_at: "2026-09-19T10:00:00Z".into(),
        };
        loc.save(&path).unwrap();
        assert_eq!(Location::load(&path).unwrap(), Some(loc.clone()));
        // Atomic: only the real file, no .tmp left behind.
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
