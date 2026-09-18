//! The modulation lab (Phase 10.3): cumulants (M3), cyclostationary
//! symbol-rate and carrier-offset estimates (M4, M5), and a synchroniser +
//! slicer that yields constellations, bits, EVM and MER (M1, M2, M6).

pub mod channel;
pub mod cumulants;
pub mod cyclo;
pub mod recover;

pub use channel::select;
pub use cumulants::{Cumulants, cumulants};
pub use cyclo::{carrier_offset, symbol_rate};
pub use recover::{Recovered, recover, rrc};
