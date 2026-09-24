//! Report-protocol HID descriptor and keyboard report construction.
//!
//! The device exposes a single input report (Report ID 1) with the standard
//! 8-byte keyboard layout: `[modifiers, reserved, key0..key5]`. Over GATT the
//! Report ID is carried by the Report Reference descriptor, not the payload.

/// Report-protocol keyboard descriptor (Report ID 1).
///
/// 8 modifier bits, 1 constant byte, 6 keycodes (usage page 0x07).
pub const REPORT_MAP: &[u8] = &[
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

/// Length of the GATT Report characteristic value (without a Report ID prefix).
pub const REPORT_LEN: usize = 8;

/// USB carries the report ID in-band, unlike the GATT characteristic.
#[must_use]
pub fn usb_report(report: [u8; REPORT_LEN]) -> [u8; REPORT_LEN + 1] {
    let mut packet = [0; REPORT_LEN + 1];
    packet[0] = 1;
    packet[1..].copy_from_slice(&report);
    packet
}

/// Build a GATT Report characteristic value from a 16-bit pressed mask and a
/// per-key HID usage map (index = physical key bit).
///
/// Modifier usages (0xE0..=0xE7) set the modifier byte; all other usages fill
/// the six keycode slots in order, ignoring disabled and duplicate mappings.
#[must_use]
pub fn build_report(pressed: u16, keymap: &[u8; 16]) -> [u8; REPORT_LEN] {
    let mut report = [0u8; REPORT_LEN];
    let mut slot = 2; // report[0]=modifiers, report[1]=reserved, report[2..]=keys
    for (bit, &usage) in keymap.iter().enumerate() {
        if pressed & (1 << bit) == 0 {
            continue;
        }
        if (0xE0..=0xE7).contains(&usage) {
            report[0] |= 1 << (usage - 0xE0);
        } else if usage != 0 && !report[2..slot].contains(&usage) && slot < REPORT_LEN {
            report[slot] = usage;
            slot += 1;
        }
    }
    report
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
}
