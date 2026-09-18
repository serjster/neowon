//! The modulation lab's residual EVM floor: QPSK recovered with and
//! without noise at 10, 20 and 40 samples per symbol, against the closed
//! form (noise RMS / amplitude). Noise-free, what remains is the RRC
//! truncation floor (~0.12%); a result far above it points at a recovery
//! stage (this is how a residual-carrier phase ramp was found).
//!
//! `cargo run -p neowon-dsp --example lab_floor`
use neowon_core::Modulation;
use neowon_dsp::modlab::recover;
use neowon_sim::{IqComponent, IqScene};
fn main() {
    for (rate, rs) in [(1e6, 100e3), (2.048e6, 102.4e3), (2.048e6, 51.2e3)] {
        for noise in [0.0, 0.05] {
            for n in [32 * 1024usize, 128 * 1024] {
                let s = IqScene {
                    sample_rate: rate,
                    components: vec![IqComponent::Digital {
                        modulation: Modulation::Qpsk,
                        symbol_rate: rs,
                        offset_hz: 0.0,
                        amplitude: 0.9,
                        rolloff: 0.35,
                    }],
                    noise_rms: noise,
                };
                let r = recover(&s.samples(1, 0, n), rate, rs, Modulation::Qpsk, 0.35).unwrap();
                println!(
                    "sps {:>4.0} noise {noise} n {n:>6}: EVM {:.3} % (expect {:.3}) timing {:.3}",
                    rate / rs,
                    r.evm_rms_pct,
                    100.0 * noise / 0.9,
                    r.timing
                );
            }
        }
    }
}
