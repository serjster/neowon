//! The SDR instrument (Phase 10): the in-tree RTL-SDR driver (`rtl`) and
//! its `Backend` (`backend`). Hardware facts and the P0.1 readouts live
//! in `docs/protocol-rtlsdr.md`; the hardware check is
//! `cargo run -p neowon-sdr --example p01`.

pub mod backend;
pub mod rtl;

pub use backend::RtlBackend;
