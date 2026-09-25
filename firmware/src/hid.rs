//! Report-protocol HID descriptor and keyboard report construction.
//!
//! The device exposes a single input report (Report ID 1) with the standard
//! 8-byte keyboard layout: `[modifiers, reserved, key0..key5]`. Over GATT the
//! Report ID is carried by the Report Reference descriptor, not the payload.

/// Report-protocol keyboard descriptor (Report ID 1).
///
/// 8 modifier bits, 1 constant byte, 6 keycodes (usage page 0x07).
const KEYBOARD_MAP_BYTES: [u8; KBD_MAP_LEN] = [
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x01, //   Report ID (1)
    0x05, 0x07, //   Usage Page (Keyboard/Keypad)
    0x19, 0xE0, //   Usage Minimum (LeftControl)
    0x29, 0xE7, //   Usage Maximum (RightGUI)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x08, //   Report Count (8)
    0x81, 0x02, //   Input (Data,Var,Abs)
    0x95, 0x01, //   Report Count (1)
    0x75, 0x08, //   Report Size (8)
    0x81, 0x03, //   Input (Cnst,Var,Abs)
    0x95, 0x06, //   Report Count (6)
    0x75, 0x08, //   Report Size (8)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0xFF, //   Logical Maximum (255)
    0x05, 0x07, //   Usage Page (Keyboard/Keypad)
    0x19, 0x00, //   Usage Minimum (0)
    0x29, 0xFF, //   Usage Maximum (255)
    0x81, 0x00, //   Input (Data,Array)
    0xC0, // End Collection
];

/// Keyboard collection only (Report ID 1), for its own USB HID interface.
///
/// One interface per collection is not cosmetic. macOS creates one
/// `IOHIDDevice` per USB HID interface and takes the *first* collection's
/// usage as that device's primary usage. With both collections on one
/// interface the pad shows up as a keyboard only: the consumer pair is still
/// listed in `DeviceUsagePairs`, but the keyboard driver owns the device and
/// never runs volume-key handling over it, so Volume Increment reaches the
/// host and does nothing. Splitting the interfaces is what makes media keys
/// work -- verified against a live macOS host.
pub const KEYBOARD_REPORT_MAP: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x85, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00,
    0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x03, 0x95, 0x06,
    0x75, 0x08, 0x15, 0x00, 0x25, 0xff, 0x05, 0x07, 0x19, 0x00, 0x29, 0xff, 0x81, 0x00, 0xc0,
];

/// Length of the GATT keyboard Report characteristic value (no Report ID
/// prefix): `[modifiers, reserved, key0..key5]`.
pub const REPORT_LEN: usize = 8;

/// Consumer usages this firmware can emit, in the order they become bits of
/// the consumer report.
///
/// The descriptor below is generated from this list, and the web editor must
/// offer exactly these codes: a code that is not here has no bit and would
/// silently do nothing. `tools/webusb-selftest.py` checks the two agree.
/// The controls this pad offers, in mask-bit order.
///
/// Deliberately excludes the Consumer usages that would let a single keypress
/// next to the volume keys shut the host down: `0x30` System Power Down and
/// `0x32` System Sleep both map to real, destructive-by-default key events on
/// Linux (`KEY_POWER`, `KEY_SLEEP`) and do the same on macOS. A numpad should
/// not be able to power off a laptop by accident.
pub const CONSUMER_KEYS: [u8; 14] = [
    0xcd, // Play / Pause
    0xb5, // Scan Next Track
    0xb6, // Scan Previous Track
    0xb7, // Stop
    0xb8, // Eject
    0xe2, // Mute
    0xe9, // Volume Increment
    0xea, // Volume Decrement
    0xbc, // Repeat
    0x40, // Menu
    0x41, // Menu Pick
    0x42, // Menu Up
    0x43, // Menu Down
    0x46, // Menu Escape
];

/// The consumer report is a bitmask over [`CONSUMER_KEYS`], one bit per key,
/// little-endian. Rounded up rather than divided: at 14 keys `/ 8` gives 1,
/// which would not even typecheck against the 2 bytes a 14-bit mask needs, but
/// `div_ceil` is both correct for any key count and says so explicitly.
pub const CONSUMER_REPORT_LEN: usize = CONSUMER_KEYS.len().div_ceil(8);

/// Generate the consumer report descriptor from [`CONSUMER_KEYS`].
///
/// Every key is declared as its own 1-bit variable field. That is deliberate
/// and was settled by testing rather than taste: with a usage *array*
/// (`Usage Minimum 0x00, Usage Maximum 0xFF, Report Size 8`) macOS accepts
/// the descriptor, creates the Consumer Control device, and receives the
/// reports -- and routes none of them. Volume only moved once each usage was
/// declared explicitly as a boolean field, which is also how Apple's own
/// keyboards declare theirs. A host has to be able to see *which* control a
/// bit belongs to; a range it must decode at runtime is not enough.
const fn consumer_report_map() -> [u8; CON_MAP_LEN] {
    let mut out = [0u8; CON_MAP_LEN];
    let mut i = 0usize;
    out[i] = 0x05;
    out[i + 1] = 0x0c;
    i += 2; // Usage Page (Consumer)
    out[i] = 0x09;
    out[i + 1] = 0x01;
    i += 2; // Usage (Consumer Control)
    out[i] = 0xa1;
    out[i + 1] = 0x01;
    i += 2; // Collection (Application)
    out[i] = 0x85;
    out[i + 1] = 0x02;
    i += 2; // Report ID (2)
    let mut k = 0usize;
    while k < CONSUMER_KEYS.len() {
        out[i] = 0x09;
        out[i + 1] = CONSUMER_KEYS[k];
        i += 2; //   Usage
        out[i] = 0x15;
        out[i + 1] = 0x00;
        i += 2; //   Logical Minimum (0)
        out[i] = 0x25;
        out[i + 1] = 0x01;
        i += 2; //   Logical Maximum (1)
        out[i] = 0x75;
        out[i + 1] = 0x01;
        i += 2; //   Report Size (1)
        out[i] = 0x95;
        out[i + 1] = 0x01;
        i += 2; //   Report Count (1)
        out[i] = 0x81;
        out[i + 1] = 0x02;
        i += 2; //   Input (Data,Var,Abs)
        k += 1;
    }
    out[i] = 0xc0; // End Collection
    out
}

/// Consumer Control collection only (Report ID 2), for its own USB interface.
pub const CONSUMER_REPORT_MAP: &[u8] = &CONSUMER_MAP_BYTES;

const CONSUMER_MAP_BYTES: [u8; CON_MAP_LEN] = consumer_report_map();

/// Concatenate two descriptors. Const slice concatenation is not available and
/// `A + B` in a const-generic return type needs the unstable
/// `generic_const_exprs`, so the sizes are named constants instead.
const KBD_MAP_LEN: usize = 47;
const CON_MAP_LEN: usize = 8 + 12 * CONSUMER_KEYS.len() + 1;

const fn concat_maps(
    a: &[u8; KBD_MAP_LEN],
    b: &[u8; CON_MAP_LEN],
) -> [u8; KBD_MAP_LEN + CON_MAP_LEN] {
    let mut out = [0u8; KBD_MAP_LEN + CON_MAP_LEN];
    let mut i = 0usize;
    while i < KBD_MAP_LEN {
        out[i] = a[i];
        i += 1;
    }
    let mut j = 0usize;
    while j < CON_MAP_LEN {
        out[KBD_MAP_LEN + j] = b[j];
        j += 1;
    }
    out
}

/// The full report map BLE HOGP advertises: one map covering every report ID,
/// so it is the two USB interface descriptors concatenated. Derived, so it
/// cannot drift from what USB actually declares.
pub const REPORT_MAP: &[u8] = &concat_maps(&KEYBOARD_MAP_BYTES, &CONSUMER_MAP_BYTES);

/// Report IDs, as declared in [`REPORT_MAP`].
pub const KEYBOARD_REPORT_ID: u8 = 1;
pub const CONSUMER_REPORT_ID: u8 = 2;

/// USB carries the report ID in-band, unlike the GATT characteristic.
#[must_use]
pub fn usb_report(report: [u8; REPORT_LEN]) -> [u8; REPORT_LEN + 1] {
    let mut packet = [0; REPORT_LEN + 1];
    packet[0] = KEYBOARD_REPORT_ID;
    packet[1..].copy_from_slice(&report);
    packet
}

/// USB-prefixed consumer report (Report ID 2).
#[must_use]
pub fn usb_consumer_report(report: [u8; CONSUMER_REPORT_LEN]) -> [u8; CONSUMER_REPORT_LEN + 1] {
    let mut packet = [0; CONSUMER_REPORT_LEN + 1];
    packet[0] = CONSUMER_REPORT_ID;
    packet[1..].copy_from_slice(&report);
    packet
}

/// Build the keyboard and consumer reports from a 16-bit pressed mask, a
/// per-key usage map and the mask of which keys are consumer keys.
///
/// Keys whose bit is set in `consumer_mask` emit on the consumer page and are
/// kept out of the keyboard report entirely; the rest go to the keyboard
/// report as before.
///
/// The consumer report is a bitmask over [`CONSUMER_KEYS`], so unlike a bare
/// usage code it can carry several media keys at once. A consumer key mapped to
/// a code outside `CONSUMER_KEYS` contributes nothing -- there is no bit for it.
#[must_use]
pub fn build_reports(
    pressed: u16,
    keymap: &[u8; 16],
    consumer_mask: u16,
) -> ([u8; REPORT_LEN], [u8; CONSUMER_REPORT_LEN]) {
    let mut kbd = [0u8; REPORT_LEN];
    let mut consumer_bits = 0u16;
    let mut slot = 2; // kbd[0]=modifiers, kbd[1]=reserved, kbd[2..]=keys
    for (bit, &usage) in keymap.iter().enumerate() {
        if pressed & (1 << bit) == 0 {
            continue;
        }
        if consumer_mask & (1 << bit) != 0 {
            if let Some(pos) = CONSUMER_KEYS.iter().position(|&k| k == usage) {
                consumer_bits |= 1 << pos;
            }
            continue;
        }
        if (0xE0..=0xE7).contains(&usage) {
            kbd[0] |= 1 << (usage - 0xE0);
        } else if usage != 0 && !kbd[2..slot].contains(&usage) && slot < REPORT_LEN {
            kbd[slot] = usage;
            slot += 1;
        }
    }
    (kbd, consumer_bits.to_le_bytes())
}

/// Keyboard-only build, for callers with no consumer keys. Kept so the
/// keyboard behaviour tests below exercise the same code the device runs.
#[must_use]
pub fn build_report(pressed: u16, keymap: &[u8; 16]) -> [u8; REPORT_LEN] {
    build_reports(pressed, keymap, 0).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_report_prefix_preserves_modifiers_and_release() {
        assert_eq!(
            usb_report([2, 0, 0x59, 0, 0, 0, 0, 0]),
            [1, 2, 0, 0x59, 0, 0, 0, 0, 0]
        );
        assert_eq!(usb_report([0; REPORT_LEN]), [1, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn gatt_report_has_no_id_prefix_and_release_clears_all_bytes() {
        let keymap = [0x5f; 16];
        assert_eq!(build_report(1, &keymap), [0, 0, 0x5f, 0, 0, 0, 0, 0]);
        assert_eq!(build_report(0, &keymap), [0; 8]);
    }

    #[test]
    fn modifiers_and_six_keys_have_distinct_slots() {
        let mut keymap = [0; 16];
        keymap[..8].copy_from_slice(&[0xe1, 0xe4, 4, 5, 6, 7, 8, 9]);
        assert_eq!(build_report(0xff, &keymap), [0x12, 0, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn disabled_keys_do_not_hide_an_enabled_key() {
        let mut keymap = [0; 16];
        keymap[6] = 0x59;
        assert_eq!(build_report(0x7f, &keymap), [0, 0, 0x59, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn duplicate_mappings_use_one_slot_and_remain_pressed_until_all_release() {
        let mut keymap = [4; 16];
        keymap[6] = 5;
        assert_eq!(build_report(0x7f, &keymap), [0, 0, 4, 5, 0, 0, 0, 0]);
        assert_eq!(build_report(3, &keymap), [0, 0, 4, 0, 0, 0, 0, 0]);
        assert_eq!(build_report(2, &keymap), [0, 0, 4, 0, 0, 0, 0, 0]);
        assert_eq!(build_report(0, &keymap), [0; 8]);
    }

    #[test]
    fn modifiers_survive_full_report_and_duplicate_modifier_mappings() {
        let keymap = [4, 5, 6, 7, 8, 9, 0xe1, 0xe1, 0xe4, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(build_report(0x1ff, &keymap), [0x12, 0, 4, 5, 6, 7, 8, 9]);
    }

    /// Expected consumer report for a set of pressed usages, derived from
    /// `CONSUMER_KEYS` rather than hardcoded, so the tests follow the table the
    /// descriptor is generated from instead of drifting from it.
    fn consumer_report_for(usages: &[u8]) -> [u8; CONSUMER_REPORT_LEN] {
        let mut bits = 0u16;
        for &u in usages {
            let pos = CONSUMER_KEYS
                .iter()
                .position(|&k| k == u)
                .unwrap_or_else(|| panic!("{u:#04x} is not a supported consumer usage"));
            bits |= 1 << pos;
        }
        bits.to_le_bytes()
    }

    #[test]
    fn a_consumer_key_goes_to_the_consumer_report_and_not_the_keyboard() {
        let mut keymap = [0x5f; 16];
        keymap[3] = 0xe9; // Volume Increment on the `/` key
        let (kbd, cons) = build_reports(1 << 3, &keymap, 1 << 3);
        assert_eq!(cons, consumer_report_for(&[0xe9]));
        assert_eq!(
            kbd, [0; REPORT_LEN],
            "consumer key must not appear as a keyboard usage"
        );
    }

    #[test]
    fn consumer_and_keyboard_keys_report_side_by_side() {
        let mut keymap = [0x5f; 16];
        keymap[0] = 0xe9; // volume up
        keymap[1] = 0xe0; // left ctrl, keyboard modifier
        keymap[2] = 0x04; // 'A'
        let mask = 0b001;
        let (kbd, cons) = build_reports(0b111, &keymap, mask);
        assert_eq!(cons, consumer_report_for(&[0xe9]));
        assert_eq!(kbd, [0x01, 0, 0x04, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn releasing_a_consumer_key_zeroes_its_report() {
        let mut keymap = [0x5f; 16];
        keymap[5] = 0xcd; // Play/Pause
        assert_eq!(
            build_reports(1 << 5, &keymap, 1 << 5).1,
            consumer_report_for(&[0xcd])
        );
        assert_eq!(build_reports(0, &keymap, 1 << 5).1, [0x00, 0x00]);
    }

    /// The report is a bitmask, so two media keys held together report both --
    /// which a single-usage report could not do.
    #[test]
    fn simultaneous_consumer_keys_report_both() {
        let mut keymap = [0x5f; 16];
        keymap[2] = 0xe9; // volume up
        keymap[7] = 0xcd; // play/pause
        let (_, cons) = build_reports(0b1000_0100, &keymap, 0b1000_0100);
        assert_eq!(cons, consumer_report_for(&[0xe9, 0xcd]));
        assert_ne!(cons, consumer_report_for(&[0xe9]));
    }

    /// A usage the descriptor never declares has no bit, so it must contribute
    /// nothing rather than landing on some other control.
    #[test]
    fn an_undeclared_consumer_usage_emits_nothing() {
        let mut keymap = [0x5f; 16];
        keymap[1] = 0x8a; // a valid u8 that is not in CONSUMER_KEYS (the AL
        // codes like Calculator 0x18A cannot even be stored in the keymap)
        let (_, cons) = build_reports(1 << 1, &keymap, 1 << 1);
        assert_eq!(cons, [0x00, 0x00]);
    }

    /// A config saved before Power/Sleep were withdrawn still has the mask bit
    /// and the usage byte. It must go inert, not leak into the keyboard report
    /// as a stray keystroke -- `continue` in `build_reports` is what makes that
    /// true, and this pins it down.
    #[test]
    fn a_previously_assigned_power_key_is_inert_not_a_stray_keystroke() {
        for usage in [0x30u8, 0x32] {
            let mut keymap = [0x5f; 16];
            keymap[4] = usage;
            let (kbd, cons) = build_reports(1 << 4, &keymap, 1 << 4);
            assert_eq!(
                cons,
                [0x00, 0x00],
                "{usage:#04x} must not emit a consumer bit"
            );
            assert_eq!(
                kbd, [0; REPORT_LEN],
                "{usage:#04x} must not fall through to the keyboard report"
            );
        }
    }

    /// Every key the descriptor declares must be reachable, and each must land
    /// on a distinct bit.
    #[test]
    fn every_declared_consumer_key_maps_to_its_own_bit() {
        let mut seen = 0u16;
        for (i, &usage) in CONSUMER_KEYS.iter().enumerate() {
            let mut keymap = [0x5f; 16];
            keymap[0] = usage;
            let (_, cons) = build_reports(1, &keymap, 1);
            let bits = u16::from_le_bytes(cons);
            assert_eq!(bits, 1 << i, "usage {usage:#04x} should set bit {i}");
            assert_eq!(bits & seen, 0, "usage {usage:#04x} reuses a bit");
            seen |= bits;
        }
        assert_eq!(
            seen,
            (1u16 << CONSUMER_KEYS.len()) - 1,
            "every declared key should be reachable and no bit left unreachable"
        );
    }

    /// A key mapped to a media code is only a media key if its mask bit is set.
    /// Without the mask it is an ordinary keyboard usage, which is what keeps a
    /// v1 config behaving exactly as it did before.
    #[test]
    fn a_media_code_without_its_mask_bit_is_an_ordinary_keyboard_usage() {
        let mut keymap = [0x5f; 16];
        keymap[3] = 0xe9;
        let (kbd, cons) = build_reports(1 << 3, &keymap, 0);
        assert_eq!(cons, [0x00, 0x00]);
        assert_eq!(kbd[2], 0xe9);
    }

    #[test]
    fn a_consumer_key_mapped_to_zero_stays_inert() {
        let mut keymap = [0x5f; 16];
        keymap[4] = 0x00; // mapped to nothing
        let (kbd, cons) = build_reports(1 << 4, &keymap, 1 << 4);
        assert_eq!(
            cons,
            [0x00, 0x00],
            "usage 0 means nothing, not a spurious press"
        );
        assert_eq!(kbd, [0; REPORT_LEN]);
    }

    #[test]
    fn usb_consumer_report_carries_report_id_2() {
        // Volume Increment is CONSUMER_KEYS[6], so bit 6 in the low byte.
        assert_eq!(usb_consumer_report([1 << 6, 0]), [2, 0x40, 0x00]);
        assert_eq!(usb_consumer_report([0, 0]), [2, 0x00, 0x00]);
        // and the keyboard report still carries ID 1
        assert_eq!(usb_report([0; REPORT_LEN])[0], KEYBOARD_REPORT_ID);
    }

    /// The two per-interface USB descriptors must reassemble into exactly the
    /// map BLE advertises. They are written out separately because const slice
    /// indexing is not stable, which means nothing else stops them drifting.
    #[test]
    fn the_split_usb_descriptors_reassemble_into_the_ble_map() {
        let joined: Vec<u8> = KEYBOARD_REPORT_MAP
            .iter()
            .chain(CONSUMER_REPORT_MAP.iter())
            .copied()
            .collect();
        assert_eq!(joined, REPORT_MAP);
    }

    /// Each USB descriptor must be self-contained: its own page, its own report
    /// ID, and a matching End Collection. A dangling collection here makes the
    /// host reject the whole interface.
    #[test]
    fn each_usb_descriptor_is_a_complete_self_contained_collection() {
        let ids = |m: &[u8]| -> Vec<u8> {
            m.windows(2)
                .filter(|w| w[0] == 0x85)
                .map(|w| w[1])
                .collect()
        };
        assert_eq!(&KEYBOARD_REPORT_MAP[..2], &[0x05, 0x01]);
        assert_eq!(KEYBOARD_REPORT_MAP.last(), Some(&0xc0));
        assert_eq!(ids(KEYBOARD_REPORT_MAP), vec![KEYBOARD_REPORT_ID]);

        assert_eq!(&CONSUMER_REPORT_MAP[..2], &[0x05, 0x0c]);
        assert_eq!(CONSUMER_REPORT_MAP.last(), Some(&0xc0));
        assert_eq!(ids(CONSUMER_REPORT_MAP), vec![CONSUMER_REPORT_ID]);

        // Neither may declare the other's report ID, or the host could route a
        // report to the wrong interface.
        assert!(!ids(CONSUMER_REPORT_MAP).contains(&KEYBOARD_REPORT_ID));
        assert!(!ids(KEYBOARD_REPORT_MAP).contains(&CONSUMER_REPORT_ID));
    }

    /// The descriptor must declare both report IDs, or a host will drop the
    /// consumer report as undeclared.
    #[test]
    fn descriptor_declares_both_report_ids() {
        let has = |id: u8| REPORT_MAP.windows(2).any(|w| w[0] == 0x85 && w[1] == id);
        assert!(has(KEYBOARD_REPORT_ID));
        assert!(has(CONSUMER_REPORT_ID));
        // and the consumer collection is on usage page 0x0C
        assert!(REPORT_MAP.windows(2).any(|w| w[0] == 0x05 && w[1] == 0x0C));
    }
}
