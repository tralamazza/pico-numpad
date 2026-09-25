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
const VERSION: u8 = 2;
/// v1 is still accepted on read. It has no consumer-key mask; see `from_bytes`.
const VERSION_LEGACY: u8 = 1;

// Field offsets in the fixed 32-byte record.
const KEYMAP_OFF: usize = 2;
const BRIGHTNESS_OFF: usize = 18;
const LED_MODE_OFF: usize = 19;
const CONSUMER_MASK_OFF: usize = 20;

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
}

impl Config {
    #[must_use]
    pub const fn default() -> Self {
        Config {
            keymap: DEFAULT_KEYMAP,
            consumer_mask: 0,
            brightness: 8,
            led_mode: led_mode::HIGHLIGHT,
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
        b[CONFIG_LEN - 1] = checksum(&b[..CONFIG_LEN - 1]);
        b
    }

    /// Deserialise, returning `None` if magic/version/checksum do not match.
    ///
    /// v1 records are accepted as well as v2. v1 has no consumer mask, so those
    /// keys read as "none" and a v1 config keeps working unchanged; it is
    /// rewritten as v2 the next time it is saved.
    #[must_use]
    pub fn from_bytes(b: &[u8; CONFIG_LEN]) -> Option<Config> {
        if b[0] != MAGIC || (b[1] != VERSION && b[1] != VERSION_LEGACY) {
            return None;
        }
        if b[CONFIG_LEN - 1] != checksum(&b[..CONFIG_LEN - 1]) {
            return None;
        }
        let mut keymap = [0u8; 16];
        keymap.copy_from_slice(&b[KEYMAP_OFF..KEYMAP_OFF + 16]);
        // Only read the mask from a v2 record. In a v1 record these two bytes
        // are reserved and could hold anything a future version put there.
        let consumer_mask = if b[1] >= VERSION {
            u16::from_le_bytes([b[CONSUMER_MASK_OFF], b[CONSUMER_MASK_OFF + 1]])
        } else {
            0
        };
        Some(Config {
            keymap,
            consumer_mask,
            brightness: b[BRIGHTNESS_OFF],
            led_mode: b[LED_MODE_OFF],
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
        0xc0, 0x02, // magic, version 2
        0x5f, 0x60, 0x61, 0x54, // 7 8 9 /
        0x5c, 0x5d, 0x5e, 0x55, // 4 5 6 *
        0x59, 0x5a, 0x5b, 0x56, // 1 2 3 -
        0x62, 0x63, 0x58, 0x57, // 0 . Enter +
        0x08, // brightness
        0x01, // led_mode = HIGHLIGHT
        0x00, 0x00, // consumer_mask = no consumer keys
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // reserved
        0x83, // checksum
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

    #[test]
    fn rejects_an_unknown_version() {
        let mut b = Config::default().to_bytes();
        b[1] = 3;
        b[CONFIG_LEN - 1] = checksum(&b[..CONFIG_LEN - 1]);
        assert_eq!(Config::from_bytes(&b), None);
    }

    #[test]
    fn round_trips_a_non_default_config() {
        let c = Config {
            keymap: [0x04; 16],
            consumer_mask: 0,
            brightness: 31,
            led_mode: led_mode::OFF,
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
