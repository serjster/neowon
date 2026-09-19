//! EiBi shortwave schedule importer (D17). Files are `;`-separated
//! Latin-1 CSVs named `sked-<a|b><yy>.csv`; season A runs from the last
//! Sunday of March to the last Sunday of October, B the rest of the year.
//! Columns: `kHz;Time(UTC);Days;ITU;Station;Lng;Target;Remarks;P;Start;Stop`.

use super::{Report, decode_latin1, fnv1a, remark_mode};
use crate::station::{Modulation, Schedule, Service, Source, Station};

pub const BASE_URL: &str = "https://www.eibispace.de/dx/";

/// The file for a UTC date, e.g. `sked-a26.csv`.
pub fn season_file((y, m, d): (i32, u8, u8)) -> String {
    let season = if (m, d) >= (3, last_sunday(y, 3)) && (m, d) < (10, last_sunday(y, 10)) {
        'a'
    } else {
        'b'
    };
    format!("sked-{season}{:02}.csv", y.rem_euclid(100))
}

fn last_sunday(y: i32, m: u8) -> u8 {
    let mut d = days_in_month(y, m);
    while weekday(y, m, d) != 0 {
        d -= 1;
    }
    d
}

fn days_in_month(y: i32, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        _ => 30,
    }
}

/// 0 = Sunday, via the civil-day count (Howard Hinnant's algorithm).
fn weekday(y: i32, m: u8, d: u8) -> u8 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    ((era * 146097 + doe - 719468 + 4).rem_euclid(7)) as u8
}

/// "0100-0300" → (60, 180); stop may be 2400.
fn parse_time(s: &str) -> Option<(u16, u16)> {
    let (a, b) = s.trim().split_once('-')?;
    let start = hhmm(a)?;
    let stop = hhmm(b)?;
    (start < 1440 && stop <= 1440).then_some((start, stop))
}

fn hhmm(s: &str) -> Option<u16> {
    let s = s.trim();
    if s.len() != 4 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: u16 = s[..2].parse().ok()?;
    let m: u16 = s[2..].parse().ok()?;
    (h <= 24 && m <= 59).then_some(h * 60 + m)
}

/// "Mo-Fr", "Sa,Su", "daily"; empty means every day.
fn parse_days(s: &str) -> Option<u8> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("daily") {
        return Some(Schedule::DAILY);
    }
    let mut mask = 0u8;
    for part in s.split([',', '/']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (day_index(a)?, day_index(b)?);
                if a <= b {
                    for d in a..=b {
                        mask |= 1 << d;
                    }
                } else {
                    for d in a..=6 {
                        mask |= 1 << d;
                    }
                    for d in 0..=b {
                        mask |= 1 << d;
                    }
                }
            }
            None => mask |= 1 << day_index(part)?,
        }
    }
    (mask != 0).then_some(mask)
}

fn day_index(s: &str) -> Option<u8> {
    let s = s.trim().to_ascii_lowercase();
    ["mo", "tu", "we", "th", "fr", "sa", "su"]
        .iter()
        .position(|d| s.starts_with(d))
        .map(|d| d as u8)
}

pub fn parse(bytes: &[u8]) -> (Vec<Station>, Report) {
    let text = decode_latin1(bytes);
    let mut stations = Vec::new();
    let mut report = Report::default();
    for (i, line) in text.lines().enumerate() {
        let row = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(';').collect();
        if f[0].to_ascii_lowercase().contains("khz") && f.len() > 1 {
            continue; // the header line
        }
        report.rows += 1;
        if f.len() < 8 {
            report.skip(row, format!("{} fields, need 8", f.len()));
            continue;
        }
        let Some(khz) = f[0].trim().parse::<f64>().ok() else {
            report.skip(row, format!("bad frequency {:?}", f[0].trim()));
            continue;
        };
        let Some((start_min, stop_min)) = parse_time(f[1]) else {
            report.skip(row, format!("bad time {:?}", f[1].trim()));
            continue;
        };
        let Some(days) = parse_days(f[2]) else {
            report.skip(row, format!("bad days {:?}", f[2].trim()));
            continue;
        };
        let name = f[4].trim().to_string();
        if name.is_empty() {
            report.skip(row, "no station name");
            continue;
        }
        let remarks = f[7].trim();
        let mut s = Station::new(
            Source::Eibi,
            format!("{:016x}", fnv1a(&format!("{khz}|{}|{}|{name}", f[1], f[2]))),
            name,
            khz * 1e3,
        );
        s.modulation = remark_mode(remarks).unwrap_or(Modulation::Am);
        s.service = Service::Broadcast;
        s.country = (!f[3].trim().is_empty()).then(|| f[3].trim().to_string());
        s.power_kw = f.get(8).and_then(|p| p.trim().parse::<f64>().ok());
        s.schedule = Some(Schedule {
            start_min,
            stop_min,
            days,
        });
        s.notes = [f[5].trim(), f[6].trim(), remarks]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        report.kept += 1;
        stations.push(s);
    }
    (stations, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_season_turns_on_the_last_sundays() {
        assert_eq!(season_file((2025, 3, 30)), "sked-a25.csv");
        assert_eq!(season_file((2025, 3, 29)), "sked-b25.csv");
        assert_eq!(season_file((2025, 10, 25)), "sked-a25.csv");
        assert_eq!(season_file((2025, 10, 26)), "sked-b25.csv"); // DST ends
        assert_eq!(season_file((2026, 1, 15)), "sked-b26.csv");
        assert_eq!(season_file((2026, 5, 1)), "sked-a26.csv");
    }

    #[test]
    fn times_and_days_parse_to_a_mask() {
        assert_eq!(parse_time("0000-2400"), Some((0, 1440)));
        assert_eq!(parse_time("2300-0100"), Some((1380, 60)));
        assert_eq!(parse_time("0060-1200"), None);
        assert_eq!(parse_time("12345-1200"), None);
        assert_eq!(parse_days("Mo-Fr"), Some(0b0001_1111));
        assert_eq!(parse_days("Sa,Su"), Some(0b0110_0000));
        assert_eq!(parse_days("Sa/Su"), Some(0b0110_0000));
        assert_eq!(parse_days(""), Some(Schedule::DAILY));
        assert_eq!(parse_days("Mo-Su"), Some(Schedule::DAILY));
        assert_eq!(parse_days("Fr-Mo"), Some(0b0111_0001)); // Fr, Sa, Su, Mo
        assert_eq!(parse_days("nonsense"), None);
    }

    #[test]
    fn a_fixture_row_becomes_an_accra_station() {
        let (stations, report) = parse(
            b"kHz;Time(UTC);Days;ITU;Station;Lng;Target;Remarks;P;Start;Stop\n\
              11780;0000-2400;Mo-Su;BRA;R. Nacional;Por;SA;DRM;50;0000;2400",
        );
        assert_eq!(report.rows, 1); // the header is not a row
        assert_eq!(report.kept, 1);
        let s = &stations[0];
        assert_eq!(s.freq_hz, 11.78e6);
        assert_eq!(s.name, "R. Nacional");
        assert_eq!(s.modulation, Modulation::Digital);
        assert_eq!(s.power_kw, Some(50.0));
        assert_eq!(s.notes, "Por · SA · DRM");
        assert_eq!(s.schedule.unwrap().duration_min(), 1440);
    }
}
