//! DAB character sets: the complete EBU Latin repertoire (charset 0) and the
//! honest fallbacks for the rest.
//!
//! The EBU Latin table is **ported from `dabradio` 0.5.0 (MIT)**,
//! `src/charsets.rs` (`xoolive/desperado`); the MIT notice is recorded in
//! `docs/protocol-dab.md`. It is the character set clause 5.2.2.2 requires for
//! labels and clause 7.4.5.2's dynamic label segments (EN 300 401 calls it the
//! "Complete EBU Latin based repertoire"; the table's home is ETSI TS 101 756,
//! table 1).
//!
//! Charset 0 decodes byte-for-byte through the table; charset 15 is UTF-8 by
//! the standard's assignment. Any other charset is *not* guessed: printable
//! ASCII passes, everything else becomes `?`, so a wrong letter never reaches
//! a display (the same rule as tier 1's label decoder, D27).

/// The EBU Latin repertoire, 0x00..=0xFF → Unicode. Reserved codes map to
/// U+FFFD; they never carry printable text.
// Ported from dabradio 0.5.0 (MIT); notice in docs/protocol-dab.md
static EBU_LATIN: [char; 256] = [
    // 0x00–0x0F
    '\u{FFFD}', // 0x00 — reserved
    '\u{0118}', // 0x01 Ę
    '\u{012E}', // 0x02 Į
    '\u{0172}', // 0x03 Ų
    '\u{0102}', // 0x04 Ă
    '\u{0116}', // 0x05 Ė
    '\u{010E}', // 0x06 Ď
    '\u{0218}', // 0x07 Ș
    '\u{021A}', // 0x08 Ț
    '\u{010A}', // 0x09 Ċ
    '\u{FFFD}', // 0x0A — preferred line break (DLS control code)
    '\u{FFFD}', // 0x0B — end of headline (DLS control code)
    '\u{0120}', // 0x0C Ġ
    '\u{0139}', // 0x0D Ĺ
    '\u{017B}', // 0x0E Ż
    '\u{0143}', // 0x0F Ń
    // 0x10–0x1F
    '\u{0105}', // 0x10 ą
    '\u{0119}', // 0x11 ę
    '\u{012F}', // 0x12 į
    '\u{0173}', // 0x13 ų
    '\u{0103}', // 0x14 ă
    '\u{0117}', // 0x15 ė
    '\u{010F}', // 0x16 ď
    '\u{0219}', // 0x17 ș
    '\u{021B}', // 0x18 ț
    '\u{010B}', // 0x19 ċ
    '\u{0147}', // 0x1A Ň
    '\u{011A}', // 0x1B Ě
    '\u{0121}', // 0x1C ġ
    '\u{013A}', // 0x1D ĺ
    '\u{017C}', // 0x1E ż
    '\u{FFFD}', // 0x1F — preferred word break (DLS control code)
    // 0x20–0x2F
    ' ',        // 0x20
    '!',        // 0x21
    '"',        // 0x22
    '#',        // 0x23
    '\u{0142}', // 0x24 ł
    '%',        // 0x25
    '&',        // 0x26
    '\'',       // 0x27
    '(',        // 0x28
    ')',        // 0x29
    '*',        // 0x2A
    '+',        // 0x2B
    ',',        // 0x2C
    '-',        // 0x2D
    '.',        // 0x2E
    '/',        // 0x2F
    // 0x30–0x3F
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    // 0x40–0x4F
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    // 0x50–0x5F
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', '\u{016E}', ']', '\u{0141}', '_',
    // 0x60–0x6F
    '\u{0104}', // 0x60 Ą
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    // 0x70–0x7F
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '\u{00AB}', // 0x7B «
    '\u{016F}', // 0x7C ů
    '\u{00BB}', // 0x7D »
    '\u{013D}', // 0x7E Ľ
    '\u{0126}', // 0x7F Ħ
    // 0x80–0x8F
    '\u{00E1}', // 0x80 á
    '\u{00E0}', // 0x81 à
    '\u{00E9}', // 0x82 é
    '\u{00E8}', // 0x83 è
    '\u{00ED}', // 0x84 í
    '\u{00EC}', // 0x85 ì
    '\u{00F3}', // 0x86 ó
    '\u{00F2}', // 0x87 ò
    '\u{00FA}', // 0x88 ú
    '\u{00F9}', // 0x89 ù
    '\u{00D1}', // 0x8A Ñ
    '\u{00C7}', // 0x8B Ç
    '\u{015E}', // 0x8C Ş
    '\u{00DF}', // 0x8D ß
    '\u{00A1}', // 0x8E ¡
    '\u{0178}', // 0x8F Ÿ
    // 0x90–0x9F
    '\u{00E2}', // 0x90 â
    '\u{00E4}', // 0x91 ä
    '\u{00EA}', // 0x92 ê
    '\u{00EB}', // 0x93 ë
    '\u{00EE}', // 0x94 î
    '\u{00EF}', // 0x95 ï
    '\u{00F4}', // 0x96 ô
    '\u{00F6}', // 0x97 ö
    '\u{00FB}', // 0x98 û
    '\u{00FC}', // 0x99 ü
    '\u{00F1}', // 0x9A ñ
    '\u{00E7}', // 0x9B ç
    '\u{015F}', // 0x9C ş
    '\u{011F}', // 0x9D ğ
    '\u{0131}', // 0x9E ı
    '\u{00FF}', // 0x9F ÿ
    // 0xA0–0xAF
    '\u{0136}', // 0xA0 Ķ
    '\u{0145}', // 0xA1 Ņ
    '\u{00A9}', // 0xA2 ©
    '\u{0122}', // 0xA3 Ģ
    '\u{011E}', // 0xA4 Ğ
    '\u{011B}', // 0xA5 ě
    '\u{0148}', // 0xA6 ň
    '\u{0151}', // 0xA7 ő
    '\u{0150}', // 0xA8 Ő
    '\u{20AC}', // 0xA9 €
    '\u{00A3}', // 0xAA £
    '$',        // 0xAB
    '\u{0100}', // 0xAC Ā
    '\u{0112}', // 0xAD Ē
    '\u{012A}', // 0xAE Ī
    '\u{016A}', // 0xAF Ū
    // 0xB0–0xBF
    '\u{0137}', // 0xB0 ķ
    '\u{0146}', // 0xB1 ņ
    '\u{013B}', // 0xB2 Ļ
    '\u{0123}', // 0xB3 ģ
    '\u{013C}', // 0xB4 ļ
    '\u{0130}', // 0xB5 İ
    '\u{0144}', // 0xB6 ń
    '\u{0171}', // 0xB7 ű
    '\u{0170}', // 0xB8 Ű
    '\u{00BF}', // 0xB9 ¿
    '\u{013E}', // 0xBA ľ
    '\u{00B0}', // 0xBB °
    '\u{0101}', // 0xBC ā
    '\u{0113}', // 0xBD ē
    '\u{012B}', // 0xBE ī
    '\u{016B}', // 0xBF ū
    // 0xC0–0xCF
    '\u{00C1}', '\u{00C0}', '\u{00C9}', '\u{00C8}', '\u{00CD}', '\u{00CC}', '\u{00D3}', '\u{00D2}',
    '\u{00DA}', '\u{00D9}', '\u{0158}', '\u{010C}', '\u{0160}', '\u{017D}', '\u{00D0}', '\u{013F}',
    // 0xD0–0xDF
    '\u{00C2}', '\u{00C4}', '\u{00CA}', '\u{00CB}', '\u{00CE}', '\u{00CF}', '\u{00D4}', '\u{00D6}',
    '\u{00DB}', '\u{00DC}', '\u{0159}', '\u{010D}', '\u{0161}', '\u{017E}', '\u{0111}', '\u{0140}',
    // 0xE0–0xEF
    '\u{00C3}', '\u{00C5}', '\u{00C6}', '\u{0152}', '\u{0177}', '\u{00DD}', '\u{00D5}', '\u{00D8}',
    '\u{00DE}', '\u{014A}', '\u{0154}', '\u{0106}', '\u{015A}', '\u{0179}', '\u{0164}', '\u{00F0}',
    // 0xF0–0xFF
    '\u{00E3}', '\u{00E5}', '\u{00E6}', '\u{0153}', '\u{0175}', '\u{00FD}', '\u{00F5}', '\u{00F8}',
    '\u{00FE}', '\u{014B}', '\u{0155}', '\u{0107}', '\u{015B}', '\u{017A}', '\u{0165}', '\u{0127}',
];

/// Decode EBU Latin (charset 0) bytes into a UTF-8 string. Reserved code points
/// become U+FFFD rather than a letter.
pub fn ebu_latin_to_utf8(bytes: &[u8]) -> String {
    bytes.iter().map(|b| EBU_LATIN[*b as usize]).collect()
}

/// Decode a DAB character field in the given charset.
///
/// Charset 0 is the EBU Latin table, charset 15 is UTF-8. The other fourteen
/// assignments (TS 101 756) are not transcribed, so their bytes are **not**
/// guessed: printable ASCII passes through and anything else becomes `?`.
pub fn decode(bytes: &[u8], charset: u8) -> String {
    match charset {
        0 => ebu_latin_to_utf8(bytes),
        15 => String::from_utf8_lossy(bytes).into_owned(),
        _ => bytes
            .iter()
            .map(|b| match *b {
                0x20..=0x7E => *b as char,
                _ => '?',
            })
            .collect(),
    }
}

/// Decode a DLS character field (charset 0 only) where the three presentation
/// control codes are honoured: 0x0A becomes a newline, 0x0B (end of headline)
/// and 0x1F (preferred word break) are formatting markers, not text, and are
/// dropped. For any other charset this is [`decode`].
pub fn decode_dls(bytes: &[u8], charset: u8) -> String {
    if charset != 0 {
        return decode(bytes, charset);
    }
    let mut out = String::with_capacity(bytes.len());
    for byte in bytes {
        match *byte {
            0x0A => out.push('\n'),
            0x0B | 0x1F => {}
            other => out.push(EBU_LATIN[other as usize]),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spot check of the ported table on both sides of ASCII: the accented
    /// letters a French or Dutch DLS uses, and the EBU-specific code points
    /// that distinguish it from Latin-1.
    #[test]
    fn ebu_latin_maps_the_published_code_points() {
        assert_eq!(ebu_latin_to_utf8(b"Hello World"), "Hello World");
        assert_eq!(ebu_latin_to_utf8(&[b'C', b'a', b'f', 0x82]), "Café");
        assert_eq!(ebu_latin_to_utf8(&[0x80, 0x81, 0x82, 0x83]), "áàéè");
        assert_eq!(ebu_latin_to_utf8(&[0xC2, 0xC3, 0xD2]), "ÉÈÊ");
        assert_eq!(ebu_latin_to_utf8(&[0x8A, 0x9A]), "Ññ");
        assert_eq!(ebu_latin_to_utf8(&[0x24]), "ł", "0x24 is ł, not $");
        assert_eq!(ebu_latin_to_utf8(&[0xAB, 0xA9, 0xAA]), "$€£");
        assert_eq!(ebu_latin_to_utf8(&[0x7B, 0x7D]), "«»");
        assert_eq!(ebu_latin_to_utf8(&[0x5C, 0x5E, 0x60]), "ŮŁĄ");
    }

    /// The three DLS control codes are not letters; they decode to the
    /// replacement character at this layer (the DLS parser handles them).
    #[test]
    fn dls_control_codes_are_not_letters() {
        assert_eq!(ebu_latin_to_utf8(&[0x0A]), "\u{FFFD}");
        assert_eq!(ebu_latin_to_utf8(&[0x0B]), "\u{FFFD}");
        assert_eq!(ebu_latin_to_utf8(&[0x1F]), "\u{FFFD}");
    }

    /// UTF-8 is exact (charset 15), and an unknown charset never invents a
    /// letter: ASCII passes, the rest is `?`.
    #[test]
    fn unknown_charsets_do_not_misletter() {
        assert_eq!(decode("Café".as_bytes(), 15), "Café");
        assert_eq!(decode(&[b'A', 0xE9, b'B'], 3), "A?B");
    }
}
