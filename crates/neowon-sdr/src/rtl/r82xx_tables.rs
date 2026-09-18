//! R82xx register tables, transcribed from librtlsdr's `tuner_r82xx.c`
//! (via `librtlsdr-rs`), and the pure arithmetic over them (PLL dividers,
//! gain steps) — everything about the chip that needs no bus.

use super::{Error, Result};

const VCO_MIN_KHZ: u64 = 1_770_000;
const VCO_MAX_KHZ: u64 = 2 * VCO_MIN_KHZ;

/// First register mirrored in the shadow array.
pub const SHADOW_START: u8 = 5;
pub const NUM_REGS: usize = 30;
pub const VER_NUM: u8 = 49;

/// Power-on values of registers 0x05..=0x1f.
pub const INIT_ARRAY: [u8; 27] = [
    0x83, 0x32, 0x75, // 05..07
    0xc0, 0x40, 0xd6, 0x6c, // 08..0b
    0xf5, 0x63, 0x75, 0x68, // 0c..0f
    0x6c, 0x83, 0x80, 0x00, // 10..13
    0x0f, 0x00, 0xc0, 0x30, // 14..17
    0x48, 0xcc, 0x60, 0x00, // 18..1b
    0x54, 0xae, 0x4a, 0xc0, // 1c..1f
];

/// RF front-end settings by LO frequency, from `mhz` up. Upstream's table
/// also carries crystal-cap values for the 20p/10p settings, but librtlsdr
/// always runs the high-cap 0p setting, whose value is 0 in every row.
pub struct FreqRange {
    pub mhz: u32,
    pub open_d: u8,
    pub rf_mux_ploy: u8,
    pub tf_c: u8,
}

const fn fr(mhz: u32, open_d: u8, rf_mux_ploy: u8, tf_c: u8) -> FreqRange {
    FreqRange {
        mhz,
        open_d,
        rf_mux_ploy,
        tf_c,
    }
}

pub const FREQ_RANGES: [FreqRange; 21] = [
    fr(0, 0x08, 0x02, 0xdf),
    fr(50, 0x08, 0x02, 0xbe),
    fr(55, 0x08, 0x02, 0x8b),
    fr(60, 0x08, 0x02, 0x7b),
    fr(65, 0x08, 0x02, 0x69),
    fr(70, 0x08, 0x02, 0x58),
    fr(75, 0x00, 0x02, 0x44),
    fr(80, 0x00, 0x02, 0x44),
    fr(90, 0x00, 0x02, 0x34),
    fr(100, 0x00, 0x02, 0x34),
    fr(110, 0x00, 0x02, 0x24),
    fr(120, 0x00, 0x02, 0x24),
    fr(140, 0x00, 0x02, 0x14),
    fr(180, 0x00, 0x02, 0x13),
    fr(220, 0x00, 0x02, 0x13),
    fr(250, 0x00, 0x02, 0x11),
    fr(280, 0x00, 0x02, 0x00),
    fr(310, 0x00, 0x41, 0x00),
    fr(450, 0x00, 0x41, 0x00),
    fr(588, 0x00, 0x40, 0x00),
    fr(650, 0x00, 0x40, 0x00),
];

/// The range whose start is the last one at or below `mhz`.
pub fn freq_range(mhz: u32) -> &'static FreqRange {
    let i = FREQ_RANGES.partition_point(|r| r.mhz <= mhz);
    &FREQ_RANGES[i.saturating_sub(1)]
}

/// Gain increments (tenths of a dB) per step of the LNA and mixer
/// indices; index 0 is the base.
pub const LNA_GAIN_STEPS: [i32; 16] = [0, 9, 13, 40, 38, 13, 31, 22, 26, 31, 26, 14, 19, 5, 35, 13];
pub const MIXER_GAIN_STEPS: [i32; 16] = [0, 5, 10, 10, 19, 9, 10, 25, 17, 10, 8, 16, 13, 6, 3, -8];

/// IF low-pass corner options, Hz, widest first.
pub const IF_LOW_PASS_BW: [i32; 10] = [
    1_700_000, 1_600_000, 1_550_000, 1_450_000, 1_200_000, 900_000, 700_000, 550_000, 450_000,
    350_000,
];
pub const FILT_HP_BW1: i32 = 350_000;
pub const FILT_HP_BW2: i32 = 380_000;

/// Index of the narrowest low-pass corner still >= `bw`.
pub fn if_lpf_index(bw: i32) -> usize {
    IF_LOW_PASS_BW
        .partition_point(|&v| v >= bw)
        .saturating_sub(1)
}

/// The R82xx returns register reads bit-reversed.
pub fn bitrev(b: u8) -> u8 {
    b.reverse_bits()
}

/// The mixer divider for an LO: the power of two in 2..=64 that puts the
/// VCO in its range, and its register code (log2(div) - 1).
pub fn mix_divider(lo_hz: u32) -> Option<(u32, u8)> {
    let khz = (lo_hz as u64 + 500) / 1000;
    let mut div = 2u32;
    while div <= 64 {
        let vco = khz * div as u64;
        if (VCO_MIN_KHZ..VCO_MAX_KHZ).contains(&vco) {
            return Some((div, div.trailing_zeros() as u8 - 1));
        }
        div <<= 1;
    }
    None
}

/// Integer and sigma-delta parts of the PLL: `f_vco = 2 * xtal * (nint +
/// sdm / 65536)`, with `nint = 13 + 4 * ni + si`.
pub fn pll_n(vco_hz: u64, xtal: u32, vco_power_ref: u8) -> Result<(u8, u8, u16)> {
    let xtal = xtal as u64;
    let vco_div = (xtal + 65536 * vco_hz) / (2 * xtal);
    let nint = (vco_div / 65536) as u32;
    let sdm = (vco_div % 65536) as u16;
    let nint_max = 128 / vco_power_ref as u32 - 1;
    if !(13..=nint_max).contains(&nint) {
        return Err(Error::Pll(format!("nint {nint} outside 13..={nint_max}")));
    }
    let ni = ((nint - 13) / 4) as u8;
    let si = (nint - 13 - 4 * ni as u32) as u8;
    Ok((ni, si, sdm))
}

/// LNA and mixer indices for a requested gain: alternate steps, LNA first,
/// until the running total reaches `tenths_db` (upstream's loop).
pub fn gain_indices(tenths_db: i32) -> (u8, u8) {
    let (mut total, mut lna, mut mix) = (0, 0usize, 0usize);
    for _ in 0..15 {
        if total >= tenths_db {
            break;
        }
        if lna < LNA_GAIN_STEPS.len() - 1 && LNA_GAIN_STEPS[lna + 1] > 0 {
            lna += 1;
            total += LNA_GAIN_STEPS[lna];
        }
        if total >= tenths_db {
            break;
        }
        if mix < MIXER_GAIN_STEPS.len() - 1 && MIXER_GAIN_STEPS[mix + 1] > 0 {
            mix += 1;
            total += MIXER_GAIN_STEPS[mix];
        }
    }
    (lna as u8, mix as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XTAL: u32 = 28_800_000;

    #[test]
    fn divider_keeps_the_vco_in_range() {
        for lo in [
            27_700_000u32,
            56_000_000,
            103_000_000,
            433_920_000,
            1_700_000_000,
        ] {
            let (div, code) = mix_divider(lo).expect("divider");
            let vco_khz = (lo as u64 + 500) / 1000 * div as u64;
            assert!((VCO_MIN_KHZ..VCO_MAX_KHZ).contains(&vco_khz), "lo {lo}");
            assert_eq!(1u32 << (code + 1), div);
        }
        // Below ~27.66 MHz even /64 cannot reach 1.77 GHz: the HF hazard.
        assert!(mix_divider(18_870_000).is_none());
    }

    #[test]
    fn pll_reconstructs_the_vco() {
        for lo in [
            27_700_000u32,
            56_000_000,
            103_570_000,
            433_920_000,
            1_700_000_000,
        ] {
            let (div, _) = mix_divider(lo).unwrap();
            let vco = lo as u64 * div as u64;
            let (ni, si, sdm) = pll_n(vco, XTAL, 2).unwrap();
            let nint = 13 + 4 * ni as u64 + si as u64;
            let back = 2.0 * XTAL as f64 * (nint as f64 + sdm as f64 / 65536.0);
            // Resolution is 2 * xtal / 65536 ~ 879 Hz of VCO.
            assert!(
                (back - vco as f64).abs() <= 2.0 * XTAL as f64 / 65536.0,
                "lo {lo}"
            );
            assert!(si < 4);
        }
    }

    #[test]
    fn gain_indices_track_the_published_table() {
        // Summing the chosen steps reproduces an entry of the gain table.
        for &g in crate::rtl::R82XX_GAINS.iter() {
            let (lna, mix) = gain_indices(g);
            let total: i32 = LNA_GAIN_STEPS[1..=lna as usize].iter().sum::<i32>()
                + MIXER_GAIN_STEPS[1..=mix as usize].iter().sum::<i32>();
            assert!(total >= g, "gain {g}: reached {total}");
            assert!(
                crate::rtl::R82XX_GAINS.contains(&total),
                "gain {g} -> {total}"
            );
        }
    }

    #[test]
    fn ranges_pick_the_band_below() {
        assert_eq!(freq_range(0).tf_c, 0xdf);
        assert_eq!(freq_range(49).tf_c, 0xdf);
        assert_eq!(freq_range(103).tf_c, 0x34);
        assert_eq!(freq_range(5000).rf_mux_ploy, 0x40);
    }

    #[test]
    fn lna_and_mixer_steps_sum_past_the_top_gain() {
        // The R82xx gain loop can reach the top of the published gain table.
        let total: i32 = LNA_GAIN_STEPS.iter().sum::<i32>()
            + MIXER_GAIN_STEPS.iter().filter(|&&s| s > 0).sum::<i32>();
        assert!(total >= 496);
    }

    #[test]
    fn bitrev_matches_the_nibble_table() {
        assert_eq!(bitrev(0x01), 0x80);
        assert_eq!(bitrev(0x69), 0x96);
    }
}
