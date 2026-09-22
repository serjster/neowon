//! The DAB dock section: the receiver switch, the sync quality that backs
//! whatever is shown, the ensemble's service list (click to select) and the
//! selected service's DLS line. The section body lives here rather than in
//! `sdr_dock.rs` so that file keeps its line budget.
//!
//! The sync line is always drawn while the receiver runs: an unlocked
//! receiver that silently showed nothing would look like an absent one.

use bevy_egui::egui;

use super::sdr_view::inject;
use crate::refmap::RefMap;
use crate::script::Script;
use crate::sdr::{DabChannel, DabService, DabVerb, SdrAction, SdrState};
use crate::uitree;

pub fn show(ui: &mut egui::Ui, sdr: &SdrState, rm: &RefMap, script: &mut Script) {
    ui.horizontal(|ui| {
        let mut on = sdr.dab.is_some();
        if ui.checkbox(&mut on, "Decode").changed() {
            inject(
                script,
                SdrAction::Dab(if on { DabVerb::On } else { DabVerb::Off }),
            );
        }
        if sdr.dab.is_some() && ui.button("Reset").clicked() {
            inject(script, SdrAction::Dab(DabVerb::Reset));
        }
        if let Some(rx) = &sdr.dab {
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
    } else if sdr.dab.is_some() {
        // The wheel does not move a DAB link's rate (sdr::zoom pins it),
        // so what it does instead is said here rather than left to look
        // like a broken gesture.
        ui.weak("rate pinned to 2.048 MS/s; wheel zooms the span only");
    }
    if sdr.dab.is_some() {
        let sim = sdr.caps.as_ref().is_some_and(|c| c.tuner == "sim");
        let mhz = sdr.config.centre_hz / 1e6;
        if !sim && !(174.0..=240.0).contains(&mhz) {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("DAB lives in Band III (174-240 MHz); the centre is {mhz:.3} MHz"),
            );
        }
    }
    let Some(rx) = &sdr.dab else {
        ui.weak("off - pick a Band III block above, then Decode");
        return;
    };
    let status = rx.status();
    let rate = status
        .fib_crc_rate()
        .map_or_else(|| "-".to_string(), |r| format!("{:.0}%", r * 100.0));
    let sync = format!(
        "FIB CRC {rate}  {} frames  {}",
        status.frames,
        if status.locked {
            // The accepted-frame score: the one behind the table.
            format!("PRS {:.2}", rx.prs_metric())
        } else {
            // While unlocked the accepted-frame metric is 0 by construction,
            // so it answers nothing. The attempt score is what says whether
            // DAB energy is present at the tuned centre at all, and the
            // rejected count shows the receiver is still looking.
            format!(
                "PRS attempt {:.2}  {} rejected",
                rx.last_attempt_metric(),
                rx.frames_rejected
            )
        }
    );
    if !status.locked {
        ui.monospace(format!("not locked\n{sync}"));
        return;
    }
    let ensemble = &status.ensemble;
    ui.monospace(format!(
        "{}  EId {:04X}\n{sync}",
        ensemble.label.as_deref().unwrap_or("<no label yet>"),
        ensemble.eid.unwrap_or(0)
    ));

    // The services: click to select. The selection is what `sdr dab service`
    // sets, so the UI and a script land on the same state.
    ui.separator();
    ui.label("Services");
    for service in ensemble.services.values() {
        let selected = sdr.dab_service == Some(service.sid);
        let label = format!(
            "{:04X}  {}  {}",
            service.sid,
            service.label.as_deref().unwrap_or("<no label yet>"),
            service.coding_label()
        );
        if ui
            .selectable_label(selected, label)
            .on_hover_text(format!(
                "sub-channel {}",
                service
                    .sub_channel
                    .map_or("-".to_string(), |id| id.to_string())
            ))
            .clicked()
        {
            inject(
                script,
                SdrAction::Dab(DabVerb::Service(DabService::Sid(service.sid))),
            );
        }
    }
    if ensemble.data_services > 0 {
        ui.weak(format!(
            "{} data services (not decoded in tier 1)",
            ensemble.data_services
        ));
    }

    // Transport: the playback worker's own report, and a meter of the last
    // decoded block. `error` shows the backend's typed reason (a stream this
    // build cannot decode is loud, not silent).
    let playing = crate::sdr::dab_audio::playing(sdr);
    ui.horizontal(|ui| {
        let selected = sdr.dab_service.is_some();
        if ui
            .add_enabled(selected && !playing, egui::Button::new("Play"))
            .on_disabled_hover_text("click a service in the list above first")
            .clicked()
        {
            inject(script, SdrAction::Dab(DabVerb::Play));
        }
        if ui.add_enabled(playing, egui::Button::new("Stop")).clicked() {
            inject(script, SdrAction::Dab(DabVerb::Stop));
        }
        match &sdr.dab_audio {
            None => {
                if let Some(reason) = &sdr.dab_play_error {
                    ui.colored_label(ui.visuals().error_fg_color, reason);
                }
                ui.weak(if selected {
                    "audio off - Press Play"
                } else {
                    "audio off - select a service"
                });
            }
            Some(worker) => {
                let status = worker.status();
                ui.label(status.state.label());
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
                // While the transport starts, the counters are the diagnosis:
                // zero decoded means no sub-channel bytes reached the codec.
                detail.push_str(&format!("  {} decoded", status.decoded));
                if status.dropped > 0 {
                    detail.push_str(&format!("  {} dropped", status.dropped));
                }
                let text = ui.weak(detail);
                uitree::node(
                    ui.ctx(),
                    ui.id().with("dab-audio"),
                    egui::accesskit::Role::Label,
                    &format!("dab audio {} peak {:.3}", status.state.label(), status.peak),
                    meter.rect.union(text.rect),
                );
                if status.state == crate::sdr::dab_audio::AudioState::Error
                    && !status.reason.is_empty()
                {
                    ui.colored_label(ui.visuals().error_fg_color, &status.reason);
                }
            }
        }
    });

    // The dynamic label of the selected service; a partial or CRC-bad label
    // is never shown (D27), so `-` means "not decoded yet".
    let dls = crate::sdr::dab::dls(sdr).unwrap_or("-");
    let response = ui.monospace(format!("DLS  {dls}"));
    uitree::node(
        ui.ctx(),
        ui.id().with("dab-dls"),
        egui::accesskit::Role::Label,
        &format!("dab dls {dls}"),
        response.rect,
    );
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

    let row = ui.horizontal(|ui| {
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
