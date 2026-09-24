//! The Band III DAB channel catalogue: the block labels and centre
//! frequencies the DAB dock's channel selector and `sdr dab channel` use to
//! land the hardware window on an ensemble without the operator knowing the
//! frequency by heart.
//!
//! **This is a table, not a formula.** The nominal raster step is 1.712 MHz,
//! but Band III's real gaps are not uniform (1.568 MHz above 13C, 1.872 MHz
//! below 6A and at several group boundaries), so a generated raster produces
//! wrong centres. The 38 entries are transcribed from the MIT reference
//! `dabradio` 0.5.0 (`constants.rs`; notice in `docs/protocol-dab.md`), and
//! the four centres our own hardware run and research recorded — 7A
//! 188.928, 8A 195.936, 11C 220.352, 12C 227.360 MHz — match it.
//!
//! The block letters are administrative (the Wiesbaden arrangement); EN 300
//! 401 defines the modulation, not the channel plan.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DabBlock {
    pub label: &'static str,
    pub centre_hz: f64,
}

/// Nominal raster spacing, Hz. Used only as the "is this centre on a block"
/// tolerance — the table's actual gaps vary.
pub const BLOCK_SPACING_HZ: f64 = 1_712_000.0;

/// `(label, centre Hz)`, in raster order — 5A to 13F.
const BAND_III: &[(&str, f64)] = &[
    ("5A", 174_928_000.0),
    ("5B", 176_640_000.0),
    ("5C", 178_352_000.0),
    ("5D", 180_064_000.0),
    ("6A", 181_936_000.0),
    ("6B", 183_648_000.0),
    ("6C", 185_360_000.0),
    ("6D", 187_072_000.0),
    ("7A", 188_928_000.0),
    ("7B", 190_640_000.0),
    ("7C", 192_352_000.0),
    ("7D", 194_064_000.0),
    ("8A", 195_936_000.0),
    ("8B", 197_648_000.0),
    ("8C", 199_360_000.0),
    ("8D", 201_072_000.0),
    ("9A", 202_928_000.0),
    ("9B", 204_640_000.0),
    ("9C", 206_352_000.0),
    ("9D", 208_064_000.0),
    ("10A", 209_936_000.0),
    ("10B", 211_648_000.0),
    ("10C", 213_360_000.0),
    ("10D", 215_072_000.0),
    ("11A", 216_928_000.0),
    ("11B", 218_640_000.0),
    ("11C", 220_352_000.0),
    ("11D", 222_064_000.0),
    ("12A", 223_936_000.0),
    ("12B", 225_648_000.0),
    ("12C", 227_360_000.0),
    ("12D", 229_072_000.0),
    ("13A", 230_784_000.0),
    ("13B", 232_496_000.0),
    ("13C", 234_208_000.0),
    ("13D", 235_776_000.0),
    ("13E", 237_488_000.0),
    ("13F", 239_200_000.0),
];

pub fn band_iii_blocks() -> impl Iterator<Item = DabBlock> + 'static {
    BAND_III
        .iter()
        .map(|&(label, centre_hz)| DabBlock { label, centre_hz })
}

/// The block a label names (`11C`, case-insensitive), if the raster has it.
pub fn band_iii_block(label: &str) -> Option<DabBlock> {
    let want = label.trim().to_ascii_uppercase();
    BAND_III
        .iter()
        .find(|(name, _)| *name == want)
        .map(|&(label, centre_hz)| DabBlock { label, centre_hz })
}

/// The block whose centre `hz` is within half a nominal raster step of, if
/// any. The wider real gaps leave a small "between blocks" band where this
/// says nothing rather than naming the wrong one.
pub fn band_iii_block_at(hz: f64) -> Option<DabBlock> {
    let half = BLOCK_SPACING_HZ / 2.0;
    BAND_III
        .iter()
        .map(|&(label, centre_hz)| DabBlock { label, centre_hz })
        .min_by(|a, b| {
            (a.centre_hz - hz)
                .abs()
                .total_cmp(&(b.centre_hz - hz).abs())
        })
        .filter(|b| (b.centre_hz - hz).abs() <= half)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_matches_the_recorded_centres() {
        let blocks: Vec<DabBlock> = band_iii_blocks().collect();
        assert_eq!(blocks.len(), 38);
        assert_eq!(blocks[0].label, "5A");
        assert_eq!(blocks[0].centre_hz, 174_928_000.0);
        assert_eq!(blocks[37].label, "13F");
        assert_eq!(blocks[37].centre_hz, 239_200_000.0);
        // docs/protocol-dab.md §Band III targets: verified on air (11C) and
        // cross-checked (12C), and the two labels a station listing shifts.
        assert_eq!(band_iii_block("11C").unwrap().centre_hz, 220_352_000.0);
        assert_eq!(band_iii_block("12C").unwrap().centre_hz, 227_360_000.0);
        assert_eq!(band_iii_block("8A").unwrap().centre_hz, 195_936_000.0);
        assert_eq!(band_iii_block("7A").unwrap().centre_hz, 188_928_000.0);
        // The raster is not a uniform formula: adjacent steps vary.
        let step = |a: &DabBlock, b: &DabBlock| b.centre_hz - a.centre_hz;
        assert_eq!(step(&blocks[3], &blocks[4]), 1_872_000.0); // 5D -> 6A
        assert_eq!(step(&blocks[34], &blocks[35]), 1_568_000.0); // 13C -> 13D
    }

    #[test]
    fn label_lookup_is_trimmed_case_insensitive_and_bounded() {
        assert_eq!(band_iii_block("  11c ").unwrap().label, "11C");
        assert!(band_iii_block("11X").is_none());
        assert!(band_iii_block("10N").is_none());
        assert!(band_iii_block("").is_none());
    }

    #[test]
    fn a_centre_names_its_block_only_when_it_is_close_to_one() {
        assert_eq!(band_iii_block_at(220_352_000.0).unwrap().label, "11C");
        assert_eq!(
            band_iii_block_at(220_352_000.0 + 100_000.0).unwrap().label,
            "11C"
        );
        assert_eq!(
            band_iii_block_at(220_352_000.0 - 800_000.0).unwrap().label,
            "11C"
        );
        // The 10D (215.072) to 11A (216.928) gap is 1.856 MHz: the midpoint
        // is outside half a nominal step of both, and this says so.
        assert!(band_iii_block_at(216_000_000.0).is_none());
        assert!(band_iii_block_at(100_000_000.0).is_none());
        assert!(band_iii_block_at(250_000_000.0).is_none());
    }
}
