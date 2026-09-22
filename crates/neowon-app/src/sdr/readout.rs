//! Control-socket readouts for SDR mode: `get sdr`, `get iq`,
//! `get detections`, `get modmeas`.

use neowon_backend::SdrGain;

use neowon_dsp::modmeas::{Band, flatness};

use super::SdrState;
use crate::refmap::RefMap;

/// A JSON string literal, escaped. Service labels come off the air, so they can
/// contain anything the standard's charset allows.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            c if (c as u32) < 0x20 => out.push('?'),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON number, or `null` where the standard leaves the value unknown.
fn opt_num(v: Option<f64>) -> String {
    match v {
        Some(v) => num(v),
        None => "null".into(),
    }
}

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
    // An active raw-IQ capture, or null. The reader knows the sample rate
    // from `sample_rate` in this same object and the format from `iqdump`.
    let iqdump = match &sdr.iq_dump {
        Some(d) => format!(
            r#"{{"path":"{}","written_pairs":{},"remaining_pairs":{}}}"#,
            d.path.replace('"', "\\\""),
            d.written_pairs,
            d.remaining_pairs
        ),
        None => "null".to_string(),
    };
    format!(
        concat!(
            r#"{{"ok":true,"active":{},"backend":"{}","serial":"{}","tuner":"{}","#,
            r#""centre_hz":{},"tuned_hz":{},"follow":{},"width_hz":{},"width_auto":{},"sample_rate":{},"gain_db":{},"agc":{},"ppm":{},"running":{},"#,
            r#""span_hz":{},"pan_hz":{},"list_px":{},"fft":{},"ref_db":{},"range_db":{},"frames_seen":{},"#,
            r#""peak_hz":{},"peak_dbfs":{},"floor_dbfs":{},"iqdump":{}}}"#
        ),
        sdr.active,
        name,
        serial,
        tuner,
        num(c.centre_hz),
        num(sdr.tuned_hz),
        sdr.follow,
        num(sdr.channel_width()),
        sdr.width_auto,
        num(c.sample_rate),
        gain,
        c.agc,
        num(c.ppm),
        c.running,
        num(sdr.span()),
        num(sdr.pan_hz),
        num(sdr.list_px as f64),
        sdr.fft_size,
        num(sdr.ref_db),
        num(sdr.range_db),
        sdr.frames_seen,
        num(peak_hz),
        num(peak_db),
        num(floor),
        iqdump,
    )
}

/// `get audio`: the demodulator and the output device, with the silent
/// states named (`off | no device | starting | muted | squelched | playing`).
pub fn audio_json(sdr: &SdrState) -> String {
    let (device, rate, available, underruns, dropped) = match &sdr.audio {
        Some(a) => (
            a.device().to_string(),
            a.rate(),
            a.available(),
            a.underruns(),
            a.dropped(),
        ),
        None => (String::new(), 0.0, false, 0, 0),
    };
    format!(
        concat!(
            r#"{{"ok":true,"demod":"{}","state":"{}","device":"{}","rate":{},"available":{},"#,
            r#""volume":{},"mute":{},"squelch_db":{},"rms":{},"channel_dbfs":{},"#,
            r#""underruns":{},"dropped":{}}}"#
        ),
        sdr.demod.map_or("off", |m| m.verb()),
        sdr.audio_state(),
        crate::control::escape(&device),
        num(rate),
        available,
        num(sdr.volume as f64),
        sdr.mute,
        num(sdr.squelch_db),
        num(sdr.audio_rms as f64),
        num(sdr.audio_channel_dbfs),
        underruns,
        dropped,
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

/// `get classify`: the DSP classifier's verdict on the signal nearest the
/// tuned frequency (runs with `sdr analyse on`), in the spec's schema plus
/// the runner-up. Trust stays "unproven" until an over-the-air evaluation
/// (D9) validates a class.
pub fn classify_json(sdr: &SdrState) -> String {
    let Some(c) = &sdr.classification else {
        return r#"{"ok":false,"error":"nothing classified yet (sdr analyse on)"}"#.into();
    };
    format!(
        concat!(
            r#"{{"ok":true,"label":"{}","confidence":{},"trust":"{}","unknown":{},"#,
            r#""top2_margin":{},"runner_up":"{}","features":{{"snr_db":{},"#,
            r#""carrier_fraction":{},"envelope_cv":{},"freq_spread":{},"cyclic_line_db":{},"#,
            r#""c42":{},"c40_abs":{}}}}}"#
        ),
        c.class.label(),
        num(c.confidence),
        c.trust.label(),
        c.unknown,
        num(c.margin),
        c.runner_up.label(),
        num(c.features.snr_db),
        num(c.features.carrier_fraction),
        num(c.features.envelope_cv),
        num(c.features.freq_spread),
        num(c.features.cyclic_line_db),
        num(c.features.c42),
        num(c.features.c40_abs),
    )
}

/// The Band III block the hardware centre sits on, as JSON: the label, its
/// exact centre, and whether the active band plan allocates it. `null` when
/// the centre is off the raster — the same honesty rule the plans follow:
/// no reference data, no claim.
fn channel_json(sdr: &SdrState, rm: &RefMap) -> String {
    let Some(block) = neowon_refdb::dab::band_iii_block_at(sdr.config.centre_hz) else {
        return "null".into();
    };
    let allocated = rm
        .plan()
        .is_some_and(|p| p.dab_blocks().iter().any(|b| b.label == block.label));
    format!(
        r#"{{"label":"{}","centre_hz":{},"allocated":{},"plan":"{}"}}"#,
        block.label,
        num(block.centre_hz),
        allocated,
        rm.stem()
    )
}

/// `get dab`: the DAB receiver's state and, once locked, the ensemble table
/// (phase 10.15.1), the per-sub-channel MSC counters and the selected
/// service's DLS text (10.15.2), and the audio transport (10.15.3). Every
/// field here is one the receiver or the playback worker produces: nothing
/// is inferred, and an unlocked receiver reports an empty table rather than
/// a partial one (D27). `audio` carries the worker's own state, backend,
/// rate, channels, peak/RMS, counters and its typed error reason; rate,
/// channels and peak are `null` until a block has actually decoded.
/// `channel` is the Band III block under the hardware centre (10.15.4's
/// selector), present whether or not the receiver is on.
pub fn dab_json(sdr: &SdrState, rm: &RefMap) -> String {
    let channel = channel_json(sdr, rm);
    let Some(rx) = &sdr.dab else {
        return format!(r#"{{"ok":true,"on":false,"channel":{channel}}}"#);
    };
    let status = rx.status();
    let rate = match status.fib_crc_rate() {
        Some(r) => num(r),
        None => "null".into(),
    };
    let sub_channels: Vec<String> = status
        .ensemble
        .sub_channels
        .values()
        .map(|sc| {
            format!(
                r#"{{"id":{},"start_cu":{},"size_cu":{},"bitrate_kbps":{},"protection":{}}}"#,
                sc.id,
                sc.start_cu,
                match sc.size_cu {
                    Some(v) => v.to_string(),
                    None => "null".into(),
                },
                opt_num(sc.bitrate_kbps),
                json_str(&sc.protection.label()),
            )
        })
        .collect();
    let services: Vec<String> = status
        .ensemble
        .services
        .values()
        .map(|sv| {
            format!(
                concat!(
                    r#"{{"sid":{},"sid_hex":"{:04X}","label":{},"sub_channel":{},"#,
                    r#""coding":{},"ascty":{},"has_audio":{}}}"#
                ),
                sv.sid,
                sv.sid,
                match &sv.label {
                    Some(l) => json_str(l),
                    None => "null".into(),
                },
                match sv.sub_channel {
                    Some(id) => id.to_string(),
                    None => "null".into(),
                },
                json_str(&sv.coding_label()),
                match sv.ascty {
                    Some(a) => a.to_string(),
                    None => "null".into(),
                },
                sv.has_audio,
            )
        })
        .collect();
    let (eid, eid_hex, label) = match status.ensemble.eid {
        Some(eid) => (
            eid.to_string(),
            format!("\"{eid:04X}\""),
            match &status.ensemble.label {
                Some(l) => json_str(l),
                None => "null".into(),
            },
        ),
        None => ("null".into(), "null".into(), "null".into()),
    };
    // The selected service (SId) and its DLS text; text is published only
    // when its reassembly completed CRC-clean (D27), so `null` here means
    // "not yet", never a guess.
    let service = match sdr.dab_service {
        Some(sid) => sid.to_string(),
        None => "null".into(),
    };
    let dls = match super::dab::dls(sdr) {
        Some(text) => json_str(text),
        None => "null".into(),
    };
    // The playback worker's own report (10.15.3) plus the sink's underruns.
    // `rate`, `channels` and `peak`/`rms` are null until a block has decoded
    // — a stream that produced nothing has no invented numbers — and an
    // `error` carries the typed reason the backend gave.
    let audio = match &sdr.dab_audio {
        Some(worker) => {
            let st = worker.status();
            let decoded = st.blocks > 0;
            format!(
                concat!(
                    r#"{{"state":"{}","backend":"{}","rate":{},"channels":{},"underruns":{},"#,
                    r#""peak":{},"rms":{},"blocks":{},"decoded":{},"errors":{},"dropped":{},"reason":{}}}"#
                ),
                st.state.label(),
                st.backend,
                if st.rate == 0 {
                    "null".to_string()
                } else {
                    st.rate.to_string()
                },
                if st.channels == 0 {
                    "null".to_string()
                } else {
                    st.channels.to_string()
                },
                sdr.audio.as_ref().map_or(0, |o| o.underruns()),
                if decoded {
                    num(f64::from(st.peak))
                } else {
                    "null".into()
                },
                if decoded {
                    num(f64::from(st.rms))
                } else {
                    "null".into()
                },
                st.blocks,
                st.decoded,
                st.errors,
                st.dropped,
                json_str(&st.reason),
            )
        }
        None => r#"{"state":"off"}"#.to_string(),
    };
    let msc: Vec<String> = status
        .msc
        .iter()
        .map(|(id, c)| {
            format!(
                concat!(
                    r#"{{"sub_channel":{},"frames":{},"bytes":{},"#,
                    r#""crc_checks":{},"crc_failures":{}}}"#
                ),
                id, c.frames, c.bytes, c.crc_checks, c.crc_failures
            )
        })
        .collect();
    format!(
        concat!(
            r#"{{"ok":true,"on":true,"channel":{},"locked":{},"frames":{},"fib_ok":{},"fib_total":{},"#,
            r#""fib_crc_rate":{},"freq_offset_hz":{},"prs_metric":{},"#,
            r#""frames_decoded":{},"frames_rejected":{},"last_attempt_metric":{},"#,
            r#""eid":{},"eid_hex":{},"label":{},"data_services":{},"#,
            r#""service":{},"dls":{},"audio":{},"msc":[{}],"#,
            r#""sub_channels":[{}],"services":[{}]}}"#
        ),
        channel,
        status.locked,
        status.frames,
        status.fib_crc_ok,
        status.fib_total,
        rate,
        num(rx.freq_offset_hz),
        num(f64::from(rx.prs_metric())),
        rx.frames_decoded,
        rx.frames_rejected,
        num(f64::from(rx.last_attempt_metric())),
        eid,
        eid_hex,
        label,
        status.ensemble.data_services,
        service,
        dls,
        audio,
        msc.join(","),
        sub_channels.join(","),
        services.join(","),
    )
}
