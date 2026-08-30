//! Decode dialog — pick a protocol, assign lines, read the result.

use bevy_egui::egui;

use crate::decode::{DecodeState, Protocol};
use crate::derived::fmt_si;
use neowon_dsp::decode::EventKind;

pub fn show(ui: &mut egui::Ui, st: &mut DecodeState) {
    ui.group(|ui| {
        ui.strong("Protocol");
        ui.horizontal_wrapped(|ui| {
            for p in Protocol::ALL {
                if ui.selectable_label(st.protocol == p, p.name()).clicked() {
                    st.protocol = p;
                }
            }
        });
        if st.protocol == Protocol::Off {
            return;
        }
        for (i, name) in st.protocol.lines().iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(*name);
                for ch in 0..2 {
                    if ui
                        .selectable_label(st.channels[i] == ch, format!("CH{}", ch + 1))
                        .clicked()
                    {
                        st.channels[i] = ch;
                    }
                }
            });
        }
        if st.protocol == Protocol::Uart {
            ui.horizontal(|ui| {
                ui.label("Baud");
                ui.add(
                    egui::DragValue::new(&mut st.uart.baud)
                        .speed(100.0)
                        .range(50.0..=20e6),
                );
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut st.uart.inverted, "Inverted");
                ui.checkbox(&mut st.uart.lsb_first, "LSB first");
            });
        }
        if st.protocol == Protocol::Spi {
            ui.horizontal(|ui| {
                ui.checkbox(&mut st.spi.cpol, "CPOL");
                ui.checkbox(&mut st.spi.cpha, "CPHA");
                ui.checkbox(&mut st.spi.msb_first, "MSB first");
            });
            ui.horizontal(|ui| {
                ui.label("Bits");
                let mut bits = st.spi.bits as u32;
                if ui
                    .add(egui::DragValue::new(&mut bits).range(1..=64))
                    .changed()
                {
                    st.spi.bits = bits as u8;
                }
            });
        }
        ui.add(
            egui::Slider::new(&mut st.hysteresis, 0.0..=0.45)
                .text("Hysteresis")
                .fixed_decimals(2),
        );
    });

    ui.group(|ui| {
        ui.strong("Result");
        if let Some(e) = &st.error {
            // The decoders refuse rather than guess; show why, verbatim.
            ui.label(
                egui::RichText::new(e)
                    .color(egui::Color32::from_rgb(230, 130, 90))
                    .small(),
            );
            return;
        }
        if st.events.is_empty() {
            ui.label(egui::RichText::new("nothing decoded yet").weak().small());
            return;
        }
        let errors = st.error_count();
        ui.label(
            egui::RichText::new(format!("{} events · {} errors", st.events.len(), errors))
                .monospace()
                .size(10.0)
                .color(if errors > 0 {
                    egui::Color32::from_rgb(230, 130, 90)
                } else {
                    egui::Color32::GRAY
                }),
        );
        let rate = st.sample_rate.max(1.0);
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("decode-grid").striped(true).show(ui, |ui| {
                    for e in &st.events {
                        let (t0, _) = e.seconds(rate);
                        ui.label(egui::RichText::new(fmt_si(t0, "s")).monospace().size(10.0));
                        let (text, color) = match &e.kind {
                            EventKind::Word { value, bits } => (
                                if *bits <= 8 {
                                    format!("0x{value:02X}  {}", printable(*value))
                                } else {
                                    format!("0x{value:X} ({bits} bits)")
                                },
                                egui::Color32::from_rgb(235, 220, 120),
                            ),
                            EventKind::Marker(m) => {
                                (m.to_string(), egui::Color32::from_rgb(120, 200, 255))
                            }
                            EventKind::Ack(true) => {
                                ("ACK".into(), egui::Color32::from_rgb(120, 220, 140))
                            }
                            EventKind::Ack(false) => {
                                ("NAK".into(), egui::Color32::from_rgb(235, 160, 90))
                            }
                            EventKind::Error(e) => {
                                (e.to_string(), egui::Color32::from_rgb(240, 110, 90))
                            }
                        };
                        ui.label(
                            egui::RichText::new(text)
                                .monospace()
                                .size(10.0)
                                .color(color),
                        );
                        ui.end_row();
                    }
                });
            });
    });
}

/// ASCII rendering of a byte, for the common case of decoding text.
fn printable(v: u64) -> String {
    let c = v as u8;
    if c.is_ascii_graphic() || c == b' ' {
        format!("'{}'", c as char)
    } else {
        String::new()
    }
}
