//! Wikidata importer (D17): SPARQL JSON around a location. Wikidata is
//! CC0; broadcast FM/AM/TV stations with a frequency (P2144) and
//! coordinates (P625) become stations. Labels come from the query's label
//! service (operator language, then English).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;

use super::Report;
use crate::geo::LatLon;
use crate::station::{Modulation, Service, Source, Station};

pub const ENDPOINT: &str = "https://query.wikidata.org/sparql";

/// Checked against Wikidata on 2026-09-19.
const RADIO_STATION: &str = "Q14350";
const TV_STATION: &str = "Q1616075";

/// Frequency unit entities → multiplier to Hz. Checked 2026-09-19.
fn unit_scale(qid: &str) -> Option<f64> {
    Some(match qid {
        "Q39369" => 1.0,   // hertz
        "Q2143992" => 1e3, // kilohertz
        "Q732707" => 1e6,  // megahertz
        "Q3276763" => 1e9, // gigahertz
        _ => return None,
    })
}

/// The SPARQL returning one row per (station, frequency) within
/// `radius_km` of `center`, with the unit entity so the value can be
/// normalised (`query_lang` names the label languages, best first).
pub fn query(center: LatLon, radius_km: f64) -> String {
    query_lang(center, radius_km, "en")
}

pub fn query_lang(center: LatLon, radius_km: f64, langs: &str) -> String {
    format!(
        r#"SELECT ?item ?itemLabel ?freq ?freqUnit ?coord ?country ?countryLabel ?callsign WHERE {{
  SERVICE wikibase:around {{
    ?item wdt:P625 ?coord .
    bd:serviceParam wikibase:center "Point({lon} {lat})"^^geo:wktLiteral .
    bd:serviceParam wikibase:radius "{radius}" .
  }}
  ?item wdt:P31/wdt:P279* ?class .
  VALUES ?class {{ wd:{RADIO_STATION} wd:{TV_STATION} }}
  ?item p:P2144 ?statement .
  ?statement ps:P2144 ?freq .
  OPTIONAL {{ ?statement psv:P2144 ?value . ?value wikibase:quantityUnit ?freqUnit }}
  OPTIONAL {{ ?item wdt:P17 ?country }}
  OPTIONAL {{ ?item wdt:P2317 ?callsign }}
  SERVICE wikibase:label {{ bd:serviceParam wikibase:language "{langs}". }}
}}"#,
        lon = center.lon,
        lat = center.lat,
        radius = radius_km,
    )
}

#[derive(Deserialize)]
struct Sparql {
    results: Results,
}

#[derive(Deserialize)]
struct Results {
    bindings: Vec<BTreeMap<String, Binding>>,
}

#[derive(Deserialize)]
struct Binding {
    value: String,
}

fn qid(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or(uri)
}

/// "Point(-9.14 38.72)" — longitude first, as WKT.
fn wkt_point(wkt: &str) -> Option<LatLon> {
    let inner = wkt
        .trim()
        .strip_prefix("Point(")
        .and_then(|s| s.strip_suffix(')'))?;
    let mut it = inner.split_whitespace();
    let lon: f64 = it.next()?.parse().ok()?;
    let lat: f64 = it.next()?.parse().ok()?;
    Some(LatLon { lat, lon })
}

/// Broadcast bands the source does not label: FM broadcast, medium wave,
/// and the TV bands (DVB-T).
fn infer_modulation(hz: f64) -> Modulation {
    if (88e6..=108e6).contains(&hz) {
        Modulation::Wfm
    } else if (148.5e3..=1.7e6).contains(&hz) {
        Modulation::Am
    } else if (174e6..=230e6).contains(&hz) || (470e6..=862e6).contains(&hz) {
        Modulation::Dvbt
    } else {
        Modulation::Unknown
    }
}

/// One station per (item, frequency); an item with several frequencies is
/// numbered `Q…#1`, `#2`, in frequency order (D17 risk note).
pub fn parse(bytes: &[u8]) -> (Vec<Station>, Report) {
    let doc: Sparql = match serde_json::from_slice(bytes) {
        Ok(doc) => doc,
        Err(e) => {
            let mut report = Report::default();
            report.skip(0, format!("sparql json: {e}"));
            return (Vec::new(), report);
        }
    };
    let mut report = Report {
        rows: doc.results.bindings.len(),
        kept: 0,
        skipped: Vec::new(),
    };
    let mut candidates: Vec<(String, f64, Station)> = Vec::new();
    for (i, b) in doc.results.bindings.iter().enumerate() {
        let row = i + 1;
        let get = |k: &str| b.get(k).map(|v| v.value.as_str());
        let Some(item) = get("item") else {
            report.skip(row, "no item");
            continue;
        };
        let item = qid(item).to_string();
        let Some(freq) = get("freq").and_then(|v| v.parse::<f64>().ok()) else {
            report.skip(row, "no parseable frequency");
            continue;
        };
        let scale = match get("freqUnit").map(qid) {
            Some(u) => match unit_scale(u) {
                Some(s) => s,
                None => {
                    report.skip(row, format!("unknown frequency unit {u}"));
                    continue;
                }
            },
            None => 1.0, // a quantity without a unit is hertz
        };
        let freq_hz = freq * scale;
        if !(freq_hz.is_finite() && freq_hz > 0.0) {
            report.skip(row, format!("frequency {freq_hz} Hz"));
            continue;
        }
        let callsign = get("callsign").map(str::to_string);
        let name = match get("itemLabel") {
            Some(l) if !l.trim().is_empty() && l != item => l.to_string(),
            _ => callsign.clone().unwrap_or_else(|| item.clone()),
        };
        let point = get("coord").and_then(wkt_point);
        let mut s = Station::new(Source::Wikidata, item.clone(), name, freq_hz);
        s.modulation = infer_modulation(freq_hz);
        s.service = Service::Broadcast;
        s.lat = point.map(|p| p.lat);
        s.lon = point.map(|p| p.lon);
        s.country = get("countryLabel").map(str::to_string);
        s.callsign = callsign;
        candidates.push((item, freq_hz, s));
    }

    // Duplicate join rows are not stations: keep the first per (item, Hz).
    let mut seen = HashSet::new();
    let mut uniq: Vec<(String, Station)> = Vec::new();
    for (item, freq, s) in candidates {
        if seen.insert((item.clone(), freq.round() as i64)) {
            uniq.push((item, s));
        }
    }
    // SPARQL row order is not guaranteed: sort before numbering so `#1`
    // and `#2` mean the same stations on every import.
    uniq.sort_by(|(ia, a), (ib, b)| ia.cmp(ib).then_with(|| a.freq_hz.total_cmp(&b.freq_hz)));
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (item, _) in &uniq {
        *counts.entry(item.clone()).or_default() += 1;
    }
    let mut numbered: HashMap<String, usize> = HashMap::new();
    let mut stations = Vec::with_capacity(uniq.len());
    for (item, mut s) in uniq {
        if counts[&item] > 1 {
            let n = numbered.entry(item.clone()).or_default();
            *n += 1;
            s.id = format!("{item}#{n}");
        }
        report.kept += 1;
        stations.push(s);
    }
    (stations, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"head":{"vars":["item","itemLabel","freq","freqUnit","coord","countryLabel","callsign"]},
      "results":{"bindings":[
        {"item":{"type":"uri","value":"http://www.wikidata.org/entity/Q1001"},
         "itemLabel":{"xml:lang":"pt","type":"literal","value":"Antena 1"},
         "freq":{"type":"literal","datatype":"http://www.w3.org/2001/XMLSchema#decimal","value":"99.4"},
         "freqUnit":{"type":"uri","value":"http://www.wikidata.org/entity/Q732707"},
         "coord":{"type":"literal","datatype":"http://www.opengis.net/ont/geosparql#wktLiteral","value":"Point(-9.14 38.72)"},
         "countryLabel":{"xml:lang":"en","type":"literal","value":"Portugal"},
         "callsign":{"type":"literal","value":"CSB1"}},
        {"item":{"type":"uri","value":"http://www.wikidata.org/entity/Q1002"},
         "itemLabel":{"xml:lang":"en","type":"literal","value":"Rádio Comercial"},
         "freq":{"type":"literal","value":"100300000"},
         "freqUnit":{"type":"uri","value":"http://www.wikidata.org/entity/Q39369"},
         "coord":{"type":"literal","value":"Point(-9.15 38.75)"}},
        {"item":{"type":"uri","value":"http://www.wikidata.org/entity/Q1003"},
         "itemLabel":{"xml:lang":"en","type":"literal","value":"Some MW"},
         "freq":{"type":"literal","value":"720"},
         "freqUnit":{"type":"uri","value":"http://www.wikidata.org/entity/Q2143992"},
         "coord":{"type":"literal","value":"Point(-9.0 38.0)"}},
        {"item":{"type":"uri","value":"http://www.wikidata.org/entity/Q1004"},
         "itemLabel":{"xml:lang":"en","type":"literal","value":"Bad unit"},
         "freq":{"type":"literal","value":"1"},
         "freqUnit":{"type":"uri","value":"http://www.wikidata.org/entity/Q99999999"},
         "coord":{"type":"literal","value":"Point(-9.0 38.0)"}}
      ]}}"#;

    #[test]
    fn scales_through_the_unit_entity() {
        let (stations, report) = parse(SAMPLE.as_bytes());
        assert_eq!(report.rows, 4);
        assert_eq!(report.kept, 3);
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].1.contains("unknown frequency unit"));
        assert_eq!(stations[0].freq_hz, 99.4e6);
        assert_eq!(stations[0].modulation, Modulation::Wfm);
        assert_eq!(stations[0].name, "Antena 1");
        assert_eq!(stations[0].id, "Q1001");
        assert_eq!(stations[0].callsign.as_deref(), Some("CSB1"));
        assert_eq!(stations[0].lat, Some(38.72));
        assert_eq!(stations[0].lon, Some(-9.14));
        assert_eq!(stations[1].freq_hz, 100.3e6);
        assert_eq!(stations[2].freq_hz, 720e3);
        assert_eq!(stations[2].modulation, Modulation::Am);
        assert!(stations.iter().all(|s| s.source == Source::Wikidata));
    }

    #[test]
    fn one_item_with_two_frequencies_is_numbered_by_frequency() {
        let json = r#"{"results":{"bindings":[
          {"item":{"type":"uri","value":"http://www.wikidata.org/entity/Q1"},
           "itemLabel":{"type":"literal","value":"Network"},
           "freq":{"type":"literal","value":"99.4"},
           "freqUnit":{"type":"uri","value":"http://www.wikidata.org/entity/Q732707"},
           "coord":{"type":"literal","value":"Point(-9.14 38.72)"}},
          {"item":{"type":"uri","value":"http://www.wikidata.org/entity/Q1"},
           "itemLabel":{"type":"literal","value":"Network"},
           "freq":{"type":"literal","value":"90.4"},
           "freqUnit":{"type":"uri","value":"http://www.wikidata.org/entity/Q732707"},
           "coord":{"type":"literal","value":"Point(-9.14 38.72)"}}
        ]}}"#;
        let (stations, report) = parse(json.as_bytes());
        assert_eq!(report.kept, 2);
        // Numbered by frequency, not by row order: stable across imports.
        assert_eq!(stations[0].id, "Q1#1");
        assert_eq!(stations[0].freq_hz, 90.4e6);
        assert_eq!(stations[1].id, "Q1#2");
        assert_eq!(stations[1].freq_hz, 99.4e6);
    }

    #[test]
    fn query_carries_the_place_and_the_radius() {
        let q = query(
            LatLon {
                lat: 38.72,
                lon: -9.14,
            },
            150.0,
        );
        assert!(q.contains(r#"Point(-9.14 38.72)"^^geo:wktLiteral"#), "{q}");
        assert!(q.contains(r#"wikibase:radius "150""#));
        assert!(q.contains("wd:Q14350"));
        assert!(q.contains("wd:Q1616075"));
        assert!(!q.contains("P1920"), "P1920 is not a callsign");
    }

    #[test]
    fn a_garbage_body_is_a_report_not_a_panic() {
        let (stations, report) = parse(b"{not json");
        assert!(stations.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }
}
