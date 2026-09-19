//! One importer per station source (D17). Every importer is pure — bytes
//! in, stations plus a report out — so the whole of 10.14.2 is testable
//! against small fixtures and never touches the network. Fetching the
//! bytes is `crate::fetch`'s job (10.14.3).

pub mod eibi;
pub mod fcc;
pub mod fmlist;
pub mod ourairports;
pub mod wikidata;

pub(crate) mod csv;

/// What an import saw: rows read, stations produced, and why the rest were
/// dropped. The report reaches the UI status line and `get refdb`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub rows: usize,
    pub kept: usize,
    /// 1-based row (or line) number and the reason.
    pub skipped: Vec<(usize, String)>,
}

impl Report {
    pub fn skip(&mut self, row: usize, reason: impl Into<String>) {
        self.skipped.push((row, reason.into()));
    }

    /// "412 rows, 409 stations, 3 skipped (row 7: bad frequency; …)".
    pub fn summary(&self) -> String {
        let mut s = format!("{} rows, {} stations", self.rows, self.kept);
        if !self.skipped.is_empty() {
            s.push_str(&format!(", {} skipped", self.skipped.len()));
            let first: Vec<String> = self
                .skipped
                .iter()
                .take(3)
                .map(|(row, why)| format!("row {row}: {why}"))
                .collect();
            s.push_str(&format!(" ({})", first.join("; ")));
        }
        s
    }
}

/// Latin-1 bytes (EiBi's files) to text; every byte is a code point, which
/// is exactly Latin-1 and cannot fail.
pub(crate) fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// FNV-1a, for a station id derived from the row's own text: stable across
/// re-imports, unlike a file position.
pub(crate) fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Does the row's free-text field mention an operating mode? Whole-word-ish
/// uppercase match ("USB" in a remark, not in "busy").
pub(crate) fn remark_mode(remarks: &str) -> Option<crate::Modulation> {
    let upper = remarks.to_ascii_uppercase();
    let has = |w: &str| {
        upper
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|t| t == w)
    };
    if has("DRM") {
        Some(crate::Modulation::Digital)
    } else if has("USB") {
        Some(crate::Modulation::Usb)
    } else if has("LSB") {
        Some(crate::Modulation::Lsb)
    } else if has("CW") {
        Some(crate::Modulation::Cw)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_decodes_accented_bytes() {
        assert_eq!(decode_latin1(&[0x52, 0xE1, 0x64, 0x69, 0x6F]), "Rádio");
    }

    #[test]
    fn ids_are_stable_and_row_specific() {
        assert_eq!(fnv1a("a"), fnv1a("a"));
        assert_ne!(fnv1a("a"), fnv1a("b"));
    }

    #[test]
    fn remarks_name_a_mode_without_false_positives() {
        assert_eq!(remark_mode("DRM test"), Some(crate::Modulation::Digital));
        assert_eq!(remark_mode("USB"), Some(crate::Modulation::Usb));
        assert_eq!(remark_mode("busy schedule"), None);
        assert_eq!(remark_mode(""), None);
    }

    #[test]
    fn report_summarises_without_dumping_every_row() {
        let mut r = Report {
            rows: 10,
            kept: 7,
            skipped: Vec::new(),
        };
        for i in 1..=4 {
            r.skip(i, "no frequency");
        }
        assert_eq!(
            r.summary(),
            "10 rows, 7 stations, 4 skipped (row 1: no frequency; row 2: no frequency; row 3: no frequency)"
        );
    }
}
