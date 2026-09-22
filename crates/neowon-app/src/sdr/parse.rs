//! The script parser for SDR actions (`sdr <verb> …`, `sim iq …`,
//! `instrument scope|sdr`). Split from `actions.rs` along its second job
//! (review M12): the action type and its application live there, the words
//! live here.

use super::actions::{DabChannel, DabService, DabVerb, IqDumpVerb, SdrAction, SurveyRequest};

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

/// `sdr dab service`: `#n` is the 1-based position in the locked table, a
/// bare number is a `SId` (decimal, or `0x` hex). The `#` keeps a service
/// number and a table position from being confused.
fn parse_dab_service(s: &str) -> Result<DabService, String> {
    if let Some(index) = s.strip_prefix('#') {
        return index
            .parse()
            .map(DabService::Index)
            .map_err(|_| format!("bad service index {index:?}"));
    }
    let (digits, radix) = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None => (s, 10),
    };
    u16::from_str_radix(digits, radix)
        .map(DabService::Sid)
        .map_err(|_| format!("bad service id {s:?}"))
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
        "dab" => match next()? {
            "on" => SdrAction::Dab(DabVerb::On),
            "off" => SdrAction::Dab(DabVerb::Off),
            "reset" => SdrAction::Dab(DabVerb::Reset),
            "play" => SdrAction::Dab(DabVerb::Play),
            "stop" => SdrAction::Dab(DabVerb::Stop),
            "service" => SdrAction::Dab(DabVerb::Service(parse_dab_service(next()?)?)),
            // A block label (`11C`) and the raster steps; the label is
            // resolved at apply time so a typo is a refused command, not a
            // parse failure that hides which argument was wrong.
            "channel" => SdrAction::Dab(DabVerb::Channel(match next()? {
                "next" => DabChannel::Next,
                "prev" => DabChannel::Prev,
                label => DabChannel::Label(label.to_string()),
            })),
            other => {
                return Err(format!(
                    "dab: expected on|off|reset|play|stop|service|channel, got {other:?}"
                ));
            }
        },
        "iqdump" => match next()? {
            "off" => SdrAction::IqDump(IqDumpVerb::Stop),
            path => SdrAction::IqDump(IqDumpVerb::Start {
                path: path.to_string(),
                seconds: num(next()?)?,
            }),
        },
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
        "pan" => SdrAction::Pan(parse_hz(next()?)?),
        "centre" | "center" => SdrAction::Centre(parse_hz(next()?)?),
        "follow" => SdrAction::Follow(on_off(next()?)?),
        "width" => match next()? {
            "auto" => SdrAction::Width(None),
            w => SdrAction::Width(Some(parse_hz(w)?)),
        },
        "demod" => match next()? {
            "off" => SdrAction::Demod(None),
            m => SdrAction::Demod(Some(
                neowon_dsp::DemodMode::parse(m)
                    .ok_or_else(|| format!("unknown demod {m:?}; use am|nfm|wfm|off"))?,
            )),
        },
        "volume" => SdrAction::Volume(num(next()?)? as f32),
        "mute" => SdrAction::Mute(on_off(next()?)?),
        "squelch" => match next()? {
            "off" => SdrAction::Squelch(None),
            db => SdrAction::Squelch(Some(num(db)?)),
        },
        "list" => SdrAction::List(next()?.parse().map_err(|_| "bad height".to_string())?),
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
            parse(&mut words("centre 99M")).unwrap(),
            SdrAction::Centre(99e6)
        );
        assert_eq!(
            parse(&mut words("follow on")).unwrap(),
            SdrAction::Follow(true)
        );
        assert_eq!(
            parse(&mut words("width auto")).unwrap(),
            SdrAction::Width(None)
        );
        assert_eq!(
            parse(&mut words("width 15k")).unwrap(),
            SdrAction::Width(Some(15e3))
        );
        assert_eq!(
            parse(&mut words("demod nfm")).unwrap(),
            SdrAction::Demod(Some(neowon_dsp::DemodMode::Nfm))
        );
        assert_eq!(
            parse(&mut words("demod off")).unwrap(),
            SdrAction::Demod(None)
        );
        assert!(parse(&mut words("demod ssb")).is_err());
        assert_eq!(
            parse(&mut words("volume 0.5")).unwrap(),
            SdrAction::Volume(0.5)
        );
        assert_eq!(parse(&mut words("mute on")).unwrap(), SdrAction::Mute(true));
        assert_eq!(
            parse(&mut words("squelch off")).unwrap(),
            SdrAction::Squelch(None)
        );
        assert_eq!(
            parse(&mut words("squelch -30")).unwrap(),
            SdrAction::Squelch(Some(-30.0))
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
        assert_eq!(
            parse(&mut words("dab on")).unwrap(),
            SdrAction::Dab(DabVerb::On)
        );
        assert_eq!(
            parse(&mut words("dab service #2")).unwrap(),
            SdrAction::Dab(DabVerb::Service(DabService::Index(2)))
        );
        assert_eq!(
            parse(&mut words("dab service 0x1001")).unwrap(),
            SdrAction::Dab(DabVerb::Service(DabService::Sid(0x1001)))
        );
        assert_eq!(
            parse(&mut words("dab service 4097")).unwrap(),
            SdrAction::Dab(DabVerb::Service(DabService::Sid(4097)))
        );
        assert_eq!(
            parse(&mut words("dab play")).unwrap(),
            SdrAction::Dab(DabVerb::Play)
        );
        assert_eq!(
            parse(&mut words("dab stop")).unwrap(),
            SdrAction::Dab(DabVerb::Stop)
        );
        assert_eq!(
            parse(&mut words("dab channel next")).unwrap(),
            SdrAction::Dab(DabVerb::Channel(DabChannel::Next))
        );
        assert_eq!(
            parse(&mut words("dab channel prev")).unwrap(),
            SdrAction::Dab(DabVerb::Channel(DabChannel::Prev))
        );
        assert_eq!(
            parse(&mut words("dab channel 11C")).unwrap(),
            SdrAction::Dab(DabVerb::Channel(DabChannel::Label("11C".into())))
        );
        assert!(parse(&mut words("dab channel")).is_err());
        assert!(parse(&mut words("dab service #x")).is_err());
        assert!(parse(&mut words("dab service warp")).is_err());
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
}
