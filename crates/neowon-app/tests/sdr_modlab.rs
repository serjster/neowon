//! The modulation lab in SDR mode, on the simulator's `rf-digital` scene:
//! QPSK at 100.3 MHz (102.4 ksym/s, amplitude 0.9) and 16QAM at 99.6 MHz
//! (51.2 ksym/s, amplitude 1.25), in noise of RMS 0.05. Expectations come
//! from the scene definition: symbol SNR = (amplitude / 0.05)², and the
//! closed-form EVM is its inverse square root.
//!
//!   cargo test -p neowon-app --test sdr_modlab -- --ignored

mod common;
use common::*;

fn raw_str<'a>(json: &'a str, key: &str) -> &'a str {
    raw(json, key).trim_matches('"')
}

#[test]
#[ignore = "opens a window"]
fn lab_identifies_and_measures_digital_signals() {
    let dir = std::env::temp_dir().join(format!("neowon-modlab-cat-{}", std::process::id()));
    let dir_s = dir.to_string_lossy().to_string();
    let (child, mut c) = launch(&["--sdr-sim"], &[("NEOWON_CATALOG", &dir_s)]);
    with_app(child, || {
        c.ok("stimulus rf-digital");
        c.ok("sdr analyse on");
        for (centre, label, rs, amp) in [
            (100.3e6, "QPSK", 102.4e3, 0.9f64),
            (99.6e6, "16QAM", 51.2e3, 1.25),
        ] {
            c.ok(&format!("sdr tune {centre}"));
            let evm = 100.0 * 0.05 / amp;
            let m = c.wait("get modmeas", 15, |r| {
                r.contains(r#""lab":{"#)
                    && (field(r, "symbol_rate_hz") / rs - 1.0).abs() < 0.01
                    && raw_str(r, "modulation") == label
            });
            println!("{label}: {m}");
            // One frame can be an honest `unknown` (low margin); the
            // verdict must settle to a confident one.
            let k = c.wait("get classify", 10, |r| {
                raw_str(r, "label") == label.to_lowercase() && r.contains(r#""unknown":false"#)
            });
            assert!(
                k.contains(r#""trust":"unproven""#) && k.contains(r#""unknown":false"#),
                "{k}"
            );
            let got = field(&m, "evm_rms_pct");
            assert!(
                (got - evm).abs() < 1.0,
                "{label}: EVM {got} vs {evm:.2}: {m}"
            );
            assert!(m.contains(r#""auto":true"#), "{m}");
        }
        // A set modulation is used as given.
        c.ok("sdr modulation qpsk");
        c.wait("get modmeas", 15, |r| {
            r.contains(r#""lab":{"#)
                && r.contains(r#""modulation":"QPSK""#)
                && r.contains(r#""auto":false"#)
        });
    });
    let _ = std::fs::remove_dir_all(&dir);
}
