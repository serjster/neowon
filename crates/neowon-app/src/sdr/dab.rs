//! The app side of the DAB consumer: the wideband IQ feed and the DLS view
//! the readout and the dock share.

use neowon_core::CaptureFrame;

use super::SdrState;
use super::actions::DabChannel;

/// A coarse upper bound on one DAB transmission frame, in samples at 2.048 MS/s,
/// used only to size the splice tolerance.
pub const FRAME_SAMPLES_HINT: i64 = 196_608;

/// How long without a single IQ frame before the DAB lock is treated as gone.
///
/// A gap the backend counted splices [`feed`] immediately; its timestamp
/// safety net bridges up to about four transmission frames (≈384 ms) of
/// *uncounted* movement, and the receiver's own expiry covers seconds of
/// *undecodable* input. This is the one case none of them can see: no input at all,
/// so there is nothing to decode and nothing to count. Two seconds is over
/// forty missing transmission frames — no USB stall or scheduler hitch on a
/// healthy 2.048 MS/s link lasts that long, while a stopped backend, a
/// disconnect or an instrument switch leaves the receiver frozen forever.
pub const NO_INPUT_TIMEOUT_S: f64 = 2.0;

/// Hand one frame's IQ to the DAB receiver.
///
/// Called from `ingest`, where **every** frame arrives, not from the display
/// path: that one is latest-wins by design (it only needs the newest frame to
/// paint), and a decoder fed from it sees a stream with holes in it and spends
/// its time re-finding the null symbol. Frames are `Arc`-shared and never copied for a consumer, and
/// the receiver keeps its own buffer, so a ragged chunk is normal input.
///
/// The decoded sub-channel bytes are fed to one PAD parser each: DLS lives in
/// the audio stream's PAD — the sim scene carries the region directly in the
/// logical frame, while on air a codec extracts it — and a partial or CRC-bad
/// label is never published.
pub fn feed(sdr: &mut SdrState, frame: &CaptureFrame) {
    let rate = frame.sample_rate;
    let start = (frame.t_start() * rate).round() as i64;
    let pairs = frame.channels[0].unit_count(frame.layout()) as i64;
    // The backend's own count is the authority: a producer that can lose
    // samples reports exactly how many, so a splice is a fact rather than an
    // inference from arrival time.
    let dropped = frame.dropped_before();
    // The timestamp check stays as the safety net for the case the counter
    // cannot cover — a producer that does not count, a stopped and restarted
    // stream, an instrument swap. Its tolerance is deliberately coarse:
    // `CaptureFrame::t_start` is derived from *arrival* time ("biased late by
    // up to one poll"), so a tight bound fires on ordinary jitter.
    const JITTER_TOLERANCE_SAMPLES: i64 = 4 * FRAME_SAMPLES_HINT;
    let jumped = matches!(sdr.dab.next_sample, Some(expected) if (start - expected).abs() > JITTER_TOLERANCE_SAMPLES);
    let spliced = dropped > 0 || jumped;
    sdr.dab.next_sample = Some(start + pairs);
    let now = sdr.dab.last_frame_at.unwrap_or_default();
    let Some(rx) = sdr.dab.rx.as_mut() else {
        return;
    };
    if spliced {
        rx.discard_buffer();
    }
    // Which ensemble is held going in, so an expiry inside this push can
    // say what went — after it the receiver's table is empty.
    let held = rx
        .is_locked()
        .then(|| super::dab_state::LostEnsemble::of(rx.ensemble()));
    rx.push_iq(&frame.channels[0].data);
    let decoded = rx.take_msc_frames();
    let locked = rx.is_locked();
    sdr.dab.gone = match (locked, held, sdr.dab.gone.take()) {
        // A lock answers every earlier loss.
        (true, _, _) => None,
        (false, Some(ensemble), _) => Some(super::Gone {
            cause: super::GoneCause::Expired,
            at: now,
            ensemble: Some(ensemble),
        }),
        // A frame arrived, so "no input" is no longer true; the receiver is
        // searching again.
        (false, None, Some(g)) if g.cause == super::GoneCause::NoInput => None,
        (false, None, other) => other,
    };
    // The table can expire inside the receiver itself (no clean FIC for
    // seconds). A selection that outlives its table would keep the dock,
    // `get dab` and the playback transport naming a service that is not
    // there, so drop it with the table; the receiver is left running to
    // re-acquire.
    if !locked && sdr.dab.service.is_some() {
        sdr.dab_forget_selection();
    }
    if decoded.is_empty() {
        return;
    }
    if spliced {
        // The MSC de-interleavers were restarted; the outputs until their
        // warm-up completes mix ring history from before the hole, so drop
        // the parsers' partial segments rather than complete them with
        // garbage. The playback worker's transports restart with them: a
        // seam is not a signal.
        sdr.dab.pad.clear();
        if let Some(audio) = &sdr.dab.audio {
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
            if let Some(audio) = &sdr.dab.audio {
                audio.push(decoded.sub_channel, &decoded.bytes);
            }
            continue;
        }
        let parser = sdr.dab.pad.entry(decoded.sub_channel).or_default();
        parser.push_pad_region(&decoded.bytes);
    }
}

/// The DLS text of the selected service, once its reassembly completed and
/// its CRCs passed. `None` when nothing is selected, the table is
/// unpublished, or no complete label has arrived yet.
pub fn dls(sdr: &SdrState) -> Option<&str> {
    let sid = sdr.dab.service?;
    let status = sdr.dab.rx.as_ref()?.status();
    let sub = status.ensemble.services.get(&sid)?.sub_channel?;
    sdr.dab.pad.get(&sub)?.dls()
}

/// Land the hardware window on a Band III block (`sdr dab channel …`).
///
/// The DAB front end measures only ±500 Hz of carrier offset, so "tuned to DAB" means the hardware centre
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
    // Moving the window is what forgets the old block's ensemble, and
    // `set_centre` is the one place that decides it: a block change goes
    // through `dab_reset` like every other retune, and re-selecting the
    // block the hardware already sits on leaves a running receiver alone.
    sdr.set_centre(target.centre_hz);
    // `set_centre` only drags Tuned along with Follow on; a channel pick is
    // a tune in its own right, so the cursor moves too.
    sdr.tuned_hz = target.centre_hz;
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
        sdr.dab.rx = Some(neowon_dsp::dab::DabReceiver::new());
        sdr.dab.service = Some(0x1001);
        sdr.dab.next_sample = Some(4096);
        assert!(channel(&mut sdr, &DabChannel::Label("11C".into())).is_ok());
        assert_eq!(sdr.dab.service, Some(0x1001), "same block: no reset");
        assert!(sdr.dab.on());

        assert!(channel(&mut sdr, &DabChannel::Next).is_ok());
        assert!(
            !sdr.dab.holds_derived(),
            "new block: every derived field goes, not just the selection"
        );
        assert!(sdr.dab.on(), "the receiver stays on, just reset");
    }

    /// One contiguous IQ frame of `pairs` pairs starting at sample `at`,
    /// reporting `dropped` pairs lost before it.
    fn iq_frame(seq: u64, at: u64, pairs: usize, dropped: u64) -> neowon_core::CaptureFrame {
        use neowon_core::{AcqMode, ChannelCapture, IqCal, SampleLayout};
        let rate = neowon_dsp::dab::SAMPLE_RATE;
        neowon_core::CaptureFrame::new(
            seq,
            Some(at as f64 / rate),
            rate,
            AcqMode::Sample,
            neowon_backend::Acquisition::Stream { chunk: pairs },
            SampleLayout::Complex,
            vec![ChannelCapture {
                ch: 0,
                data: vec![0.0; 2 * pairs],
                cal: IqCal::real(1.0, 0.0),
                clipped: false,
                freq_meter: None,
            }],
        )
        .unwrap()
        .with_dropped_before(dropped)
    }

    /// A gap the backend counted must reach both the decoder (as a
    /// splice) and the operator (as a number). The timestamps here are
    /// deliberately contiguous-looking — well inside the coarse jitter
    /// tolerance — so the *only* thing that can trip the splice is the
    /// reported count: drop either half of the wiring and this fails.
    #[test]
    fn a_reported_drop_splices_the_receiver_and_shows_in_get_sdr() {
        const N: usize = 8192;
        let mut sdr = tune(220_352_000.0);
        sdr.active = true;
        sdr.dab.rx = Some(neowon_dsp::dab::DabReceiver::new());
        let buffered = |s: &SdrState| s.dab.rx.as_ref().unwrap().buffered();

        // Contiguous frames accumulate: no gap, no splice.
        for seq in 0..2u64 {
            let f = iq_frame(seq, seq * N as u64, N, 0);
            sdr.note_frame(&f, seq as f64);
            feed(&mut sdr, &f);
        }
        assert_eq!(buffered(&sdr), 2 * N, "a clean stream is never spliced");
        assert_eq!((sdr.dropped_pairs, sdr.drop_events), (0, 0));

        // A frame that reports a gap: the samples before it are not
        // contiguous with it, so the receiver's buffer must go.
        let f = iq_frame(2, 2 * N as u64, N, 4096);
        sdr.note_frame(&f, 2.0);
        feed(&mut sdr, &f);
        assert_eq!(
            buffered(&sdr),
            N,
            "a counted drop must discard the buffer, not be bridged"
        );

        // And the same gap must be visible to the operator.
        assert_eq!((sdr.dropped_pairs, sdr.drop_events), (4096, 1));
        let json = super::super::sdr_json(&sdr, None);
        assert!(
            json.contains(r#""dropped_pairs":4096"#),
            "get sdr must report the gap: {json}"
        );
        assert!(json.contains(r#""drop_events":1"#), "{json}");

        // The next clean frame accumulates again, and the totals hold.
        let f = iq_frame(3, 3 * N as u64 + 4096, N, 0);
        sdr.note_frame(&f, 3.0);
        feed(&mut sdr, &f);
        assert_eq!(buffered(&sdr), 2 * N, "one gap, spliced once");
        assert_eq!((sdr.dropped_pairs, sdr.drop_events), (4096, 1));
    }
}
