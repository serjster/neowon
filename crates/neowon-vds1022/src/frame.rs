//! Wire-format frame parsing.
//!
//! One frame per enabled channel, 5211 bytes:
//! ```text
//! 0    u8       channel (0 = CH1, 1 = CH2)
//! 1    u32 LE   time_sum   (frequency-meter accumulator)
//! 5    u32 LE   period_num (frequency-meter period count)
//! 9    u16 LE   cursor — sample count from the right
//! 11   i8[100]  ADC trigger buffer (meaningful only at tiny timebases)
//! 111  i8[5100] ADC samples: 50 pre + 5000 usable + 50 post
//! ```

use crate::consts::{ADC_CLIP, ADC_OFFSET, ADC_SIZE, CLOCK_HZ, FRAME_SIZE, SAMPLES};
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct RawFrame {
    /// 0 = CH1, 1 = CH2.
    pub channel: u8,
    pub time_sum: u32,
    pub period_num: u32,
    pub cursor: u16,
    /// Full 5100-sample ADC payload (50 pre + 5000 + 50 post).
    pub adc: Vec<i8>,
}

impl RawFrame {
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() != FRAME_SIZE {
            return Err(Error::Protocol(format!(
                "frame size {} != {FRAME_SIZE}",
                buf.len()
            )));
        }
        let channel = buf[0];
        if channel > 1 {
            return Err(Error::Protocol(format!("bad channel byte {channel:#04x}")));
        }
        let time_sum = u32::from_le_bytes(buf[1..5].try_into().unwrap());
        let period_num = u32::from_le_bytes(buf[5..9].try_into().unwrap());
        let cursor = u16::from_le_bytes(buf[9..11].try_into().unwrap());
        let adc = buf[ADC_OFFSET..ADC_OFFSET + ADC_SIZE]
            .iter()
            .map(|&b| b as i8)
            .collect();
        Ok(Self {
            channel,
            time_sum,
            period_num,
            cursor,
            adc,
        })
    }

    /// The 5000 usable samples (drops 50 pre and 50 post).
    pub fn samples(&self) -> &[i8] {
        &self.adc[50..50 + SAMPLES]
    }

    pub fn clipped(&self) -> bool {
        self.samples()
            .iter()
            .any(|&s| s.unsigned_abs() >= ADC_CLIP as u8)
    }

    /// Hardware frequency-meter reading, if it counted anything.
    pub fn freq_meter(&self) -> Option<f64> {
        if self.time_sum == 0 || self.period_num == 0 {
            None
        } else {
            Some(self.period_num as f64 / self.time_sum as f64 * CLOCK_HZ)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip() {
        let mut buf = vec![0u8; FRAME_SIZE];
        buf[0] = 1;
        buf[1..5].copy_from_slice(&1_000_000u32.to_le_bytes());
        buf[5..9].copy_from_slice(&10u32.to_le_bytes());
        buf[9..11].copy_from_slice(&123u16.to_le_bytes());
        buf[ADC_OFFSET + 50] = 0x85; // -123 as i8, first usable sample
        let f = RawFrame::parse(&buf).unwrap();
        assert_eq!(f.channel, 1);
        assert_eq!(f.cursor, 123);
        assert_eq!(f.samples().len(), SAMPLES);
        assert_eq!(f.samples()[0], -123);
        // 10 periods over 1e6 ticks of 100 MHz -> 1 kHz
        assert!((f.freq_meter().unwrap() - 1000.0).abs() < 1e-9);
    }
}
