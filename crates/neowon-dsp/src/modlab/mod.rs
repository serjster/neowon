//! The modulation lab: cumulants, cyclostationary symbol-rate and
//! carrier-offset estimates, and a synchroniser + slicer that yields
//! constellations, bits, EVM and MER.

pub mod channel;
pub mod cumulants;
pub mod cyclo;
pub mod recover;

pub use channel::select;
pub use cumulants::{Cumulants, cumulants};
pub use cyclo::{carrier_offset, symbol_rate};
pub use recover::{Recovered, recover, rrc};
