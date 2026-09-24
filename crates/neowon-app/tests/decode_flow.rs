//! Protocol decoding end to end through the control socket: the simulator
//! generates real UART traffic, the app decodes it, and the result comes
//! back over the socket.
//!
//! Opens a window, so `#[ignore]` by default:
//!   cargo test -p neowon-app --test decode_flow -- --ignored

mod common;
use common::*;

use std::time::{Duration, Instant};

/// Decoded word values, in order, from a `get decode` reply.
fn words(json: &str) -> Vec<u64> {
    json.split(r#""kind":"word","value":"#)
        .skip(1)
        .filter_map(|s| s.split(',').next()?.parse().ok())
        .collect()
}

#[test]
#[ignore = "opens a window"]
fn decodes_uart_traffic_from_the_simulator() {
    let (mut child, mut conn) = launch(
        &["--sim"],
        &[("NEOWON_WINDOW", "1520x820"), ("NEOWON_UI_SCALE", "1.0")],
    );
    conn.set_timeout(15);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for cmd in [
            "stimulus uart-hello",
            "enable 0 1",
            "vdiv 0 0.5",
            "rate 250e3",
            "decode uart",
            "decodebaud 9600",
        ] {
            conn.ok(cmd);
        }

        // The traffic is "Hi!" repeated, so the decode must contain it.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let json = conn.request("get decode");
            let w = words(&json);
            let text: String = w.iter().map(|&v| v as u8 as char).collect();
            if text.contains("Hi!") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "never decoded the transmitted bytes: {json}"
            );
            std::thread::sleep(Duration::from_millis(250));
        }

        // And it refuses, with a reason, when the time base cannot resolve
        // the protocol — rather than emitting plausible bytes.
        conn.ok("decode onewire");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let json = conn.request("get decode");
            if json.contains("resolution too low") {
                assert!(json.contains("faster time base"), "{json}");
                break;
            }
            assert!(Instant::now() < deadline, "no refusal reported: {json}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }));

    let _ = child.kill();
    let _ = child.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
