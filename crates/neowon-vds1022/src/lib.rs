//! Driver for the OWON VDS1022 / VDS1022I USB oscilloscope.
//!
//! Protocol reference: `OWON-VDS1022/api/python/vds1022/vds1022.py`
//! (community repo, florentbr) cross-validated against the decompiled vendor
//! Java app. Everything hardware-verified goes into
//! `docs/protocol-vds1022.md`.
//!
//! The device is a dumb USB front-end: the host uploads an FPGA bitstream on
//! every cold start, reads factory calibration from flash, then drives a
//! byte-addressed register file over bulk transfers.

pub mod consts;
pub mod device;
pub mod error;
pub mod flash;
pub mod fpga;
pub mod frame;

pub use device::{ChannelSetup, Vds1022};
pub use error::Error;
pub use flash::FlashCal;
pub use frame::RawFrame;
