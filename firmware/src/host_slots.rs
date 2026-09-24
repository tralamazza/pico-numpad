//! Persistent host selection and physical-key controls, independent of the BLE HAL.

// Slot ids are `u8` (persisted and handed to the BLE layer) while array indices
// are `usize`; SLOT_COUNT is 3, so these narrowings cannot truncate.
#![allow(clippy::cast_possible_truncation)]

pub const SLOT_COUNT: usize = 3;
pub const PLUS: u16 = 1 << 15;
pub const SLOT_KEYS: [u16; SLOT_COUNT] = [1 << 8, 1 << 9, 1 << 10];
pub const MENU_HOLD_MS: u64 = 3_000;
pub const CLEAR_HOLD_MS: u64 = 3_000;
const MENU_TIMEOUT_MS: u64 = 10_000;

#[cfg_attr(not(test), derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Slots<B> {
    pub version: u8,
    pub active: u8,
    pub bonds: [Option<B>; SLOT_COUNT],
}

impl<B> Slots<B> {
    pub fn from_legacy(bond: Option<B>) -> Self {
        Self {
            version: 1,
            active: 0,
            bonds: [bond, None, None],
        }
    }

    pub fn valid(&self) -> bool {
        self.version == 1 && (self.active as usize) < SLOT_COUNT
    }

    pub fn apply(&mut self, action: Action) {
        self.active = action.slot();
        if let Action::Clear(slot) = action {
            self.bonds[slot as usize] = None;
        }
    }
}

/// Keep the original identity for slot 1 so its existing pairing survives migration.
#[must_use]
pub fn address(slot: u8) -> [u8; 6] {
    [0xff - slot, 0x8f, 0x1a, 0x05, 0xe4, 0xff]
}

#[must_use]
pub fn name(slot: u8) -> &'static str {
    match slot {
        0 => "pico-numpad",
        1 => "pico-numpad-2",
        _ => "pico-numpad-3",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Select(u8),
    Clear(u8),
}

impl Action {
    #[must_use]
    pub fn slot(self) -> u8 {
        match self {
            Self::Select(slot) | Self::Clear(slot) => slot,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Closed,
    Open,
    Holding(u8),
}

/// Bonded slots are green, empty slots blue. The active slot pulses without
/// changing its status color; a held candidate is amber until the clear fires.
#[must_use]
pub fn menu_colors(
    bonded: [bool; SLOT_COUNT],
    active: u8,
    menu: Menu,
    now: u64,
) -> [[u8; 3]; SLOT_COUNT] {
    core::array::from_fn(|i| {
        if menu == Menu::Holding(i as u8) {
            return [100, 40, 0];
        }
        let level = if i == active as usize && (now / 400).is_multiple_of(2) {
            25
        } else {
            100
        };
        if bonded[i] {
            [0, level, 0]
        } else {
            [0, 0, level]
        }
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct Input {
    pub keys: u16,
    pub menu: Menu,
    pub action: Option<Action>,
}

enum State {
    Ready,
    PlusPending(u64),
    PlusTap(u64),
    Passthrough,
    MenuRelease,
    Menu(u64),
    Choosing { slot: u8, since: u64 },
    Drain,
}

pub struct Controls {
    state: State,
}

impl Default for Controls {
    fn default() -> Self {
        Self::new()
    }
}

impl Controls {
    #[must_use]
    pub fn new() -> Self {
        // Do not type keys still held through a slot-switch reboot.
        Self {
            state: State::Drain,
        }
    }

    pub fn update(&mut self, now: u64, pressed: u16) -> Input {
        let mut input = Input {
            keys: 0,
            menu: Menu::Closed,
            action: None,
        };
        match self.state {
            State::Ready => {
                if pressed == PLUS {
                    self.state = State::PlusPending(now);
                } else {
                    input.keys = pressed;
                    if pressed & PLUS != 0 {
                        self.state = State::Passthrough;
                    }
                }
            }
            State::PlusPending(since) => {
                if pressed == 0 {
                    // A short tap still types the configured usage for the + key.
                    self.state = State::PlusTap(now);
                    input.keys = PLUS;
                } else if pressed != PLUS {
                    self.state = State::Passthrough;
                    input.keys = pressed;
                } else if now - since >= MENU_HOLD_MS {
                    self.state = State::MenuRelease;
                    input.menu = Menu::Open;
                }
            }
            State::PlusTap(since) => {
                // Keep a tap visible long enough for the BLE task to send it.
                input.keys = pressed;
                if now - since < 30 {
                    input.keys |= PLUS;
                } else {
                    self.state = State::Ready;
                }
            }
            State::Passthrough => {
                input.keys = pressed;
                if pressed & PLUS == 0 {
                    self.state = State::Ready;
                }
            }
            State::MenuRelease => {
                input.menu = Menu::Open;
                if pressed == 0 {
                    self.state = State::Menu(now);
                }
            }
            State::Menu(since) => {
                input.menu = Menu::Open;
                if now - since >= MENU_TIMEOUT_MS {
                    self.state = State::Drain;
                    input.menu = Menu::Closed;
                } else if let Some(slot) = SLOT_KEYS.iter().position(|key| *key == pressed) {
                    self.state = State::Choosing {
                        slot: slot as u8,
                        since: now,
                    };
                    input.menu = Menu::Holding(slot as u8);
                } else if pressed != 0 {
                    self.state = State::Drain;
                    input.menu = Menu::Closed;
                }
            }
            State::Choosing { slot, since } => {
                input.menu = Menu::Holding(slot);
                if pressed == 0 {
                    input.action = Some(Action::Select(slot));
                    self.state = State::Drain;
                } else if pressed != SLOT_KEYS[slot as usize] {
                    self.state = State::Drain;
                    input.menu = Menu::Closed;
                } else if now - since >= CLEAR_HOLD_MS {
                    input.action = Some(Action::Clear(slot));
                    self.state = State::Drain;
                }
            }
            State::Drain => {
                if pressed == 0 {
                    self.state = State::Ready;
                }
            }
        }
        input
    }
}

#[cfg(test)]
#[path = "debounce.rs"]
mod debounce;

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Controls {
        let mut controls = Controls::new();
        controls.update(0, 0);
        assert_eq!(controls.update(1, PLUS).keys, 0);
        assert_eq!(controls.update(3_001, PLUS).menu, Menu::Open);
        assert_eq!(controls.update(3_002, 0).keys, 0);
        controls
    }

    #[test]
    fn short_plus_tap_and_normal_keys_work() {
        let mut c = Controls::new();
        c.update(0, 0);
        assert_eq!(c.update(1, 1).keys, 1);
        c.update(2, 0);
        assert_eq!(c.update(3, PLUS).keys, 0);
        assert_eq!(c.update(100, 0).keys, PLUS);
        assert_eq!(c.update(130, 0).keys, 0);
    }

    #[test]
    fn menu_requires_three_seconds_and_release_before_selection() {
        let mut c = Controls::new();
        c.update(0, 0);
        c.update(1, PLUS);
        assert_eq!(c.update(1_001, PLUS).menu, Menu::Closed);
        assert_eq!(c.update(3_000, PLUS).menu, Menu::Closed);
        assert_eq!(c.update(3_001, PLUS).menu, Menu::Open);
        assert_eq!(c.update(3_100, PLUS | SLOT_KEYS[0]).action, None);
        assert_eq!(c.update(7_000, SLOT_KEYS[0]).action, None);
        c.update(7_001, 0);
        c.update(7_002, SLOT_KEYS[0]);
        assert_eq!(c.update(7_100, 0).action, Some(Action::Select(0)));
    }

    #[test]
    fn bounced_slot_press_still_clears_instead_of_selecting() {
        let mut c = menu();
        let mut d = debounce::Debouncer::default();
        d.update(3_002, 0);
        let key = SLOT_KEYS[1];
        // Press, momentary open contact, then continued hold. Feed the actual
        // debouncer output to the menu, as Keypad::read_pressed does on device.
        for (time, raw) in [(4_000, key), (4_005, 0), (4_010, key), (4_029, key)] {
            assert_eq!(c.update(time, d.update(time, raw)).action, None);
        }
        assert_eq!(c.update(4_030, d.update(4_030, key)).action, None);
        // A further short bounce during the hold must not count as a release.
        assert_eq!(c.update(4_100, d.update(4_100, 0)).action, None);
        assert_eq!(c.update(4_105, d.update(4_105, key)).action, None);
        assert_eq!(c.update(7_029, d.update(7_029, key)).action, None);
        assert_eq!(
            c.update(7_030, d.update(7_030, key)).action,
            Some(Action::Clear(1))
        );
    }

    #[test]
    fn plus_chords_and_keys_during_a_short_tap_are_not_lost() {
        let mut c = Controls::new();
        c.update(0, 0);
        c.update(1, PLUS);
        assert_eq!(c.update(10, PLUS | 1).keys, PLUS | 1);
        assert_eq!(c.update(4_000, PLUS | 1).menu, Menu::Closed);
        c.update(4_001, 0);
        c.update(4_002, PLUS);
        c.update(4_003, 0);
        assert_eq!(c.update(4_010, 1).keys, PLUS | 1);
        assert_eq!(c.update(4_033, 1).keys, 1);
    }

    #[test]
    fn slot_selection_never_types_menu_keys() {
        for (slot, &key) in SLOT_KEYS.iter().enumerate() {
            let mut c = menu();
            let input = c.update(4_000, key);
            assert_eq!(input.keys, 0);
            assert_eq!(input.action, None);
            assert_eq!(c.update(4_100, 0).action, Some(Action::Select(slot as u8)));
        }
    }

    #[test]
    fn long_press_clears_once_and_drains_keys() {
        let mut c = menu();
        c.update(4_000, SLOT_KEYS[1]);
        assert_eq!(c.update(6_999, SLOT_KEYS[1]).action, None);
        assert_eq!(c.update(7_000, SLOT_KEYS[1]).action, Some(Action::Clear(1)));
        assert_eq!(c.update(9_000, SLOT_KEYS[1]).action, None);
        assert_eq!(c.update(9_001, SLOT_KEYS[1]).keys, 0);
        c.update(9_002, 0);
        assert_eq!(c.update(9_003, SLOT_KEYS[1]).keys, SLOT_KEYS[1]);
    }

    #[test]
    fn cancel_timeout_and_ambiguous_chords_do_not_select_or_clear() {
        let mut c = menu();
        assert_eq!(c.update(4_000, SLOT_KEYS[0] | SLOT_KEYS[1]).action, None);
        assert_eq!(c.update(9_000, SLOT_KEYS[0]).action, None);
        let mut c = menu();
        assert_eq!(c.update(13_002, 0).menu, Menu::Closed);
        assert_eq!(c.update(13_003, SLOT_KEYS[0]).action, None);
        let mut c = menu();
        c.update(4_000, SLOT_KEYS[0]);
        assert_eq!(c.update(5_000, SLOT_KEYS[1]).action, None);
        assert_eq!(c.update(8_000, SLOT_KEYS[1]).action, None);
    }

    #[test]
    fn migration_and_clear_preserve_other_slots() {
        let mut slots = Slots::from_legacy(Some(42));
        assert_eq!(slots.bonds, [Some(42), None, None]);
        slots.bonds[1] = Some(43);
        slots.bonds[2] = Some(44);
        slots.apply(Action::Select(2));
        assert_eq!(slots.bonds, [Some(42), Some(43), Some(44)]);
        slots.apply(Action::Clear(1));
        assert_eq!(slots.active, 1);
        assert_eq!(slots.bonds, [Some(42), None, Some(44)]);
        assert!(slots.valid());
        slots.active = 3;
        assert!(!slots.valid());
    }

    #[test]
    fn distinct_static_addresses_preserve_original_slot_one() {
        assert_eq!(address(0), [0xff, 0x8f, 0x1a, 0x05, 0xe4, 0xff]);
        assert_ne!(address(0), address(1));
        assert_ne!(address(1), address(2));
        assert_ne!(address(0), address(2));
        for slot in 0..3 {
            assert_eq!(address(slot)[5] & 0xc0, 0xc0);
        }
    }

    #[test]
    fn menu_colors_distinguish_bonds_and_hold_without_losing_active_status() {
        assert_eq!(
            menu_colors([true, false, true], 0, Menu::Open, 400),
            [[0, 100, 0], [0, 0, 100], [0, 100, 0]]
        );
        assert_eq!(
            menu_colors([true, false, true], 0, Menu::Open, 800)[0],
            [0, 25, 0]
        );
        assert_eq!(
            menu_colors([true, false, true], 0, Menu::Holding(1), 400)[1],
            [100, 40, 0]
        );
    }
}
