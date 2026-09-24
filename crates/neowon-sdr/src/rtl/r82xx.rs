//! Rafael Micro R820T / R828D tuner, ported from librtlsdr's
//! `tuner_r82xx.c` via `librtlsdr-rs` (`tuner/r82xx`). Register writes go
//! through a shadow copy so unchanged registers are not re-sent, exactly as
//! upstream. The Blog V4 upconverter paths are not ported.
//!
//! Every call expects the demodulator's I2C repeater to be on; the device
//! layer brackets tuner calls with it.

use super::r82xx_tables::*;
use super::usb::I2c;
use super::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    R820T,
    R828D,
}

impl Chip {
    pub fn i2c_addr(self) -> u8 {
        match self {
            Chip::R820T => 0x34,
            Chip::R828D => 0x74,
        }
    }

    /// Reference VCO fine-tune code; the divider is nudged towards it.
    fn vco_power_ref(self) -> u8 {
        match self {
            Chip::R820T => 2,
            Chip::R828D => 1,
        }
    }
}

/// Register 0 reads this on both chips (unreversed).
pub const CHECK_VAL: u8 = 0x69;
/// Largest I2C message the RTL2832 bridge forwards, register byte included.
const MAX_I2C_MSG: usize = 8;
/// IF of the default ("TV standard") setup; `set_bw` may change it.
pub const DEFAULT_IF_HZ: u32 = 3_570_000;

pub struct R82xx {
    chip: Chip,
    addr: u8,
    xtal: u32,
    regs: [u8; NUM_REGS],
    /// Current IF, Hz; the demodulator's IF must follow it.
    if_hz: u32,
    fil_cal_code: u8,
    input: u8,
    init_done: bool,
}

fn mask(reg: u8, val: u8, bits: u8) -> u8 {
    (reg & !bits) | (val & bits)
}

impl R82xx {
    pub fn new(chip: Chip, xtal: u32) -> Self {
        Self {
            chip,
            addr: chip.i2c_addr(),
            xtal,
            regs: [0; NUM_REGS],
            if_hz: DEFAULT_IF_HZ,
            fil_cal_code: 0,
            input: 0,
            init_done: false,
        }
    }

    pub fn chip(&self) -> Chip {
        self.chip
    }

    pub fn if_hz(&self) -> u32 {
        self.if_hz
    }

    /// Crystal after ppm correction; takes effect at the next tune.
    pub fn set_xtal(&mut self, xtal: u32) {
        self.xtal = xtal;
    }

    // ---- register I/O through the shadow ----

    fn shadow_index(reg: u8) -> Option<usize> {
        (reg as usize)
            .checked_sub(SHADOW_START as usize)
            .filter(|&i| i < NUM_REGS)
    }

    fn write(&mut self, bus: &impl I2c, reg: u8, vals: &[u8]) -> Result<()> {
        let start = Self::shadow_index(reg);
        if let Some(i) = start
            && i + vals.len() <= NUM_REGS
            && self.regs[i..i + vals.len()] == *vals
        {
            return Ok(());
        }
        let mut msg = Vec::with_capacity(MAX_I2C_MSG);
        for (k, chunk) in vals.chunks(MAX_I2C_MSG - 1).enumerate() {
            msg.clear();
            msg.push(reg + (k * (MAX_I2C_MSG - 1)) as u8);
            msg.extend_from_slice(chunk);
            bus.i2c_write(self.addr, &msg)?;
        }
        if let Some(i) = start {
            let n = vals.len().min(NUM_REGS - i);
            self.regs[i..i + n].copy_from_slice(&vals[..n]);
        }
        Ok(())
    }

    fn write_reg(&mut self, bus: &impl I2c, reg: u8, val: u8) -> Result<()> {
        self.write(bus, reg, &[val])
    }

    fn write_mask(&mut self, bus: &impl I2c, reg: u8, val: u8, bits: u8) -> Result<()> {
        let i = Self::shadow_index(reg)
            .ok_or_else(|| Error::I2c(format!("register {reg:#04x} is not shadowed")))?;
        self.write(bus, reg, &[mask(self.regs[i], val, bits)])
    }

    /// Read `n` status registers from 0; the chip returns them bit-reversed.
    fn read(&self, bus: &impl I2c, n: usize) -> Result<Vec<u8>> {
        bus.i2c_write(self.addr, &[0x00])?;
        Ok(bus
            .i2c_read(self.addr, n)?
            .into_iter()
            .map(bitrev)
            .collect())
    }

    // ---- bring-up ----

    pub fn init(&mut self, bus: &impl I2c) -> Result<()> {
        self.regs = [0; NUM_REGS];
        self.write(bus, 0x05, &INIT_ARRAY)?;
        self.set_tv_standard(bus)?;
        self.sysfreq_sel(bus)?;
        self.init_done = true;
        Ok(())
    }

    /// librtlsdr's "TV standard" setup: IF 3.57 MHz and the IF filter
    /// calibration, retried once if the code comes back 0 or 0xf.
    fn set_tv_standard(&mut self, bus: &impl I2c) -> Result<()> {
        const HP_COR: u8 = 0x6b;
        const FILT_CAL_LO_HZ: u32 = 56_000_000;
        self.regs[..INIT_ARRAY.len()].copy_from_slice(&INIT_ARRAY);
        self.write_mask(bus, 0x0c, 0x00, 0x0f)?;
        self.write_mask(bus, 0x13, VER_NUM, 0x3f)?;
        self.write_mask(bus, 0x1d, 0x00, 0x38)?;
        self.if_hz = DEFAULT_IF_HZ;
        for _ in 0..2 {
            self.write_mask(bus, 0x0b, HP_COR, 0x60)?;
            self.write_mask(bus, 0x0f, 0x04, 0x04)?;
            self.write_mask(bus, 0x10, 0x00, 0x03)?;
            self.set_pll(bus, FILT_CAL_LO_HZ)
                .map_err(|e| Error::Pll(format!("filter calibration: {e}")))?;
            // Start, then stop, the calibration.
            self.write_mask(bus, 0x0b, 0x10, 0x10)?;
            self.write_mask(bus, 0x0b, 0x00, 0x10)?;
            self.write_mask(bus, 0x0f, 0x00, 0x04)?;
            self.fil_cal_code = self.read(bus, 5)?[4] & 0x0f;
            if self.fil_cal_code != 0 && self.fil_cal_code != 0x0f {
                break;
            }
        }
        if self.fil_cal_code == 0x0f {
            self.fil_cal_code = 0;
        }
        self.write_mask(bus, 0x0a, 0x10 | self.fil_cal_code, 0x1f)?;
        self.write_mask(bus, 0x0b, HP_COR, 0xef)?;
        self.write_mask(bus, 0x07, 0x00, 0x80)?; // image rejection off
        self.write_mask(bus, 0x06, 0x10, 0x30)?; // filter gain
        self.write_mask(bus, 0x1e, 0x60, 0x60)?; // ext enable
        self.write_mask(bus, 0x05, 0x01, 0x80)?; // loop through
        self.write_mask(bus, 0x1f, 0x00, 0x80)?; // loop-through attenuation
        self.write_mask(bus, 0x0f, 0x00, 0x80)?; // filter extension widest off
        self.write_mask(bus, 0x19, 0x60, 0x60)?; // polyphase filter current
        Ok(())
    }

    /// librtlsdr's `r82xx_sysfreq_sel` for SDR use (no predetect).
    fn sysfreq_sel(&mut self, bus: &impl I2c) -> Result<()> {
        const MIXER_TOP: u8 = 0x24;
        const LNA_TOP: u8 = 0xe5;
        self.write_mask(bus, 0x1d, LNA_TOP, 0xc7)?;
        self.write_mask(bus, 0x1c, MIXER_TOP, 0xf8)?;
        self.write_reg(bus, 0x0d, 0x53)?; // LNA VTH/VTL
        self.write_reg(bus, 0x0e, 0x75)?; // mixer VTH/VTL
        self.input = 0x00;
        self.write_mask(bus, 0x05, 0x00, 0x60)?; // air/cable-1 input
        self.write_mask(bus, 0x06, 0x00, 0x08)?; // cable-2 input
        self.write_mask(bus, 0x11, 0x38, 0x38)?; // charge-pump current
        self.write_mask(bus, 0x17, 0x30, 0x30)?; // divider buffer current
        self.write_mask(bus, 0x0a, 0x40, 0x60)?; // filter current
        // LNA TOP: detect 1 low, then discharge.
        self.write_mask(bus, 0x1d, 0x00, 0x38)?;
        self.write_mask(bus, 0x1c, 0x00, 0x04)?;
        self.write_mask(bus, 0x06, 0x00, 0x40)?;
        self.write_mask(bus, 0x1a, 0x30, 0x30)?;
        self.write_mask(bus, 0x1d, 0x18, 0x38)?;
        self.write_mask(bus, 0x1c, MIXER_TOP, 0x04)?;
        self.write_mask(bus, 0x1e, 14, 0x1f)?; // LNA discharge
        self.write_mask(bus, 0x1a, 0x20, 0x30)
    }

    // ---- tuning ----

    fn set_mux(&mut self, bus: &impl I2c, lo_hz: u32) -> Result<()> {
        let r = freq_range(lo_hz / 1_000_000);
        self.write_mask(bus, 0x17, r.open_d, 0x08)?;
        self.write_mask(bus, 0x1a, r.rf_mux_ploy, 0xc3)?;
        self.write_reg(bus, 0x1b, r.tf_c)?;
        // Crystal cap: the high-cap 0p setting librtlsdr uses (value 0).
        self.write_mask(bus, 0x10, 0x00, 0x0b)?;
        self.write_mask(bus, 0x08, 0x00, 0x3f)?;
        self.write_mask(bus, 0x09, 0x00, 0x3f)
    }

    fn set_pll(&mut self, bus: &impl I2c, lo_hz: u32) -> Result<()> {
        if self.xtal == 0 {
            return Err(Error::Pll("crystal frequency is zero".into()));
        }
        // PLL auto-tune clock 128 kHz.
        self.write_mask(bus, 0x1a, 0x00, 0x0c)?;
        let s = (0x10 - SHADOW_START) as usize;
        let mut regs: [u8; 7] = self.regs[s..s + 7].try_into().expect("7 registers");
        regs[0] = mask(regs[0], 0x00, 0x10); // refdiv2 off
        regs[2] = mask(regs[2], 0x80, 0xe0); // VCO current
        let (mix_div, mut div_num) = mix_divider(lo_hz)
            .ok_or_else(|| Error::Pll(format!("no VCO divider for LO {lo_hz} Hz")))?;
        let fine = (self.read(bus, 5)?[4] & 0x30) >> 4;
        let pref = self.chip.vco_power_ref();
        if fine > pref {
            div_num = div_num.saturating_sub(1);
        } else if fine < pref {
            div_num += 1;
        }
        regs[0] = mask(regs[0], div_num << 5, 0xe0);
        let (ni, si, sdm) = pll_n(
            lo_hz as u64 * mix_div as u64,
            self.xtal,
            self.chip.vco_power_ref(),
        )
        .map_err(|e| Error::Pll(format!("LO {lo_hz} Hz: {e}")))?;
        tracing::trace!(
            lo_hz,
            mix_div,
            div_num,
            ni,
            si,
            sdm,
            xtal = self.xtal,
            "R82xx PLL"
        );
        regs[4] = ni + (si << 6);
        regs[2] = mask(regs[2], if sdm == 0 { 0x08 } else { 0x00 }, 0x08);
        regs[5] = sdm as u8;
        regs[6] = (sdm >> 8) as u8;
        self.write(bus, 0x10, &regs)?;
        // Lock check; on the first miss raise the VCO current and retry.
        for attempt in 0..2 {
            if self.read(bus, 3)?[2] & 0x40 != 0 {
                return self.write_mask(bus, 0x1a, 0x08, 0x08); // auto-tune 8 kHz
            }
            if attempt == 0 {
                self.write_mask(bus, 0x12, 0x60, 0xe0)?;
            }
        }
        Err(Error::Pll(format!("not locked at LO {lo_hz} Hz")))
    }

    /// Tune the RF centre to `freq_hz` (LO = centre + IF).
    pub fn set_freq(&mut self, bus: &impl I2c, freq_hz: u32) -> Result<()> {
        let lo = freq_hz.saturating_add(self.if_hz);
        self.set_mux(bus, lo)?;
        self.set_pll(bus, lo)?;
        if self.chip == Chip::R828D {
            let input = if freq_hz > 345_000_000 { 0x00 } else { 0x60 };
            if input != self.input {
                self.input = input;
                self.write_mask(bus, 0x05, input, 0x60)?;
            }
        }
        Ok(())
    }

    /// Set the IF filter for `bw` Hz; returns the resulting IF, which the
    /// demodulator must be told.
    pub fn set_bw(&mut self, bus: &impl I2c, bw: u32) -> Result<u32> {
        let bw = bw.min(i32::MAX as u32) as i32;
        let (reg_0a, reg_0b, if_hz) = if bw > 7_000_000 {
            (0x10, 0x0b, 4_570_000)
        } else if bw > 6_000_000 {
            (0x10, 0x2a, 4_570_000)
        } else if bw > IF_LOW_PASS_BW[0] + FILT_HP_BW1 + FILT_HP_BW2 {
            (0x10, 0x6b, 3_570_000)
        } else {
            let mut reg_0b = 0x80u8;
            let mut if_hz = 2_300_000i32;
            let mut real_bw = 0;
            let mut bw = bw;
            if bw > IF_LOW_PASS_BW[0] + FILT_HP_BW1 {
                bw -= FILT_HP_BW2;
                if_hz += FILT_HP_BW2;
                real_bw += FILT_HP_BW2;
            } else {
                reg_0b |= 0x20;
            }
            if bw > IF_LOW_PASS_BW[0] {
                bw -= FILT_HP_BW1;
                if_hz += FILT_HP_BW1;
                real_bw += FILT_HP_BW1;
            } else {
                reg_0b |= 0x40;
            }
            let i = if_lpf_index(bw);
            reg_0b |= 15 - i as u8;
            real_bw += IF_LOW_PASS_BW[i];
            if_hz -= real_bw / 2;
            (0x00, reg_0b, if_hz as u32)
        };
        self.if_hz = if_hz;
        self.write_mask(bus, 0x0a, reg_0a, 0x10)?;
        self.write_mask(bus, 0x0b, reg_0b, 0xef)?;
        Ok(if_hz)
    }

    /// Manual gain: step LNA and mixer alternately until `tenths_db` is
    /// reached; the VGA stays fixed at 16.3 dB.
    pub fn set_gain(&mut self, bus: &impl I2c, tenths_db: i32) -> Result<()> {
        self.write_mask(bus, 0x05, 0x10, 0x10)?; // LNA manual
        self.write_mask(bus, 0x07, 0x00, 0x10)?; // mixer manual
        let _ = self.read(bus, 4)?; // upstream reads status here
        self.write_mask(bus, 0x0c, 0x08, 0x9f)?; // VGA fixed
        let (lna, mix) = gain_indices(tenths_db);
        self.write_mask(bus, 0x05, lna, 0x0f)?;
        self.write_mask(bus, 0x07, mix, 0x0f)
    }

    /// Hand gain to the tuner's own LNA/mixer AGC.
    pub fn set_auto_gain(&mut self, bus: &impl I2c) -> Result<()> {
        self.write_mask(bus, 0x05, 0x00, 0x10)?;
        self.write_mask(bus, 0x07, 0x10, 0x10)?;
        self.write_mask(bus, 0x0c, 0x0b, 0x9f) // VGA under control of the demod
    }

    /// Low-power state; a following `init` brings the chip back.
    pub fn standby(&mut self, bus: &impl I2c) -> Result<()> {
        if !self.init_done {
            return Ok(());
        }
        for (reg, val) in [
            (0x06, 0xb1),
            (0x05, 0xa0),
            (0x07, 0x3a),
            (0x08, 0x40),
            (0x09, 0xc0),
            (0x0a, 0x36),
            (0x0c, 0x35),
            (0x0f, 0x68),
            (0x11, 0x03),
            (0x17, 0xf4),
            (0x19, 0x0c),
        ] {
            self.write_reg(bus, reg, val)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const XTAL: u32 = 28_800_000;

    #[test]
    fn bandwidth_picks_upstream_if() {
        struct Nop;
        impl I2c for Nop {
            fn i2c_write(&self, _: u8, _: &[u8]) -> Result<()> {
                Ok(())
            }
            fn i2c_read(&self, _: u8, n: usize) -> Result<Vec<u8>> {
                Ok(vec![0; n])
            }
        }
        let mut t = R82xx::new(Chip::R820T, XTAL);
        t.regs[..INIT_ARRAY.len()].copy_from_slice(&INIT_ARRAY);
        assert_eq!(t.set_bw(&Nop, 8_000_000).unwrap(), 4_570_000);
        assert_eq!(t.set_bw(&Nop, 2_500_000).unwrap(), 3_570_000);
        // 2.048 MHz <= 1.7 + 0.35 MHz: only the first high-pass stage, then
        // the 1.7 MHz LPF: 2.3 + 0.35 - (0.35 + 1.7) / 2 = 1.625 MHz.
        assert_eq!(t.set_bw(&Nop, 2_048_000).unwrap(), 1_625_000);
    }

    /// Records I2C traffic; answers reads with a locked PLL and a valid
    /// filter-calibration code.
    #[derive(Default)]
    struct Bus {
        writes: RefCell<Vec<Vec<u8>>>,
    }

    impl I2c for Bus {
        fn i2c_write(&self, addr: u8, data: &[u8]) -> Result<()> {
            assert_eq!(addr, 0x34);
            self.writes.borrow_mut().push(data.to_vec());
            Ok(())
        }
        fn i2c_read(&self, _: u8, n: usize) -> Result<Vec<u8>> {
            // Status reg 2 bit 6 = locked; reg 4 = cal code 5, fine-tune 2
            // (values as seen after bit reversal).
            let mut v = vec![0u8; n];
            if n > 2 {
                v[2] = bitrev(0x40);
            }
            if n > 4 {
                v[4] = bitrev(0x25);
            }
            Ok(v)
        }
    }

    #[test]
    fn init_sends_the_power_on_table_in_bridge_sized_chunks() {
        let bus = Bus::default();
        let mut t = R82xx::new(Chip::R820T, XTAL);
        t.init(&bus).unwrap();
        let w = bus.writes.borrow();
        // 27 init bytes from reg 5 in messages of <= 7 data bytes.
        assert_eq!(w[0], [&[0x05][..], &INIT_ARRAY[..7]].concat());
        assert_eq!(w[1][0], 0x0c);
        assert_eq!(w[3], [&[0x1a][..], &INIT_ARRAY[21..]].concat());
        assert!(w.iter().all(|m| m.len() <= MAX_I2C_MSG));
        assert_eq!(t.fil_cal_code, 5);
    }

    #[test]
    fn unchanged_registers_are_not_resent() {
        let bus = Bus::default();
        let mut t = R82xx::new(Chip::R820T, XTAL);
        t.init(&bus).unwrap();
        t.set_freq(&bus, 100_000_000).unwrap();
        let n = bus.writes.borrow().len();
        t.set_freq(&bus, 100_000_000).unwrap();
        // A repeat tune only re-arms the PLL auto-tune bits (plus the
        // register-0 address byte every status read sends).
        let extra: Vec<Vec<u8>> = bus.writes.borrow()[n..].to_vec();
        assert!(
            extra.iter().all(|m| m[0] == 0x1a || m[..] == [0x00]),
            "{extra:02x?}"
        );
    }
}
