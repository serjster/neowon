//! SDR mode end to end on the simulated SDR, over the control socket.
//!
//! Every expectation is derived, not recorded: the carrier's frequency and
//! level come from the scene definition, and the IQ fingerprint is
//! recomputed from the D8 generator at the sample index the app reports.
//!
//! Needs a window (briefly), so `#[ignore]` by default:
//!   cargo test -p neowon-app --test sdr_mode -- --ignored

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use neowon_sim::IqScene;
use neowon_sim::iq::{fnv1a64, to_le_bytes};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Conn {
    out: TcpStream,
    lines: std::io::Lines<BufReader<TcpStream>>,
}

impl Conn {
    fn request(&mut self, line: &str) -> String {
        writeln!(self.out, "{line}").unwrap();
        self.lines.next().expect("connection closed").unwrap()
    }

    /// Poll `query` until `ok` accepts the reply, or fail after `secs`.
    fn wait(&mut self, query: &str, secs: u64, ok: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let r = self.request(query);
            if ok(&r) {
                return r;
            }
            assert!(Instant::now() < deadline, "{query} never settled: {r}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The raw text of a field of a flat JSON object.
fn raw<'a>(json: &'a str, key: &str) -> &'a str {
    let pat = format!("\"{key}\":");
    let start = json
        .find(&pat)
        .unwrap_or_else(|| panic!("{key} missing: {json}"))
        + pat.len();
    let rest = &json[start..];
    rest[..rest.find([',', '}']).unwrap()].trim()
}

/// A numeric field (`null` → NaN).
fn field(json: &str, key: &str) -> f64 {
    raw(json, key).parse().unwrap_or(f64::NAN)
}

#[test]
#[ignore = "opens a window"]
fn sdr_mode_tunes_measures_and_stays_deterministic() {
    let port = free_port();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_neowon-app"))
        .arg("--sdr-sim")
        .env("NEOWON_CONTROL", port.to_string())
        .env_remove("NEOWON_SCRIPT")
        .spawn()
        .expect("launch app");
    let deadline = Instant::now() + Duration::from_secs(20);
    let stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => {
                let _ = child.kill();
                panic!("cannot connect: {e}");
            }
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut conn = Conn {
        out: stream.try_clone().unwrap(),
        lines: BufReader::new(stream).lines(),
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Default: rf-reference tuned to 100 MHz is a 0.5 FS tone at
        // +100 kHz. Bin width 2.048 MHz / 4096 = 500 Hz.
        let s = conn.wait("get sdr", 15, |r| field(r, "frames_seen") >= 3.0);
        assert!(
            s.contains(r#""active":true"#) && s.contains(r#""tuner":"sim""#),
            "{s}"
        );
        assert!((field(&s, "peak_hz") - 100.1e6).abs() <= 250.0, "{s}");
        assert!(
            (field(&s, "peak_dbfs") - 20.0 * 0.5f64.log10()).abs() < 0.1,
            "{s}"
        );

        // Retune into the FM-band scene: its strongest in-band emitter at
        // 99 MHz ± 1.024 MHz is 99.4 MHz at 0.3 FS.
        assert!(conn.request("stimulus rf-fm-band").contains(r#""ok":true"#));
        assert!(conn.request("sdr tune 99M").contains(r#""ok":true"#));
        let s = conn.wait("get sdr", 10, |r| {
            (field(r, "peak_hz") - 99.4e6).abs() <= 250.0 && field(r, "centre_hz") == 99e6
        });
        assert!(
            (field(&s, "peak_dbfs") - 20.0 * 0.3f64.log10()).abs() < 0.1,
            "{s}"
        );

        // Out-of-caps requests are refused and leave the config alone.
        assert!(conn.request("sdr rate 1234").contains(r#""ok":true"#));
        let st = conn.wait("get status", 5, |r| r.contains("rate 1234"));
        assert!(st.contains("error"), "{st}");
        assert_eq!(field(&conn.request("get sdr"), "sample_rate"), 2.048e6);

        // Determinism through the whole app: back on the reference scene,
        // reseed, then the latest frame's bytes must be the D8 generator's.
        assert!(
            conn.request("stimulus rf-reference")
                .contains(r#""ok":true"#)
        );
        assert!(conn.request("sdr tune 100M").contains(r#""ok":true"#));
        assert!(conn.request("sim iq --seed 7").contains(r#""ok":true"#));
        std::thread::sleep(Duration::from_millis(500));
        // Freeze so the frame cannot change between reading it and checking.
        assert!(conn.request("sdr run 0").contains(r#""ok":true"#));
        std::thread::sleep(Duration::from_millis(300));
        let iq = conn.request("get iq");
        assert_eq!(field(&iq, "seed"), 7.0, "{iq}");
        let (n, start) = (field(&iq, "n") as usize, field(&iq, "start") as u64);
        let expect = fnv1a64(&to_le_bytes(&IqScene::reference().samples(7, start, n)));
        // Parsed as u64: a 64-bit hash does not survive a trip through f64.
        assert_eq!(
            raw(&iq, "bytes_fnv").parse::<u64>().unwrap(),
            expect,
            "{iq}"
        );
    }));
    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
