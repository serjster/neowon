//! In-tree RTL-SDR driver: RTL2832U demodulator + R820T/R828D tuner over
//! `nusb`. Ported from `librtlsdr-rs`
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
/// Discrete R82xx gains, tenths of a dB. The ladder itself has one home
/// in core (the sim advertises the same one without depending on this
/// crate); the register tables below are this driver's own.
pub use neowon_core::ladders::R82XX_GAINS_TDB as R82XX_GAINS;

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
