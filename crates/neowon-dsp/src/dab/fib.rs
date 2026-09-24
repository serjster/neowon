//! The Fast Information Block: its CRC, and the walk that turns one FIB into
//! FIGs.
//!
//! **Clause 5.2.1 / annex E** (ETSI EN 300 401 V2.1.1): a FIB is 256 bits —
//! 30 data bytes followed by a 16-bit CRC — and the data field is a sequence of
//! FIGs whose header is 3 bits of type and 5 bits of length, MSb first. The CRC
//! is the X.25 one, `G(X) = X^16 + X^12 + X^5 + 1`, initialized to all ones and
//! complemented before transmission; annex E gives the procedure in full.
//!
//! A trailing padding byte of `0x00`, and the end marker `0xFF`, both end the
//! walk (clause 5.2.2.0): an unknown FIG is legal and must be skipped by
//! length, never guessed at.

use super::fec::crc16;
use super::fig;
use super::{Ensemble, FIB_BYTES, FIB_DATA_BYTES};

/// The CRC-16 of a FIB's data field, complemented as the standard requires
/// (clause 5.2.1: "the CRC is complemented prior to transmission"). The
/// parameters are annex E's, shared with the DLS and MSC checks via
/// [`crc16`].
pub fn fib_crc(fib: &[u8; FIB_BYTES]) -> u16 {
    crc16(&fib[..FIB_DATA_BYTES])
}

/// The CRC carried in the last two bytes of the FIB, MSb first.
pub fn fib_crc_received(fib: &[u8; FIB_BYTES]) -> u16 {
    ((fib[30] as u16) << 8) | fib[31] as u16
}

/// Does this FIB carry a valid CRC? The receiver trusts nothing it has not
/// checked.
pub fn fib_crc_ok(fib: &[u8; FIB_BYTES]) -> bool {
    fib_crc(fib) == fib_crc_received(fib)
}

/// Walk the FIGs of one FIB into the ensemble table, returning how many FIGs
/// were seen.
///
/// Unknown FIG types and extensions are skipped by their declared length:
/// they are legal, frequent on real air, and must not corrupt the table.
pub fn walk_figs(fib: &[u8; FIB_BYTES], ensemble: &mut Ensemble) -> usize {
    let mut count = 0;
    let mut pos = 0usize;
    while pos < FIB_DATA_BYTES {
        let header = fib[pos];
        let fig_type = (header >> 5) & 0x07;
        let fig_length = (header & 0x1F) as usize;
        // 0x00 padding, 0xFF end marker, or a FIG that does not fit.
        if header == 0x00 || fig_type == 0x07 || fig_length == 0 {
            break;
        }
        let start = pos + 1;
        let end = start + fig_length;
        if end > FIB_DATA_BYTES {
            break;
        }
        let payload = &fib[start..end];
        match fig_type {
            0 => fig::fig0(ensemble, payload),
            1 => fig::fig1(ensemble, payload),
            // Type 2 (extended labels) and the rest: skipped by length.
            _ => {}
        }
        count += 1;
        pos = end;
    }
    count
}

/// Build a FIB from a data field, filling in the CRC — the transmitter side of
/// the check above, shared by the tests in this module and in `fig`.
#[cfg(test)]
pub(crate) fn make_fib(data: &[u8; FIB_DATA_BYTES]) -> [u8; FIB_BYTES] {
    let mut fib = [0u8; FIB_BYTES];
    fib[..FIB_DATA_BYTES].copy_from_slice(data);
    let crc = crc16(data);
    fib[30] = (crc >> 8) as u8;
    fib[31] = crc as u8;
    fib
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CRC parameters are the ones the standard pins: X.25 polynomial,
    /// all-ones init, complemented. A single flipped bit must fail.
    #[test]
    fn crc_rejects_a_single_bit_error() {
        let data = [0x11u8; FIB_DATA_BYTES];
        let fib = make_fib(&data);
        assert!(fib_crc_ok(&fib));
        for byte in 0..FIB_DATA_BYTES {
            for bit in 0..8 {
                let mut damaged = fib;
                damaged[byte] ^= 1 << bit;
                assert!(!fib_crc_ok(&damaged), "byte {byte} bit {bit}");
            }
        }
    }

    /// The CRC is not the identity: a zero data field must not produce a zero
    /// CRC word (a common way to get a check that never fires).
    #[test]
    fn crc_is_not_degenerate() {
        let fib = make_fib(&[0u8; FIB_DATA_BYTES]);
        assert_ne!(fib_crc(&fib), 0);
        assert!(fib_crc_ok(&fib));
    }

    /// Padding and the end marker both stop the walk.
    #[test]
    fn walk_stops_on_padding_and_end_marker() {
        let mut data = [0u8; FIB_DATA_BYTES];
        // FIG 0/0, length 3 (flags byte + EId), carrying EId 0x1234.
        data[0] = 3; // FIG type 0, length 3
        data[1] = 0x00;
        data[2] = 0x12;
        data[3] = 0x34;
        let fib = make_fib(&data);
        let mut ensemble = Ensemble::default();
        assert_eq!(walk_figs(&fib, &mut ensemble), 1);
        assert_eq!(ensemble.eid, Some(0x1234));

        // The same with an explicit end marker after the FIG.
        let mut data = [0u8; FIB_DATA_BYTES];
        data[0] = 3;
        data[1] = 0x00;
        data[2] = 0xAB;
        data[3] = 0xCD;
        data[4] = 0xFF;
        let fib = make_fib(&data);
        let mut ensemble = Ensemble::default();
        assert_eq!(walk_figs(&fib, &mut ensemble), 1);
        assert_eq!(ensemble.eid, Some(0xABCD));
    }

    /// A FIG whose declared length runs past the data field is dropped rather
    /// than read out of bounds.
    #[test]
    fn overlong_fig_is_ignored() {
        let mut data = [0u8; FIB_DATA_BYTES];
        data[0] = 31;
        let fib = make_fib(&data);
        let mut ensemble = Ensemble::default();
        assert_eq!(walk_figs(&fib, &mut ensemble), 0);
    }
}
