//! The modulation lab in SDR mode: analyse the tracked signal nearest the
//! tuned frequency — its symbol rate, modulation (set, or picked by nearest
//! cumulants), recovered constellation, EVM and MER.

use neowon_core::{CaptureFrame, Modulation};
use neowon_dsp::Track;
use neowon_dsp::modlab::{Cumulants, cumulants, recover, select, symbol_rate};

/// Roll-off the lab assumes (the common RRC choice; also the simulator's).
pub const ROLLOFF: f64 = 0.35;
/// Pairs of a frame the lab looks at (bounds its cost; ≥ 800 symbols at
/// the rates the simulator uses).
const PAIRS: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct Analysis {
    pub track: u64,
    pub centre_hz: f64,
    pub symbol_rate_hz: f64,
    pub modulation: Modulation,
    /// The modulation was picked from cumulants, not set.
    pub auto: bool,
    pub evm_rms_pct: f64,
    pub mer_db: f64,
    pub cumulants: Cumulants,
    /// Recovered decision-point samples (up to 2048), for display.
    pub symbols: Vec<[f32; 2]>,
}

/// The modulation whose ideal (C42, |C40|) is nearest the measured ones;
/// both are rotation-invariant, so a blind front end is enough.
pub fn nearest(c: &Cumulants) -> Modulation {
    let ideal = |m: Modulation| match m {
        Modulation::Bpsk => (-2.0, 2.0),
        Modulation::Qpsk => (-1.0, 1.0),
        Modulation::Psk8 => (-1.0, 0.0),
        Modulation::Qam16 => (-0.68, 0.68),
        Modulation::Qam64 => (-0.619, 0.619),
    };
    Modulation::ALL
        .into_iter()
        .min_by(|&a, &b| {
            let d = |m| {
                let (c42, c40) = ideal(m);
                (c.c42 - c42).powi(2) + (c.c40.norm() - c40).powi(2)
            };
            d(a).total_cmp(&d(b))
        })
        .unwrap_or(Modulation::Qpsk)
}

/// Analyse `track` in `frame` (tuned to `centre_hz`), with `setting` the
/// user's modulation or `None` to pick one.
pub fn analyse(
    frame: &CaptureFrame,
    centre_hz: f64,
    track: &Track,
    setting: Option<Modulation>,
) -> Option<Analysis> {
    let rate = frame.sample_rate;
    let data = &frame.channels[0].data;
    let data = &data[..data.len().min(2 * PAIRS)];
    let obw = track.last.bandwidth_hz().max(rate / 1000.0);
    // Channel: the occupied band plus margin. An RRC signal reaches
    // ±Rs(1 + β)/2 ≈ ±OBW/2, so the cutoff sits well outside it, with taps
    // enough for the transition band to clear the signal's edge.
    let taps = ((8.0 * rate / obw) as usize).clamp(65, 401);
    let chan = select(
        data,
        rate,
        track.last.centre_hz - centre_hz,
        0.8 * obw,
        taps,
    );
    // OBW(99%) of an RRC signal is about Rs·(1 + β): search around it.
    let rs = symbol_rate(&chan, rate, 0.4 * obw, 1.2 * obw)?;
    let blind = recover(&chan, rate, rs, Modulation::Qpsk, ROLLOFF)?;
    let syms: Vec<f32> = blind
        .symbols
        .iter()
        .flat_map(|z| [z.re as f32, z.im as f32])
        .collect();
    let c = cumulants(&syms)?;
    let modulation = setting.unwrap_or_else(|| nearest(&c));
    let r = recover(&chan, rate, rs, modulation, ROLLOFF)?;
    Some(Analysis {
        track: track.id,
        centre_hz: track.last.centre_hz,
        symbol_rate_hz: rs,
        modulation,
        auto: setting.is_none(),
        evm_rms_pct: r.evm_rms_pct,
        mer_db: r.mer_db,
        cumulants: c,
        symbols: r
            .symbols
            .iter()
            .take(2048)
            .map(|z| [z.re as f32, z.im as f32])
            .collect(),
    })
}
