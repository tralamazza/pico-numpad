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

/// Appended to the advertised name while a slot holds no bond.
pub const PAIRING_SUFFIX: &str = "-pairing";

/// Longest advertised local name that still fits the 31-byte legacy advertising
/// payload alongside everything else we send: 3 (flags) + 4 (16-bit HID service
/// UUID) + 2 (name AD header) = 9, leaving 22.
pub const MAX_ADV_NAME_LEN: usize = 22;

/// Write the advertised local name for `slot` into `buf` and return the filled
/// prefix. An unbonded slot carries `PAIRING_SUFFIX`.
///
/// Why the bond state is in the name at all: a host cannot be *told* that its
/// bond is gone. The central owns its bond store and BLE gives a peripheral no
/// way to invalidate a pairing it does not hold. When a stale host-side bond
/// tries to reconnect, all this device can do is refuse -- trouble-host
/// disconnects with `AuthenticationFailure` -- and macOS in particular keeps the
/// dead pairing and silently retries it forever. The advertised name is the only
/// signal about bond state that reliably reaches a human on the host.
///
/// Only the advertised name varies. The GATT Generic Access device name stays the
/// stable identity from `name()`.
///
/// # Panics
///
/// If `buf` is shorter than the name. Callers size it from `MAX_ADV_NAME_LEN`, so
/// this is a programming error rather than a runtime condition.
#[must_use]
pub fn adv_name(slot: u8, bonded: bool, buf: &mut [u8]) -> &[u8] {
    let base = name(slot).as_bytes();
    let suffix: &[u8] = if bonded {
        &[]
    } else {
        PAIRING_SUFFIX.as_bytes()
    };
    let len = base.len() + suffix.len();
    assert!(
        len <= buf.len(),
        "advertising name buffer too small for slot {slot}"
    );
    buf[..base.len()].copy_from_slice(base);
    buf[base.len()..len].copy_from_slice(suffix);
    &buf[..len]
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
    /// `+` is being held toward opening the menu. Carries 0..=100 progress so the
    /// LED can fill: without it a three-second hold looks completely dead and the
    /// user has no way to learn the gesture exists.
    Entering(u8),
    Open,
    /// A slot key is held. Carries 0..=100 progress toward the destructive clear
    /// at `CLEAR_HOLD_MS` -- releasing below the threshold only selects the slot,
    /// holding to full wipes its bond, so the user must be able to tell how much
    /// is left.
    Holding {
        slot: u8,
        progress: u8,
    },
}

/// Percent (0..=100) of the way through a hold of `hold_ms` that began at
/// `since`. Drives the fill so a long press tells the user to keep going.
#[must_use]
pub fn hold_progress(since: u64, now: u64, hold_ms: u64) -> u8 {
    if hold_ms == 0 {
        return 100;
    }
    now.saturating_sub(since)
        .saturating_mul(100)
        .saturating_div(hold_ms)
        .min(100) as u8
}

/// Linear ramp between two endpoints by a 0..=100 percentage. The result is
/// always between two `u8` endpoints, so the fallible conversion cannot fail in
/// practice; `unwrap_or` keeps it total without a sign-loss cast.
#[must_use]
fn ramp(from: u8, to: u8, progress: u8) -> u8 {
    let p = i32::from(progress);
    let a = i32::from(from);
    let b = i32::from(to);
    u8::try_from(a + ((b - a) * p / 100)).unwrap_or(from)
}

/// Color of the `+` key while its hold is still building. The key the user is
/// actually pressing fills from dim to bright over the three seconds, which is
/// the only signal that the gesture registered and they should keep going.
#[must_use]
pub fn enter_fill(progress: u8) -> [u8; 3] {
    let level = ramp(25, 120, progress);
    [level, level, level]
}

/// Bonded slots are green, empty slots blue. The active slot pulses without
/// changing its status color.
///
/// A held slot ramps amber to red as the clear approaches: release while it is
/// still amber and you have only selected the slot, hold it to full red and the
/// bond is wiped. The color shift is the "this is about to become destructive"
/// cue that a static amber cannot give.
#[must_use]
pub fn menu_colors(
    bonded: [bool; SLOT_COUNT],
    active: u8,
    menu: Menu,
    now: u64,
) -> [[u8; 3]; SLOT_COUNT] {
    core::array::from_fn(|i| {
        if let Menu::Holding { slot, progress } = menu {
            if slot == i as u8 {
                return [ramp(70, 150, progress), ramp(55, 0, progress), 0];
            }
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
                } else {
                    // Report progress while the hold is still building so the LED
                    // can fill. Without this the key looks inert for three seconds.
                    input.menu = Menu::Entering(hold_progress(since, now, MENU_HOLD_MS));
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
                    input.menu = Menu::Holding {
                        slot: slot as u8,
                        progress: 0,
                    };
                } else if pressed != 0 {
                    self.state = State::Drain;
                    input.menu = Menu::Closed;
                }
            }
            State::Choosing { slot, since } => {
                input.menu = Menu::Holding {
                    slot,
                    progress: hold_progress(since, now, CLEAR_HOLD_MS),
                };
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
        // The hold reports progress rather than staying Closed, so the LED can
        // tell the user the gesture registered and they should keep pressing.
        assert_eq!(c.update(1_001, PLUS).menu, Menu::Entering(33));
        assert_eq!(c.update(3_000, PLUS).menu, Menu::Entering(99));
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
            menu_colors(
                [true, false, true],
                0,
                Menu::Holding {
                    slot: 1,
                    progress: 0
                },
                400
            )[1],
            [70, 55, 0]
        );
    }

    #[test]
    fn unbonded_slots_advertise_that_they_need_pairing() {
        let mut buf = [0u8; MAX_ADV_NAME_LEN];
        assert_eq!(adv_name(0, true, &mut buf), &b"pico-numpad"[..]);
        assert_eq!(adv_name(1, true, &mut buf), &b"pico-numpad-2"[..]);
        assert_eq!(adv_name(2, false, &mut buf), &b"pico-numpad-3-pairing"[..]);
        assert_eq!(adv_name(0, false, &mut buf), &b"pico-numpad-pairing"[..]);
    }

    #[test]
    fn advertised_names_fit_the_advertising_payload() {
        let mut buf = [0u8; MAX_ADV_NAME_LEN];
        for slot in 0..SLOT_COUNT as u8 {
            for bonded in [true, false] {
                let len = adv_name(slot, bonded, &mut buf).len();
                assert!(
                    len <= MAX_ADV_NAME_LEN,
                    "slot {slot} bonded={bonded} advertises {len} bytes, limit is {MAX_ADV_NAME_LEN}"
                );
            }
        }
    }

    #[test]
    fn hold_progress_ramps_linearly_and_clamps_at_full() {
        assert_eq!(hold_progress(0, 0, 3_000), 0);
        assert_eq!(hold_progress(0, 1_500, 3_000), 50);
        assert_eq!(hold_progress(0, 2_999, 3_000), 99);
        assert_eq!(hold_progress(0, 3_000, 3_000), 100);
        // Never overshoot, and never wrap on a clock that went backwards.
        assert_eq!(hold_progress(0, 99_000, 3_000), 100);
        assert_eq!(hold_progress(5_000, 1_000, 3_000), 0);
        assert_eq!(hold_progress(0, 0, 0), 100);
    }

    #[test]
    fn plus_hold_reports_entering_progress_across_the_whole_three_seconds() {
        let mut c = Controls::new();
        c.update(0, 0);
        c.update(1, PLUS);
        let mut seen = Vec::new();
        for t in (2..3_000).step_by(300) {
            match c.update(t, PLUS).menu {
                Menu::Entering(p) => seen.push(p),
                other => panic!("expected Entering at t={t}, got {other:?}"),
            }
        }
        assert!(seen.len() >= 9, "expected samples across the hold");
        assert!(
            seen.windows(2).all(|w| w[0] <= w[1]),
            "progress must not go backwards: {seen:?}"
        );
        assert!(seen.first() < Some(&20), "should start dim: {seen:?}");
        assert!(
            seen.last() >= Some(&80),
            "should be nearly full before opening: {seen:?}"
        );
    }

    #[test]
    fn slot_hold_reports_progress_toward_the_destructive_clear() {
        let mut c = menu();
        c.update(10_000, SLOT_KEYS[1]);
        match c.update(11_500, SLOT_KEYS[1]).menu {
            Menu::Holding { slot, progress } => {
                assert_eq!(slot, 1);
                assert_eq!(progress, 50);
            }
            other => panic!("expected Holding, got {other:?}"),
        }
        // Still only a select at 2.9s, but the progress says "nearly there".
        assert_eq!(c.update(12_900, 0).action, Some(Action::Select(1)));
    }

    #[test]
    fn held_slot_shifts_amber_to_red_as_the_clear_approaches() {
        let at = |p: u8| {
            menu_colors(
                [false, false, false],
                0,
                Menu::Holding {
                    slot: 1,
                    progress: p,
                },
                0,
            )[1]
        };
        let start = at(0);
        let end = at(100);
        assert!(start[1] > 40, "starts with real amber-green: {start:?}");
        assert!(end[1] == 0, "green fully drained at full: {end:?}");
        assert!(
            end[0] > start[0],
            "red climbs toward the destructive end: {start:?} -> {end:?}"
        );
        // Monotonic in both channels so the ramp reads as continuous.
        let mut prev = at(0);
        for p in 1..=100 {
            let cur = at(p);
            assert!(cur[0] >= prev[0], "red must not dip at {p}: {cur:?}");
            assert!(cur[1] <= prev[1], "green must not rise at {p}: {cur:?}");
            prev = cur;
        }
        // A non-held slot keeps its own status color while another is held.
        assert_eq!(
            menu_colors(
                [false, false, true],
                0,
                Menu::Holding {
                    slot: 1,
                    progress: 100
                },
                100
            )[2],
            [0, 100, 0]
        );
    }

    #[test]
    fn plus_key_fills_brighter_as_the_hold_builds() {
        let start = enter_fill(0);
        let end = enter_fill(100);
        assert_eq!(start, [25, 25, 25]);
        assert_eq!(end, [120, 120, 120]);
        assert!(
            start[0] > 0,
            "visible even at zero progress, backlight may be off"
        );
        assert!(end[0] > start[0]);
        assert_eq!(start[0], start[1]);
        assert_eq!(start[1], start[2], "neutral white, not a status color");
    }
}
