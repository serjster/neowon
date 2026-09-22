//! The app side of the DAB consumer (10.15.2): the wideband IQ feed and the
//! DLS view the readout and the dock share. Split from `mod.rs` along its
//! second job, so the mode's state file does not grow past its budget.

use neowon_core::CaptureFrame;

use super::SdrState;
use super::actions::DabChannel;

/// A coarse upper bound on one DAB transmission frame, in samples at 2.048 MS/s,
/// used only to size the splice tolerance.
pub const FRAME_SAMPLES_HINT: i64 = 196_608;

/// How long without a single IQ frame before the DAB lock is treated as gone.
///
/// The deliberate splice tolerance in [`feed`] bridges gaps up to about four
/// transmission frames (≈384 ms); the receiver's own expiry covers seconds of
/// *undecodable* input. This is the one case neither can see: no input at all,
/// so there is nothing to decode and nothing to count. Two seconds is over
/// forty missing transmission frames — no USB stall or scheduler hitch on a
/// healthy 2.048 MS/s link lasts that long, while a stopped backend, a
/// disconnect or an instrument switch leaves the receiver frozen forever.
pub const NO_INPUT_TIMEOUT_S: f64 = 2.0;

/// Forget everything the DAB receiver derived from a signal that is gone: the
/// lock, the ensemble table, the selection, the DLS parsers and playback.
///
/// For a **retune, a rate change or an instrument switch** — the hardware
/// moved, so the old table describes a different signal and keeping it would
/// be a stale claim (D27). The deliberate splice path is [`feed`]'s
/// `discard_buffer`, which keeps the table across a short gap; this is not
/// that.
pub fn reset(sdr: &mut SdrState) {
    if let Some(rx) = sdr.dab.as_mut() {
        rx.reset();
    }
    sdr.dab_next_sample = None;
    sdr.dab_last_frame_at = None;
    forget_selection(sdr);
}

/// The owner reports that the frame stream stopped (a stopped backend or a
/// disconnect). Unlike a retune there is no new window to start from — the
/// receiver keeps its cumulative counters — but the lock, the table and the
/// timing grid describe a stream that is no longer there, so they go now
/// instead of waiting for [`NO_INPUT_TIMEOUT_S`].
pub fn no_input(sdr: &mut SdrState) {
    if let Some(rx) = sdr.dab.as_mut() {
        rx.no_input();
    }
    sdr.dab_next_sample = None;
    sdr.dab_last_frame_at = None;
    forget_selection(sdr);
}

/// Drop the selection, its parsers and playback, leaving the receiver alone:
/// used when the receiver's table has expired under a selection, so no path
/// keeps naming a service the table no longer holds (D27).
fn forget_selection(sdr: &mut SdrState) {
    super::dab_audio::stop(sdr);
    sdr.dab_pad.clear();
    sdr.dab_service = None;
    sdr.dab_play_error = None;
}

/// Hand one frame's IQ to the DAB receiver.
///
/// Called from `ingest`, where **every** frame arrives, not from the display
/// path: that one is latest-wins by design (it only needs the newest frame to
/// paint), and a decoder fed from it sees a stream with holes in it. Measured on
/// air before this moved: 110 frames decoded out of ~3 700 received, i.e. about
/// one frame in six, because the receiver spent the rest of the time re-finding
/// the null symbol. Frames are `Arc`-shared and never copied for a consumer, and
/// the receiver keeps its own buffer, so a ragged chunk is normal input.
///
/// The decoded sub-channel bytes are fed to one PAD parser each: DLS lives in
/// the audio stream's PAD — the sim scene carries the region directly in the
/// logical frame, while on air a codec extracts it (deviation 12 of the
/// phase-10.15 spec) — and a partial or CRC-bad label is never published (D27).
pub fn feed(sdr: &mut SdrState, frame: &CaptureFrame) {
    let rate = frame.sample_rate;
    let start = (frame.t_start() * rate).round() as i64;
    let pairs = frame.channels[0].unit_count(frame.layout) as i64;
    // Tolerance is deliberately coarse. `CaptureFrame::t_start` is derived from
    // *arrival* time ("biased late by up to one poll"), so a tight bound fires
    // on ordinary jitter — which is how this check cost a real air session ~87%
    // of its attempts. It is a safety net for a stall or a retune, not splice
    // detection: that needs a dropped-sample counter from the backend, and it is
    // recorded as an open item in docs/protocol-dab.md.
    const JITTER_TOLERANCE_SAMPLES: i64 = 4 * FRAME_SAMPLES_HINT;
    let spliced = matches!(sdr.dab_next_sample, Some(expected) if (start - expected).abs() > JITTER_TOLERANCE_SAMPLES);
    sdr.dab_next_sample = Some(start + pairs);
    let Some(rx) = sdr.dab.as_mut() else {
        return;
    };
    if spliced {
        rx.discard_buffer();
    }
    rx.push_iq(&frame.channels[0].data);
    let decoded = rx.take_msc_frames();
    // The table can expire inside the receiver itself (no clean FIC for
    // seconds, D27). A selection that outlives its table would keep the dock,
    // `get dab` and the playback transport naming a service that is not
    // there, so drop it with the table; the receiver is left running to
    // re-acquire.
    if !rx.is_locked() && sdr.dab_service.is_some() {
        forget_selection(sdr);
    }
    if decoded.is_empty() {
        return;
    }
    if spliced {
        // The MSC de-interleavers were restarted; the outputs until their
        // warm-up completes mix ring history from before the hole, so drop
        // the parsers' partial segments rather than complete them with
        // garbage. The playback worker's transports restart with them
        // (10.15.3): a seam is not a signal.
        sdr.dab_pad.clear();
        if let Some(audio) = &sdr.dab_audio {
            audio.reset();
        }
    }
    // While a service plays, its sub-channel's logical frames are the
    // codec's stream and go to the playback worker — which also extracts
    // the stream's PAD and hands it back as a region. Every other
    // sub-channel feeds the raw PAD parser: that is the sim scene's PAD
    // carrier transport (dab_scene), and on air it is the MP2 tail the
    // parser reads from the end of the frame.
    let playing = super::dab_audio::playing_sub_channel(sdr);
    for decoded in decoded {
        if Some(decoded.sub_channel) == playing {
            if let Some(audio) = &sdr.dab_audio {
                audio.push(decoded.sub_channel, &decoded.bytes);
            }
            continue;
        }
        let parser = sdr.dab_pad.entry(decoded.sub_channel).or_default();
        parser.push_pad_region(&decoded.bytes);
    }
}

/// The DLS text of the selected service, once its reassembly completed and
/// its CRCs passed (D27). `None` when nothing is selected, the table is
/// unpublished, or no complete label has arrived yet.
pub fn dls(sdr: &SdrState) -> Option<&str> {
    let sid = sdr.dab_service?;
    let status = sdr.dab.as_ref()?.status();
    let sub = status.ensemble.services.get(&sid)?.sub_channel?;
    sdr.dab_pad.get(&sub)?.dls()
}

/// Land the hardware window on a Band III block (`sdr dab channel …`).
///
/// The DAB front end measures only ±500 Hz of carrier offset (deviation 7
/// of the phase-10.15 spec), so "tuned to DAB" means the hardware centre
/// sits on the block's centre — a band-plan click, which lands on the
/// allocation's centre, is 18 MHz off for Band III and locks nothing. The
/// block centres are `neowon-refdb`'s table, not a list invented here.
///
/// A block change is a different ensemble, so a running receiver forgets
/// its lock and table immediately (the reason `sdr dab on` builds a fresh
/// one), rather than reporting the old ensemble's services until its lock
/// window notices. Returns the label and centre for the status line.
pub fn channel(sdr: &mut SdrState, choice: &DabChannel) -> Result<(String, f64), String> {
    let target = match choice {
        DabChannel::Label(label) => neowon_refdb::dab::band_iii_block(label)
            .ok_or_else(|| format!("dab channel: unknown Band III block {label:?} (5A..13F)"))?,
        DabChannel::Next | DabChannel::Prev => {
            let blocks: Vec<neowon_refdb::dab::DabBlock> =
                neowon_refdb::dab::band_iii_blocks().collect();
            let next = matches!(choice, DabChannel::Next);
            // The step within the raster, wrapping at its ends.
            let step = |current: neowon_refdb::dab::DabBlock| {
                let i = blocks
                    .iter()
                    .position(|b| b.label == current.label)
                    .unwrap_or(0);
                let n = blocks.len();
                blocks[if next { (i + 1) % n } else { (i + n - 1) % n }]
            };
            match neowon_refdb::dab::band_iii_block_at(sdr.config.centre_hz) {
                Some(current) => Some(step(current)),
                // Off the raster: `next` starts at 5A, `prev` at 13F.
                None if next => neowon_refdb::dab::band_iii_blocks().next(),
                None => neowon_refdb::dab::band_iii_blocks().last(),
            }
            .ok_or("dab channel: no Band III blocks")?
        }
    };
    let changed = (sdr.config.centre_hz - target.centre_hz).abs() > 0.5;
    sdr.set_centre(target.centre_hz);
    // `set_centre` only drags Tuned along with Follow on; a channel pick is
    // a tune in its own right, so the cursor moves too.
    sdr.tuned_hz = target.centre_hz;
    if changed {
        super::dab_audio::stop(sdr);
        if let Some(rx) = sdr.dab.as_mut() {
            rx.reset();
        }
        sdr.dab_pad.clear();
        sdr.dab_service = None;
        sdr.dab_play_error = None;
    }
    sdr.dirty = true;
    Ok((target.label.to_string(), target.centre_hz))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tune(centre_hz: f64) -> SdrState {
        let mut sdr = SdrState::default();
        sdr.config.centre_hz = centre_hz;
        sdr.tuned_hz = centre_hz;
        sdr
    }

    #[test]
    fn a_label_lands_the_hardware_on_the_block_centre() {
        let mut sdr = tune(100e6);
        let (label, hz) = channel(&mut sdr, &DabChannel::Label("11c".into())).unwrap();
        assert_eq!(label, "11C");
        assert_eq!(hz, 220_352_000.0);
        assert_eq!(sdr.config.centre_hz, 220_352_000.0);
        assert_eq!(sdr.tuned_hz, 220_352_000.0);
        assert!(sdr.dirty);
        assert!(channel(&mut sdr, &DabChannel::Label("11X".into())).is_err());
    }

    #[test]
    fn next_and_prev_step_the_raster_and_wrap() {
        let mut sdr = tune(220_352_000.0); // 11C
        let step = |sdr: &mut SdrState, ch| channel(sdr, &ch).unwrap().0;
        assert_eq!(step(&mut sdr, DabChannel::Next), "11D");
        assert_eq!(step(&mut sdr, DabChannel::Prev), "11C");
        assert_eq!(step(&mut sdr, DabChannel::Next), "11D");

        // Off the raster: next starts at 5A, prev at 13F.
        let mut below = tune(100e6);
        assert_eq!(step(&mut below, DabChannel::Next), "5A");
        assert_eq!(step(&mut below, DabChannel::Prev), "13F");

        // Wraparound at the ends of the raster.
        let mut edge = tune(239_200_000.0); // 13F
        assert_eq!(step(&mut edge, DabChannel::Next), "5A");
        let mut edge = tune(174_928_000.0); // 5A
        assert_eq!(step(&mut edge, DabChannel::Prev), "13F");
    }

    /// Changing block drops an ensemble table that belongs to the old one;
    /// re-selecting the same block leaves a running receiver alone.
    #[test]
    fn a_block_change_forgets_the_old_ensembles_table() {
        let mut sdr = tune(220_352_000.0);
        sdr.dab = Some(neowon_dsp::dab::DabReceiver::new());
        sdr.dab_service = Some(0x1001);
        assert!(channel(&mut sdr, &DabChannel::Label("11C".into())).is_ok());
        assert_eq!(sdr.dab_service, Some(0x1001), "same block: no reset");
        assert!(sdr.dab.is_some());

        assert!(channel(&mut sdr, &DabChannel::Next).is_ok());
        assert_eq!(sdr.dab_service, None, "new block: selection dropped");
        assert!(sdr.dab.is_some(), "the receiver stays on, just reset");
    }
}
