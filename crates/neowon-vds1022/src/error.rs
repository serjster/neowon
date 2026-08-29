use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(
        "no VDS1022 found (VID 0x5345 PID 0x1234) — is it plugged in, and is the vendor app closed?"
    )]
    NoDevice,
    #[error("USB error: {0}")]
    Usb(#[from] nusb::Error),
    #[error("USB transfer failed: {0}")]
    Transfer(#[from] nusb::transfer::TransferError),
    #[error("USB transfer timed out")]
    Timeout,
    #[error("device reports no data ready")]
    NotReady,
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("unexpected machine type {0} (expected 1 = VDS1022)")]
    WrongMachine(u32),
    #[error("calibration flash invalid: {0}")]
    Flash(String),
    #[error("FPGA bitstream: {0}")]
    Fpga(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
