//! SDR script actions: `sdr <verb> …` and `sim iq --seed <n>`, parsed here
//! and applied to `SdrState` (the UI injects the same actions).

use bevy::log::error;
use neowon_backend::{Command, SdrGain};

use super::{FFT_SIZES, SdrState};
use crate::Link;

#[derive(Debug, Clone, PartialEq)]
pub enum SdrAction {
    Tune(f64),
    /// Relative retune, Hz.
    Step(f64),
    Rate(f64),
    /// `None` = tuner AGC.
    Gain(Option<f64>),
    Agc(bool),
    Ppm(f64),
    Span(f64),
    Fft(usize),
    Level {
        ref_db: f64,
        range_db: f64,
    },
    Run(bool),
    Seed(u64),
    Detect(bool),
    /// Start a survey; `None` stops a running one.
    Survey(Option<SurveyRequest>),
    /// Run the modulation lab on the signal nearest the tuned frequency.
    Analyse(bool),
    /// The modulation the lab assumes; `None` picks it from cumulants.
    Modulation(Option<neowon_core::Modulation>),
    /// Detection threshold over the floor, dB.
    Threshold(f64),
    /// `instrument sdr` (true) or `instrument scope`: switch instrument.
    Instrument(bool),
}

/// `sdr survey <start> <stop> [cap N] [skip lo:hi]…`.
#[derive(Debug, Clone, PartialEq)]
pub struct SurveyRequest {
    pub start_hz: f64,
    pub stop_hz: f64,
    /// Most peaks kept per step.
    pub peak_cap: usize,
    /// Ranges not to scan.
    pub skip: Vec<(f64, f64)>,
}

/// `99.4M`, `100k`, `1.2G` or plain Hz.
pub fn parse_hz(s: &str) -> Result<f64, String> {
    let (num, mul) = match s.chars().last() {
        Some('k' | 'K') => (&s[..s.len() - 1], 1e3),
        Some('M') => (&s[..s.len() - 1], 1e6),
        Some('G' | 'g') => (&s[..s.len() - 1], 1e9),
        _ => (s, 1.0),
    };
    num.parse::<f64>()
        .map(|v| v * mul)
        .map_err(|_| format!("bad frequency {s:?}"))
}

fn on_off(s: &str) -> Result<bool, String> {
    match s {
        "on" | "1" => Ok(true),
        "off" | "0" => Ok(false),
        _ => Err(format!("expected on/off, got {s:?}")),
    }
}

/// `sdr <verb> …` (the words after `sdr`).
pub fn parse<'a>(next: &mut dyn FnMut() -> Result<&'a str, String>) -> Result<SdrAction, String> {
    let num = |s: &str| s.parse::<f64>().map_err(|_| format!("bad number {s:?}"));
    Ok(match next()? {
        "tune" => SdrAction::Tune(parse_hz(next()?)?),
        "step" => SdrAction::Step(parse_hz(next()?)?),
        "rate" => SdrAction::Rate(parse_hz(next()?)?),
        "gain" => match next()? {
            "auto" => SdrAction::Gain(None),
            db => SdrAction::Gain(Some(num(db)?)),
        },
        "agc" => SdrAction::Agc(on_off(next()?)?),
        "ppm" => SdrAction::Ppm(num(next()?)?),
        "span" => SdrAction::Span(parse_hz(next()?)?),
        "fft" => SdrAction::Fft(next()?.parse().map_err(|_| "bad FFT size".to_string())?),
        "level" => SdrAction::Level {
            ref_db: num(next()?)?,
            range_db: num(next()?)?,
        },
        "run" => SdrAction::Run(on_off(next()?)?),
        "detect" => SdrAction::Detect(on_off(next()?)?),
        "survey" => match next()? {
            "stop" => SdrAction::Survey(None),
            start => {
                let (start, stop) = (parse_hz(start)?, parse_hz(next()?)?);
                let (mut cap, mut skip) = (32, Vec::new());
                while let Ok(w) = next() {
                    match w {
                        "cap" => cap = next()?.parse().map_err(|_| "bad cap".to_string())?,
                        "skip" => {
                            let r = next()?;
                            let (a, b) = r.split_once(':').ok_or("skip lo:hi")?;
                            skip.push((parse_hz(a)?, parse_hz(b)?));
                        }
                        other => return Err(format!("survey: unexpected {other:?}")),
                    }
                }
                SdrAction::Survey(Some(SurveyRequest {
                    start_hz: start,
                    stop_hz: stop,
                    peak_cap: cap,
                    skip,
                }))
            }
        },
        "analyse" | "analyze" => SdrAction::Analyse(on_off(next()?)?),
        "modulation" => match next()? {
            "auto" => SdrAction::Modulation(None),
            m => SdrAction::Modulation(Some(
                neowon_core::Modulation::parse(m)
                    .ok_or_else(|| format!("unknown modulation {m:?}"))?,
            )),
        },
        "threshold" => SdrAction::Threshold(num(next()?)?),
        other => return Err(format!("unknown sdr verb {other:?}")),
    })
}

/// `sim iq --seed <s>` (the words after `sim`).
pub fn parse_sim<'a>(
    next: &mut dyn FnMut() -> Result<&'a str, String>,
) -> Result<SdrAction, String> {
    match (next()?, next()?) {
        ("iq", "--seed") => Ok(SdrAction::Seed(
            next()?.parse().map_err(|_| "bad seed".to_string())?,
        )),
        _ => Err("expected: sim iq --seed <n>".into()),
    }
}

/// `instrument scope|sdr` (the word after `instrument`).
pub fn parse_instrument<'a>(
    next: &mut dyn FnMut() -> Result<&'a str, String>,
) -> Result<SdrAction, String> {
    match next()? {
        "sdr" => Ok(SdrAction::Instrument(true)),
        "scope" => Ok(SdrAction::Instrument(false)),
        m => Err(format!("unknown instrument {m:?}; use scope|sdr")),
    }
}

/// The script line for an action. Every variant has one (the match is
/// exhaustive), and `parse` reads it back to the same action: the UI
/// injects these actions, so this is the script-parity rule by
/// construction.
impl std::fmt::Display for SdrAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let on = |b: bool| if b { "on" } else { "off" };
        match self {
            SdrAction::Tune(hz) => write!(f, "sdr tune {hz}"),
            SdrAction::Step(hz) => write!(f, "sdr step {hz}"),
            SdrAction::Rate(r) => write!(f, "sdr rate {r}"),
            SdrAction::Gain(None) => write!(f, "sdr gain auto"),
            SdrAction::Gain(Some(db)) => write!(f, "sdr gain {db}"),
            SdrAction::Agc(b) => write!(f, "sdr agc {}", on(*b)),
            SdrAction::Ppm(p) => write!(f, "sdr ppm {p}"),
            SdrAction::Span(hz) => write!(f, "sdr span {hz}"),
            SdrAction::Fft(n) => write!(f, "sdr fft {n}"),
            SdrAction::Level { ref_db, range_db } => write!(f, "sdr level {ref_db} {range_db}"),
            SdrAction::Run(b) => write!(f, "sdr run {}", on(*b)),
            SdrAction::Seed(s) => write!(f, "sim iq --seed {s}"),
            SdrAction::Detect(b) => write!(f, "sdr detect {}", on(*b)),
            SdrAction::Survey(None) => write!(f, "sdr survey stop"),
            SdrAction::Survey(Some(r)) => {
                write!(
                    f,
                    "sdr survey {} {} cap {}",
                    r.start_hz, r.stop_hz, r.peak_cap
                )?;
                r.skip
                    .iter()
                    .try_for_each(|(a, b)| write!(f, " skip {a}:{b}"))
            }
            SdrAction::Analyse(b) => write!(f, "sdr analyse {}", on(*b)),
            SdrAction::Modulation(None) => write!(f, "sdr modulation auto"),
            SdrAction::Modulation(Some(m)) => write!(f, "sdr modulation {}", m.label()),
            SdrAction::Threshold(db) => write!(f, "sdr threshold {db}"),
            SdrAction::Instrument(sdr) => {
                write!(f, "instrument {}", if *sdr { "sdr" } else { "scope" })
            }
        }
    }
}

/// Run a script action, reporting a refusal in the log and the status
/// line (what `get status` returns).
pub fn run(a: SdrAction, sdr: &mut SdrState, link: &mut Link) {
    if let Err(e) = apply(a, sdr, link) {
        error!("script: sdr: {e}");
        link.status = format!("error: {e}");
    }
}

/// Apply a script action. Invalid values are refused, not clamped, so a
/// script learns it asked for something the instrument cannot do.
pub fn apply(a: SdrAction, sdr: &mut SdrState, link: &mut Link) -> Result<(), String> {
    let caps = sdr.caps.clone();
    let in_range = |hz: f64| match &caps {
        Some(c) if !(c.freq_range_hz.0..=c.freq_range_hz.1).contains(&hz) => Err(format!(
            "{hz} Hz outside {}..={} Hz",
            c.freq_range_hz.0, c.freq_range_hz.1
        )),
        _ => Ok(hz),
    };
    match a {
        SdrAction::Tune(hz) => sdr.config.centre_hz = in_range(hz)?,
        SdrAction::Step(hz) => sdr.config.centre_hz = in_range(sdr.config.centre_hz + hz)?,
        SdrAction::Rate(r) => {
            if let Some(c) = &caps
                && !c.sample_rates.iter().any(|&x| (x - r).abs() < 0.5)
            {
                return Err(format!("rate {r} not in {:?}", c.sample_rates));
            }
            if r <= 0.0 {
                return Err("rate must be positive".into());
            }
            sdr.config.sample_rate = r;
        }
        SdrAction::Gain(None) => sdr.config.gain = SdrGain::Auto,
        SdrAction::Gain(Some(db)) => {
            // Snap to the tuner's table so the readout says what it runs.
            let db = caps
                .as_ref()
                .and_then(|c| {
                    c.gains_db
                        .iter()
                        .copied()
                        .min_by(|a, b| (a - db).abs().total_cmp(&(b - db).abs()))
                })
                .unwrap_or(db);
            sdr.config.gain = SdrGain::Manual(db);
        }
        SdrAction::Agc(on) => sdr.config.agc = on,
        SdrAction::Ppm(p) => sdr.config.ppm = p,
        SdrAction::Run(on) => sdr.config.running = on,
        SdrAction::Span(hz) => {
            if hz < 0.0 {
                return Err("span must be >= 0".into());
            }
            sdr.span_hz = hz;
            return Ok(());
        }
        SdrAction::Fft(n) => {
            if !FFT_SIZES.contains(&n) {
                return Err(format!("FFT size {n} not in {FFT_SIZES:?}"));
            }
            sdr.fft_size = n;
            return Ok(());
        }
        SdrAction::Level { ref_db, range_db } => {
            if range_db <= 0.0 {
                return Err("range must be positive".into());
            }
            (sdr.ref_db, sdr.range_db) = (ref_db, range_db);
            return Ok(());
        }
        SdrAction::Analyse(on) => {
            sdr.analyse_on = on;
            if !on {
                sdr.analysis = None;
            }
            return Ok(());
        }
        SdrAction::Modulation(m) => {
            sdr.modulation = m;
            sdr.analysis = None;
            return Ok(());
        }
        SdrAction::Survey(None) => {
            sdr.survey = None;
            return Ok(());
        }
        SdrAction::Survey(Some(r)) => {
            let plan = neowon_sdr::survey::SurveyPlan {
                start_hz: r.start_hz,
                stop_hz: r.stop_hz,
                sample_rate: sdr.config.sample_rate,
                peak_cap: r.peak_cap,
                skip: r.skip,
                ..Default::default()
            };
            return super::scan::start(sdr, plan);
        }
        SdrAction::Detect(on) => {
            sdr.detect_on = on;
            if !on {
                sdr.tracker.clear();
            }
            return Ok(());
        }
        SdrAction::Threshold(db) => {
            if !(3.0..=60.0).contains(&db) {
                return Err(format!("threshold {db} dB outside 3..=60"));
            }
            sdr.threshold_db = db;
            return Ok(());
        }
        SdrAction::Instrument(to_sdr) => return super::instrument::switch(to_sdr, sdr, link),
        SdrAction::Seed(s) => {
            sdr.seed = s;
            let _ = link.sup.commands.send(Command::Seed(s));
            return Ok(());
        }
    }
    sdr.dirty = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words<'a>(s: &'a str) -> impl FnMut() -> Result<&'a str, String> {
        let mut w = s.split_whitespace();
        move || w.next().ok_or_else(|| "missing argument".to_string())
    }

    #[test]
    fn frequencies_take_suffixes() {
        assert_eq!(parse_hz("99.4M").unwrap(), 99.4e6);
        assert_eq!(parse_hz("100k").unwrap(), 100e3);
        assert_eq!(parse_hz("1.2G").unwrap(), 1.2e9);
        assert_eq!(parse_hz("2048000").unwrap(), 2.048e6);
        assert!(parse_hz("fast").is_err());
    }

    #[test]
    fn verbs_parse() {
        assert_eq!(
            parse(&mut words("tune 99.4M")).unwrap(),
            SdrAction::Tune(99.4e6)
        );
        assert_eq!(
            parse(&mut words("gain auto")).unwrap(),
            SdrAction::Gain(None)
        );
        assert_eq!(
            parse(&mut words("gain 29.7")).unwrap(),
            SdrAction::Gain(Some(29.7))
        );
        assert_eq!(parse(&mut words("agc on")).unwrap(), SdrAction::Agc(true));
        assert_eq!(
            parse(&mut words("level -10 80")).unwrap(),
            SdrAction::Level {
                ref_db: -10.0,
                range_db: 80.0
            }
        );
        assert_eq!(
            parse(&mut words("detect off")).unwrap(),
            SdrAction::Detect(false)
        );
        assert_eq!(
            parse(&mut words("threshold 9")).unwrap(),
            SdrAction::Threshold(9.0)
        );
        assert_eq!(
            parse(&mut words("modulation 16qam")).unwrap(),
            SdrAction::Modulation(Some(neowon_core::Modulation::Qam16))
        );
        assert_eq!(
            parse(&mut words("modulation auto")).unwrap(),
            SdrAction::Modulation(None)
        );
        assert!(parse(&mut words("modulation fm")).is_err());
        assert_eq!(
            parse(&mut words("survey 97M 101M cap 8 skip 99M:99.5M")).unwrap(),
            SdrAction::Survey(Some(SurveyRequest {
                start_hz: 97e6,
                stop_hz: 101e6,
                peak_cap: 8,
                skip: vec![(99e6, 99.5e6)],
            }))
        );
        assert_eq!(
            parse(&mut words("survey stop")).unwrap(),
            SdrAction::Survey(None)
        );
        assert!(parse(&mut words("warp 9")).is_err());
        assert_eq!(
            parse_sim(&mut words("iq --seed 7")).unwrap(),
            SdrAction::Seed(7)
        );
        assert!(parse_sim(&mut words("iq 7")).is_err());
        assert_eq!(
            parse_instrument(&mut words("sdr")).unwrap(),
            SdrAction::Instrument(true)
        );
        assert!(parse_instrument(&mut words("audio")).is_err());
    }

    /// Which variant an action is. Exhaustive, so a new variant fails to
    /// compile here until `every_action_round_trips` covers it.
    fn variant(a: &SdrAction) -> usize {
        match a {
            SdrAction::Tune(_) => 0,
            SdrAction::Step(_) => 1,
            SdrAction::Rate(_) => 2,
            SdrAction::Gain(_) => 3,
            SdrAction::Agc(_) => 4,
            SdrAction::Ppm(_) => 5,
            SdrAction::Span(_) => 6,
            SdrAction::Fft(_) => 7,
            SdrAction::Level { .. } => 8,
            SdrAction::Run(_) => 9,
            SdrAction::Seed(_) => 10,
            SdrAction::Detect(_) => 11,
            SdrAction::Survey(_) => 12,
            SdrAction::Analyse(_) => 13,
            SdrAction::Modulation(_) => 14,
            SdrAction::Threshold(_) => 15,
            SdrAction::Instrument(_) => 16,
        }
    }

    /// Script parity: every action the UI can inject prints as a script
    /// line that parses back to the same action.
    #[test]
    fn every_action_round_trips() {
        let all = [
            SdrAction::Tune(99.412_345e6),
            SdrAction::Step(-1e5),
            SdrAction::Rate(2.048e6),
            SdrAction::Gain(None),
            SdrAction::Gain(Some(29.7)),
            SdrAction::Agc(true),
            SdrAction::Ppm(-1.3),
            SdrAction::Span(200e3),
            SdrAction::Fft(8192),
            SdrAction::Level {
                ref_db: -12.5,
                range_db: 90.0,
            },
            SdrAction::Run(false),
            SdrAction::Seed(7),
            SdrAction::Detect(true),
            SdrAction::Survey(None),
            SdrAction::Survey(Some(SurveyRequest {
                start_hz: 88e6,
                stop_hz: 108e6,
                peak_cap: 8,
                skip: vec![(99e6, 99.5e6), (101e6, 101.2e6)],
            })),
            SdrAction::Analyse(true),
            SdrAction::Modulation(None),
            SdrAction::Modulation(Some(neowon_core::Modulation::Psk8)),
            SdrAction::Threshold(9.5),
            SdrAction::Instrument(true),
            SdrAction::Instrument(false),
        ];
        let mut seen = std::collections::BTreeSet::new();
        for a in all {
            seen.insert(variant(&a));
            let line = a.to_string();
            let (head, rest) = line.split_once(' ').unwrap();
            let mut w = words(rest);
            let back = match head {
                "sdr" => parse(&mut w),
                "sim" => parse_sim(&mut w),
                "instrument" => parse_instrument(&mut w),
                _ => panic!("{line}"),
            };
            assert_eq!(back.unwrap(), a, "{line}");
        }
        assert_eq!(seen.len(), 17, "a variant has no round-trip sample");
    }
}
