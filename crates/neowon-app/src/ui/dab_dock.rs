//! The sync line is always drawn while the receiver runs: an unlocked
//! receiver that silently showed nothing would look like an absent one.
//!
//! **Everything here fits the rail**: the dock scrolls vertically
//! only, so a row wider than the rail is cut off, unreachably. Rows that can
//! grow (the block row, the transport's status) wrap, and long readouts are
//! labels, which wrap, rather than one-line monospace runs.

use bevy_egui::egui;

use super::sdr_view::inject;
use crate::refmap::RefMap;
use crate::script::Script;
use crate::sdr::{DabChannel, DabService, DabVerb, GoneCause, SdrAction, SdrState};
use crate::uitree;

pub fn show(
    ui: &mut egui::Ui,
    sdr: &SdrState,
    caps: Option<&neowon_backend::SdrCaps>,
    rm: &RefMap,
    script: &mut Script,
    now: f64,
) {
    ui.horizontal_wrapped(|ui| {
        let mut on = sdr.dab.on();
        if ui.checkbox(&mut on, "Decode").changed() {
            inject(
                script,
                SdrAction::Dab(if on { DabVerb::On } else { DabVerb::Off }),
            );
        }
        if sdr.dab.on() && ui.button("Reset").clicked() {
            inject(script, SdrAction::Dab(DabVerb::Reset));
        }
        if let Some(rx) = &sdr.dab.rx {
            ui.label(format!("{:+} Hz", rx.freq_offset_hz.round()));
        }
    });

    channel_row(ui, sdr, rm, script);

    // DAB is wideband and Mode I is defined at exactly 2.048 MS/s; the
    // receiver cannot lock at any other rate, and "not locked" is not a
    // diagnosis. Say the requirement and offer the one click that meets it.
    let rate_ok = (sdr.config.sample_rate - neowon_dsp::dab::SAMPLE_RATE).abs() < 1.0;
    if !rate_ok {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "DAB needs 2.048 MS/s; this link runs at {:.3} MS/s (Receiver -> Rate)",
                sdr.config.sample_rate / 1e6
            ),
        );
        if ui.button("Set 2.048 MS/s").clicked() {
            inject(script, SdrAction::Rate(neowon_dsp::dab::SAMPLE_RATE));
        }
    } else if sdr.dab.on() {
        // The wheel does not move a DAB link's rate (sdr::zoom pins it),
        // so what it does instead is said here rather than left to look
        // like a broken gesture.
        ui.weak("rate pinned to 2.048 MS/s; wheel zooms the span only");
    }
    if sdr.dab.on() {
        let sim = caps.is_some_and(|c| c.tuner == "sim");
        let mhz = sdr.config.centre_hz / 1e6;
        if !sim && !(174.0..=240.0).contains(&mhz) {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("DAB lives in Band III (174-240 MHz); the centre is {mhz:.3} MHz"),
            );
        }
    }
    let Some(rx) = &sdr.dab.rx else {
        ui.weak("off - pick a Band III block above, then Decode");
        return;
    };
    let status = rx.status();
    if !status.locked {
        return unlocked(ui, sdr, rx, &status, now);
    }
    let ensemble = &status.ensemble;
    ui.monospace(format!(
        "{}  EId {:04X}",
        ensemble.label.as_deref().unwrap_or("<no label yet>"),
        ensemble.eid.unwrap_or(0)
    ));
    ui.label(format!("locked  PRS {:.2}", rx.prs_metric()));
    cumulative(ui, rx, &status);

    // The transport and the DLS line sit above the list: they are what a
    // click on a service changes, and at the default window a list of five
    // services pushed them below the fold.
    ui.separator();
    transport(ui, sdr, &status, script);
    // The dynamic label of the selected service; a partial or CRC-bad label
    // is never shown, so `-` means "not decoded yet".
    let dls = crate::sdr::dab::dls(sdr).unwrap_or("-");
    let response = ui.label(egui::RichText::new(format!("DLS  {dls}")).monospace());
    uitree::node(
        ui.ctx(),
        ui.id().with("dab-dls"),
        egui::accesskit::Role::Label,
        &format!("dab dls {dls}"),
        response.rect,
    );

    // The selection is what `sdr dab service` sets, so the UI and a script
    // land on the same state. A service this build can never play is greyed
    // and says why — the reason is the one `sdr dab play` would give, from
    // the same function.
    ui.separator();
    ui.label("Services");
    for service in ensemble.services.values() {
        let selected = sdr.dab.service == Some(service.sid);
        let why_not = crate::sdr::dab_audio::service_spec(&status, service.sid).err();
        let text = format!(
            "{:04X}  {}  {}",
            service.sid,
            service.label.as_deref().unwrap_or("<no label yet>"),
            service.coding_label()
        );
        let text = match &why_not {
            Some(_) => egui::RichText::new(text).weak(),
            None => egui::RichText::new(text),
        };
        let sub = service
            .sub_channel
            .map_or("-".to_string(), |id| id.to_string());
        let row = ui
            .selectable_label(selected, text)
            .on_hover_text(match &why_not {
                Some(why) => format!("sub-channel {sub}\ncannot play: {why}"),
                None => format!("sub-channel {sub}"),
            });
        if row.clicked() {
            inject(
                script,
                SdrAction::Dab(DabVerb::Service(DabService::Sid(service.sid))),
            );
        }
        let mut rect = row.rect;
        if let Some(why) = &why_not {
            rect = rect.union(ui.small(format!("   cannot play: {why}")).rect);
        }
        uitree::node(
            ui.ctx(),
            ui.id().with(("dab-service", service.sid)),
            egui::accesskit::Role::Label,
            &match &why_not {
                Some(why) => format!("dab service {:04X} cannot play: {why}", service.sid),
                None => format!("dab service {:04X} playable", service.sid),
            },
            rect,
        );
    }
    if ensemble.data_services > 0 {
        ui.weak(format!(
            "{} data services (not decoded in tier 1)",
            ensemble.data_services
        ));
    }
}

/// Scroll the DAB section to the top of the rail on the frame the receiver
/// turns on, whoever turned it on (dock, View menu, script): the switch
/// that reveals DAB has to reveal it, not open a section below the fold.
pub fn reveal(ui: &egui::Ui, header: &egui::Response, on: bool) {
    let was_on = egui::Id::new("sdr-dock-dab-was-on");
    if on && !ui.data(|d| d.get_temp::<bool>(was_on).unwrap_or(false)) {
        header.scroll_to_me(Some(egui::Align::TOP));
    }
    ui.data_mut(|d| d.insert_temp(was_on, on));
}

/// The unlocked readout: what happened, then how hard the receiver is
/// looking. "not locked" alone reads the same for a receiver that never
/// found anything, one whose ensemble just went away and one with no input
/// at all, so each is said in words.
fn unlocked(
    ui: &mut egui::Ui,
    sdr: &SdrState,
    rx: &neowon_dsp::dab::DabReceiver,
    status: &neowon_dsp::dab::DabStatus,
    now: f64,
) {
    let warn = ui.visuals().warn_fg_color;
    let what = match &sdr.dab.gone {
        _ if !sdr.config.running => "no input - the SDR is stopped (front panel Run)".to_string(),
        Some(g) => {
            let age = (now - g.at).max(0.0);
            let which = g
                .ensemble
                .as_ref()
                .map_or_else(|| "the ensemble".to_string(), |e| e.describe());
            match g.cause {
                GoneCause::Expired => format!(
                    "lock lost {age:.0} s ago: {which} - no clean FIC since, its table expired"
                ),
                GoneCause::NoInput => {
                    format!("no input for {age:.0} s: the IQ stream stopped; {which} dropped")
                }
            }
        }
        None => "not locked - searching for an ensemble at this centre".to_string(),
    };
    let heading = ui.colored_label(warn, &what);
    uitree::node(
        ui.ctx(),
        ui.id().with("dab-lock"),
        egui::accesskit::Role::Label,
        &format!("dab lock {what}"),
        heading.rect,
    );
    // While unlocked the accepted-frame metric is 0 by construction, so it
    // answers nothing. The attempt score is what says whether DAB energy is
    // present at the tuned centre at all.
    ui.label(format!(
        "last attempt PRS {:.2} (lock needs {:.2})",
        rx.last_attempt_metric(),
        neowon_dsp::dab::PRS_METRIC_MIN
    ));
    cumulative(ui, rx, status);
}

/// The receiver's lifetime counters, labelled as such: a FIB CRC rate of
/// 100% beside "lock lost" is not a contradiction once it says it counts
/// since the receiver started.
fn cumulative(
    ui: &mut egui::Ui,
    rx: &neowon_dsp::dab::DabReceiver,
    status: &neowon_dsp::dab::DabStatus,
) {
    let rate = status
        .fib_crc_rate()
        .map_or_else(|| "-".to_string(), |r| format!("{:.0}%", r * 100.0));
    let text = format!(
        "since on/reset: FIB CRC {rate} of {}, {} frames, {} rejected",
        status.fib_total, status.frames, rx.frames_rejected
    );
    let r = ui.weak(&text).on_hover_text(
        "cumulative counters since Decode was switched on or last reset; \
         the lock state above is the current one",
    );
    uitree::node(
        ui.ctx(),
        ui.id().with("dab-cumulative"),
        egui::accesskit::Role::Label,
        &format!("dab cumulative {text}"),
        r.rect,
    );
}

/// Play/Stop and the playback worker's own report. `error` shows the
/// backend's typed reason (a stream this build cannot decode is loud, not
/// silent); a selection that can never play has Play disabled with the
/// reason, rather than offered and refused.
fn transport(
    ui: &mut egui::Ui,
    sdr: &SdrState,
    status: &neowon_dsp::dab::DabStatus,
    script: &mut Script,
) {
    let playing = crate::sdr::dab_audio::playing(sdr);
    let why_not = sdr
        .dab
        .service
        .map(|sid| crate::sdr::dab_audio::service_spec(status, sid).err());
    ui.horizontal_wrapped(|ui| {
        let can_play = matches!(why_not, Some(None)) && !playing;
        let play = ui
            .add_enabled(can_play, egui::Button::new("Play"))
            .on_disabled_hover_text(match &why_not {
                None => "click a service in the list below first".to_string(),
                Some(Some(why)) => format!("cannot play: {why}"),
                Some(None) => "playing".to_string(),
            });
        if play.clicked() {
            inject(script, SdrAction::Dab(DabVerb::Play));
        }
        if ui.add_enabled(playing, egui::Button::new("Stop")).clicked() {
            inject(script, SdrAction::Dab(DabVerb::Stop));
        }
        match &sdr.dab.audio {
            None => ui.weak(match why_not {
                None => "audio off - select a service",
                Some(Some(_)) => "this service cannot play",
                Some(None) => "audio off - press Play",
            }),
            // The device's state, from the one place that names it, so the
            // dock, `get audio` and `get dab` cannot disagree.
            Some(_) => ui.label(sdr.audio_state()),
        };
    });
    let Some(worker) = &sdr.dab.audio else {
        if let Some(reason) = &sdr.dab.play_error {
            ui.colored_label(ui.visuals().error_fg_color, reason);
        }
        return;
    };
    let status = worker.status();
    ui.horizontal_wrapped(|ui| {
        let meter = ui.add(
            egui::ProgressBar::new(status.peak.clamp(0.0, 1.0))
                .desired_width(90.0)
                .text(format!("{:.3}", status.peak)),
        );
        let mut detail = if status.rate > 0 {
            format!(
                "{}  {} Hz  {} ch",
                status.backend, status.rate, status.channels
            )
        } else {
            status.backend.to_string()
        };
        // While the transport starts, the counters are the diagnosis: zero
        // decoded means no sub-channel bytes reached the codec.
        detail.push_str(&format!("  {} decoded", status.decoded));
        if status.dropped > 0 {
            detail.push_str(&format!("  {} dropped", status.dropped));
        }
        let text = ui.weak(detail);
        uitree::node(
            ui.ctx(),
            ui.id().with("dab-audio"),
            egui::accesskit::Role::Label,
            &format!("dab audio {} peak {:.3}", sdr.audio_state(), status.peak),
            meter.rect.union(text.rect),
        );
    });
    if status.state == crate::sdr::dab_audio::AudioState::Error && !status.reason.is_empty() {
        ui.colored_label(ui.visuals().error_fg_color, &status.reason);
    }
}

/// The Band III block row: the block under the hardware centre, a picker
/// built from the active band plan's DAB allocation, and raster steps.
/// A plan with no DAB allocation is said out loud — the raster exists, the
/// plan's silence is a fact about the plan, not something to paper over
/// with invented per-country frequencies.
fn channel_row(ui: &mut egui::Ui, sdr: &SdrState, rm: &RefMap, script: &mut Script) {
    let blocks = rm.plan().map(|p| p.dab_blocks()).unwrap_or_default();
    let stem = rm.stem().to_string();
    let current = neowon_refdb::dab::band_iii_block_at(sdr.config.centre_hz);
    let allocated = current
        .as_ref()
        .is_some_and(|c| blocks.iter().any(|b| b.label == c.label));

    // Wrapped: the plan's verdict beside the block is the widest thing in
    // the section, and on one line it pushed the rail's content past its
    // right edge.
    let row = ui.horizontal_wrapped(|ui| {
        ui.label("Block");
        ui.add_enabled_ui(!blocks.is_empty(), |ui| {
            egui::ComboBox::from_id_salt("dab-channel")
                .selected_text(current.map_or("-", |b| b.label))
                .show_ui(ui, |ui| {
                    for b in &blocks {
                        if ui
                            .selectable_label(current.is_some_and(|c| c.label == b.label), b.label)
                            .clicked()
                        {
                            inject(
                                script,
                                SdrAction::Dab(DabVerb::Channel(DabChannel::Label(
                                    b.label.to_string(),
                                ))),
                            );
                        }
                    }
                });
        });
        if ui
            .button("<")
            .on_hover_text("previous Band III block")
            .clicked()
        {
            inject(script, SdrAction::Dab(DabVerb::Channel(DabChannel::Prev)));
        }
        if ui
            .button(">")
            .on_hover_text("next Band III block")
            .clicked()
        {
            inject(script, SdrAction::Dab(DabVerb::Channel(DabChannel::Next)));
        }
        match current {
            Some(b) => {
                ui.monospace(format!("{:.3} MHz", b.centre_hz / 1e6));
                if allocated {
                    ui.weak(format!("allocated by {stem}"));
                } else {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("not allocated by {stem}"),
                    );
                }
            }
            None => {
                ui.weak("no Band III block at this centre");
            }
        }
    });
    let text = match current {
        Some(b) => format!(
            "dab channel {} {:.3} MHz {} {stem}",
            b.label,
            b.centre_hz / 1e6,
            if allocated {
                "allocated by"
            } else {
                "not allocated by"
            }
        ),
        None => "dab channel none".to_string(),
    };
    uitree::node(
        ui.ctx(),
        ui.id().with("dab-channel"),
        egui::accesskit::Role::Label,
        &text,
        row.response.rect,
    );

    if !blocks.is_empty() {
        return;
    }
    if stem.is_empty() {
        ui.weak("no band plan loaded");
        return;
    }
    // "No DAB here" is not a dead end: name plans that do declare one.
    let hints: Vec<&str> = rm
        .plans
        .iter()
        .filter(|(_, p)| p.declares_dab())
        .map(|(s, _)| s.as_str())
        .take(4)
        .collect();
    let mut line = format!("{stem} declares no DAB allocation");
    if !hints.is_empty() {
        line.push_str(&format!(" - try bandplan {}", hints.join(" | ")));
    }
    ui.weak(line);
}
