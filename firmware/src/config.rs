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
const VERSION: u8 = 1;

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
    pub keymap: [u8; 16],
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
            brightness: 8,
            led_mode: led_mode::HIGHLIGHT,
        }
    }

    /// Serialise to a fixed record with a trailing checksum.
    #[must_use]
    pub fn to_bytes(self) -> [u8; CONFIG_LEN] {
        let mut b = [0u8; CONFIG_LEN];
        b[0] = MAGIC;
        b[1] = VERSION;
        b[2..18].copy_from_slice(&self.keymap);
        b[18] = self.brightness;
        b[19] = self.led_mode;
        b[CONFIG_LEN - 1] = checksum(&b[..CONFIG_LEN - 1]);
        b
    }

    /// Deserialise, returning `None` if magic/version/checksum do not match.
    #[must_use]
    pub fn from_bytes(b: &[u8; CONFIG_LEN]) -> Option<Config> {
        if b[0] != MAGIC || b[1] != VERSION {
            return None;
        }
        if b[CONFIG_LEN - 1] != checksum(&b[..CONFIG_LEN - 1]) {
            return None;
        }
        let mut keymap = [0u8; 16];
        keymap.copy_from_slice(&b[2..18]);
        Some(Config {
            keymap,
            brightness: b[18],
            led_mode: b[19],
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
        0xc0, 0x01, // magic, version
        0x5f, 0x60, 0x61, 0x54, // 7 8 9 /
        0x5c, 0x5d, 0x5e, 0x55, // 4 5 6 *
        0x59, 0x5a, 0x5b, 0x56, // 1 2 3 -
        0x62, 0x63, 0x58, 0x57, // 0 . Enter +
        0x08, // brightness
        0x01, // led_mode = HIGHLIGHT
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // reserved
        0x82, // checksum
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
    fn round_trips_a_non_default_config() {
        let c = Config {
            keymap: [0x04; 16],
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
        b[1] = 2;
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
