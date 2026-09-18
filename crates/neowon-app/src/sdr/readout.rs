//! Control-socket readouts for SDR mode: `get sdr`, `get iq`,
//! `get detections`, `get modmeas`.

use neowon_backend::SdrGain;

use neowon_dsp::modmeas::{Band, flatness};

use super::SdrState;

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

/// `get iq`: identity of the latest IQ frame. `start` is its first sample
/// index (t_start × rate), so a test can regenerate the same bytes from
/// the D8 generator and compare `bytes_fnv`.
pub fn iq_json(sdr: &SdrState) -> String {
    let Some(f) = &sdr.latest else {
        return r#"{"ok":false,"error":"no IQ frame yet"}"#.into();
    };
    let bytes = neowon_sim::iq::to_le_bytes(&f.channels[0].data);
    format!(
        r#"{{"ok":true,"seed":{},"n":{},"start":{},"layout":"complex","bytes_fnv":{}}}"#,
        sdr.seed,
        f.channels[0].unit_count(f.layout),
        (f.t_start() * f.sample_rate).round(),
        neowon_sim::iq::fnv1a64(&bytes)
    )
}

/// `get sdr`: the SDR mode's settings and live readouts.
pub fn sdr_json(sdr: &SdrState) -> String {
    let c = &sdr.config;
    let gain = match c.gain {
        SdrGain::Auto => r#""auto""#.to_string(),
        SdrGain::Manual(db) => num(db),
    };
    let (name, serial, tuner) = sdr
        .caps
        .as_ref()
        .map(|c| (c.name.as_str(), c.serial.as_str(), c.tuner.as_str()))
        .unwrap_or_default();
    let (peak_hz, peak_db) = sdr.peak().unwrap_or((f64::NAN, f64::NAN));
    let floor = sdr.spectrum.as_ref().map_or(f64::NAN, |s| s.median_db());
    format!(
        concat!(
            r#"{{"ok":true,"active":{},"backend":"{}","serial":"{}","tuner":"{}","#,
            r#""centre_hz":{},"sample_rate":{},"gain_db":{},"agc":{},"ppm":{},"running":{},"#,
            r#""span_hz":{},"fft":{},"ref_db":{},"range_db":{},"frames_seen":{},"#,
            r#""peak_hz":{},"peak_dbfs":{},"floor_dbfs":{}}}"#
        ),
        sdr.active,
        name,
        serial,
        tuner,
        num(c.centre_hz),
        num(c.sample_rate),
        gain,
        c.agc,
        num(c.ppm),
        c.running,
        num(sdr.span()),
        sdr.fft_size,
        num(sdr.ref_db),
        num(sdr.range_db),
        sdr.frames_seen,
        num(peak_hz),
        num(peak_db),
        num(floor),
    )
}

/// `get detections`: the active (debounced) tracks, strongest first, in
/// the spec's schema plus the track id.
pub fn detections_json(sdr: &SdrState) -> String {
    let mut tracks: Vec<_> = sdr.tracker.active().collect();
    tracks.sort_by(|a, b| b.last.power_dbfs.total_cmp(&a.last.power_dbfs));
    let items: Vec<String> = tracks
        .iter()
        .map(|t| {
            format!(
                concat!(
                    r#"{{"id":{},"centre_hz":{},"bandwidth_hz":{},"power_dbfs":{},"#,
                    r#""snr_db":{},"first_seen_s":{},"last_seen_s":{}}}"#
                ),
                t.id,
                num(t.last.centre_hz),
                num(t.last.bandwidth_hz()),
                num(t.last.power_dbfs),
                num(t.last.snr_db),
                num(t.first_seen),
                num(t.last_seen),
            )
        })
        .collect();
    format!(
        r#"{{"ok":true,"detect":{},"threshold_db":{},"detections":[{}]}}"#,
        sdr.detect_on,
        num(sdr.threshold_db),
        items.join(",")
    )
}

/// `get modmeas`: measurements of the strongest active track, plus the
/// modulation lab's results for the signal it analysed (`sdr analyse on`).
/// Complex cumulants (C20, C40, C41) are reported as magnitudes: their
/// phase is the constellation's orientation, not a property of the
/// modulation. Lab fields are null until the lab has run.
pub fn modmeas_json(sdr: &SdrState) -> String {
    let (Some(t), Some(s)) = (
        sdr.tracker
            .active()
            .max_by(|a, b| a.last.power_dbfs.total_cmp(&b.last.power_dbfs)),
        sdr.spectrum.as_ref(),
    ) else {
        return r#"{"ok":false,"error":"no active signal"}"#.into();
    };
    let c = sdr.config.centre_hz;
    let band = Band::of(s, t.last.lo_hz - c, t.last.hi_hz - c);
    let lab = match &sdr.analysis {
        None => {
            r#""lab":null,"symbol_rate_hz":null,"evm_rms_pct":null,"cumulants":null"#.to_string()
        }
        Some(a) => {
            let k = &a.cumulants;
            format!(
                concat!(
                    r#""lab":{{"track":{},"centre_hz":{},"modulation":"{}","auto":{},"mer_db":{}}},"#,
                    r#""symbol_rate_hz":{},"evm_rms_pct":{},"#,
                    r#""cumulants":{{"c20":{},"c21":{},"c40":{},"c41":{},"c42":{},"c63":{}}}"#
                ),
                a.track,
                num(a.centre_hz),
                a.modulation.label(),
                a.auto,
                num(a.mer_db),
                num(a.symbol_rate_hz),
                num(a.evm_rms_pct),
                num(k.c20.norm()),
                num(k.c21),
                num(k.c40.norm()),
                num(k.c41.norm()),
                num(k.c42),
                num(k.c63),
            )
        }
    };
    format!(
        concat!(
            r#"{{"ok":true,"id":{},"centre_hz":{},"obw99_hz":{},"channel_power_dbfs":{},"#,
            r#""snr_db":{},"flatness":{},{}}}"#
        ),
        t.id,
        num(t.last.centre_hz),
        num(t.last.bandwidth_hz()),
        num(t.last.power_dbfs),
        num(t.last.snr_db),
        num(flatness(s, band)),
        lab,
    )
}
