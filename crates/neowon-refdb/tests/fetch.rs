//! Every fetch path runs against a local `TcpListener` stub serving the
//! importer fixtures (exact station equality), an HTTP 500 is retried once
//! and then succeeds, a persistent failure names its source, and
//! `locate_ip` reads the stub's JSON. No test in this file touches the
//! public network.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use neowon_refdb::fetch::{BaseUrls, FetchTarget, fetch, locate_ip};
use neowon_refdb::geo::{LatLon, LocationSource};
use neowon_refdb::sources::{eibi, fcc, ourairports, wikidata};
use neowon_refdb::station::Source;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .expect("fixture")
}

struct Reply {
    status: u16,
    /// What the stub promises in Content-Length; larger than the body is a
    /// truncated response.
    declared: usize,
    body: Vec<u8>,
}

impl Reply {
    fn ok(body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        Self {
            status: 200,
            declared: body.len(),
            body,
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            declared: 0,
            body: Vec::new(),
        }
    }

    fn truncated(body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        Self {
            status: 200,
            declared: body.len() + 4096,
            body,
        }
    }
}

/// A stub HTTP server: `n` connections at most, each answered by `reply`.
/// The request heads arrive on a channel for the test to inspect; the
/// thread never panics and always stops within its deadline, so a failed
/// assertion cannot hang the suite.
struct Stub {
    base: String,
    requests: mpsc::Receiver<String>,
    stop: Arc<AtomicBool>,
}

impl Stub {
    fn new<F>(n: usize, reply: F) -> Self
    where
        F: Fn(&str) -> Reply + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + Duration::from_secs(15);
        let flag = stop.clone();
        std::thread::spawn(move || {
            let mut served = 0;
            while served < n && Instant::now() < deadline && !flag.load(Ordering::SeqCst) {
                let (mut sock, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                served += 1;
                let _ = sock.set_read_timeout(Some(Duration::from_secs(3)));
                let mut head = String::new();
                let mut buf = [0u8; 8192];
                while !head.contains("\r\n\r\n") {
                    match sock.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.push_str(&String::from_utf8_lossy(&buf[..n])),
                    }
                }
                let path = head
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                let reply = reply(&path);
                let reason = if reply.status == 200 {
                    "OK"
                } else {
                    "Internal Server Error"
                };
                let http = format!(
                    "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.status, reply.declared
                );
                let _ = sock.write_all(http.as_bytes());
                let _ = sock.write_all(&reply.body);
                let _ = sock.flush();
                let _ = tx.send(head);
            }
        });
        Self {
            base,
            requests,
            stop,
        }
    }

    /// The request heads seen, waiting briefly for `n` of them.
    fn take(&self, n: usize) -> Vec<String> {
        let mut out = Vec::new();
        while out.len() < n {
            match self.requests.recv_timeout(Duration::from_secs(5)) {
                Ok(head) => out.push(head),
                Err(_) => break,
            }
        }
        out
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn path_of(head: &str) -> String {
    head.lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string()
}

fn target(center: Option<LatLon>) -> FetchTarget {
    FetchTarget {
        center,
        radius_km: 150.0,
        date: (2026, 1, 15),
    }
}

fn lisbon() -> LatLon {
    LatLon {
        lat: 38.7223,
        lon: -9.1393,
    }
}

#[test]
fn wikidata_fetch_equals_parse_on_the_served_fixture() {
    let fixture = fixture("wikidata.json");
    let served = fixture.clone();
    let stub = Stub::new(1, move |path| {
        assert!(path.contains("/sparql"), "{path}");
        assert!(path.contains("format=json"), "{path}");
        assert!(path.contains("wikibase"), "no SPARQL query in {path}");
        Reply::ok(served.clone())
    });
    let base = BaseUrls {
        wikidata: format!("{}/sparql", stub.base),
        ..BaseUrls::default()
    };
    let (stations, report) = fetch(Source::Wikidata, &target(Some(lisbon())), &base).unwrap();
    assert_eq!(stations, wikidata::parse(&fixture).0);
    assert_eq!(report.kept, 5);
    let heads = stub.take(1);
    assert!(
        heads[0].to_ascii_lowercase().contains(&format!(
            "user-agent: {}",
            neowon_refdb::fetch::user_agent().to_ascii_lowercase()
        )),
        "{}",
        heads[0]
    );
}

#[test]
fn eibi_fetch_picks_the_season_file_and_equals_parse() {
    let fixture = fixture("eibi.csv");
    let served = fixture.clone();
    let stub = Stub::new(1, move |path| {
        assert_eq!(path, "/dx/sked-b26.csv");
        Reply::ok(served.clone())
    });
    let base = BaseUrls {
        eibi: format!("{}/dx/", stub.base),
        ..BaseUrls::default()
    };
    let (stations, report) = fetch(Source::Eibi, &target(None), &base).unwrap();
    assert_eq!(stations, eibi::parse(&fixture).0);
    assert_eq!(report.kept, 3);
    stub.take(1);
}

#[test]
fn ourairports_fetch_joins_the_two_served_files() {
    let frequencies = fixture("ourairports-frequencies.csv");
    let airports = fixture("ourairports-airports.csv");
    let stub = Stub::new(2, move |path| {
        if path.contains("frequencies") {
            Reply::ok(frequencies.clone())
        } else {
            Reply::ok(airports.clone())
        }
    });
    let base = BaseUrls {
        ourairports_frequencies: format!("{}/frequencies.csv", stub.base),
        ourairports_airports: format!("{}/airports.csv", stub.base),
        ..BaseUrls::default()
    };
    let (stations, report) = fetch(Source::OurAirports, &target(None), &base).unwrap();
    assert_eq!(
        stations,
        ourairports::parse(
            &fixture("ourairports-frequencies.csv"),
            &fixture("ourairports-airports.csv")
        )
        .0
    );
    assert_eq!(report.kept, 5);
    let paths: Vec<String> = stub.take(2).iter().map(|h| path_of(h)).collect();
    assert_eq!(paths, ["/frequencies.csv", "/airports.csv"]);
}

#[test]
fn fcc_fetch_sends_the_radius_query_and_equals_parse() {
    let fixture = fixture("fcc.txt");
    let served = fixture.clone();
    let stub = Stub::new(2, move |path| {
        assert!(path.contains("list=4"), "{path}");
        assert!(path.contains("dist=150"), "{path}");
        if path.contains("/fmq") {
            assert!(path.contains("serv=FM"), "{path}");
            Reply::ok(served.clone())
        } else {
            assert!(path.contains("serv=AM"), "{path}");
            assert!(path.contains("dlat2=38"), "{path}");
            assert!(path.contains("EW=W"), "{path}");
            Reply::ok(Vec::new())
        }
    });
    let base = BaseUrls {
        fcc_fm: format!("{}/fmq", stub.base),
        fcc_am: format!("{}/amq", stub.base),
        ..BaseUrls::default()
    };
    let (stations, report) = fetch(Source::Fcc, &target(Some(lisbon())), &base).unwrap();
    // The AM endpoint served nothing, so the result is the FM fixture's.
    assert_eq!(stations, fcc::parse(&fixture).0);
    assert_eq!(report.kept, 3);
    stub.take(2);
}

#[test]
fn sources_that_need_a_location_say_so() {
    let base = BaseUrls::default();
    for src in [Source::Wikidata, Source::Fcc] {
        let err = fetch(src, &target(None), &base).unwrap_err();
        assert!(err.to_string().contains("needs a location"), "{err}");
        assert!(err.to_string().contains(src.label()), "{err}");
    }
    let err = fetch(Source::Fmlist, &target(None), &base).unwrap_err();
    assert!(err.to_string().contains("import-only"), "{err}");
}

#[test]
fn a_status_500_is_retried_once_and_then_succeeds() {
    let fixture = fixture("wikidata.json");
    let served = fixture.clone();
    let calls = Arc::new(AtomicBool::new(false));
    let first = calls.clone();
    let stub = Stub::new(2, move |_| {
        if !first.swap(true, Ordering::SeqCst) {
            Reply::status(500)
        } else {
            Reply::ok(served.clone())
        }
    });
    let base = BaseUrls {
        wikidata: format!("{}/sparql", stub.base),
        ..BaseUrls::default()
    };
    let (stations, _) = fetch(Source::Wikidata, &target(Some(lisbon())), &base).unwrap();
    assert_eq!(stations, wikidata::parse(&fixture).0);
    assert_eq!(stub.take(2).len(), 2, "the retry must happen");
}

#[test]
fn failures_name_their_source() {
    // Persistently failing HTTP.
    let stub = Stub::new(2, |_| Reply::status(500));
    let base = BaseUrls {
        wikidata: format!("{}/sparql", stub.base),
        ..BaseUrls::default()
    };
    let err = fetch(Source::Wikidata, &target(Some(lisbon())), &base).unwrap_err();
    assert!(err.to_string().contains("Wikidata"), "{err}");
    assert!(err.to_string().contains("500"), "{err}");
    stub.take(2);

    // A body the server never finishes sending.
    let stub = Stub::new(2, |_| Reply::truncated(fixture("eibi.csv")));
    let base = BaseUrls {
        eibi: format!("{}/dx/", stub.base),
        ..BaseUrls::default()
    };
    let err = fetch(Source::Eibi, &target(None), &base).unwrap_err();
    assert!(err.to_string().contains("EiBi"), "{err}");
    assert!(err.to_string().contains("body"), "{err}");
    stub.take(2);
}

#[test]
fn locate_ip_reads_the_stub_response() {
    let stub = Stub::new(1, |path| {
        assert_eq!(path, "/json");
        Reply::ok(
            r#"{"ip":"1.2.3.4","city":"Lisbon","country_code":"PT",
                       "latitude":38.72,"longitude":-9.14}"#,
        )
    });
    let loc = locate_ip(&format!("{}/json", stub.base)).unwrap();
    assert_eq!(loc.lat, 38.72);
    assert_eq!(loc.lon, -9.14);
    assert_eq!(loc.country_code.as_deref(), Some("PT"));
    assert_eq!(loc.source, LocationSource::Ip);
    stub.take(1);

    let stub = Stub::new(1, |_| Reply::ok("{}"));
    let err = locate_ip(&format!("{}/json", stub.base)).unwrap_err();
    assert!(err.to_string().contains("ip lookup"), "{err}");
    stub.take(1);
}
