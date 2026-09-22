//! DAB check words — ETSI EN 300 401 V2.1.1 annex E, applied by
//! ETSI TS 102 563 V2.1.1 clause 5.2 to the DAB+ superframe.
//!
//! Two different check words protect a DAB+ superframe:
//!
//! * the **per-AU CRC** (`au_crc[n]`), the CRC-16 of EN 300 401 annex E
//!   with `G(x) = x^16 + x^12 + x^5 + 1`, shift register initialised to
//!   all ones and the word complemented before transmission (TS 102 563
//!   clause 5.2), and
//! * the **header Fire code** (`header_firecode`), a 16-bit burst-error
//!   code with `G(x) = (x^11 + 1)(x^5 + x^3 + x^2 + x + 1)`, register
//!   initialised to all zeros, computed over the nine superframe bytes
//!   2..10 (TS 102 563 clause 5.2, Table 2).
//!
//! Annex E fixes the bit order for both: data is applied MSb first and
//! the register shifts towards its MSb stage, so the implementations
//! below shift left with the polynomial's lower 16 bits as the XOR mask.

/// CRC-16 of EN 300 401 annex E, the check word TS 102 563 clause 5.2
/// puts after every access unit.
///
/// `G(x) = x^16 + x^12 + x^5 + 1` (0x1021), register initialised to
/// all ones, word complemented before transmission. The published check
/// value is 0xD64E for `b"123456789"` (same procedure as the FIB CRC).
#[must_use]
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc ^ 0xFFFF
}

/// TS 102 563 clause 5.2 Fire code polynomial, lower 16 bits:
/// `G(x) = (x^11 + 1)(x^5 + x^3 + x^2 + x + 1)`
/// `= x^16 + x^14 + x^13 + x^12 + x^11 + x^5 + x^3 + x^2 + x + 1`.
pub const FIRE_CODE_POLY: u16 = 0x782F;

/// The `header_firecode` of TS 102 563 clause 5.2: register initialised
/// to all zeros, computed over nine bytes (superframe bytes 2..10).
///
/// The decoder uses the code for error **detection** only. Burst
/// correction (up to 6 bits) is permitted by the standard but not
/// implemented here; the acceptance rows do not need it.
#[must_use]
pub fn fire_code(data: &[u8]) -> u16 {
    let mut register: u16 = 0;
    for byte in data {
        for bit in (0..8).rev() {
            let feedback = ((register >> 15) & 1) ^ u16::from((byte >> bit) & 1);
            register <<= 1;
            if feedback != 0 {
                register ^= FIRE_CODE_POLY;
            }
        }
    }
    register
}
