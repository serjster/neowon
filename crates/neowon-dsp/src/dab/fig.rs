//! FIG parsing: ensemble identity, sub-channels, services and their
//! labels.
//!
//! Clauses cited are from **ETSI EN 300 401 V2.1.1 (2017-01)**:
//!
//! - **5.2.2.1** FIG type 0's first byte is `C/N | OE | P/D | extension (5 bits)`.
//! - **6.4.1** FIG 0/0: ensemble information, `EId` in the first 16 bits.
//! - **6.2.1** FIG 0/1: sub-channel organization, 3-byte (UEP) or 4-byte (EEP)
//!   entries, several per FIG.
//! - **6.3.1** FIG 0/2: basic service and service component definition.
//! - **5.2.2.2, 8.1.13, 8.1.14.1** FIG type 1: `Charset (4) | Rfu (1) |
//!   extension (3)`, then a 16-bit identifier, the 16-byte character field, and
//!   a 16-bit character flag. Extension 0 is the **ensemble** label (identifier
//!   = `EId`), extension 1 the **programme service** label (identifier = `SId`)
//!   — table 4 is explicit about those two.
//!
//! Data services (`P/D = 1`, 32-bit `SId`) are counted but not tabled. A
//! short-form sub-channel's table index is resolved through table 8 (clause 11.3.1), so
//! UEP sub-channels carry their exact size and bit rate like EEP ones.

use super::{Protection, Service, SubChannel};

/// One capacity unit is 64 bits, and a CIF carries 864 of them in 24 ms
/// (clauses 5.1, 13), so a size in CUs fixes the bit rate exactly.
fn bitrate_kbps(size_cu: u16) -> f64 {
    size_cu as f64 * 64.0 / 0.024 / 1000.0
}

/// FIG type 0 — service and ensemble organization.
///
/// `data[0]` is the type 0 flags byte (`C/N | OE | P/D | extension`), the rest
/// is the extension's own data field.
pub fn fig0(ensemble: &mut super::Ensemble, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let extension = data[0] & 0x1F;
    let pd = (data[0] >> 5) & 1;
    let payload = &data[1..];
    match extension {
        0 => fig0_ensemble(ensemble, payload),
        1 => fig0_subchannel(ensemble, payload),
        2 => fig0_service(ensemble, payload, pd == 1),
        // Other extensions (time/date, programme type, region, CA ...) are
        // legal and simply not decoded here.
        _ => {}
    }
}

/// FIG 0/0 — ensemble information (clause 6.4.1): the `EId` is the first 16 bits.
fn fig0_ensemble(ensemble: &mut super::Ensemble, payload: &[u8]) {
    if payload.len() < 2 {
        return;
    }
    ensemble.eid = Some(((payload[0] as u16) << 8) | payload[1] as u16);
}

/// FIG 0/1 — sub-channel organization (clause 6.2.1).
///
/// Entries are 3 bytes (short form, table index) or 4 bytes (long form, explicit
/// size and EEP), and one FIG may carry several; the form bit in the third byte
/// says which, so the walk can always advance.
///
/// A short form's index is resolved through clause 11.3.1's table 8, so a UEP
/// sub-channel reports its true size and bit rate rather than an index with no
/// meaning. Table switch 1 is reserved by the
/// standard, so nothing is resolved for it — the index is still reported.
fn fig0_subchannel(ensemble: &mut super::Ensemble, payload: &[u8]) {
    let mut pos = 0usize;
    while pos + 2 < payload.len() {
        let id = (payload[pos] >> 2) & 0x3F;
        let start_cu = (((payload[pos] & 0x03) as u16) << 8) | payload[pos + 1] as u16;
        let form = (payload[pos + 2] >> 7) & 1;
        let (protection, size_cu, bitrate_kbps, consumed) = if form == 0 {
            // Short form: table switch (bit 6) + 6-bit table index.
            let table_switch = (payload[pos + 2] >> 6) & 1;
            let table_index = payload[pos + 2] & 0x3F;
            let resolved = if table_switch == 0 {
                super::fec::uep_profile(table_index)
            } else {
                None
            };
            (
                Protection::Uep { table_index },
                resolved.map(|p| p.size_cu),
                resolved.map(|p| f64::from(p.bitrate_kbps)),
                3,
            )
        } else {
            // Long form: 3-bit option, 2-bit level, 10-bit size.
            let option = (payload[pos + 2] >> 4) & 0x07;
            let level = (payload[pos + 2] >> 2) & 0x03;
            let size = (((payload[pos + 2] & 0x03) as u16) << 8) | payload[pos + 3] as u16;
            let size = Some(size);
            (
                Protection::Eep { option, level },
                size,
                size.map(bitrate_kbps),
                4,
            )
        };
        ensemble.sub_channels.insert(
            id,
            SubChannel {
                id,
                start_cu,
                size_cu,
                protection,
                bitrate_kbps,
            },
        );
        pos += consumed;
    }
}

/// FIG 0/2 — basic service and service component definition (clause 6.3.1).
///
/// `Rfa (1) | CAId (3) | number of components (4)`, then one 16-bit component
/// description per component: `TMId (2) | ASCTy/DSCTy (6) | SubChId (6) |
/// primary (1) | CA (1)`.
fn fig0_service(ensemble: &mut super::Ensemble, payload: &[u8], data_service: bool) {
    let sid_len = if data_service { 4 } else { 2 };
    if payload.len() < sid_len + 1 {
        return;
    }
    // Programme services are tabled; data services are counted so the
    // readout can say they exist without pretending to have decoded them.
    if data_service {
        ensemble.data_services += 1;
        return;
    }
    let sid = ((payload[0] as u16) << 8) | payload[1] as u16;
    let component_count = (payload[2] & 0x0F) as usize;
    let pos = sid_len + 1;

    let service = ensemble.services.entry(sid).or_insert_with(|| Service {
        sid,
        ..Service::default()
    });

    for component in 0..component_count {
        let at = pos + component * 2;
        if at + 1 >= payload.len() {
            break;
        }
        let tmid = (payload[at] >> 6) & 0x03;
        let ascty = payload[at] & 0x3F;
        let sub_channel = (payload[at + 1] >> 2) & 0x3F;
        let primary = (payload[at + 1] >> 1) & 1;
        // TMId 0 is an MSC stream audio component (clause 8.1.14/table 33).
        // Only the primary audio component names the service's sub-channel.
        if tmid == 0 {
            service.has_audio = true;
            if primary == 1 || service.sub_channel.is_none() {
                service.sub_channel = Some(sub_channel);
                service.ascty = Some(ascty);
            }
        }
    }
}

/// FIG type 1 — labels (clause 5.2.2.2). Extension 0 is the ensemble label,
/// extension 1 the programme service label (table 4).
///
/// `data[0]` is `Charset (4) | Rfu (1) | extension (3)`.
pub fn fig1(ensemble: &mut super::Ensemble, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let charset = (data[0] >> 4) & 0x0F;
    let extension = data[0] & 0x07;
    let payload = &data[1..];
    if payload.len() < 2 + 16 {
        return;
    }
    let identifier = ((payload[0] as u16) << 8) | payload[1] as u16;
    let label = label(charset, &payload[2..18]);
    if label.is_empty() {
        return;
    }
    match extension {
        // The ensemble label is referenced by the EId.
        0 if ensemble.eid == Some(identifier) || ensemble.eid.is_none() => {
            ensemble.label = Some(label);
        }
        1 => {
            if let Some(service) = ensemble.services.get_mut(&identifier) {
                service.label = Some(label);
            }
        }
        _ => {}
    }
}

/// Decode a 16-byte DAB character field.
///
/// The field is filled from the start and padded with `0x00` (clause 5.2.2.2);
/// some broadcasters pad with spaces instead, which receivers trim too.
///
/// Charset 0 (complete EBU Latin, table 47 of the standard's label clause) and
/// charset 15 (UTF-8) decode exactly. Any other charset is not transcribed, so
/// its bytes are not guessed: printable ASCII passes and the rest is `?`.
fn label(charset: u8, bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .rposition(|b| !matches!(*b, 0x00 | 0x20 | 0xFF))
        .map(|i| i + 1)
        .unwrap_or(0);
    let bytes = &bytes[..end];
    if bytes.is_empty() {
        return String::new();
    }
    super::charset::decode(bytes, charset).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::Ensemble;
    use crate::dab::fib::make_fib;

    /// A realistic FIB: FIG 0/0 (EId), FIG 0/1 (one EEP sub-channel) and
    /// FIG 0/2 (one audio service using it). Each FIG's declared length covers
    /// its flags byte (clause 5.2.2.1).
    fn build_fib() -> [u8; 32] {
        let mut data = [0u8; 30];
        let mut pos = 0;
        // FIG 0/0, length 3: flags byte + EId 0xF044.
        data[pos] = 3; // FIG type 0, length 3
        data[pos + 1] = 0x00; // C/N 0, OE 0, P/D 0, extension 0
        data[pos + 2] = 0xF0;
        data[pos + 3] = 0x44;
        pos += 4;
        // FIG 0/1, length 5: flags byte + one 4-byte EEP entry
        // (sub-channel 5, start CU 128, option 0, level 2 = 3, size 96 CU).
        data[pos] = 5; // FIG type 0, length 5
        data[pos + 1] = 0x01; // extension 1
        data[pos + 2] = 5 << 2; // SubChId, start address high bits 0
        data[pos + 3] = 0x80;
        data[pos + 4] = 0x80 | (2 << 2); // long form, option 0, level 2, size high bits 0
        data[pos + 5] = 96;
        pos += 6;
        // FIG 0/2, length 6: flags byte + SId 0x1001 + one component
        // (TMId 0 = audio, ASCTy 63 = DAB+, sub-channel 5, primary).
        data[pos] = 6; // FIG type 0, length 6
        data[pos + 1] = 0x02; // extension 2
        data[pos + 2] = 0x10;
        data[pos + 3] = 0x01;
        data[pos + 4] = 0x01; // Rfa 0, CAId 0, one component
        data[pos + 5] = 63; // TMId 0, ASCTy 63
        data[pos + 6] = (5 << 2) | (1 << 1); // SubChId 5, primary
        make_fib(&data)
    }

    #[test]
    fn ensemble_subchannel_and_service_are_parsed() {
        let fib = build_fib();
        let mut ensemble = Ensemble::default();
        assert_eq!(crate::dab::walk_figs(&fib, &mut ensemble), 3);
        assert_eq!(ensemble.eid, Some(0xF044));
        assert!(ensemble.data_services == 0);
        let sc = ensemble.sub_channels.get(&5).expect("sub-channel 5");
        assert_eq!(sc.start_cu, 128);
        assert_eq!(sc.size_cu, Some(96));
        assert_eq!(
            sc.protection,
            Protection::Eep {
                option: 0,
                level: 2
            }
        );
        let kbps = sc.bitrate_kbps.expect("EEP size is explicit");
        assert!((kbps - 256.0).abs() < 1e-9, "{kbps} kbit/s");
        let service = ensemble.services.get(&0x1001).expect("service");
        assert_eq!(service.sub_channel, Some(5));
        assert_eq!(service.ascty, Some(63));
        assert!(service.has_audio);
        assert_eq!(service.coding_label(), "DAB+ (HE-AAC v2)");
    }

    /// The labels: FIG 1/0 names the ensemble, FIG 1/1 the service.
    #[test]
    fn labels_land_on_the_right_objects() {
        let mut ensemble = Ensemble {
            eid: Some(0xF044),
            ..Default::default()
        };
        ensemble.services.insert(
            0x1001,
            crate::dab::Service {
                sid: 0x1001,
                ..Default::default()
            },
        );

        // FIG 1/0, charset 0: EId 0xF044, label with a non-ASCII byte.
        let mut data = vec![0x00]; // charset 0, extension 0
        data.extend_from_slice(&[0xF0, 0x44]);
        data.extend_from_slice(b"M");
        data.push(0xC2); // EBU Latin 'E' with acute: exact through table 47
        data.extend_from_slice(b"tropolitain");
        data.resize(1 + 2 + 16, 0x00);
        fig1(&mut ensemble, &data);
        assert_eq!(ensemble.label.as_deref(), Some("MÉtropolitain"));

        // FIG 1/1, charset 0: SId 0x1001, label "FRANCE INTER".
        let mut data = vec![0x01]; // charset 0, extension 1
        data.extend_from_slice(&[0x10, 0x01]);
        data.extend_from_slice(b"FRANCE INTER");
        data.resize(1 + 2 + 16, 0x20);
        fig1(&mut ensemble, &data);
        assert_eq!(
            ensemble.services[&0x1001].label.as_deref(),
            Some("FRANCE INTER")
        );
    }

    /// Padding is trimmed whether it is the standard's 0x00 or a broadcaster's
    /// spaces.
    #[test]
    fn label_padding_is_trimmed() {
        let mut with_zero = vec![b'A', b'B', 0x00, 0x00];
        assert_eq!(label(0, &with_zero), "AB");
        with_zero = vec![b'A', b'B', 0x20, 0x20];
        assert_eq!(label(0, &with_zero), "AB");
        assert_eq!(label(0, &[0x00; 16]), "");
    }

    /// A UEP sub-channel resolves its table-8 index into an exact size and bit
    /// rate; the index itself is still reported, because that is what the FIC
    /// signalled.
    #[test]
    fn uep_index_resolves_through_table_8() {
        let mut data = [0u8; 30];
        data[0] = 4; // FIG type 0, length = flags + one 3-byte entry
        data[1] = 0x01; // extension 1
        data[2] = 7 << 2; // SubChId 7
        data[3] = 10;
        data[4] = 5; // short form, table switch 0, index 5
        let fib = make_fib(&data);
        let mut ensemble = Ensemble::default();
        crate::dab::walk_figs(&fib, &mut ensemble);
        let sc = ensemble.sub_channels.get(&7).expect("sub-channel");
        assert_eq!(sc.protection, Protection::Uep { table_index: 5 });
        // Table 8 index 5: 48 kbit/s, level 5, 24 CUs.
        assert_eq!(sc.size_cu, Some(24));
        assert_eq!(sc.bitrate_kbps, Some(48.0));
        assert_eq!(sc.protection.label(), "UEP index 5");
    }

    /// Table switch 1 is reserved by the standard (clause 6.2.1), so the index
    /// is reported but nothing is resolved through table 8.
    #[test]
    fn reserved_table_switch_resolves_nothing() {
        let mut data = [0u8; 30];
        data[0] = 4;
        data[1] = 0x01;
        data[2] = 7 << 2;
        data[3] = 10;
        data[4] = 0x40 | 5; // short form, table switch 1, index 5
        let fib = make_fib(&data);
        let mut ensemble = Ensemble::default();
        crate::dab::walk_figs(&fib, &mut ensemble);
        let sc = ensemble.sub_channels.get(&7).expect("sub-channel");
        assert_eq!(sc.protection, Protection::Uep { table_index: 5 });
        assert_eq!(sc.size_cu, None);
        assert_eq!(sc.bitrate_kbps, None);
    }

    /// A data service (P/D = 1, 32-bit SId) is counted, not tabled.
    #[test]
    fn data_services_are_counted_not_tabled() {
        let mut data = [0u8; 30];
        data[0] = 7; // FIG type 0, length 7
        data[1] = 0x22; // extension 2, P/D 1
        data[2] = 0x12;
        data[3] = 0x34;
        data[4] = 0x56;
        data[5] = 0x78;
        data[6] = 0x01; // one component
        data[7] = 0x00;
        data[8] = 0x05;
        let fib = make_fib(&data);
        let mut ensemble = Ensemble::default();
        crate::dab::walk_figs(&fib, &mut ensemble);
        assert_eq!(ensemble.data_services, 1);
        assert!(ensemble.services.is_empty());
    }
}
