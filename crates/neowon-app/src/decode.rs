//! Protocol decoding in the app: pick a protocol, point it at channels, and
//! get an annotated event list back.
//!
//! All the work is `neowon_dsp::decode`, which is engine-free and knows
//! nothing about instruments. This module only holds the settings, runs the
//! decoder once per acquisition, and keeps the results for the UI, the
//! control socket, and the on-plot annotations.

use bevy::prelude::*;
use neowon_dsp::decode::{self, Digital, Event, EventKind, Threshold, digitize};

use crate::Link;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    #[default]
    Off,
    Uart,
    I2c,
    Spi,
    OneWire,
}

impl Protocol {
    pub const ALL: [Protocol; 5] = [
        Protocol::Off,
        Protocol::Uart,
        Protocol::I2c,
        Protocol::Spi,
        Protocol::OneWire,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Protocol::Off => "off",
            Protocol::Uart => "uart",
            Protocol::I2c => "i2c",
            Protocol::Spi => "spi",
            Protocol::OneWire => "1-wire",
        }
    }

    pub fn parse(s: &str) -> Option<Protocol> {
        Protocol::ALL
            .into_iter()
            .find(|p| p.name() == s || (s == "onewire" && *p == Protocol::OneWire))
    }

    /// Which channels this protocol needs, in order, for the UI to label.
    pub fn lines(self) -> &'static [&'static str] {
        match self {
            Protocol::Off => &[],
            Protocol::Uart | Protocol::OneWire => &["data"],
            Protocol::I2c => &["SCL", "SDA"],
            Protocol::Spi => &["SCK", "data", "CS"],
        }
    }
}

#[derive(Resource)]
pub struct DecodeState {
    pub protocol: Protocol,
    /// Channel index per line, indexed as `Protocol::lines`.
    pub channels: [usize; 3],
    pub uart: decode::uart::Config,
    pub spi: decode::spi::Config,
    /// Hysteresis as a fraction of the signal's peak-to-peak swing.
    pub hysteresis: f64,
    /// Results of the last run, and what went wrong if it did not run.
    pub events: Vec<Event>,
    pub error: Option<String>,
    /// Sample rate the results were produced at, for time readouts.
    pub sample_rate: f64,
    last_seq: u64,
}

impl Default for DecodeState {
    fn default() -> Self {
        Self {
            protocol: Protocol::Off,
            channels: [0, 1, 1],
            uart: decode::uart::Config::default(),
            spi: decode::spi::Config::default(),
            hysteresis: 0.2,
            events: Vec::new(),
            error: None,
            sample_rate: 0.0,
            last_seq: 0,
        }
    }
}

impl DecodeState {
    pub fn error_count(&self) -> usize {
        self.events.iter().filter(|e| e.kind.is_error()).count()
    }
}

/// Digitize one channel of the live record.
fn line(link: &Link, ch: usize, hysteresis: f64) -> Option<Digital> {
    let frame = link.latest.as_ref()?;
    let cap = frame.channels.iter().find(|c| c.ch == ch)?;
    digitize(
        &cap.raw,
        frame.sample_rate,
        Threshold::Relative { hysteresis },
    )
}

pub fn run(link: Res<Link>, mut st: ResMut<DecodeState>) {
    if st.protocol == Protocol::Off {
        if !st.events.is_empty() {
            st.events.clear();
            st.error = None;
        }
        return;
    }
    let Some(frame) = &link.latest else { return };
    if frame.seq == st.last_seq {
        return;
    }
    st.last_seq = frame.seq;
    st.sample_rate = frame.sample_rate;

    let h = st.hysteresis;
    let ch = st.channels;
    let result = match st.protocol {
        Protocol::Off => unreachable!(),
        Protocol::Uart => match line(&link, ch[0], h) {
            Some(d) => decode::uart::decode(&d, st.uart).map_err(|e| e.to_string()),
            None => Err("no usable signal on the data line".into()),
        },
        Protocol::OneWire => match line(&link, ch[0], h) {
            Some(d) => decode::onewire::decode(&d).map_err(|e| e.to_string()),
            None => Err("no usable signal on the data line".into()),
        },
        Protocol::I2c => match (line(&link, ch[0], h), line(&link, ch[1], h)) {
            (Some(scl), Some(sda)) => decode::i2c::decode(&scl, &sda).map_err(|e| e.to_string()),
            _ => Err("I2C needs a usable signal on both SCL and SDA".into()),
        },
        Protocol::Spi => match (line(&link, ch[0], h), line(&link, ch[1], h)) {
            (Some(sck), Some(data)) => {
                let cs = line(&link, ch[2], h);
                decode::spi::decode(&sck, &data, cs.as_ref(), st.spi).map_err(|e| e.to_string())
            }
            _ => Err("SPI needs a usable signal on both SCK and the data line".into()),
        },
    };
    match result {
        Ok(events) => {
            st.events = events;
            st.error = None;
        }
        Err(e) => {
            st.events.clear();
            st.error = Some(e);
        }
    }
}

/// Annotations over the trace: a tick at each event with its value, drawn
/// along the bottom of the plot.
pub fn draw(
    st: Res<DecodeState>,
    link: Res<Link>,
    layout: Res<crate::ui::layout::Layout>,
    deep: Res<crate::deep::DeepView>,
    mut gizmos: Gizmos,
) {
    // Annotations are positioned in record space; the timeline view is a
    // different axis entirely, so they would land in the wrong place.
    if st.protocol == Protocol::Off || st.events.is_empty() || deep.on {
        return;
    }
    let Some(frame) = &link.latest else { return };
    let n = frame.channels.first().map_or(0, |c| c.raw.len()).max(1);
    let w = layout.plot.width();
    let h = layout.plot.height();
    let o = layout.plot_center;
    let left = o.x - w / 2.0;
    let y = o.y - h / 2.0 + 14.0;
    for e in &st.events {
        let x0 = left + (e.start as f32 / n as f32) * w;
        let x1 = left + (e.end.max(e.start + 1) as f32 / n as f32) * w;
        let color = match e.kind {
            EventKind::Error(_) => Color::srgb(0.95, 0.3, 0.2),
            EventKind::Marker(_) => Color::srgb(0.4, 0.8, 1.0),
            EventKind::Ack(true) => Color::srgb(0.4, 0.9, 0.5),
            EventKind::Ack(false) => Color::srgb(0.95, 0.6, 0.2),
            EventKind::Word { .. } => Color::srgb(0.9, 0.85, 0.4),
        };
        gizmos.line_2d(Vec2::new(x0, y), Vec2::new(x1, y), color);
        gizmos.line_2d(Vec2::new(x0, y - 4.0), Vec2::new(x0, y + 4.0), color);
    }
}
