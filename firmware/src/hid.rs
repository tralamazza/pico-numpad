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
    //
    // Second collection: Consumer Control (media keys), Report ID 2.
    //
    // A separate collection and report ID are required because consumer usages
    // live on usage page 0x0C, not 0x07. They cannot be mixed into the keyboard
    // report -- a host decodes a report by its ID against this descriptor, so
    // putting a 0x0C usage in the keyboard report would be read as a keyboard
    // usage and land on the wrong key.
    //
    // One byte carries one consumer usage at a time, which matches how these
    // keys are used (volume up, play/pause) and keeps the report small. The
    // usage range is declared 0x00..=0xFF rather than an explicit list so the
    // editor can offer any control in that range without a descriptor change.
    //
    // The 8-bit width is not arbitrary: the config stores one byte per key, so
    // a consumer code above 0xFF is not expressible. That excludes the AL
    // application-launch usages (0x18A Calculator, 0x196 Internet Browser,
    // and friends), which are 16-bit. Widening this report alone would not
    // unlock them; the keymap would have to widen with it.
    0x05, 0x0C, // Usage Page (Consumer)
    0x09, 0x01, // Usage (Consumer Control)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x02, //   Report ID (2)
    0x19, 0x00, //   Usage Minimum (0)
    0x29, 0xFF, //   Usage Maximum (255)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0xFF, //   Logical Maximum (255)
    0x75, 0x08, //   Report Size (8)
    0x95, 0x01, //   Report Count (1)
    0x81, 0x00, //   Input (Data,Array)
    0xC0, // End Collection
];

/// Length of the GATT Report characteristic value (without a Report ID prefix).
pub const REPORT_LEN: usize = 8;

/// Length of the consumer (media) input report. One usage per report.
pub const CONSUMER_REPORT_LEN: usize = 1;

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
/// Keys whose bit is set in `consumer_mask` emit `keymap[bit]` on the consumer
/// page and are kept out of the keyboard report entirely; the rest go to the
/// keyboard report as before.
///
/// Only one consumer key is reported at a time -- the first pressed one wins.
/// A real consumer-control report is a set of usages, but every common media
/// key is used singly, and one byte keeps the report cheap over BLE.
#[must_use]
pub fn build_reports(
    pressed: u16,
    keymap: &[u8; 16],
    consumer_mask: u16,
) -> ([u8; REPORT_LEN], [u8; CONSUMER_REPORT_LEN]) {
    let mut kbd = [0u8; REPORT_LEN];
    let mut cons = [0u8; CONSUMER_REPORT_LEN];
    let mut slot = 2; // kbd[0]=modifiers, kbd[1]=reserved, kbd[2..]=keys
    for (bit, &usage) in keymap.iter().enumerate() {
        if pressed & (1 << bit) == 0 {
            continue;
        }
        if consumer_mask & (1 << bit) != 0 {
            // Usage 0 means "nothing", so a consumer key mapped to 0 is inert
            // rather than emitting a spurious usage.
            if cons[0] == 0 && usage != 0 {
                cons[0] = usage;
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
    (kbd, cons)
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

    #[test]
    fn a_consumer_key_goes_to_the_consumer_report_and_not_the_keyboard() {
        let mut keymap = [0x5f; 16];
        keymap[3] = 0xe9; // Volume Increment on the `/` key
        let (kbd, cons) = build_reports(1 << 3, &keymap, 1 << 3);
        assert_eq!(cons, [0xe9]);
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
        assert_eq!(cons, [0xe9]);
        assert_eq!(kbd, [0x01, 0, 0x04, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn releasing_a_consumer_key_zeroes_its_report() {
        let mut keymap = [0x5f; 16];
        keymap[5] = 0xcd; // Play/Pause
        assert_eq!(build_reports(1 << 5, &keymap, 1 << 5).1, [0xcd]);
        assert_eq!(build_reports(0, &keymap, 1 << 5).1, [0x00]);
    }

    /// Only one consumer usage fits the report, so the lowest pressed bit wins.
    /// Documented behaviour, not a bug: simultaneous media keys are not a real
    /// interaction.
    #[test]
    fn simultaneous_consumer_keys_report_the_lowest_bit() {
        let mut keymap = [0x5f; 16];
        keymap[2] = 0xe9; // volume up
        keymap[7] = 0xcd; // play/pause
        let (_, cons) = build_reports(0b1000_0100, &keymap, 0b1000_0100);
        assert_eq!(cons, [0xe9]);
    }

    /// A key mapped to a media code is only a media key if its mask bit is set.
    /// Without the mask it is an ordinary keyboard usage, which is what keeps a
    /// v1 config behaving exactly as it did before.
    #[test]
    fn a_media_code_without_its_mask_bit_is_an_ordinary_keyboard_usage() {
        let mut keymap = [0x5f; 16];
        keymap[3] = 0xe9;
        let (kbd, cons) = build_reports(1 << 3, &keymap, 0);
        assert_eq!(cons, [0x00]);
        assert_eq!(kbd[2], 0xe9);
    }

    #[test]
    fn a_consumer_key_mapped_to_zero_stays_inert() {
        let mut keymap = [0x5f; 16];
        keymap[4] = 0x00; // mapped to nothing
        let (kbd, cons) = build_reports(1 << 4, &keymap, 1 << 4);
        assert_eq!(cons, [0x00], "usage 0 means nothing, not a spurious press");
        assert_eq!(kbd, [0; REPORT_LEN]);
    }

    #[test]
    fn usb_consumer_report_carries_report_id_2() {
        assert_eq!(usb_consumer_report([0xe9]), [2, 0xe9]);
        assert_eq!(usb_consumer_report([0x00]), [2, 0x00]);
        // and the keyboard report still carries ID 1
        assert_eq!(usb_report([0; REPORT_LEN])[0], KEYBOARD_REPORT_ID);
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
