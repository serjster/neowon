//! Each importer parses its fixture to a golden count
//! and to field-exact golden stations, malformed rows are skipped with a
//! reason, and **every** truncation of every fixture (all byte prefixes, a
//! superset of every line boundary) parses without panicking.

use neowon_refdb::sources::{eibi, fcc, fmlist, ourairports, wikidata};
use neowon_refdb::station::{Modulation, Schedule, Service, Source, Station};

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A station with only the fields the importer under test fills.
fn station(source: Source, id: &str, name: &str, freq_hz: f64) -> Station {
    Station::new(source, id.into(), name.into(), freq_hz)
}

#[test]
fn wikidata_fixture_matches_its_golden_rows() {
    let (stations, report) = wikidata::parse(&fixture("wikidata.json"));
    assert_eq!(report.rows, 7);
    assert_eq!(report.kept, 5);
    assert_eq!(report.skipped.len(), 2, "{:?}", report.skipped);
    assert!(report.skipped[0].1.contains("unknown frequency unit"));
    assert!(report.skipped[1].1.contains("no parseable frequency"));

    let mut a = station(Source::Wikidata, "Q1001", "Antena 1", 99.4e6);
    a.modulation = Modulation::Wfm;
    a.service = Service::Broadcast;
    a.lat = Some(38.72);
    a.lon = Some(-9.14);
    a.country = Some("Portugal".into());
    a.callsign = Some("CSB1".into());
    assert_eq!(stations[0], a);

    let mut b = station(Source::Wikidata, "Q1002", "Rádio Renascença", 103.4e6);
    b.modulation = Modulation::Wfm;
    b.service = Service::Broadcast;
    b.lat = Some(38.75);
    b.lon = Some(-9.15);
    b.country = Some("Portugal".into());
    assert_eq!(stations[1], b);

    let mut c = station(Source::Wikidata, "Q1003", "RDP África", 720e3);
    c.modulation = Modulation::Am;
    c.service = Service::Broadcast;
    c.lat = Some(38.0);
    c.lon = Some(-9.0);
    c.country = Some("Portugal".into());
    assert_eq!(stations[2], c);

    // A station with no label falls back to its callsign; a TV band is
    // inferred DVB-T.
    assert_eq!(stations[3].id, "Q1005");
    assert_eq!(stations[3].name, "CSX9");
    assert_eq!(stations[4].modulation, Modulation::Dvbt);
    assert_eq!(stations[4].freq_hz, 650e6);
}

#[test]
fn eibi_fixture_is_decoded_from_latin1_and_scheduled() {
    let (stations, report) = eibi::parse(&fixture("eibi.csv"));
    assert_eq!(report.rows, 4);
    assert_eq!(report.kept, 3);
    assert_eq!(
        report.skipped,
        vec![(5, "bad frequency \"bogus\"".to_string())]
    );

    let mut a = station(Source::Eibi, "d459fbc3e933dc06", "Rádio Nacional", 5940e3);
    a.modulation = Modulation::Am;
    a.service = Service::Broadcast;
    a.country = Some("POR".into());
    a.power_kw = Some(100.0);
    a.schedule = Some(Schedule {
        start_min: 0,
        stop_min: 1440,
        days: Schedule::DAILY,
    });
    a.notes = "Por · Eu".into();
    assert_eq!(stations[0], a);

    let mut b = station(
        Source::Eibi,
        "f9fd02fe0a65ad1c",
        "Evangeliums-Rundfunk",
        6070e3,
    );
    b.modulation = Modulation::Usb;
    b.service = Service::Broadcast;
    b.country = Some("DEU".into());
    b.power_kw = Some(10.0);
    b.schedule = Some(Schedule {
        start_min: 300,
        stop_min: 360,
        days: 0b0110_0000, // Sa, Su
    });
    b.notes = "Ger · Eu · USB".into();
    assert_eq!(stations[1], b);

    let mut c = station(Source::Eibi, "60c2b2a2632243b6", "RFI", 17830e3);
    c.modulation = Modulation::Digital;
    c.service = Service::Broadcast;
    c.country = Some("FRA".into());
    c.power_kw = Some(100.0);
    c.schedule = Some(Schedule {
        start_min: 1020,
        stop_min: 1080,
        days: 0b0001_1111,
    });
    c.notes = "Fra · Af · DRM".into();
    assert_eq!(stations[2], c);
}

#[test]
fn ourairports_fixture_joins_coordinates_and_names() {
    let (stations, report) = ourairports::parse(
        &fixture("ourairports-frequencies.csv"),
        &fixture("ourairports-airports.csv"),
    );
    assert_eq!(report.rows, 7);
    assert_eq!(report.kept, 5);
    assert_eq!(report.skipped.len(), 2, "{:?}", report.skipped);
    assert!(report.skipped[0].1.contains("airport 99"));
    assert!(report.skipped[1].1.contains("no frequency"));

    let mut a = station(
        Source::OurAirports,
        "10",
        "LPPT TWR — Lisbon Airport",
        118.1e6,
    );
    a.modulation = Modulation::Am;
    a.service = Service::Aviation;
    a.lat = Some(38.7813);
    a.lon = Some(-9.13592);
    a.country = Some("PT".into());
    a.notes = "Lisbon Tower".into();
    assert_eq!(stations[0], a);

    let mut b = station(
        Source::OurAirports,
        "11",
        "LPPT ATIS — Lisbon Airport",
        126.5e6,
    );
    b.modulation = Modulation::Am;
    b.service = Service::Aviation;
    b.lat = Some(38.7813);
    b.lon = Some(-9.13592);
    b.country = Some("PT".into());
    b.notes = "Lisbon Information".into();
    assert_eq!(stations[1], b);

    let mut c = station(
        Source::OurAirports,
        "13",
        "LPPR GND — Porto Airport",
        121.9e6,
    );
    c.modulation = Modulation::Am;
    c.service = Service::Aviation;
    c.lat = Some(41.2481);
    c.lon = Some(-8.68139);
    c.country = Some("PT".into());
    c.notes = "Porto Ground".into();
    assert_eq!(stations[3], c);
}

#[test]
fn fcc_fixture_reads_dms_coordinates() {
    // Rows captured from the live fmq/amq service 2026-09-19 (public
    // domain); the parser sees the real, headerless layout.
    let (stations, report) = fcc::parse(&fixture("fcc.txt"));
    assert_eq!(report.rows, 4);
    assert_eq!(report.kept, 3);
    assert_eq!(report.skipped.len(), 1);

    let mut a = station(Source::Fcc, "16687", "KUVO", 89.3e6);
    a.modulation = Modulation::Wfm;
    a.service = Service::Broadcast;
    a.lat = Some(39.0 + 40.0 / 60.0 + 24.3 / 3600.0);
    a.lon = Some(-(105.0 + 13.0 / 60.0 + 4.5 / 3600.0));
    a.country = Some("US".into());
    a.power_kw = Some(12.0);
    a.notes = "DENVER · CO · ROCKY MOUNTAIN PUBLIC MEDIA, INC.".into();
    assert_eq!(stations[0], a);

    let mut b = station(Source::Fcc, "51426", "KAZN", 1300e3);
    b.modulation = Modulation::Am;
    b.service = Service::Broadcast;
    b.lat = Some(34.0 + 7.0 / 60.0 + 8.0 / 3600.0);
    b.lon = Some(-(118.0 + 4.0 / 60.0 + 57.2 / 3600.0));
    b.country = Some("US".into());
    b.power_kw = Some(23.0);
    b.notes = "PASADENA · CA · MULTICULTURAL RADIO BROADCASTING LICENSEE, LLC".into();
    assert_eq!(stations[1], b);

    let mut c = station(Source::Fcc, "35501", "KQED-FM", 88.5e6);
    c.modulation = Modulation::Wfm;
    c.service = Service::Broadcast;
    c.lat = Some(37.0 + 41.0 / 60.0 + 22.8 / 3600.0);
    c.lon = Some(-(122.0 + 26.0 / 60.0 + 16.9 / 3600.0));
    c.country = Some("US".into());
    c.power_kw = Some(110.0);
    c.notes = "SAN FRANCISCO · CA · KQED INC.".into();
    assert_eq!(stations[2], c);
    assert!(report.skipped[0].1.contains("no frequency"));
}

#[test]
fn fmlist_fixture_is_header_mapped() {
    let (stations, report) = fmlist::parse(&fixture("fmlist.csv"));
    assert_eq!(report.rows, 4);
    assert_eq!(report.kept, 3);
    assert_eq!(report.skipped, vec![(5, "no frequency".to_string())]);

    let mut a = station(Source::Fmlist, "p1", "Antena 1", 99.4e6);
    a.modulation = Modulation::Wfm;
    a.service = Service::Broadcast;
    a.lat = Some(38.72);
    a.lon = Some(-9.14);
    a.country = Some("PT".into());
    a.power_kw = Some(12.5);
    a.notes = "Monsanto".into();
    assert_eq!(stations[0], a);

    let mut b = station(Source::Fmlist, "p2", "Rádio Comercial", 97.4e6);
    b.modulation = Modulation::Wfm;
    b.service = Service::Broadcast;
    b.lat = Some(41.16);
    b.lon = Some(-8.63);
    b.country = Some("PT".into());
    b.power_kw = Some(10.0);
    b.notes = "Porto".into();
    assert_eq!(stations[1], b);

    assert_eq!(stations[2].id, "p3");
    assert_eq!(stations[2].freq_hz, 106e6);
    assert_eq!(stations[2].power_kw, Some(5.0));
}

/// Every fixture at every byte prefix — a superset of every line boundary.
/// A malformed row must reach the report; a panic fails the suite.
#[test]
fn truncated_fixtures_never_panic() {
    type Case = (&'static str, Vec<u8>, fn(&[u8]));
    let airports = fixture("ourairports-airports.csv");
    let frequencies = fixture("ourairports-frequencies.csv");
    let cases: [Case; 6] = [
        ("wikidata.json", fixture("wikidata.json"), |b| {
            wikidata::parse(b);
        }),
        ("eibi.csv", fixture("eibi.csv"), |b| {
            eibi::parse(b);
        }),
        ("ourairports-frequencies.csv", frequencies.clone(), |b| {
            ourairports::parse(b, &fixture("ourairports-airports.csv"));
        }),
        ("ourairports-airports.csv", airports.clone(), |b| {
            ourairports::parse(&fixture("ourairports-frequencies.csv"), b);
        }),
        ("fcc.txt", fixture("fcc.txt"), |b| {
            fcc::parse(b);
        }),
        ("fmlist.csv", fixture("fmlist.csv"), |b| {
            fmlist::parse(b);
        }),
    ];
    for (name, bytes, run) in &cases {
        assert!(bytes.len() > 100, "{name} looks empty");
        for n in 0..=bytes.len() {
            run(&bytes[..n]);
        }
    }
}
