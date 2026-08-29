//! Hardware verification for Phase 6 triggers. Requires the scope with the
//! 1 kHz probe-comp signal on CH1 (x10 probe -> 0..0.5 V at the BNC,
//! 500 us high / 500 us low).
//!
//!   cargo run -p neowon-vds1022 --example trigtest

use std::time::Duration;

use neowon_core::{Coupling, PulseCondition, Slope, Sweep, TriggerKind};
use neowon_vds1022::device::{ChannelSetup, Vds1022};

fn main() {
    let fpga = neowon_vds1022::backend::default_fpga_dir();
    let mut dev = Vds1022::open(Some(&fpga)).expect("open");
    dev.configure_channel(
        0,
        ChannelSetup {
            enabled: true,
            vb: 5,
            coupling: Coupling::Dc,
            probe: 1.0,
            offset: 0.0,
        },
    )
    .unwrap();
    dev.set_sample_rate(250e3).unwrap();

    // (name, kind, level, expect_trigger)
    let cases: Vec<(&str, TriggerKind, f64, bool)> = vec![
        (
            "edge rising (control)",
            TriggerKind::Edge {
                slope: Slope::Rising,
            },
            0.25,
            true,
        ),
        (
            "pulse +> 400us (500us high pulse)",
            TriggerKind::Pulse {
                condition: PulseCondition::PositiveGreater,
                width: 400e-6,
            },
            0.25,
            true,
        ),
        (
            "pulse +> 600us (no such pulse)",
            TriggerKind::Pulse {
                condition: PulseCondition::PositiveGreater,
                width: 600e-6,
            },
            0.25,
            false,
        ),
        (
            "pulse +< 600us (500us qualifies)",
            TriggerKind::Pulse {
                condition: PulseCondition::PositiveLess,
                width: 600e-6,
            },
            0.25,
            true,
        ),
        (
            "pulse -> 400us (500us low pulse)",
            TriggerKind::Pulse {
                condition: PulseCondition::NegativeGreater,
                width: 400e-6,
            },
            0.25,
            true,
        ),
        (
            "slope rising < 100us (any fast edge qualifies)",
            TriggerKind::Slope {
                condition: PulseCondition::PositiveLess,
                width: 100e-6,
                upper: 0.4,
                lower: 0.1,
            },
            0.25,
            true,
        ),
        (
            "slope rising > 400us (edge is far faster)",
            TriggerKind::Slope {
                condition: PulseCondition::PositiveGreater,
                width: 400e-6,
                upper: 0.4,
                lower: 0.1,
            },
            0.25,
            false,
        ),
    ];

    let mut failures = 0;
    for (name, kind, level, expect) in cases {
        dev.set_trigger(0, &kind, level, Sweep::Normal).unwrap();
        dev.run().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        // 4 s: the first gated capture right after another session used the
        // device can take >2 s to arm (post-reconnect transient).
        let got = dev.capture(Duration::from_secs(4)).is_ok();
        let ok = got == expect;
        if !ok {
            failures += 1;
        }
        println!(
            "{}  {name}: expected {}, got {}",
            if ok { "PASS" } else { "FAIL" },
            if expect { "trigger" } else { "starve" },
            if got { "trigger" } else { "starve" },
        );
    }
    // MULTI port + PF output smoke: just verify the writes are acked.
    dev.set_multi(neowon_backend::MultiMode::PassFailOut)
        .unwrap();
    dev.set_pf_level(true).unwrap();
    dev.set_pf_level(false).unwrap();
    dev.set_multi(neowon_backend::MultiMode::TriggerOut)
        .unwrap();
    println!("PASS  MULTI port + PF level writes acked");

    dev.stop().unwrap();
    std::process::exit(if failures > 0 { 1 } else { 0 });
}
