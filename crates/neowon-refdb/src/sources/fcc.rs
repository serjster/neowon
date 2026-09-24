//! FCC importer (US public domain): the `fmq`/`amq` pipe-delimited
//! output (`list=4`). That format has **no header row**; the columns were
//! confirmed against the live service:
//!
//! ```text
//! 1 call sign | 2 "89.3 MHz" | 3 service | … | 10 city | 11 state |
//! 12 country | … | 18 facility id | 19 N | 20–22 lat D M S |
//! 23 W | 24–26 lon D M S | 27 licensee | …
//! ```
//!
//! The parser anchors on the frequency field and the hemisphere/DMS runs
//! instead of counting columns from the start, so a shift elsewhere in the
//! row cannot silently move the coordinates.

use super::Report;
use crate::geo::LatLon;
use crate::station::{Modulation, Service, Source, Station};

pub const ENDPOINT_FM: &str = "https://transition.fcc.gov/fcc-bin/fmq";
pub const ENDPOINT_AM: &str = "https://transition.fcc.gov/fcc-bin/amq";

/// The station-in-a-radius parameters of the FM/AM Query form, with
/// `list=4`; `dist` is kilometres (`serv` is "FM" or "AM").
pub fn search_query(center: LatLon, radius_km: f64, service: &str) -> Vec<(String, String)> {
    let lat = dms_parts(center.lat);
    let lon = dms_parts(center.lon);
    let s = |f: f64| format!("{f:.1}");
    vec![
        ("serv".into(), service.into()),
        ("list".into(), "4".into()),
        ("dist".into(), format!("{radius_km}")),
        ("dlat2".into(), lat.0.to_string()),
        ("mlat2".into(), lat.1.to_string()),
        ("slat2".into(), s(lat.2)),
        ("NS".into(), if center.lat < 0.0 { "S" } else { "N" }.into()),
        ("dlon2".into(), lon.0.to_string()),
        ("mlon2".into(), lon.1.to_string()),
        ("slon2".into(), s(lon.2)),
        ("EW".into(), if center.lon < 0.0 { "W" } else { "E" }.into()),
    ]
}

/// Degrees, minutes, seconds (always positive; the hemisphere travels
/// separately in these queries).
fn dms_parts(v: f64) -> (u32, u32, f64) {
    let mut d = v.abs().floor();
    let mut m = ((v.abs() - d) * 60.0).floor();
    let mut s = ((v.abs() - d - m / 60.0) * 3600.0 * 10.0).round() / 10.0;
    if s >= 60.0 {
        s -= 60.0;
        m += 1.0;
    }
    if m >= 60.0 {
        m -= 60.0;
        d += 1.0;
    }
    (d as u32, m as u32, s)
}

/// "89.3  MHz " → (89.3e6, scale); a field without a unit is Hz.
fn frequency(field: &str) -> Option<f64> {
    let t = field.trim();
    let (num, unit) = t.split_once(char::is_whitespace)?;
    let scale = match unit.trim() {
        "MHz" => 1e6,
        "kHz" => 1e3,
        _ => return None,
    };
    let v: f64 = num.parse().ok()?;
    (v > 0.0).then_some(v * scale)
}

/// The `(index, signed degrees)` of the first `hemi`-plus-D/M/S run.
fn dms_run(f: &[&str], hemi: &str, negative: bool) -> Option<(usize, f64)> {
    (0..f.len().saturating_sub(3)).find_map(|i| {
        if !f[i].trim().eq_ignore_ascii_case(hemi) {
            return None;
        }
        let d: f64 = f[i + 1].trim().parse().ok()?;
        let m: f64 = f[i + 2].trim().parse().ok()?;
        let s: f64 = f[i + 3].trim().parse().ok()?;
        let v = d + m / 60.0 + s / 3600.0;
        Some((i, if negative { -v } else { v }))
    })
}

/// "12. kW " / "1000. W" → kW.
fn power_kw(field: &str) -> Option<f64> {
    let (num, unit) = field.trim().split_once(char::is_whitespace)?;
    let v: f64 = num.parse().ok()?;
    match unit.trim() {
        "kW" => Some(v),
        "W" => Some(v / 1e3),
        _ => None,
    }
}

pub fn parse(bytes: &[u8]) -> (Vec<Station>, Report) {
    let text = String::from_utf8_lossy(bytes);
    let mut stations = Vec::new();
    let mut report = Report::default();
    let mut seen = std::collections::HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let row = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        report.rows += 1;
        let f: Vec<&str> = line.split('|').map(str::trim).collect();
        let Some(callsign) = f.iter().find(|x| !x.is_empty()) else {
            report.skip(row, "empty record");
            continue;
        };
        let Some((fi, freq_hz)) = f
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(i, x)| frequency(x).map(|hz| (i, hz)))
        else {
            report.skip(row, "no frequency");
            continue;
        };
        let service = f.get(fi + 1).copied().unwrap_or("");
        // Latitude is the N/S run; longitude then is the W/E run after it.
        let Some((lat_i, lat)) = dms_run(&f, "N", false).or_else(|| dms_run(&f, "S", true)) else {
            report.skip(row, "no latitude");
            continue;
        };
        let Some((_, lon)) =
            dms_run(&f[lat_i + 4..], "E", false).or_else(|| dms_run(&f[lat_i + 4..], "W", true))
        else {
            report.skip(row, "no longitude");
            continue;
        };
        if !seen.insert((callsign.to_string(), freq_hz.round() as i64)) {
            report.skip(row, "duplicate station row");
            continue;
        }
        let facility = lat_i
            .checked_sub(1)
            .and_then(|i| f.get(i))
            .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
            .copied();
        let mut s = Station::new(
            Source::Fcc,
            facility
                .map(str::to_string)
                .unwrap_or_else(|| format!("{callsign}:{freq_hz}")),
            callsign.to_string(),
            freq_hz,
        );
        s.modulation = if service.to_ascii_uppercase().starts_with("AM") {
            Modulation::Am
        } else {
            Modulation::Wfm
        };
        s.service = Service::Broadcast;
        s.lat = Some(lat);
        s.lon = Some(lon);
        s.country = Some("US".into());
        s.power_kw = f.get(14).and_then(|v| power_kw(v));
        let field_at = |back: usize| lat_i.checked_sub(back).and_then(|i| f.get(i)).copied();
        s.notes = [
            field_at(9),               // city
            field_at(8),               // state
            f.get(lat_i + 8).copied(), // licensee
        ]
        .into_iter()
        .flatten()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        report.kept += 1;
        stations.push(s);
    }
    (stations, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from the live service 2026-09-19 (public domain).
    const OUTPUT: &str = "\
|KUVO        |89.3  MHz |FM |207 |DA  |H                   |C1 |-  |LIC    |DENVER                   |CO |US |BLED-20181105AAP    |12.    kW |11.38  kW |342.0   |342.0   |16687      |N |39 |40 |24.3  |W |105 |13 |4.5   |ROCKY MOUNTAIN PUBLIC MEDIA, INC.                                           |   0.00 km |   0.00 mi |  0.00 deg |2364.  m|2364.0 m|-         |0.      |-       |       m|201811058 |7420f64609524d2992a6c1b6063ebeb7   |ed03f234c9ea40f7ada5c1b6063ebeb7   |
|KAZN        |1300  kHz |AM |    |DAY |Daytime             |B  |-  |LIC    |PASADENA                 |CA |US |BL-20130718AHX         |23.0   kW |Directional|        |       |51426      |N |34 |7  |8.0   |W |118 |4  |57.2  |MULTICULTURAL RADIO BROADCASTING LICENSEE, LLC                              |   0.00 km |   0.00 mi |  0.00 deg |1565198   |f0d0b81bc85a430a8dd7c1b6063ebeb7   |bac674a09acf4f90ac85c1b6063ebeb7   |
|BROKEN|FM|nowhere
";

    #[test]
    fn a_pipe_dump_becomes_stations() {
        let (stations, report) = parse(OUTPUT.as_bytes());
        assert_eq!(report.rows, 3);
        assert_eq!(report.kept, 2);
        assert_eq!(report.skipped.len(), 1);
        let fm = &stations[0];
        assert_eq!(fm.name, "KUVO");
        assert_eq!(fm.freq_hz, 89.3e6);
        assert_eq!(fm.modulation, Modulation::Wfm);
        assert_eq!(fm.id, "16687");
        assert_eq!(fm.lat, Some(39.0 + 40.0 / 60.0 + 24.3 / 3600.0));
        assert_eq!(fm.lon, Some(-(105.0 + 13.0 / 60.0 + 4.5 / 3600.0)));
        assert_eq!(fm.power_kw, Some(12.0));
        assert_eq!(fm.country.as_deref(), Some("US"));
        assert_eq!(fm.notes, "DENVER · CO · ROCKY MOUNTAIN PUBLIC MEDIA, INC.");
        let am = &stations[1];
        assert_eq!(am.freq_hz, 1300e3);
        assert_eq!(am.modulation, Modulation::Am);
        assert_eq!(am.id, "51426");
        assert!(am.lat.unwrap() > 34.0);
        assert_eq!(
            am.notes,
            "PASADENA · CA · MULTICULTURAL RADIO BROADCASTING LICENSEE, LLC"
        );
        assert!(report.skipped[0].1.contains("no frequency"));
    }

    #[test]
    fn the_radius_query_speaks_the_forms_dialect() {
        let q = search_query(
            LatLon {
                lat: 38.7219,
                lon: -9.1393,
            },
            150.0,
            "FM",
        );
        let get = |k: &str| {
            q.iter()
                .find(|(a, _)| a == k)
                .map(|(_, b)| b.as_str())
                .unwrap()
        };
        assert_eq!(get("serv"), "FM");
        assert_eq!(get("list"), "4");
        assert_eq!(get("dist"), "150");
        assert_eq!(get("dlat2"), "38");
        assert_eq!(get("mlat2"), "43");
        assert!(get("slat2").starts_with("18.8"));
        assert_eq!(get("NS"), "N");
        assert_eq!(get("dlon2"), "9");
        assert_eq!(get("EW"), "W");
    }

    #[test]
    fn dms_parts_carry_seconds_into_minutes() {
        // 38.99999° is 38° 59' 60.0" before the carry.
        let (d, m, s) = dms_parts(38.999_999);
        assert_eq!((d, m), (39, 0));
        assert_eq!(s, 0.0);
    }
}
