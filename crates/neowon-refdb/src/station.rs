//! A known station as the reference sources describe it: one row per
//! frequency, with the provenance that lets the UI say where it came from.
//! Nothing here knows about the operator's catalog.

use serde::{Deserialize, Serialize};

/// The modulation a station transmits, in the vocabulary the SDR's
/// demodulator understands. Unknown is honest: most sources do not
/// say, and guessing would tune the radio wrongly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modulation {
    Am,
    Fm,
    Wfm,
    Nfm,
    Usb,
    Lsb,
    Cw,
    Dab,
    Dvbt,
    Atsc,
    Digital,
    Unknown,
}

impl Modulation {
    pub fn label(self) -> &'static str {
        match self {
            Modulation::Am => "AM",
            Modulation::Fm => "FM",
            Modulation::Wfm => "WFM",
            Modulation::Nfm => "NFM",
            Modulation::Usb => "USB",
            Modulation::Lsb => "LSB",
            Modulation::Cw => "CW",
            Modulation::Dab => "DAB",
            Modulation::Dvbt => "DVB-T",
            Modulation::Atsc => "ATSC",
            Modulation::Digital => "digital",
            Modulation::Unknown => "?",
        }
    }

    /// The SDR demodulator that fits, as the script verb spells it;
    /// `None` leaves the demod unchanged.
    pub fn demod(self) -> Option<&'static str> {
        match self {
            Modulation::Am => Some("am"),
            Modulation::Wfm | Modulation::Fm => Some("wfm"),
            Modulation::Nfm => Some("nfm"),
            _ => None,
        }
    }

    /// The label (or a synonym) back to the variant, for filters and verbs.
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word.trim().to_ascii_lowercase().as_str() {
            "am" => Modulation::Am,
            "fm" => Modulation::Fm,
            "wfm" => Modulation::Wfm,
            "nfm" => Modulation::Nfm,
            "usb" => Modulation::Usb,
            "lsb" => Modulation::Lsb,
            "cw" => Modulation::Cw,
            "dab" => Modulation::Dab,
            "dvbt" | "dvb-t" => Modulation::Dvbt,
            "atsc" => Modulation::Atsc,
            "digital" | "drm" => Modulation::Digital,
            "?" | "unknown" => Modulation::Unknown,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Modulation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Broadcast,
    Aviation,
    Marine,
    Amateur,
    Utility,
    Other,
}

impl Service {
    pub fn label(self) -> &'static str {
        match self {
            Service::Broadcast => "broadcast",
            Service::Aviation => "aviation",
            Service::Marine => "marine",
            Service::Amateur => "amateur",
            Service::Utility => "utility",
            Service::Other => "other",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Some(match word.trim().to_ascii_lowercase().as_str() {
            "broadcast" => Service::Broadcast,
            "aviation" | "air" => Service::Aviation,
            "marine" => Service::Marine,
            "amateur" | "ham" => Service::Amateur,
            "utility" => Service::Utility,
            "other" => Service::Other,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// The public database a station came from. The file stem is also its
/// snapshot name in the refdb store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Wikidata,
    Eibi,
    OurAirports,
    Fcc,
    Fmlist,
}

impl Source {
    pub const ALL: [Source; 5] = [
        Source::Wikidata,
        Source::Eibi,
        Source::OurAirports,
        Source::Fcc,
        Source::Fmlist,
    ];

    pub fn stem(self) -> &'static str {
        match self {
            Source::Wikidata => "wikidata",
            Source::Eibi => "eibi",
            Source::OurAirports => "ourairports",
            Source::Fcc => "fcc",
            Source::Fmlist => "fmlist",
        }
    }

    pub fn from_stem(s: &str) -> Option<Self> {
        Source::ALL.into_iter().find(|x| x.stem() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Source::Wikidata => "Wikidata",
            Source::Eibi => "EiBi",
            Source::OurAirports => "OurAirports",
            Source::Fcc => "FCC",
            Source::Fmlist => "FMLIST",
        }
    }

    /// The default licence note for a fresh snapshot; the operator's
    /// import carries its own.
    pub fn licence(self) -> &'static str {
        match self {
            Source::Wikidata => "CC0",
            Source::Eibi => "EiBi (Eike Bierwirth); see eibispace.de for terms",
            Source::OurAirports => "Public domain",
            Source::Fcc => "Public domain (US government work)",
            Source::Fmlist => "FMLIST terms; operator's own export",
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A broadcast's weekly schedule: UTC minutes since midnight and a weekday
/// mask. `days` bit 0 is Monday; a `stop_min` at or before `start_min` wraps
/// past midnight (EiBi's overnight relays).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub start_min: u16,
    pub stop_min: u16,
    pub days: u8,
}

impl Schedule {
    pub const MONDAY: u8 = 0;
    pub const SUNDAY: u8 = 6;
    pub const DAILY: u8 = 0b0111_1111;

    pub fn day_bit(weekday: u8) -> u8 {
        1 << (weekday % 7)
    }

    /// Is the station on air at `weekday` (0 = Monday) `minute` UTC?
    pub fn on_air(&self, weekday: u8, minute: u16) -> bool {
        let w = weekday % 7;
        let m = minute.min(1440);
        if self.start_min < self.stop_min {
            m >= self.start_min && m < self.stop_min && self.days & Self::day_bit(w) != 0
        } else {
            // Wraps midnight: the tail belongs to the previous day.
            (m >= self.start_min && self.days & Self::day_bit(w) != 0)
                || (m < self.stop_min && self.days & Self::day_bit((w + 6) % 7) != 0)
        }
    }

    /// Transmission length in minutes, wrap included (0000–2400 is a full
    /// day, equal endpoints are empty).
    pub fn duration_min(&self) -> u16 {
        if self.stop_min >= self.start_min {
            self.stop_min - self.start_min
        } else {
            self.stop_min + 1440 - self.start_min
        }
    }

    /// "Mo,Tu,We,Th,Fr" — the days in week order.
    pub fn days_text(&self) -> String {
        const NAMES: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
        (0..7)
            .filter(|d| self.days & (1 << d) != 0)
            .map(|d| NAMES[d as usize])
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Station {
    pub source: Source,
    /// Stable within the source (`Q…`, an EiBi row key, a facility id).
    pub id: String,
    pub name: String,
    pub freq_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth_hz: Option<f64>,
    pub modulation: Modulation,
    pub service: Service,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_kw: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

impl Station {
    pub fn new(source: Source, id: String, name: String, freq_hz: f64) -> Self {
        Self {
            source,
            id,
            name,
            freq_hz,
            bandwidth_hz: None,
            modulation: Modulation::Unknown,
            service: Service::Other,
            lat: None,
            lon: None,
            country: None,
            callsign: None,
            power_kw: None,
            schedule: None,
            notes: String::new(),
        }
    }

    pub fn at(&self) -> Option<crate::geo::LatLon> {
        match (self.lat, self.lon) {
            (Some(lat), Some(lon)) => Some(crate::geo::LatLon { lat, lon }),
            _ => None,
        }
    }

    /// Substring match for the search box, over the fields worth reaching.
    /// `needle` is expected already lowercased (see `Query`).
    pub fn matches_text(&self, needle: &str) -> bool {
        let hit = |s: &str| s.to_lowercase().contains(needle);
        hit(&self.name)
            || hit(&self.id)
            || hit(&self.notes)
            || self.callsign.as_deref().is_some_and(hit)
            || self.country.as_deref().is_some_and(hit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daytime_and_overnight_schedules_are_honest() {
        // Mo-Fr 09:00–17:00.
        let day = Schedule {
            start_min: 9 * 60,
            stop_min: 17 * 60,
            days: 0b0001_1111,
        };
        assert!(day.on_air(0, 12 * 60));
        assert!(day.on_air(0, 9 * 60)); // inclusive start
        assert!(!day.on_air(0, 9 * 60 - 1));
        assert!(!day.on_air(0, 17 * 60)); // exclusive stop
        assert!(!day.on_air(5, 12 * 60)); // Saturday
        assert_eq!(day.duration_min(), 480);
        assert_eq!(day.days_text(), "Mo,Tu,We,Th,Fr");

        // Mondays 23:00–01:00: Tuesday 00:30 is Monday's tail, Monday
        // 00:30 would be Sunday's and Sunday is not in the mask.
        let night = Schedule {
            start_min: 23 * 60,
            stop_min: 60,
            days: Schedule::day_bit(Schedule::MONDAY),
        };
        assert!(night.on_air(0, 23 * 60 + 30));
        assert!(night.on_air(1, 30)); // still Monday's relay
        assert!(!night.on_air(0, 30));
        assert!(!night.on_air(2, 30));
        assert_eq!(night.duration_min(), 120);

        // 0000–2400 is the whole day, and only on its days.
        let all = Schedule {
            start_min: 0,
            stop_min: 1440,
            days: Schedule::day_bit(Schedule::SUNDAY),
        };
        assert!(all.on_air(6, 0));
        assert!(all.on_air(6, 1439));
        assert!(!all.on_air(0, 12 * 60));
    }

    #[test]
    fn modulation_speaks_the_demodulator_vocabulary() {
        assert_eq!(Modulation::Wfm.demod(), Some("wfm"));
        assert_eq!(Modulation::Am.demod(), Some("am"));
        assert_eq!(Modulation::Nfm.demod(), Some("nfm"));
        assert_eq!(Modulation::Usb.demod(), None);
        assert_eq!(Modulation::Dvbt.label(), "DVB-T");
    }
}
