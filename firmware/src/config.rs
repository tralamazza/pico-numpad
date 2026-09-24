//! Runtime configuration: key remap + LED settings, shared between the BLE key
//! loop (reader) and the USB config handler (writer).
//!
//! The config is serialised to a fixed 32-byte little-endian record so it can be
//! shipped over USB and stored in flash without a heap or serde.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

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
pub struct Config {
    /// HID usage code emitted for each physical key bit.
    pub keymap: [u8; 16],
    /// Global APA102 brightness, 0..=31.
    pub brightness: u8,
    /// LED behaviour, see [`led_mode`].
    pub led_mode: u8,
}

impl Config {
    pub const fn default() -> Self {
        Config {
            keymap: DEFAULT_KEYMAP,
            brightness: 8,
            led_mode: led_mode::HIGHLIGHT,
        }
    }

    /// Serialise to a fixed record with a trailing checksum.
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

/// Shared config. Lock with `.lock().await` from any task.
pub static CONFIG: Mutex<CriticalSectionRawMutex, Config> = Mutex::new(Config::default());
