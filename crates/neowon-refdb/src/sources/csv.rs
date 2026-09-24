//! A hand-written delimiter-separated-values reader (no `csv` crate).
//! RFC 4180-ish: quotes protect separators, newlines and doubled quotes at
//! the start of a field; CR is ignored; blank lines vanish.

/// Whether `;`, tab or `,` separates the fields of a header line — the
/// FMLIST export's delimiter is not fixed.
pub(crate) fn detect_delimiter(header: &str) -> u8 {
    (*b",;\t")
        .into_iter()
        .max_by_key(|d| header.matches(*d as char).count())
        .unwrap_or(b',')
}

pub(crate) fn parse(text: &str, delim: u8) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.is_empty() {
            quoted = true;
        } else if c == delim as char {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' {
            if !(field.is_empty() && row.is_empty()) {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
        } else if c != '\r' {
            field.push(c);
        }
    }
    if !(field.is_empty() && row.is_empty()) {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// The column whose header matches one of `names`, exactly first then as a
/// substring (so "Frequency (MHz)" finds "frequency" and "Latitude (deg)"
/// finds "lat").
pub(crate) fn find(header: &[String], names: &[&str]) -> Option<usize> {
    let norm = |s: &str| s.trim().to_ascii_lowercase();
    let exact = header
        .iter()
        .position(|h| names.iter().any(|n| norm(h) == norm(n)));
    exact.or_else(|| {
        header.iter().position(|h| {
            names
                .iter()
                .any(|n| !n.is_empty() && norm(h).contains(&norm(n)))
        })
    })
}

pub(crate) fn cell(row: &[String], i: Option<usize>) -> Option<&str> {
    let s = row.get(i?)?.trim();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_newlines_and_blanks() {
        let rows = parse("a,b\n\"x, y\",\"say \"\"hi\"\"\"\n\nlast,line", b',');
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], ["x, y", "say \"hi\""]);
        assert_eq!(rows[2], ["last", "line"]);
        let rows = parse("a;b\n1;\"two\nlines\"", b';');
        assert_eq!(rows[1], ["1", "two\nlines"]);
    }

    #[test]
    fn headers_match_by_name() {
        let h: Vec<String> = ["Call Sign", "Freq (MHz)", "State"]
            .map(String::from)
            .to_vec();
        assert_eq!(find(&h, &["frequency", "freq"]), Some(1));
        assert_eq!(find(&h, &["callsign", "call sign"]), Some(0));
        assert_eq!(find(&h, &["latitude", "lat"]), None);
        assert_eq!(detect_delimiter("a;b;c"), b';');
        assert_eq!(detect_delimiter("a,b,c"), b',');
        assert_eq!(detect_delimiter("a\tb\tc"), b'\t');
    }
}
