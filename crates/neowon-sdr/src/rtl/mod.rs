//! In-tree RTL-SDR driver: RTL2832U demodulator + R820T/R828D tuner over
//! `nusb` (Phase 10 D2). Ported from `librtlsdr-rs`
//! (`tmp-inspiration/librtlsdr-rs`), itself a faithful port of librtlsdr;
//! register facts verified on hardware live in `docs/protocol-rtlsdr.md`.

mod device;
mod r82xx;
mod r82xx_tables;
mod stream;
mod usb;

pub use device::{DeviceInfo, DirectSampling, RtlSdr, TunerKind, list, resampler, transfer_len};
pub use stream::Stream;

/// Crystal of the RTL2832U (and of an R820T on the same clock).
pub const RTL_XTAL_HZ: u32 = 28_800_000;
/// Tuner range: below this the R820T PLL has no divider (HF needs direct
/// sampling); above it the VCO runs out.
pub const TUNER_MIN_HZ: u32 = 24_000_000;
pub const TUNER_MAX_HZ: u32 = 1_766_000_000;
/// Discrete R82xx gains, tenths of a dB.
pub const R82XX_GAINS: [i32; 29] = [
    0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372,
    386, 402, 421, 434, 439, 445, 480, 496,
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no RTL-SDR found")]
    NoDevice,
    #[error("usb: {0}")]
    Usb(#[from] nusb::Error),
    #[error("usb transfer: {0}")]
    Transfer(#[from] nusb::transfer::TransferError),
    #[error("no supported tuner (R820T/R828D) answered")]
    NoTuner,
    #[error("tuner PLL: {0}")]
    Pll(String),
    #[error("i2c: {0}")]
    I2c(String),
    #[error("invalid parameter: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
