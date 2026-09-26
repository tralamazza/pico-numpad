//! Runtime configuration: key remap + LED settings, shared between the BLE key
//! loop (reader) and the USB config handler (writer). Serialised to a fixed
//! 32-byte little-endian record -- no heap, no serde -- and kept free of external
//! crates so `just test` compiles it with bare `rustc --test`.

/// Wire/flash size of a serialised [`Config`].
pub const CONFIG_LEN: usize = 32;

const MAGIC: u8 = 0xC0;
/// Record version written by this firmware.
const VERSION: u8 = 3;
/// Oldest version still accepted on read.
const VERSION_LEGACY: u8 = 1;
/// Version that introduced `consumer_mask`; bytes reserved before it must not be read.
const VERSION_CONSUMER: u8 = 2;

// Field offsets in the fixed 32-byte record.
const KEYMAP_OFF: usize = 2;
const BRIGHTNESS_OFF: usize = 18;
const LED_MODE_OFF: usize = 19;
const CONSUMER_MASK_OFF: usize = 20;
/// Three host slots x RGB, in the last of the reserved space. No spare bytes
/// remain: a future field means growing the record.
const SLOT_COLORS_OFF: usize = 22;

/// Per-slot identity colour at full range; the renderer scales it per context.
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
    /// HID usage code emitted for each physical key bit. The usage *page* comes
    /// from [`Config::consumer_mask`], not the value: page 0x07 unless the bit
    /// is set, then 0x0C. A mask keeps the record at 32 bytes and stops the
    /// modifier usages 0xE0..=0xE7 reading as consumer codes.
    pub keymap: [u8; 16],
    /// Bit `i` set => physical key `i` emits `keymap[i]` on the Consumer page
    /// (0x0C) as a Report ID 2 report, instead of the keyboard report.
    pub consumer_mask: u16,
    /// Global APA102 brightness, 0..=31.
    pub brightness: u8,
    /// LED behaviour, see [`led_mode`].
    pub led_mode: u8,
    /// Identity colour per host slot, index 0..2, stored at full range and
    /// scaled by the renderer for each context.
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

    /// Deserialise, or `None` if magic, version or checksum mismatch.
    /// Post: a field is only read if the record's version actually has it;
    /// bytes that were reserved in an older record are never interpreted.
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
    /// Pins the wire format from the other end: change the defaults, magic,
    /// offsets or checksum and this fails, and `web/app.js` must follow.
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

    /// A v1 record: identical layout, version byte 1, no consumer mask.
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

    #[test]
    fn a_v2_record_gets_default_colours_not_its_reserved_bytes() {
        let mut b = Config::default().to_bytes();
        b[1] = 2;
        for slot in 0..3 {
            for c in 0..3 {
                b[SLOT_COLORS_OFF + slot * 3 + c] = 0;
            }
        }
        b[CONFIG_LEN - 1] = checksum(&b[..CONFIG_LEN - 1]);
        let cfg = Config::from_bytes(&b).expect("v2 must still parse");
        assert_eq!(cfg.slot_colors, DEFAULT_SLOT_COLORS);
        assert_eq!(cfg.keymap, DEFAULT_KEYMAP);
        assert_eq!(cfg.brightness, 8);
    }

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

    /// Colours must not overlap a real field in the record. Compile-time check.
    const _: () = assert!(SLOT_COLORS_OFF + 9 == CONFIG_LEN - 1);
    const _: () = assert!(SLOT_COLORS_OFF > CONSUMER_MASK_OFF + 1);

    #[test]
    fn a_v1_record_migrates_to_the_defaults_with_no_consumer_keys() {
        let c = Config::from_bytes(&WEB_FACTORY_V1).expect("v1 record should be accepted");
        assert_eq!(c, Config::default());
        assert_eq!(c.consumer_mask, 0);
    }

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
        assert_eq!(back.consumer_mask, 0b1000_0000_0000_0101);
        for bit in [0, 2, 15] {
            assert_ne!(back.consumer_mask & (1 << bit), 0, "key {bit} is consumer");
        }
        assert_eq!(
            back.consumer_mask & (1 << 1),
            0,
            "key 1 is not a consumer key"
        );
    }

    /// Derived from the version constants so bumping `VERSION` cannot silently
    /// turn this into a test that asserts the current version is rejected.
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
