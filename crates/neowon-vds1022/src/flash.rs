//! Factory calibration blob stored in device flash (2002 bytes).
//!
//! Layout (little-endian):
//! ```text
//! 0    u16      header: device returns AA 55 (host writes 55 AA)
//! 2    u32      version, must be 2
//! 6    u16[10]  CH1 gain          (per voltbase index)
//! 26   u16[10]  CH2 gain
//! 46   u16[10]  CH1 amplitude
//! 66   u16[10]  CH2 amplitude
//! 86   u16[10]  CH1 compensation
//! 106  u16[10]  CH2 compensation
//! 206  u8       OEM flag
//! 207  cstr     hardware version (e.g. "V2.5")
//! ...  cstr     serial number
//! ...  u8[100]  locale flags
//! ...  u16      phase fine (0..255)
//! ```

use crate::consts::FLASH_SIZE;
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct FlashCal {
    pub version: u32,
    /// Indexed `[channel][voltbase]`.
    pub gain: [[u16; 10]; 2],
    pub ampl: [[u16; 10]; 2],
    pub comp: [[u16; 10]; 2],
    pub oem: u8,
    pub hw_version: String,
    pub serial: String,
    pub phasefine: u16,
}

fn u16_at(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn u16x10(buf: &[u8], off: usize) -> [u16; 10] {
    std::array::from_fn(|i| u16_at(buf, off + 2 * i))
}

fn cstr_at(buf: &[u8], off: usize) -> (String, usize) {
    let end = buf[off..]
        .iter()
        .position(|&b| b == 0)
        .map(|p| off + p)
        .unwrap_or(buf.len());
    let s = String::from_utf8_lossy(&buf[off..end]).into_owned();
    (s, end + 1)
}

impl FlashCal {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() != FLASH_SIZE {
            return Err(Error::Flash(format!(
                "expected {FLASH_SIZE} bytes, got {}",
                buf.len()
            )));
        }
        // Device read returns AA 55; a freshly written blob may read 55 AA.
        let hdr = (buf[0], buf[1]);
        if hdr != (0xAA, 0x55) && hdr != (0x55, 0xAA) {
            return Err(Error::Flash(format!(
                "bad header {:02X} {:02X}",
                buf[0], buf[1]
            )));
        }
        let version = u32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]);
        if version != 2 {
            return Err(Error::Flash(format!("unsupported flash version {version}")));
        }
        let gain = [u16x10(buf, 6), u16x10(buf, 26)];
        let ampl = [u16x10(buf, 46), u16x10(buf, 66)];
        let comp = [u16x10(buf, 86), u16x10(buf, 106)];
        let oem = buf[206];
        let (hw_version, next) = cstr_at(buf, 207);
        let (serial, next) = cstr_at(buf, next);
        let phasefine_off = next + 100; // skip locale flags
        let phasefine = if phasefine_off + 2 <= buf.len() {
            u16_at(buf, phasefine_off)
        } else {
            0
        };
        Ok(Self {
            version,
            gain,
            ampl,
            comp,
            oem,
            hw_version,
            serial,
            phasefine,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_synthetic_blob() {
        let mut buf = vec![0u8; FLASH_SIZE];
        buf[0] = 0xAA;
        buf[1] = 0x55;
        buf[2..6].copy_from_slice(&2u32.to_le_bytes());
        // ch1 gain[7] = 770
        buf[6 + 14..6 + 16].copy_from_slice(&770u16.to_le_bytes());
        // ch2 comp[0] = 550
        buf[106..108].copy_from_slice(&550u16.to_le_bytes());
        buf[206] = 1;
        let vers = b"V2.5\0VDS1022I1809215\0";
        buf[207..207 + vers.len()].copy_from_slice(vers);
        let pf_off = 207 + vers.len() + 100;
        buf[pf_off..pf_off + 2].copy_from_slice(&130u16.to_le_bytes());

        let cal = FlashCal::parse(&buf).unwrap();
        assert_eq!(cal.gain[0][7], 770);
        assert_eq!(cal.comp[1][0], 550);
        assert_eq!(cal.hw_version, "V2.5");
        assert_eq!(cal.serial, "VDS1022I1809215");
        assert_eq!(cal.phasefine, 130);
        assert_eq!(cal.oem, 1);
    }
}
