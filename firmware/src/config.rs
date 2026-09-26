//! Runtime configuration: key remap + LED settings, shared between the BLE key
//! loop (reader) and the USB config handler (writer).
//!
//! The config is serialised to a fixed 32-byte little-endian record so it can be
//! shipped over USB and stored in flash without a heap or serde.
//!
//! Deliberately free of external crates so `just test` can compile it with bare
//! `rustc --test`. The lockable shared instance lives in [`crate::config_bus`].

/// Wire/flash size of a serialised [`Config`].
pub const CONFIG_LEN: usize = 32;

const MAGIC: u8 = 0xC0;
/// Record version written by this firmware.
const VERSION: u8 = 3;
/// Oldest version still accepted on read.
const VERSION_LEGACY: u8 = 1;
/// Version that introduced `consumer_mask`. Older records must not have these
/// bytes read: they were reserved and could hold anything a future version put
/// there.
const VERSION_CONSUMER: u8 = 2;

// Field offsets in the fixed 32-byte record.
const KEYMAP_OFF: usize = 2;
const BRIGHTNESS_OFF: usize = 18;
const LED_MODE_OFF: usize = 19;
const CONSUMER_MASK_OFF: usize = 20;
/// Three host slots x RGB, and the last of the reserved space. There are no
/// spare bytes left after this; a future field means growing the record.
const SLOT_COLORS_OFF: usize = 22;

/// Per-slot identity colours, full range. The renderer scales these down per
/// context rather than storing a colour per context, so one value per slot
/// drives both the idle tint and the menu.
pub const DEFAULT_SLOT_COLORS: [[u8; 3]; 3] = [
    [255, 0, 216], // magenta
    [0, 216, 255], // cyan
    [255, 216, 0], // yellow
];

/// LED behaviour.
pub mod led_mode {
    /// All LEDs off.
    pub const OFF: u8 = 0;
    /// Dim idle, highlight pressed keys.
    pub const HIGHLIGHT: u8 = 1;
}

/// Default physical-key-bit -> HID usage map (numpad layout).
pub const DEFAULT_KEYMAP: [u8; 16] = [
    0x5F, 0x60, 0x61, 0x54, // 7 8 9 /
    0x5C, 0x5D, 0x5E, 0x55, // 4 5 6 *
    0x59, 0x5A, 0x5B, 0x56, // 1 2 3 -
    0x62, 0x63, 0x58, 0x57, // 0 . Enter +
];

#[derive(Clone, Copy, PartialEq)]
#[cfg_attr(test, derive(Debug))]
pub struct Config {
    /// HID usage code emitted for each physical key bit.
    ///
    /// Which usage *page* the code belongs to is decided by [`Config::consumer_mask`],
    /// not by the value: page 0x07 (keyboard) unless the bit is set, in which
    /// case page 0x0C (consumer). A mask is used rather than a second keymap so
    /// the record stays 32 bytes, and so the modifier usages 0xE0..=0xE7 --
    /// which are also above 0x80 -- keep meaning Left Ctrl..Right GUI instead of
    /// being misread as consumer codes.
    pub keymap: [u8; 16],
    /// Bit `i` set => physical key `i` emits `keymap[i]` on the Consumer page
    /// (0x0C) as a Report ID 2 report, instead of the keyboard report.
    pub consumer_mask: u16,
    /// Global APA102 brightness, 0..=31.
    pub brightness: u8,
    /// LED behaviour, see [`led_mode`].
    pub led_mode: u8,
    /// Identity colour per host slot, index 0..2.
    ///
    /// Stored at full range and scaled by the renderer for each context, so a
    /// user picking "cyan" gets cyan whether it is the dim idle tint or the
    /// brighter menu highlight.
    pub slot_colors: [[u8; 3]; 3],
}

impl Config {
    #[must_use]
    pub const fn default() -> Self {
        Config {
            keymap: DEFAULT_KEYMAP,
            consumer_mask: 0,
            brightness: 8,
            led_mode: led_mode::HIGHLIGHT,
            slot_colors: DEFAULT_SLOT_COLORS,
        }
    }

    /// Whether physical key `bit` is a consumer (media) key.
    #[must_use]
    pub const fn is_consumer(&self, bit: usize) -> bool {
        self.consumer_mask & (1u16 << bit) != 0
    }

    /// Serialise to a fixed record with a trailing checksum.
    #[must_use]
    pub fn to_bytes(self) -> [u8; CONFIG_LEN] {
        let mut b = [0u8; CONFIG_LEN];
        b[0] = MAGIC;
        b[1] = VERSION;
        b[KEYMAP_OFF..KEYMAP_OFF + 16].copy_from_slice(&self.keymap);
        b[BRIGHTNESS_OFF] = self.brightness;
        b[LED_MODE_OFF] = self.led_mode;
        b[CONSUMER_MASK_OFF..CONSUMER_MASK_OFF + 2]
            .copy_from_slice(&self.consumer_mask.to_le_bytes());
        for (i, c) in self.slot_colors.iter().enumerate() {
            b[SLOT_COLORS_OFF + i * 3..SLOT_COLORS_OFF + i * 3 + 3].copy_from_slice(c);
        }
        b[CONFIG_LEN - 1] = checksum(&b[..CONFIG_LEN - 1]);
        b
    }

    /// Deserialise, returning `None` if magic/version/checksum do not match.
    ///
    /// Records older than this firmware are accepted field by field: a field is
    /// only read if the record's version actually has it, otherwise the default
    /// applies. Bytes that were *reserved* in an older record must not be
    /// interpreted, because they could hold whatever some other version put
    /// there.
    #[must_use]
    pub fn from_bytes(b: &[u8; CONFIG_LEN]) -> Option<Config> {
        if b[0] != MAGIC || b[1] < VERSION_LEGACY || b[1] > VERSION {
            return None;
        }
        if b[CONFIG_LEN - 1] != checksum(&b[..CONFIG_LEN - 1]) {
            return None;
        }
        let mut keymap = [0u8; 16];
        keymap.copy_from_slice(&b[KEYMAP_OFF..KEYMAP_OFF + 16]);
        let consumer_mask = if b[1] >= VERSION_CONSUMER {
            u16::from_le_bytes([b[CONSUMER_MASK_OFF], b[CONSUMER_MASK_OFF + 1]])
        } else {
            0
        };
        let slot_colors = if b[1] >= VERSION {
            core::array::from_fn(|i| {
                [
                    b[SLOT_COLORS_OFF + i * 3],
                    b[SLOT_COLORS_OFF + i * 3 + 1],
                    b[SLOT_COLORS_OFF + i * 3 + 2],
                ]
            })
        } else {
            DEFAULT_SLOT_COLORS
        };
        Some(Config {
            keymap,
            consumer_mask,
            brightness: b[BRIGHTNESS_OFF],
            led_mode: b[LED_MODE_OFF],
            slot_colors,
        })
    }
}

fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &x| acc.wrapping_add(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact 32-byte record the web editor produces for the factory layout.
    ///
    /// This pins the wire format from the *other* end. The web editor is
    /// JavaScript and reimplements this record by hand, so nothing else catches
    /// the two drifting. If you change the default config, the magic, the field
    /// offsets or the checksum, this test fails and `web/app.js` needs updating
    /// to match.
    const WEB_FACTORY: [u8; CONFIG_LEN] = [
        0xc0, 0x03, // magic, version 3
        0x5f, 0x60, 0x61, 0x54, // 7 8 9 /
        0x5c, 0x5d, 0x5e, 0x55, // 4 5 6 *
        0x59, 0x5a, 0x5b, 0x56, // 1 2 3 -
        0x62, 0x63, 0x58, 0x57, // 0 . Enter +
        0x08, // brightness
        0x01, // led_mode = HIGHLIGHT
        0x00, 0x00, // consumer_mask = no consumer keys
        0xff, 0x00, 0xd8, // slot 1 magenta
        0x00, 0xd8, 0xff, // slot 2 cyan
        0xff, 0xd8, 0x00, // slot 3 yellow
        0x09, // checksum
    ];

    /// A v1 record: identical layout, but version byte 1 and no consumer mask.
    /// Devices flashed before consumer keys existed carry this.
    const WEB_FACTORY_V1: [u8; CONFIG_LEN] = [
        0xc0, 0x01, // magic, version 1
        0x5f, 0x60, 0x61, 0x54, 0x5c, 0x5d, 0x5e, 0x55, 0x59, 0x5a, 0x5b, 0x56, 0x62, 0x63, 0x58,
        0x57, 0x08, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x82,
    ];

    #[test]
    fn factory_defaults_match_the_web_editors_bytes() {
        assert_eq!(Config::default().to_bytes(), WEB_FACTORY);
    }

    #[test]
    fn web_editor_bytes_parse_back_to_the_defaults() {
        assert_eq!(Config::from_bytes(&WEB_FACTORY), Some(Config::default()));
    }

    /// A v2 record has no colours. Its reserved bytes must not be read as one --
    /// a device that never had the feature gets the defaults, not black slots.
    #[test]
    fn a_v2_record_gets_default_colours_not_its_reserved_bytes() {
        let mut b = Config::default().to_bytes();
        b[1] = 2;
        // Reserved-as-colours bytes left zeroed, as a real v2 record has them.
        for slot in 0..3 {
            for c in 0..3 {
                b[SLOT_COLORS_OFF + slot * 3 + c] = 0;
            }
        }
        b[CONFIG_LEN - 1] = checksum(&b[..CONFIG_LEN - 1]);
        let cfg = Config::from_bytes(&b).expect("v2 must still parse");
        assert_eq!(cfg.slot_colors, DEFAULT_SLOT_COLORS);
        // and everything v2 did have survives
        assert_eq!(cfg.keymap, DEFAULT_KEYMAP);
        assert_eq!(cfg.brightness, 8);
    }

    /// The same bytes read as colours on v3 must NOT be read as a mask on v1.
    #[test]
    fn a_v1_record_ignores_the_bytes_that_are_colours_in_v3() {
        let mut b = Config::default().to_bytes();
        b[1] = 1;
        b[CONFIG_LEN - 1] = checksum(&b[..CONFIG_LEN - 1]);
        let cfg = Config::from_bytes(&b).expect("v1 must still parse");
        assert_eq!(cfg.consumer_mask, 0);
        assert_eq!(cfg.slot_colors, DEFAULT_SLOT_COLORS);
    }

    #[test]
    fn slot_colors_round_trip() {
        let mut cfg = Config::default();
        cfg.slot_colors = [[1, 2, 3], [250, 200, 4], [9, 255, 130]];
        assert_eq!(Config::from_bytes(&cfg.to_bytes()), Some(cfg));
    }

    /// Colours live in the last of the reserved space; if that ever overlaps a
    /// real field the record corrupts itself silently. Checked at compile time,
    /// since it cannot change at runtime anyway.
    const _: () = assert!(SLOT_COLORS_OFF + 9 == CONFIG_LEN - 1);
    const _: () = assert!(SLOT_COLORS_OFF > CONSUMER_MASK_OFF + 1);

    /// A device flashed before consumer keys existed must keep working: the v1
    /// record reads back as the same config with no consumer keys set, and is
    /// rewritten as v2 on the next save.
    #[test]
    fn a_v1_record_migrates_to_the_defaults_with_no_consumer_keys() {
        let c = Config::from_bytes(&WEB_FACTORY_V1).expect("v1 record should be accepted");
        assert_eq!(c, Config::default());
        assert_eq!(c.consumer_mask, 0);
    }

    /// The v1 reader must not pick up junk from the bytes that only became a
    /// mask in v2 -- they were reserved, and a future version may put
    /// something else there.
    #[test]
    fn a_v1_record_ignores_the_bytes_that_are_a_mask_in_v2() {
        let mut b = WEB_FACTORY_V1;
        b[CONSUMER_MASK_OFF] = 0xff;
        b[CONSUMER_MASK_OFF + 1] = 0xff;
        b[CONFIG_LEN - 1] = checksum(&b[..CONFIG_LEN - 1]);
        let c = Config::from_bytes(&b).expect("v1 record should still be accepted");
        assert_eq!(c.consumer_mask, 0);
    }

    #[test]
    fn consumer_mask_round_trips_and_selects_keys() {
        let mut c = Config::default();
        c.consumer_mask = 0b1000_0000_0000_0101;
        c.keymap[0] = 0xe9; // Volume Increment
        c.keymap[15] = 0xcd; // Play/Pause
        let back = Config::from_bytes(&c.to_bytes()).unwrap();
        assert_eq!(back, c);
        assert!(back.is_consumer(0));
        assert!(back.is_consumer(2));
        assert!(!back.is_consumer(1));
        assert!(back.is_consumer(15));
    }

    /// Derived from the version constants rather than a hardcoded number, so
    /// bumping `VERSION` cannot silently turn this test into one that asserts
    /// the current version is rejected. That has already happened once here.
    #[test]
    fn accepts_every_supported_version_and_rejects_the_rest() {
        let base = Config::default().to_bytes();
        for v in 0u8..=255 {
            let mut b = base;
            b[1] = v;
            b[CONFIG_LEN - 1] = checksum(&b[..CONFIG_LEN - 1]);
            let accepted = (VERSION_LEGACY..=VERSION).contains(&v);
            assert_eq!(
                Config::from_bytes(&b).is_some(),
                accepted,
                "version {v} should {}be accepted",
                if accepted { "" } else { "not " }
            );
        }
    }

    /// Guard the accepted range against a silly edit making it vacuous.
    const _: () = assert!(VERSION > VERSION_LEGACY);

    #[test]
    fn round_trips_a_non_default_config() {
        let c = Config {
            keymap: [0x04; 16],
            consumer_mask: 0,
            brightness: 31,
            led_mode: led_mode::OFF,
            slot_colors: [[3, 0, 250], [0, 250, 3], [250, 3, 0]],
        };
        assert_eq!(Config::from_bytes(&c.to_bytes()), Some(c));
    }

    #[test]
    fn rejects_a_corrupted_checksum() {
        let mut b = Config::default().to_bytes();
        b[CHECKSUM_INDEX] ^= 0xff;
        assert_eq!(Config::from_bytes(&b), None);
    }

    #[test]
    fn rejects_wrong_magic_and_version() {
        let mut b = Config::default().to_bytes();
        b[0] = 0xc1;
        assert_eq!(Config::from_bytes(&b), None);

        let mut b = Config::default().to_bytes();
        // 0 is not a version this firmware knows. (2 used to be an invalid value
        // here; it is now the current record version, so it must be accepted.)
        b[1] = 0;
        assert_eq!(Config::from_bytes(&b), None);
    }

    #[test]
    fn a_single_keymap_edit_changes_only_its_byte_and_the_checksum() {
        let base = Config::default().to_bytes();
        let mut c = Config::default();
        c.keymap[7] = 0x04;
        let edited = c.to_bytes();
        let diff: Vec<usize> = (0..CONFIG_LEN).filter(|&i| base[i] != edited[i]).collect();
        assert_eq!(diff, vec![2 + 7, CONFIG_LEN - 1]);
    }

    const CHECKSUM_INDEX: usize = CONFIG_LEN - 1;
}
