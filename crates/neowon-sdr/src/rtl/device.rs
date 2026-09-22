//! `RtlSdr`: an opened RTL2832U with its R82xx tuner — librtlsdr's device
//! layer (`librtlsdr-rs` `device/`), R82xx only.

use nusb::MaybeFuture;
use tracing::{debug, info, warn};

use super::r82xx::{CHECK_VAL, Chip, DEFAULT_IF_HZ, R82xx};
use super::stream::Stream;
use super::usb::{Block, USB_SYSCTL, Usb};
use super::{Error, R82XX_GAINS, RTL_XTAL_HZ, Result, TUNER_MIN_HZ};

pub use super::r82xx::Chip as TunerKind;

const VID: u16 = 0x0bda;
const PIDS: [u16; 2] = [0x2832, 0x2838];
/// librtlsdr's default: 15 transfers in flight.
const TRANSFERS: usize = 15;
/// Chunks the consumer may fall behind before the stream drops (and counts).
const QUEUE: usize = 64;

/// An attached dongle, before it is opened.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub bus: String,
    pub address: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
}

fn is_rtl(d: &nusb::DeviceInfo) -> bool {
    d.vendor_id() == VID && PIDS.contains(&d.product_id())
}

fn describe(d: &nusb::DeviceInfo) -> DeviceInfo {
    DeviceInfo {
        bus: d.bus_id().to_string(),
        address: d.device_address(),
        vendor_id: d.vendor_id(),
        product_id: d.product_id(),
        manufacturer: d.manufacturer_string().map(str::to_owned),
        product: d.product_string().map(str::to_owned),
        serial: d.serial_number().map(str::to_owned),
    }
}

/// Attached RTL2832U dongles. Enumeration only; nothing is claimed.
pub fn list() -> Result<Vec<DeviceInfo>> {
    Ok(nusb::list_devices()
        .wait()?
        .filter(is_rtl)
        .map(|d| describe(&d))
        .collect())
}

/// Which ADC feeds the demodulator. `I`/`Q` bypass the tuner and sample
/// the antenna directly (HF); the RTL-SDR Blog V3 wires its HF input to Q.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectSampling {
    Off,
    I,
    Q,
}

pub struct RtlSdr {
    usb: Usb,
    _device: nusb::Device,
    tuner: R82xx,
    info: DeviceInfo,
    rtl_xtal: u32,
    tun_xtal: u32,
    rate: u32,
    /// Tuned centre, Hz; 0 = not tuned.
    freq: u32,
    /// Requested tuner bandwidth; 0 = follow the sample rate.
    bw: u32,
    ppm: i32,
    direct: DirectSampling,
    /// `None` = tuner AGC, `Some(tenths)` = manual; re-applied after a
    /// tuner re-init.
    gain: Option<i32>,
}

/// Resampler ratio for `rate` and the rate it really yields: the ratio is
/// a 28-bit value with bit 27 mirrored into bit 28 (librtlsdr).
pub fn resampler(rtl_xtal: u32, rate: u32) -> (u32, u32) {
    let ratio = ((rtl_xtal as f64 * (1u64 << 22) as f64) / rate as f64) as u32 & 0x0fff_fffc;
    let real = ratio | ((ratio & 0x0800_0000) << 1);
    let real_rate = (rtl_xtal as f64 * (1u64 << 22) as f64 / real as f64) as u32;
    (ratio, real_rate)
}

/// The demodulator's IF register value (22-bit, negative frequency).
pub fn if_word(if_hz: u32, rtl_xtal: u32) -> i32 {
    -((if_hz as f64 * (1u64 << 22) as f64) / rtl_xtal as f64) as i32
}

/// Sample-clock correction word for `ppm` (14-bit, sign-extended by chip).
pub fn ppm_word(ppm: i32) -> i16 {
    (-ppm as f64 * (1u64 << 24) as f64 / 1e6) as i16
}

fn corrected(xtal: u32, ppm: i32) -> u32 {
    (xtal as f64 * (1.0 + ppm as f64 / 1e6)) as u32
}

/// Bulk-transfer length for streaming at `rate_hz` pairs/s, bytes: the
/// shared stream-frame policy ([`neowon_core::stream_chunk_pairs`]) sized
/// for one frame per transfer, so the display cadence stays ~20 rows/s
/// instead of collapsing at low rates. 32 KiB…256 KiB, and always a whole
/// number of 64-byte USB packets.
pub fn transfer_len(rate_hz: f64) -> usize {
    neowon_core::stream_chunk_pairs(rate_hz) * 2
}

impl RtlSdr {
    /// Open the dongle with `serial`, or the first one.
    pub fn open(serial: Option<&str>) -> Result<Self> {
        let dev = nusb::list_devices()
            .wait()?
            .filter(is_rtl)
            .find(|d| serial.is_none_or(|s| d.serial_number() == Some(s)))
            .ok_or(Error::NoDevice)?;
        let info = describe(&dev);
        let device = dev.open().wait()?;
        let usb = Usb::new(device.detach_and_claim_interface(0).wait()?);

        // A dummy write tests the link; librtlsdr resets the device if it
        // fails and carries on.
        if usb.write_reg(Block::Usb, USB_SYSCTL, 0x09, 1).is_err() {
            warn!("dummy write failed, resetting the device");
            let _ = device.reset().wait();
        }
        usb.init_baseband()?;

        usb.set_i2c_repeater(true)?;
        let probe = |chip: Chip| usb.i2c_read_reg(chip.i2c_addr(), 0).ok() == Some(CHECK_VAL);
        let chip = if probe(Chip::R820T) {
            Chip::R820T
        } else if probe(Chip::R828D) {
            Chip::R828D
        } else {
            let _ = usb.set_i2c_repeater(false);
            return Err(Error::NoTuner);
        };
        // A plain R828D runs on its own 16 MHz crystal.
        let tun_xtal = match chip {
            Chip::R820T => RTL_XTAL_HZ,
            Chip::R828D => 16_000_000,
        };
        let mut sdr = Self {
            tuner: R82xx::new(chip, tun_xtal),
            usb,
            _device: device,
            info,
            rtl_xtal: RTL_XTAL_HZ,
            tun_xtal,
            rate: 0,
            freq: 0,
            bw: 0,
            ppm: 0,
            direct: DirectSampling::Off,
            gain: None,
        };
        // R82xx: low IF, so no Zero-IF, I ADC only, spectrum inverted.
        sdr.usb.demod_write(1, 0xb1, 0x1a, 1)?;
        sdr.usb.demod_write(0, 0x08, 0x4d, 1)?;
        sdr.set_if_freq(DEFAULT_IF_HZ)?;
        sdr.usb.demod_write(1, 0x15, 0x01, 1)?;
        sdr.tuner.init(&sdr.usb)?;
        sdr.usb.set_i2c_repeater(false)?;
        info!(?chip, serial = ?sdr.info.serial, "RTL-SDR open");
        Ok(sdr)
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn tuner(&self) -> TunerKind {
        self.tuner.chip()
    }

    pub fn gains(&self) -> &'static [i32] {
        &R82XX_GAINS
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn center_freq(&self) -> u32 {
        self.freq
    }

    pub fn ppm(&self) -> i32 {
        self.ppm
    }

    pub fn direct_sampling(&self) -> DirectSampling {
        self.direct
    }

    /// The tuner's IF, Hz: the LO sits at centre + IF, so a crystal
    /// correction moves the band by (centre + IF) * ppm.
    pub fn tuner_if_hz(&self) -> u32 {
        self.tuner.if_hz()
    }

    /// Run `f` on the tuner with the I2C repeater open; always closes it.
    fn with_tuner<R>(&mut self, f: impl FnOnce(&mut R82xx, &Usb) -> Result<R>) -> Result<R> {
        self.usb.set_i2c_repeater(true)?;
        let r = f(&mut self.tuner, &self.usb);
        let closed = self.usb.set_i2c_repeater(false);
        let r = r?;
        closed?;
        Ok(r)
    }

    fn set_if_freq(&self, if_hz: u32) -> Result<()> {
        let w = if_word(if_hz, corrected(self.rtl_xtal, self.ppm));
        self.usb
            .demod_write(1, 0x19, ((w >> 16) & 0x3f) as u16, 1)?;
        self.usb.demod_write(1, 0x1a, ((w >> 8) & 0xff) as u16, 1)?;
        self.usb.demod_write(1, 0x1b, (w & 0xff) as u16, 1)
    }

    fn set_sample_freq_correction(&self, ppm: i32) -> Result<()> {
        let w = ppm_word(ppm);
        self.usb.demod_write(1, 0x3f, (w & 0xff) as u16, 1)?;
        self.usb.demod_write(1, 0x3e, ((w >> 8) & 0x3f) as u16, 1)
    }

    /// Point the tuner's IF filter at the current bandwidth and make the
    /// demodulator follow the IF it chose.
    fn apply_bandwidth(&mut self) -> Result<()> {
        let bw = if self.bw > 0 { self.bw } else { self.rate };
        if bw == 0 {
            return Ok(());
        }
        let if_hz = self.with_tuner(|t, bus| t.set_bw(bus, bw))?;
        self.set_if_freq(if_hz)
    }

    /// Set the IQ rate; returns the rate the resampler really runs at.
    /// Valid: 225.001–300 kHz and 900.001 kHz–3.2 MHz.
    pub fn set_sample_rate(&mut self, rate: u32) -> Result<u32> {
        if rate <= 225_000 || rate > 3_200_000 || (300_000 < rate && rate <= 900_000) {
            return Err(Error::Invalid(format!("sample rate {rate} Hz")));
        }
        let (ratio, real) = resampler(self.rtl_xtal, rate);
        self.rate = real;
        if self.direct == DirectSampling::Off {
            self.apply_bandwidth()?;
            if self.freq > 0 {
                self.set_center_freq(self.freq)?;
            }
        }
        self.usb.demod_write(1, 0x9f, (ratio >> 16) as u16, 2)?;
        self.usb.demod_write(1, 0xa1, (ratio & 0xffff) as u16, 2)?;
        self.set_sample_freq_correction(self.ppm)?;
        // Soft reset so the new ratio takes.
        self.usb.demod_write(1, 0x01, 0x14, 1)?;
        self.usb.demod_write(1, 0x01, 0x10, 1)?;
        debug!(rate, real, "sample rate");
        Ok(real)
    }

    /// Tune. With direct sampling on, this sets the digital downconverter
    /// instead (any frequency up to ~14.4 MHz is usable, then aliases).
    pub fn set_center_freq(&mut self, hz: u32) -> Result<()> {
        let r = if self.direct != DirectSampling::Off {
            self.set_if_freq(hz)
        } else {
            self.with_tuner(|t, bus| t.set_freq(bus, hz))
        };
        self.freq = if r.is_ok() { hz } else { 0 };
        r
    }

    /// Crystal correction for both the sample clock and the tuner.
    pub fn set_ppm(&mut self, ppm: i32) -> Result<()> {
        if ppm == self.ppm {
            return Ok(());
        }
        self.set_sample_freq_correction(ppm)?;
        self.ppm = ppm;
        self.tuner.set_xtal(corrected(self.tun_xtal, ppm));
        if self.freq > 0 {
            self.set_center_freq(self.freq)?;
        }
        Ok(())
    }

    /// Manual tuner gain in tenths of a dB (the tuner steps to the nearest
    /// table entry at or above it), or `None` for the tuner's AGC.
    pub fn set_gain(&mut self, tenths_db: Option<i32>) -> Result<()> {
        self.gain = tenths_db;
        match tenths_db {
            Some(g) => self.with_tuner(|t, bus| t.set_gain(bus, g)),
            None => self.with_tuner(|t, bus| t.set_auto_gain(bus)),
        }
    }

    /// The RTL2832's digital AGC — separate from the tuner gain.
    pub fn set_rtl_agc(&self, on: bool) -> Result<()> {
        self.usb
            .demod_write(0, 0x19, if on { 0x25 } else { 0x05 }, 1)
    }

    /// Tuner IF bandwidth, Hz; 0 follows the sample rate.
    pub fn set_bandwidth(&mut self, bw: u32) -> Result<()> {
        self.bw = bw;
        if self.direct == DirectSampling::Off {
            self.apply_bandwidth()?;
            if self.freq > 0 {
                self.set_center_freq(self.freq)?;
            }
        }
        Ok(())
    }

    /// Antenna power on GPIO 0 (RTL-SDR Blog bias-T).
    pub fn set_bias_tee(&self, on: bool) -> Result<()> {
        self.usb.set_gpio_output(0)?;
        self.usb.set_gpio_bit(0, on)
    }

    /// Switch between the tuner and direct (HF) sampling.
    ///
    /// Leaving direct sampling re-initialises the tuner and restores its
    /// bandwidth and gain. Unlike librtlsdr it does **not** retune when the
    /// current centre is below the tuner's range (that fails in the PLL,
    /// docs/protocol-rtlsdr.md); the device is then untuned
    /// (`center_freq() == 0`) until the caller tunes it.
    pub fn set_direct_sampling(&mut self, mode: DirectSampling) -> Result<()> {
        if mode == self.direct {
            return Ok(());
        }
        if mode != DirectSampling::Off {
            if self.direct == DirectSampling::Off {
                self.with_tuner(|t, bus| t.standby(bus))?;
            }
            self.usb.demod_write(1, 0xb1, 0x1a, 1)?; // Zero-IF off
            self.usb.demod_write(1, 0x15, 0x00, 1)?; // no spectrum inversion
            self.usb.demod_write(0, 0x08, 0x4d, 1)?; // I ADC only
            // Swapping the ADCs selects the Q input.
            let swap = if mode == DirectSampling::Q {
                0x90
            } else {
                0x80
            };
            self.usb.demod_write(0, 0x06, swap, 1)?;
            self.direct = mode;
            if self.freq > 0 {
                self.set_center_freq(self.freq)?;
            }
        } else {
            self.direct = DirectSampling::Off;
            self.with_tuner(|t, bus| t.init(bus))?;
            self.apply_bandwidth()?;
            let gain = self.gain;
            self.set_gain(gain)?;
            self.usb.demod_write(1, 0x15, 0x01, 1)?; // spectrum inversion
            self.usb.demod_write(0, 0x06, 0x80, 1)?; // default ADC datapath
            if self.freq >= TUNER_MIN_HZ {
                self.set_center_freq(self.freq)?;
            } else {
                self.freq = 0;
            }
        }
        info!(?mode, "direct sampling");
        Ok(())
    }

    /// Flush the FIFO and start streaming u8 offset-binary I,Q pairs. The
    /// transfer length follows the current sample rate.
    pub fn stream(&self) -> Result<Stream> {
        self.usb.reset_buffer()?;
        let len = transfer_len(self.rate as f64);
        debug!(rate = self.rate, transfer_len = len, "start stream");
        Stream::start(self.usb.interface(), TRANSFERS, len, QUEUE)
    }
}

impl Drop for RtlSdr {
    fn drop(&mut self) {
        if self.direct == DirectSampling::Off
            && let Err(e) = self.with_tuner(|t, bus| t.standby(bus))
        {
            debug!("tuner standby on close: {e}");
        }
        if let Err(e) = self.usb.deinit_baseband() {
            debug!("baseband deinit on close: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_rates_are_exact() {
        // Seen on hardware: 2.048 MS/s is reported exactly.
        for rate in [250_000u32, 1_024_000, 2_048_000, 2_400_000, 3_200_000] {
            assert_eq!(resampler(RTL_XTAL_HZ, rate).1, rate, "{rate}");
        }
        assert_eq!(resampler(RTL_XTAL_HZ, 2_048_000).0, 0x0384_0000);
    }

    #[test]
    fn if_word_is_negative_and_fits_22_bits() {
        let w = if_word(DEFAULT_IF_HZ, RTL_XTAL_HZ);
        // 3.57 MHz * 2^22 / 28.8 MHz = 519 918.9...
        assert_eq!(w, -519_918);
        assert!(w.unsigned_abs() < 1 << 21);
    }

    #[test]
    fn ppm_word_scales_by_two_to_the_24() {
        assert_eq!(ppm_word(0), 0);
        assert_eq!(ppm_word(100), -1677);
        assert_eq!(ppm_word(-100), 1677);
    }

    #[test]
    fn transfer_len_is_time_sized_bounded_and_aligned() {
        // The cadence-bug anchors: the floor binds at 250 kS/s (65.5 ms a
        // transfer), while 1.024 and 2.048 MS/s are the plain 50 ms.
        assert_eq!(transfer_len(250_000.0), 32_768);
        assert_eq!(transfer_len(1_024_000.0), 102_400);
        assert_eq!(transfer_len(2_048_000.0), 204_800);
        // Bounds: 32 KiB at the bottom, librtlsdr's 256 KiB at the top.
        assert_eq!(transfer_len(0.0), 32_768);
        assert_eq!(transfer_len(1e9), 262_144);
        // Every offered rate stays in bounds, on the 64-byte transfer grid,
        // and the length never falls as the rate rises.
        let mut prev = 0;
        for &r in crate::backend::SAMPLE_RATES.iter() {
            let len = transfer_len(r);
            assert!((32_768..=262_144).contains(&len), "{r}: {len}");
            assert_eq!(len % 64, 0, "{r}: {len}");
            assert!(len >= prev, "{r}: {len} < {prev}");
            prev = len;
        }
    }
}
