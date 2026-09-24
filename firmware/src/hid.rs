//! HID report descriptor and report construction for the boot-keyboard profile.
//!
//! The device exposes a single input report (Report ID 1) with the standard
//! 8-byte boot-keyboard layout: `[modifiers, reserved, key0..key5]`. Over GATT
//! the Report characteristic value is prefixed with the Report ID, so the bytes
//! actually notified are `[1, modifiers, reserved, key0..key5]` (9 bytes).

/// Boot-keyboard HID report descriptor (Report ID 1).
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

/// Report ID for the keyboard input report.
pub const REPORT_ID: u8 = 0x01;

/// Length of the GATT Report characteristic value (report ID + 8-byte boot report).
pub const REPORT_LEN: usize = 9;

/// Default physical-key-bit -> HID usage code map (numpad layout).
///
/// Index is the TCA9555 bit (0..15). The physical orientation of the bits is
/// assumed row-major; this map is the single place to adjust once the real
/// key positions are confirmed on hardware.
pub const KEYMAP: [u8; 16] = [
    0x5F, 0x60, 0x61, 0x54, // 7 8 9 /
    0x5C, 0x5D, 0x5E, 0x55, // 4 5 6 *
    0x59, 0x5A, 0x5B, 0x56, // 1 2 3 -
    0x62, 0x63, 0x58, 0x57, // 0 . Enter +
];

/// Build a GATT Report characteristic value from a 16-bit pressed mask.
///
/// Modifier usages (0xE0..=0xE7) set the modifier byte; all other usages fill
/// the six keycode slots in order.
pub fn build_report(pressed: u16) -> [u8; REPORT_LEN] {
    let mut report = [0u8; REPORT_LEN];
    report[0] = REPORT_ID;
    let mut slot = 3; // report[1]=modifiers, report[2]=reserved, report[3..]=keys
    for bit in 0..16 {
        if pressed & (1 << bit) == 0 {
            continue;
        }
        let usage = KEYMAP[bit];
        if (0xE0..=0xE7).contains(&usage) {
            report[1] |= 1 << (usage - 0xE0);
        } else if slot < REPORT_LEN {
            report[slot] = usage;
            slot += 1;
        }
    }
    report
}
