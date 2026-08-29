//! Protocol constants. Source of truth: the vendor Java app's
//! `DeviceAddressTable`/`CommandTable` (decompiled), cross-checked against
//! `vds1022.py`. Where the two disagree, the Java values win (see TRG_TYPE_*).

pub const USB_VID: u16 = 0x5345;
pub const USB_PID: u16 = 0x1234;

/// One acquisition frame on the wire: header + trigger buffer + ADC data.
pub const FRAME_SIZE: usize = 5211;
/// ADC payload per frame: 50 pre + 5000 usable + 50 post samples.
pub const ADC_SIZE: usize = 5100;
/// Usable samples per frame.
pub const SAMPLES: usize = 5000;
/// Offset of the 100-byte trigger buffer within a frame.
pub const TRIGGER_BUF_OFFSET: usize = 11;
/// Offset of the ADC samples within a frame.
pub const ADC_OFFSET: usize = 111;
/// ADC full-scale span in counts across 10 vertical divisions.
pub const ADC_RANGE: f64 = 250.0;
/// Samples at or beyond ±125 are clipped.
pub const ADC_CLIP: i8 = 125;

/// Calibration flash blob size.
pub const FLASH_SIZE: usize = 2002;

/// The frequency-meter and prescaler reference clock.
pub const CLOCK_HZ: f64 = 100e6;

/// Empirical horizontal-trigger-position correction used by the Python API
/// (the Java app does not apply it). Verify per unit.
pub const HTP_ERR: i32 = 11;

/// Registers / opcodes: `(address, byte width)` as `write`-target metadata.
/// Byte-addressed register file: a multi-byte write fills consecutive
/// addresses.
pub mod reg {
    /// Probe: write `b'V'`; reply value 1 = VDS1022, 3 = VDS2052.
    pub const MACHINE_TYPE: u32 = 0x4001;
    /// Reply value 1 = FPGA already loaded.
    pub const QUERY_FPGA: u32 = 0x0223;
    /// Write total bitstream length (u32); reply value = chunk frame size.
    pub const LOAD_FPGA: u32 = 0x4000;
    /// Write 1 (u8); device answers with the 2002-byte flash blob.
    pub const READ_FLASH: u32 = 0x01B0;
    pub const WRITE_FLASH: u32 = 0x01A0;
    /// Write u16 channel-state word; device answers with data frames.
    pub const GET_DATA: u32 = 0x1000;

    pub const GET_TRIGGERED: u32 = 0x01;
    pub const SET_MULTI: u32 = 0x06;
    /// Pass/fail TTL level on the MULTI port (u8, 0 or 1).
    pub const SET_PF: u32 = 0x07;
    pub const SET_PEAKMODE: u32 = 0x09;
    pub const SET_ROLLMODE: u32 = 0x0A;
    pub const SET_CHL_ON: u32 = 0x0B;
    pub const SET_FORCETRG: u32 = 0x0C;
    /// Slope-trigger threshold pair: `(upper & 0xFF) | ((lower & 0xFF) << 8)`.
    pub const SET_SLOPE_THRED_CH1: u32 = 0x10;
    pub const SET_SLOPE_THRED_CH2: u32 = 0x12;
    pub const SET_PHASEFINE: u32 = 0x18;
    pub const SET_TRIGGER: u32 = 0x24;
    pub const SET_TRG_HOLDOFF_CH1: u32 = 0x26;
    pub const SET_TRG_HOLDOFF_CH2: u32 = 0x2A;
    pub const SET_EDGE_LEVEL_CH1: u32 = 0x2E;
    pub const SET_EDGE_LEVEL_CH2: u32 = 0x30;
    /// Video-trigger line number (only meaningful for sync = LineNum).
    pub const SET_VIDEOLINE: u32 = 0x32;
    /// Pulse/slope trigger width, units of 10 ns, split u16/u16 (FPGA >= V3):
    /// `trg_cdt_gl` holds `m & 0xFFFF`, `trg_cdt_hl` holds `m >> 16`.
    pub const SET_TRG_WIDTH_GL_CH1: u32 = 0x42;
    pub const SET_TRG_WIDTH_HL_CH1: u32 = 0x44;
    pub const SET_TRG_WIDTH_GL_CH2: u32 = 0x46;
    pub const SET_TRG_WIDTH_HL_CH2: u32 = 0x48;
    pub const SET_FREQREF_CH1: u32 = 0x4A;
    pub const SET_FREQREF_CH2: u32 = 0x4B;
    pub const SET_TIMEBASE: u32 = 0x52;
    pub const SET_SUF_TRG: u32 = 0x56;
    pub const SET_PRE_TRG: u32 = 0x5A;
    pub const SET_DEEPMEMORY: u32 = 0x5C;
    pub const SET_RUNSTOP: u32 = 0x61;
    pub const GET_DATAFINISHED: u32 = 0x7A;
    pub const SET_ZERO_OFF_CH2: u32 = 0x0108;
    pub const SET_ZERO_OFF_CH1: u32 = 0x010A;
    /// Written (=1) after every horizontal-trigger-position change.
    pub const SET_EMPTY: u32 = 0x010C;
    pub const SET_CHANNEL_CH2: u32 = 0x0110;
    pub const SET_CHANNEL_CH1: u32 = 0x0111;
    pub const SET_VOLT_GAIN_CH2: u32 = 0x0114;
    pub const SET_VOLT_GAIN_CH1: u32 = 0x0116;
}

/// Response status bytes.
pub mod status {
    pub const OK: u8 = b'S';
    /// Busy / no data ready yet.
    pub const EMPTY: u8 = b'E';
    pub const D: u8 = b'D';
    pub const G: u8 = b'G';
    pub const V: u8 = b'V';
}

/// Volts/div ladder, index 0..=9 (millivolts per division).
pub const VOLTBASE_MV: [u32; 10] = [5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000];

/// Full-scale range in volts (10 divisions) per voltbase index.
pub fn full_scale_volts(vb: usize) -> f64 {
    VOLTBASE_MV[vb] as f64 * 10.0 / 1000.0
}

/// Nearest voltbase index for a volts/div request.
pub fn nearest_voltbase(volts_div: f64) -> usize {
    let mut best = 0;
    let mut best_d = f64::MAX;
    for (i, &mv) in VOLTBASE_MV.iter().enumerate() {
        let d = ((mv as f64 / 1000.0).ln() - volts_div.max(1e-6).ln()).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

/// The input attenuation relay engages from this voltbase index upward
/// (500 mV/div, i.e. 5 V full scale). Java: `vb > VB_200mv(5)`.
pub const ATTENUATION_VB: usize = 6;

/// Hardware coupling codes for the channel config byte.
pub fn coupling_code(c: neowon_core::Coupling) -> u8 {
    match c {
        neowon_core::Coupling::Ac => 0,
        neowon_core::Coupling::Dc => 1,
        neowon_core::Coupling::Gnd => 2,
    }
}

/// The distinct sample rates the prescaler ladder produces, S/s.
pub const SAMPLE_RATES: [f64; 24] = [
    2.5, 5.0, 12.5, 25.0, 50.0, 125.0, 250.0, 500.0, 1.25e3, 2.5e3, 5e3, 12.5e3, 25e3, 50e3, 125e3,
    250e3, 500e3, 1.25e6, 2.5e6, 5e6, 12.5e6, 25e6, 50e6, 100e6,
];

/// Prescaler for a requested rate, snapped to the nearest rate the hardware
/// ladder supports; actual rate is `CLOCK_HZ / prescaler`.
pub fn prescaler_for_rate(rate: f64) -> u32 {
    let snapped = SAMPLE_RATES
        .iter()
        .copied()
        .min_by(|a, b| {
            // Compare in log space so 2.5 vs 5 weighs like 50M vs 100M.
            let da = (a.ln() - rate.max(1e-3).ln()).abs();
            let db = (b.ln() - rate.max(1e-3).ln()).abs();
            da.total_cmp(&db)
        })
        .unwrap();
    (CLOCK_HZ / snapped).round().max(1.0) as u32
}

/// Below this sample rate the device must run in roll mode.
pub const ROLLMODE_THRESHOLD: f64 = 2500.0;

/// Hardware trigger-type codes (Java `Tiny_TrgType`; note vds1022.py has
/// Slope and Video swapped — do not copy that).
pub mod trg_type {
    pub const EDGE: u16 = 0;
    pub const SLOPE: u16 = 1;
    pub const VIDEO: u16 = 2;
    pub const PULSE: u16 = 3;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prescaler_ladder() {
        assert_eq!(prescaler_for_rate(100e6), 1);
        assert_eq!(prescaler_for_rate(250e3), 400);
        assert_eq!(prescaler_for_rate(2.5), 40_000_000);
    }

    #[test]
    fn ranges() {
        assert_eq!(full_scale_volts(0), 0.05);
        assert_eq!(full_scale_volts(7), 10.0);
        assert_eq!(full_scale_volts(9), 50.0);
    }
}
