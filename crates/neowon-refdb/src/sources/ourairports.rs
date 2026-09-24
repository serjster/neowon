//! OurAirports importer (public domain): `airport-frequencies.csv`
//! joined to `airports.csv` on the airport id, for coordinates and names.

use std::collections::HashMap;

use super::Report;
use super::csv;
use crate::station::{Modulation, Service, Source, Station};

pub const URL_FREQUENCIES: &str =
    "https://davidmegginson.github.io/ourairports-data/airport-frequencies.csv";
pub const URL_AIRPORTS: &str = "https://davidmegginson.github.io/ourairports-data/airports.csv";

struct Airport {
    ident: String,
    name: String,
    lat: f64,
    lon: f64,
    country: Option<String>,
}

fn airports(bytes: &[u8]) -> HashMap<String, Airport> {
    let text = String::from_utf8_lossy(bytes);
    let rows = csv::parse(&text, b',');
    let mut out = HashMap::new();
    let Some(head) = rows.first() else {
        return out;
    };
    let (id, ident, name, lat, lon, country) = (
        csv::find(head, &["id"]),
        csv::find(head, &["ident"]),
        csv::find(head, &["name"]),
        csv::find(head, &["latitude_deg", "latitude"]),
        csv::find(head, &["longitude_deg", "longitude"]),
        csv::find(head, &["iso_country", "country"]),
    );
    for row in &rows[1..] {
        let (Some(id), Some(a)) = (
            csv::cell(row, id),
            match (csv::cell(row, lat), csv::cell(row, lon)) {
                (Some(a), Some(b)) => match (a.parse::<f64>(), b.parse::<f64>()) {
                    (Ok(a), Ok(b)) => Some((a, b)),
                    _ => None,
                },
                _ => None,
            },
        ) else {
            continue;
        };
        out.insert(
            id.to_string(),
            Airport {
                ident: csv::cell(row, ident).unwrap_or_default().to_string(),
                name: csv::cell(row, name).unwrap_or_default().to_string(),
                lat: a.0,
                lon: a.1,
                country: csv::cell(row, country).map(str::to_string),
            },
        );
    }
    out
}

pub fn parse(frequencies: &[u8], airports_csv: &[u8]) -> (Vec<Station>, Report) {
    let apts = airports(airports_csv);
    let text = String::from_utf8_lossy(frequencies);
    let rows = csv::parse(&text, b',');
    let mut stations = Vec::new();
    let mut report = Report::default();
    let Some(head) = rows.first() else {
        return (stations, report);
    };
    let (id, reference, ident, kind, description) = (
        csv::find(head, &["id"]),
        csv::find(head, &["airport_ref"]),
        csv::find(head, &["airport_ident", "ident"]),
        csv::find(head, &["type"]),
        csv::find(head, &["description"]),
    );
    let mhz = csv::find(head, &["frequency_mhz"]);
    let khz = csv::find(head, &["frequency_khz"]);
    for (i, row) in rows[1..].iter().enumerate() {
        let line = i + 2;
        report.rows += 1;
        let Some(reference) = csv::cell(row, reference) else {
            report.skip(line, "no airport_ref");
            continue;
        };
        let Some(apt) = apts.get(reference) else {
            report.skip(line, format!("airport {reference} not in airports.csv"));
            continue;
        };
        let freq_hz = csv::cell(row, mhz)
            .and_then(|v| v.parse::<f64>().ok())
            .map(|m| m * 1e6)
            .or_else(|| {
                csv::cell(row, khz)
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|k| k * 1e3)
            });
        let Some(freq_hz) = freq_hz.filter(|f| *f > 0.0) else {
            report.skip(line, "no frequency");
            continue;
        };
        let kind = csv::cell(row, kind).unwrap_or("");
        let icao = csv::cell(row, ident).unwrap_or(&apt.ident);
        let name = if apt.name.is_empty() {
            format!("{icao} {kind}").trim().to_string()
        } else {
            format!("{icao} {kind} — {}", apt.name)
        };
        let mut s = Station::new(
            Source::OurAirports,
            csv::cell(row, id)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{:016x}", super::fnv1a(&name))),
            name,
            freq_hz,
        );
        s.modulation = Modulation::Am;
        s.service = Service::Aviation;
        s.lat = Some(apt.lat);
        s.lon = Some(apt.lon);
        s.country = apt.country.clone();
        s.notes = csv::cell(row, description).unwrap_or("").to_string();
        report.kept += 1;
        stations.push(s);
    }
    (stations, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AIRPORTS: &str = "\
id,ident,type,name,latitude_deg,longitude_deg,iso_country
1,LPPT,large_airport,Lisbon Airport,38.7813,-9.13592,PT
2,LPPR,medium_airport,Porto Airport,41.2481,-8.68139,PT
3,BAD,small_airport,No Coordinates,,,PT
";

    const FREQUENCIES: &str = "\
id,airport_ref,airport_ident,type,description,frequency_mhz,frequency_khz
10,1,LPPT,TWR,Lisbon Tower,118.1,
11,1,LPPT,ATIS,Lisbon Information,,126500
12,2,LPPR,APP,Porto Approach,119.9,
13,3,BAD,TWR,Bad Airport,118.0,
14,99,XXXX,TWR,Nowhere,118.0,
";

    #[test]
    fn joins_the_two_files_and_keeps_khz_frequencies() {
        let (stations, report) = parse(FREQUENCIES.as_bytes(), AIRPORTS.as_bytes());
        assert_eq!(report.rows, 5);
        assert_eq!(report.kept, 3);
        assert_eq!(report.skipped.len(), 2);
        let s = &stations[0];
        assert_eq!(s.name, "LPPT TWR — Lisbon Airport");
        assert_eq!(s.freq_hz, 118.1e6);
        assert_eq!(s.service, Service::Aviation);
        assert_eq!(s.modulation, Modulation::Am);
        assert_eq!(s.lat, Some(38.7813));
        assert_eq!(s.country.as_deref(), Some("PT"));
        assert_eq!(s.id, "10");
        assert_eq!(stations[1].freq_hz, 126.5e6);
        assert_eq!(stations[2].freq_hz, 119.9e6);
        assert!(
            report.skipped[0].1.contains("airport 3"),
            "{:?}",
            report.skipped
        );
        assert!(
            report.skipped[1].1.contains("airport 99"),
            "{:?}",
            report.skipped
        );
    }

    #[test]
    fn empty_input_is_not_a_panic() {
        let (stations, report) = parse(b"", b"");
        assert!(stations.is_empty());
        assert_eq!(report.rows, 0);
    }
}
