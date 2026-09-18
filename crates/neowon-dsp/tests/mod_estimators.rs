//! Phase 10.3 golden rows for the modulation lab. Signals are RRC-shaped
//! (roll-off 0.35) at 1 MS/s, 100 ksym/s (10 samples per symbol), from the
//! D8 generator; SNR is symbol Es/N0 at the matched-filter output. Each
//! row prints a JSON readout.
//!
//! `cargo test -p neowon-dsp --test mod_estimators -- --nocapture`

use neowon_core::Modulation;
use neowon_dsp::modlab::{cumulants, recover, symbol_rate};
use neowon_sim::iq::symbol_bits;
use neowon_sim::{IqComponent, IqScene};

const RATE: f64 = 1e6;
const RS: f64 = 100e3;
const BETA: f64 = 0.35;
const AMP: f64 = 0.5;

fn scene(m: Modulation, snr_db: Option<f64>, offset_hz: f64) -> IqScene {
    IqScene {
        sample_rate: RATE,
        components: vec![IqComponent::Digital {
            modulation: m,
            symbol_rate: RS,
            offset_hz,
            amplitude: AMP,
            rolloff: BETA,
        }],
        noise_rms: snr_db.map_or(0.0, |s| AMP * 10f64.powf(-s / 20.0)),
    }
}

/// Fraction of recovered labels equal to the transmitted ones, under the
/// best of the constellation's rotational ambiguities (blind phase cannot
/// tell them apart), searching the alignment near the filter start.
fn bit_match(m: Modulation, labels: &[u32], seed: u64) -> f64 {
    let rotations = match m {
        Modulation::Bpsk => 2,
        Modulation::Psk8 => 8,
        _ => 4,
    };
    let rotate = |l: u32, r: u32| {
        let (i, q) = m.point(l);
        let a = std::f64::consts::TAU * r as f64 / rotations as f64;
        m.slice((i * a.cos() - q * a.sin(), i * a.sin() + q * a.cos()))
    };
    let mask = (1u32 << m.bits_per_symbol()) - 1;
    let mut best = 0.0f64;
    for lag in 0..40i64 {
        for r in 0..rotations {
            let ok = labels
                .iter()
                .enumerate()
                .filter(|(i, l)| rotate(**l, r) == symbol_bits(seed, *i as i64 + lag) & mask)
                .count();
            best = best.max(ok as f64 / labels.len() as f64);
        }
    }
    best
}

#[test]
fn qam16_evm_at_30db_is_the_closed_form() {
    let seed = 42;
    let n = 8192 * 10;
    let s = scene(Modulation::Qam16, Some(30.0), 1500.0);
    let r = recover(&s.samples(seed, 0, n), RATE, RS, Modulation::Qam16, BETA).unwrap();
    let closed = 100.0 * 10f64.powf(-30.0 / 20.0);
    let bits = bit_match(Modulation::Qam16, &r.labels, seed);
    println!(
        r#"{{"row":"16qam_evm","evm_pct":{:.3},"closed_form_pct":{closed:.3},"mer_db":{:.2},"carrier_offset_hz":{:.1},"symbols":{},"label_match":{bits:.5}}}"#,
        r.evm_rms_pct,
        r.mer_db,
        r.carrier_offset_hz,
        r.symbols.len()
    );
    assert!(
        (r.evm_rms_pct - closed).abs() <= 0.5,
        "EVM {} vs {closed}",
        r.evm_rms_pct
    );
    assert!(
        (r.carrier_offset_hz - 1500.0).abs() < 5.0,
        "{}",
        r.carrier_offset_hz
    );
    assert!(bits > 0.999, "labels {bits}");
}

#[test]
fn qpsk_symbol_rate_within_one_percent() {
    let s = scene(Modulation::Qpsk, Some(20.0), 0.0);
    let est = symbol_rate(&s.samples(7, 0, 65536), RATE, 10e3, 450e3).unwrap();
    println!(r#"{{"row":"qpsk_symbol_rate","estimate_hz":{est:.1},"true_hz":{RS}}}"#);
    assert!((est / RS - 1.0).abs() < 0.01, "{est}");
}

#[test]
fn bpsk_qpsk_c42_match_the_published_values() {
    for (m, published) in [(Modulation::Bpsk, -2.0), (Modulation::Qpsk, -1.0)] {
        let s = scene(m, None, 0.0);
        let r = recover(&s.samples(3, 0, (8192 + 30) * 10), RATE, RS, m, BETA).unwrap();
        let syms: Vec<f32> = r
            .symbols
            .iter()
            .take(8192)
            .flat_map(|z| [z.re as f32, z.im as f32])
            .collect();
        let c = cumulants(&syms).unwrap();
        println!(
            r#"{{"row":"c42","modulation":"{}","c42":{:.4},"published":{published},"c40_abs":{:.4},"c63":{:.3},"n":{}}}"#,
            m.label(),
            c.c42,
            c.c40.norm(),
            c.c63,
            syms.len() / 2
        );
        assert!((c.c42 - published).abs() <= 0.02, "{m:?}: C42 {}", c.c42);
    }
}
