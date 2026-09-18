//! RTL2832U register access over vendor control transfers.
//!
//! Encoding (librtlsdr): request 0; a block register is `value = addr`,
//! `index = block << 8` (| 0x10 to write); a demodulator register is
//! `value = addr << 8 | 0x20`, `index = page` (| 0x10 to write). Multi-byte
//! writes are big-endian.

use std::time::Duration;

use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
use nusb::{Interface, MaybeFuture};

use super::{Error, Result};

const TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy)]
pub enum Block {
    Usb = 1,
    Sys = 2,
    Iic = 6,
}

pub const USB_SYSCTL: u16 = 0x2000;
pub const USB_EPA_CTL: u16 = 0x2148;
pub const USB_EPA_MAXPKT: u16 = 0x2158;
pub const DEMOD_CTL: u16 = 0x3000;
pub const GPO: u16 = 0x3001;
pub const GPOE: u16 = 0x3003;
pub const GPD: u16 = 0x3004;
pub const DEMOD_CTL_1: u16 = 0x300b;

/// Default decimation FIR: 8 int8 taps then 8 int12 taps.
pub const FIR_DEFAULT: [i32; 16] = [
    -54, -36, -41, -40, -32, -14, 14, 53, 101, 156, 215, 273, 327, 372, 404, 421,
];

/// The tuner's view of the bus, so the tuner can be tested without USB.
pub trait I2c {
    fn i2c_write(&self, addr: u8, data: &[u8]) -> Result<()>;
    fn i2c_read(&self, addr: u8, len: usize) -> Result<Vec<u8>>;
}

#[derive(Clone)]
pub struct Usb {
    iface: Interface,
}

impl Usb {
    pub fn new(iface: Interface) -> Self {
        Self { iface }
    }

    pub fn interface(&self) -> &Interface {
        &self.iface
    }

    fn ctrl_in(&self, value: u16, index: u16, length: u16) -> Result<Vec<u8>> {
        let req = ControlIn {
            control_type: ControlType::Vendor,
            recipient: Recipient::Device,
            request: 0,
            value,
            index,
            length,
        };
        Ok(self.iface.control_in(req, TIMEOUT).wait()?)
    }

    fn ctrl_out(&self, value: u16, index: u16, data: &[u8]) -> Result<()> {
        let req = ControlOut {
            control_type: ControlType::Vendor,
            recipient: Recipient::Device,
            request: 0,
            value,
            index,
            data,
        };
        Ok(self.iface.control_out(req, TIMEOUT).wait()?)
    }

    pub fn read_reg(&self, block: Block, addr: u16) -> Result<u8> {
        let d = self.ctrl_in(addr, (block as u16) << 8, 1)?;
        d.first()
            .copied()
            .ok_or_else(|| Error::I2c(format!("empty read of {block:?}:{addr:#06x}")))
    }

    pub fn write_reg(&self, block: Block, addr: u16, val: u16, len: u8) -> Result<()> {
        self.ctrl_out(addr, ((block as u16) << 8) | 0x10, &be_bytes(val, len))
    }

    pub fn demod_read(&self, page: u8, addr: u16) -> Result<u8> {
        let d = self.ctrl_in((addr << 8) | 0x20, page as u16, 1)?;
        Ok(d.first().copied().unwrap_or(0))
    }

    /// Write a demodulator register. librtlsdr follows every demod write
    /// with a dummy read of page 0x0a register 0x01, which the chip needs
    /// to latch the write; its result is ignored.
    pub fn demod_write(&self, page: u8, addr: u16, val: u16, len: u8) -> Result<()> {
        let r = self.ctrl_out((addr << 8) | 0x20, 0x10 | page as u16, &be_bytes(val, len));
        let _ = self.demod_read(0x0a, 0x01);
        r
    }

    pub fn i2c_read_reg(&self, addr: u8, reg: u8) -> Result<u8> {
        self.i2c_write(addr, &[reg])?;
        Ok(self.i2c_read(addr, 1)?[0])
    }

    pub fn set_gpio_output(&self, gpio: u8) -> Result<()> {
        let mask = 1u16 << gpio;
        let r = self.read_reg(Block::Sys, GPD)? as u16;
        self.write_reg(Block::Sys, GPD, r & !mask, 1)?;
        let r = self.read_reg(Block::Sys, GPOE)? as u16;
        self.write_reg(Block::Sys, GPOE, r | mask, 1)
    }

    pub fn set_gpio_bit(&self, gpio: u8, on: bool) -> Result<()> {
        let mask = 1u16 << gpio;
        let r = self.read_reg(Block::Sys, GPO)? as u16;
        self.write_reg(Block::Sys, GPO, if on { r | mask } else { r & !mask }, 1)
    }

    /// Gate the demodulator's I2C bridge to the tuner.
    pub fn set_i2c_repeater(&self, on: bool) -> Result<()> {
        self.demod_write(1, 0x01, if on { 0x18 } else { 0x10 }, 1)
    }

    pub fn set_fir(&self, fir: &[i32; 16]) -> Result<()> {
        for (i, b) in fir_bytes(fir)?.iter().enumerate() {
            self.demod_write(1, 0x1c + i as u16, *b as u16, 1)?;
        }
        Ok(())
    }

    /// librtlsdr's `rtlsdr_init_baseband`, in order.
    pub fn init_baseband(&self) -> Result<()> {
        self.write_reg(Block::Usb, USB_SYSCTL, 0x09, 1)?;
        self.write_reg(Block::Usb, USB_EPA_MAXPKT, 0x0002, 2)?;
        self.write_reg(Block::Usb, USB_EPA_CTL, 0x1002, 2)?;
        // Power on the demodulator and its ADCs.
        self.write_reg(Block::Sys, DEMOD_CTL_1, 0x22, 1)?;
        self.write_reg(Block::Sys, DEMOD_CTL, 0xe8, 1)?;
        // Soft reset.
        self.demod_write(1, 0x01, 0x14, 1)?;
        self.demod_write(1, 0x01, 0x10, 1)?;
        // No spectrum inversion, no adjacent-channel rejection, IF 0.
        self.demod_write(1, 0x15, 0x00, 1)?;
        self.demod_write(1, 0x16, 0x0000, 2)?;
        for i in 0..6 {
            self.demod_write(1, 0x16 + i, 0x00, 1)?;
        }
        self.set_fir(&FIR_DEFAULT)?;
        // SDR mode, digital AGC off.
        self.demod_write(0, 0x19, 0x05, 1)?;
        // FSM state-holding registers.
        self.demod_write(1, 0x93, 0xf0, 1)?;
        self.demod_write(1, 0x94, 0x0f, 1)?;
        // AGC loop off, PID filter off, RF/IF AGC to default, ADC_I/ADC_Q
        // datapath, Zero-IF, 8-bit output.
        self.demod_write(1, 0x11, 0x00, 1)?;
        self.demod_write(1, 0x04, 0x00, 1)?;
        self.demod_write(0, 0x61, 0x60, 1)?;
        self.demod_write(0, 0x06, 0x80, 1)?;
        self.demod_write(1, 0xb1, 0x1b, 1)?;
        self.demod_write(0, 0x0d, 0x83, 1)
    }

    pub fn deinit_baseband(&self) -> Result<()> {
        self.write_reg(Block::Sys, DEMOD_CTL, 0x20, 1)
    }

    /// Flush the bulk FIFO (librtlsdr `rtlsdr_reset_buffer`).
    pub fn reset_buffer(&self) -> Result<()> {
        self.write_reg(Block::Usb, USB_EPA_CTL, 0x1002, 2)?;
        self.write_reg(Block::Usb, USB_EPA_CTL, 0x0000, 2)
    }
}

impl I2c for Usb {
    fn i2c_write(&self, addr: u8, data: &[u8]) -> Result<()> {
        self.ctrl_out(addr as u16, ((Block::Iic as u16) << 8) | 0x10, data)
    }

    fn i2c_read(&self, addr: u8, len: usize) -> Result<Vec<u8>> {
        let d = self.ctrl_in(addr as u16, (Block::Iic as u16) << 8, len as u16)?;
        if d.len() != len {
            return Err(Error::I2c(format!("read {} of {len} bytes", d.len())));
        }
        Ok(d)
    }
}

fn be_bytes(val: u16, len: u8) -> Vec<u8> {
    if len == 1 {
        vec![val as u8]
    } else {
        vec![(val >> 8) as u8, val as u8]
    }
}

/// Pack the FIR: taps 0..8 are int8, taps 8..16 are int12 packed two per
/// three bytes.
pub fn fir_bytes(fir: &[i32; 16]) -> Result<[u8; 20]> {
    let mut out = [0u8; 20];
    for (i, &v) in fir[..8].iter().enumerate() {
        if !(-128..=127).contains(&v) {
            return Err(Error::Invalid(format!("FIR tap {i} = {v} is not int8")));
        }
        out[i] = v as u8;
    }
    for i in (0..8).step_by(2) {
        let (a, b) = (fir[8 + i], fir[8 + i + 1]);
        if !(-2048..=2047).contains(&a) || !(-2048..=2047).contains(&b) {
            return Err(Error::Invalid(format!(
                "FIR taps {}/{} not int12",
                8 + i,
                9 + i
            )));
        }
        let o = 8 + i * 3 / 2;
        out[o] = (a >> 4) as u8;
        out[o + 1] = ((a << 4) | ((b >> 8) & 0x0f)) as u8;
        out[o + 2] = b as u8;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_fir_packs_like_librtlsdr() {
        let b = fir_bytes(&FIR_DEFAULT).unwrap();
        // int8 taps are two's complement bytes.
        assert_eq!(b[0], (-54i8) as u8);
        // 101 = 0x065, 156 = 0x09c -> 0x06, 0x50, 0x9c.
        assert_eq!(&b[8..11], &[0x06, 0x50, 0x9c]);
        // 404 = 0x194, 421 = 0x1a5 -> 0x19, 0x41, 0xa5.
        assert_eq!(&b[17..20], &[0x19, 0x41, 0xa5]);
    }

    #[test]
    fn fir_rejects_out_of_range_taps() {
        let mut f = FIR_DEFAULT;
        f[0] = 200;
        assert!(fir_bytes(&f).is_err());
    }
}
