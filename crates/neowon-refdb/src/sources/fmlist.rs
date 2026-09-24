//! FMLIST importer: **import only** — the operator exports the file
//! with their own account. No column names are fixed: the header decides
//! what each column means (unknown columns are ignored), and a row without
//! a frequency is skipped and counted in the report.

use super::csv;
use super::{Report, fnv1a};
use crate::station::{Modulation, Service, Source, Station};

/// A number from a European-styled cell: "99,4", "99.4 MHz", "100000".
fn number(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    if let Ok(v) = s.parse::<f64>() {
        return v.is_finite().then_some(v);
    }
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
        .collect();
    cleaned.parse().ok().filter(|v: &f64| v.is_finite())
}

fn modulation(s: &str) -> Modulation {
    let u = s.trim().to_ascii_uppercase();
    if u.contains("DAB") {
        Modulation::Dab
    } else if u.contains("DVB") {
        Modulation::Dvbt
    } else if u.contains("DRM") {
        Modulation::Digital
    } else if u.contains("NFM") {
        Modulation::Nfm
    } else if u.contains("WFM") || u.contains("FM") {
        Modulation::Wfm
    } else if u.contains("USB") {
        Modulation::Usb
    } else if u.contains("LSB") {
        Modulation::Lsb
    } else if u.contains("CW") {
        Modulation::Cw
    } else if u.contains("AM") {
        Modulation::Am
    } else {
        Modulation::Unknown
    }
}

pub fn parse(bytes: &[u8]) -> (Vec<Station>, Report) {
    let text = String::from_utf8_lossy(bytes);
    let mut report = Report::default();
    let Some(first) = text.lines().next() else {
        return (Vec::new(), report);
    };
    let delim = csv::detect_delimiter(first);
    let rows = csv::parse(&text, delim);
    let Some(head) = rows.first() else {
        return (Vec::new(), report);
    };
    let id = csv::find(head, &["fmlist id", "station id", "id"]);
    let name = csv::find(
        head,
        &[
            "programme",
            "program",
            "station name",
            "station",
            "name",
            "sender",
        ],
    );
    let freq = csv::find(head, &["frequency", "frequenz", "freq", "mhz", "khz"]);
    let unit = freq
        .and_then(|i| head.get(i))
        .map(|h| h.to_ascii_lowercase())
        .unwrap_or_default();
    let (khz_header, mhz_header) = (unit.contains("khz"), unit.contains("mhz"));
    let country = csv::find(head, &["country code", "country", "itu", "iso"]);
    let lat = csv::find(head, &["latitude", "lat"]);
    let lon = csv::find(head, &["longitude", "lon", "lng"]);
    let power = csv::find(head, &["power", "erp", "pwr", "kw"]);
    let modulation_col = csv::find(head, &["modulation", "mod", "mode"]);
    let callsign = csv::find(head, &["call sign", "callsign"]);
    let notes = csv::find(head, &["notes", "remarks", "remark", "comment"]);

    let mut stations = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(1) {
        let line = i + 1;
        report.rows += 1;
        let Some(freq_hz) = csv::cell(row, freq).and_then(number).map(|v| {
            if khz_header {
                v * 1e3
            } else if mhz_header || v < 2000.0 {
                v * 1e6
            } else {
                // A generic "Frequency" column holding 106000 is kHz.
                v * 1e3
            }
        }) else {
            report.skip(line, "no frequency");
            continue;
        };
        let station_name = csv::cell(row, name)
            .or_else(|| csv::cell(row, callsign))
            .unwrap_or("")
            .to_string();
        let mut s = Station::new(
            Source::Fmlist,
            csv::cell(row, id)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{:016x}", fnv1a(&format!("{station_name}|{freq_hz}")))),
            station_name,
            freq_hz,
        );
        s.modulation = csv::cell(row, modulation_col)
            .map(modulation)
            .unwrap_or(Modulation::Unknown);
        s.service = Service::Broadcast;
        s.country = csv::cell(row, country).map(str::to_string);
        s.lat = csv::cell(row, lat).and_then(number);
        s.lon = csv::cell(row, lon).and_then(number);
        s.power_kw = csv::cell(row, power).and_then(number);
        s.callsign = csv::cell(row, callsign).map(str::to_string);
        s.notes = csv::cell(row, notes).unwrap_or("").to_string();
        report.kept += 1;
        stations.push(s);
    }
    (stations, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_survive_european_and_annotated_cells() {
        assert_eq!(number("99,4"), Some(99.4));
        assert_eq!(number("99.4 MHz"), Some(99.4));
        assert_eq!(number("100000"), Some(100_000.0));
        assert_eq!(number(""), None);
    }

    #[test]
    fn modes_map_to_the_vocabulary() {
        assert_eq!(modulation("NFM"), Modulation::Nfm);
        assert_eq!(modulation("FM"), Modulation::Wfm);
        assert_eq!(modulation("DAB+"), Modulation::Dab);
        assert_eq!(modulation(""), Modulation::Unknown);
    }

    #[test]
    fn a_generic_frequency_column_guesses_from_magnitude() {
        let csv = "Programme;Frequency\nA;99.4\nB;106000\nC;12\n";
        let (stations, report) = parse(csv.as_bytes());
        assert_eq!(report.kept, 3);
        assert_eq!(stations[0].freq_hz, 99.4e6);
        assert_eq!(stations[1].freq_hz, 106e6);
        assert_eq!(stations[2].freq_hz, 12e6);
    }

    #[test]
    fn a_header_mapped_semicolon_export_parses() {
        let csv = "\
FMLIST ID;Programme;Frequency (MHz);Country;Latitude;Longitude;ERP (kW);Modulation;Notes
p1;Antena 1;99,4;PT;38.72;-9.14;12.5;FM;site A
p2;Some DAB;12;PT;;;1;DAB+;block 12B
p3;No Freq;;PT;;;1;FM;broken row
";
        let (stations, report) = parse(csv.as_bytes());
        assert_eq!(report.rows, 3);
        assert_eq!(report.kept, 2);
        assert_eq!(report.skipped, vec![(4, "no frequency".to_string())]);
        assert_eq!(stations[0].id, "p1");
        assert_eq!(stations[0].freq_hz, 99.4e6);
        assert_eq!(stations[0].lat, Some(38.72));
        assert_eq!(stations[0].power_kw, Some(12.5));
        assert_eq!(stations[1].freq_hz, 12e6);
        assert_eq!(stations[1].modulation, Modulation::Dab);
    }
}
