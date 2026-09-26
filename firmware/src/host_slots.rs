//! Persistent host selection and physical-key controls, independent of the BLE HAL.

// Slot ids are u8 (persisted and handed to the BLE layer) and SLOT_COUNT is 3,
// so the narrowings below cannot truncate.
#![allow(clippy::cast_possible_truncation)]

pub const SLOT_COUNT: usize = 3;
pub const PLUS: u16 = 1 << 15;
pub const SLOT_KEYS: [u16; SLOT_COUNT] = [1 << 8, 1 << 9, 1 << 10];
pub const MENU_HOLD_MS: u64 = 3_000;
pub const CLEAR_HOLD_MS: u64 = 3_000;
const MENU_TIMEOUT_MS: u64 = 10_000;
/// A short `+` tap is re-injected for this long so the BLE task still gets a
/// chance to emit it before the state machine returns to `Ready`.
const TAP_HOLD_MS: u64 = 30;

/// Blank the backlight after this long with no key activity. The 16 APA102s are
/// the board's largest single draw (~700mA at full white).
pub const LED_IDLE_OFF_MS: u64 = 60_000;

/// Advertisement cycles a bonded slot keeps the fast interval before settling
/// (~30s of easy discovery after boot or a link drop).
pub const ADV_FAST_CYCLES: u32 = 30;

/// Whether a slot should advertise at the fast interval.
/// Post: unbonded slots always advertise fast; bonded ones stop after
/// `ADV_FAST_CYCLES`.
#[must_use]
pub const fn advertise_fast(bonded: bool, fast_cycles: u32) -> bool {
    !bonded || fast_cycles < ADV_FAST_CYCLES
}

/// Whether the backlight may blank.
/// Post: true only with `Menu::Closed` and `LED_IDLE_OFF_MS` of inactivity; an
/// open menu never blanks, so a hold's progress fill stays visible.
#[must_use]
pub fn backlight_blank(now: u64, last_activity: u64, menu: Menu) -> bool {
    menu == Menu::Closed && now.saturating_sub(last_activity) >= LED_IDLE_OFF_MS
}

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

/// Longest advertised local name that fits the 31-byte legacy payload alongside
/// the rest: 3 (flags) + 4 (16-bit HID UUID) + 2 (name header) = 9, leaving 22.
pub const MAX_ADV_NAME_LEN: usize = 22;

/// Write the advertised local name for `slot` into `buf`; `PAIRING_SUFFIX` is
/// appended while unbonded. The GATT Generic Access name stays stable.
/// # Panics
/// If `buf` is shorter than the name -- callers size it from `MAX_ADV_NAME_LEN`.
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
    /// `+` held toward opening the menu, carrying 0..=100 hold progress.
    Entering(u8),
    Open,
    /// A slot key held, carrying 0..=100 progress toward the destructive clear
    /// at `CLEAR_HOLD_MS`; releasing below the threshold only selects.
    Holding {
        slot: u8,
        progress: u8,
    },
}

/// Percent (0..=100) of the way through a hold of `hold_ms` begun at `since`.
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

/// Linear ramp between two `u8` endpoints by a 0..=100 percentage. Post: the
/// result always lies between the endpoints, so the conversion cannot fail.
#[must_use]
fn ramp(from: u8, to: u8, progress: u8) -> u8 {
    let p = i32::from(progress);
    let a = i32::from(from);
    let b = i32::from(to);
    u8::try_from(a + ((b - a) * p / 100)).unwrap_or(from)
}

/// Idle tint level: the configured slot colour scaled to this brightness.
pub const IDLE_LEVEL: u8 = 12;

const MENU_BONDED_LEVEL: u8 = 100;
const MENU_EMPTY_LEVEL: u8 = 28;
/// The active slot pulses by dropping to a quarter of its own level.
const MENU_PULSE_DIVISOR: u16 = 4;

/// Scale an RGB colour by a 0..=255 level, channel-wise.
#[must_use]
pub const fn scale(c: [u8; 3], level: u8) -> [u8; 3] {
    [
        (c[0] as u16 * level as u16 / 255) as u8,
        (c[1] as u16 * level as u16 / 255) as u8,
        (c[2] as u16 * level as u16 / 255) as u8,
    ]
}

/// The `+` key while its hold builds: amber, ramping dim to bright.
#[must_use]
pub fn enter_fill(progress: u8) -> [u8; 3] {
    [ramp(90, 255, progress), ramp(40, 130, progress), 0]
}

/// Menu colours: identity hue per slot, bond state in brightness (bonded
/// bright, empty dim), active slot pulsing down from its own base.
/// Post: a held slot renders amber->red regardless of its identity colour, so a
/// user preference can never disguise the destructive gesture.
#[must_use]
pub fn menu_colors(
    bonded: [bool; SLOT_COUNT],
    active: u8,
    menu: Menu,
    now: u64,
    colors: [[u8; 3]; SLOT_COUNT],
) -> [[u8; 3]; SLOT_COUNT] {
    core::array::from_fn(|i| {
        if let Menu::Holding { slot, progress } = menu
            && slot == i as u8
        {
            return [ramp(70, 150, progress), ramp(55, 0, progress), 0];
        }
        let own = if bonded[i] {
            MENU_BONDED_LEVEL
        } else {
            MENU_EMPTY_LEVEL
        };
        let level = if i == active as usize && (now / 400).is_multiple_of(2) {
            (u16::from(own) / MENU_PULSE_DIVISOR) as u8
        } else {
            own
        };
        scale(colors[i], level)
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
                    self.state = State::PlusTap(now);
                    input.keys = PLUS;
                } else if pressed != PLUS {
                    self.state = State::Passthrough;
                    input.keys = pressed;
                } else if now - since >= MENU_HOLD_MS {
                    self.state = State::MenuRelease;
                    input.menu = Menu::Open;
                } else {
                    input.menu = Menu::Entering(hold_progress(since, now, MENU_HOLD_MS));
                }
            }
            State::PlusTap(since) => {
                input.keys = pressed;
                if now - since < TAP_HOLD_MS {
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

    /// Milliseconds from `now` until `update` must run again even with no key
    /// change, or `None` when the state machine can idle indefinitely.
    /// Post: every state that owns a timer reports it; states waiting only on a
    /// key release report `None`. The tests below pin that mapping down.
    #[must_use]
    pub fn next_deadline(&self, now: u64) -> Option<u64> {
        let due = match self.state {
            // These wait on a key release, not on the clock.
            State::Ready | State::MenuRelease | State::Passthrough | State::Drain => return None,
            State::PlusPending(since) => since + MENU_HOLD_MS,
            State::PlusTap(since) => since + TAP_HOLD_MS,
            State::Menu(since) => since + MENU_TIMEOUT_MS,
            State::Choosing { since, .. } => since + CLEAR_HOLD_MS,
        };
        Some(due.saturating_sub(now))
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
        for (time, raw) in [(4_000, key), (4_005, 0), (4_010, key), (4_029, key)] {
            assert_eq!(c.update(time, d.update(time, raw)).action, None);
        }
        assert_eq!(c.update(4_030, d.update(4_030, key)).action, None);
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
    fn scale_is_exact_at_the_ends_and_proportional_between() {
        let c = [255u8, 128, 64];
        assert_eq!(scale(c, 0), [0, 0, 0]);
        assert_eq!(scale(c, 255), c, "full level must be the colour unchanged");
        assert_eq!(scale(c, 128), [128, 64, 32]);
        assert_eq!(scale([0, 0, 0], 255), [0, 0, 0], "black stays black");
    }

    #[test]
    fn menu_keeps_each_slots_own_hue_and_uses_brightness_for_bond_state() {
        let colors = [[255, 0, 216], [0, 216, 255], [255, 216, 0]];
        let c = menu_colors([true, false, true], 9, Menu::Open, 0, colors);
        let empty: [[u8; 3]; SLOT_COUNT] =
            core::array::from_fn(|i| scale(colors[i], MENU_EMPTY_LEVEL));
        assert_eq!(c[0], scale(colors[0], MENU_BONDED_LEVEL));
        assert_eq!(c[1], scale(colors[1], MENU_EMPTY_LEVEL));
        assert_eq!(c[2], scale(colors[2], MENU_BONDED_LEVEL));

        for (i, is_bonded) in [true, false, true].iter().enumerate() {
            if !is_bonded {
                continue;
            }
            for ch in 0..3 {
                assert!(
                    c[i][ch] >= 3 * empty[i][ch] || c[i][ch] == 0,
                    "slot {i} channel {ch}: bonded {} is not clearly above empty {}",
                    c[i][ch],
                    empty[i][ch]
                );
            }
        }
    }

    #[test]
    fn the_active_slot_pulses_downward_from_its_own_level() {
        let colors = [[255, 0, 216], [0, 216, 255], [255, 216, 0]];
        let (steady, pulsed) = (
            menu_colors([true, false, true], 0, Menu::Open, 400, colors)[0],
            menu_colors([true, false, true], 0, Menu::Open, 800, colors)[0],
        );
        assert_eq!(steady, scale(colors[0], MENU_BONDED_LEVEL));
        assert!(
            pulsed[0] < steady[0] && pulsed[2] < steady[2],
            "active pulse {pulsed:?} did not drop below {steady:?}"
        );
        let (es, ep) = (
            menu_colors([true, false, true], 1, Menu::Open, 400, colors)[1],
            menu_colors([true, false, true], 1, Menu::Open, 800, colors)[1],
        );
        assert_eq!(es, scale(colors[1], MENU_EMPTY_LEVEL));
        assert!(
            ep[1] < es[1] && ep[2] < es[2],
            "active empty pulse must drop too, not rise into bonded"
        );
    }

    #[test]
    fn a_destructive_hold_ignores_the_slot_colour_entirely() {
        let colors = [[255, 216, 0], [255, 216, 0], [255, 216, 0]];
        let held = menu_colors(
            [true, true, true],
            0,
            Menu::Holding {
                slot: 2,
                progress: 0,
            },
            0,
            colors,
        )[2];
        assert_eq!(held, [70, 55, 0], "hold must start amber, not yellow");
        let full = menu_colors(
            [true, true, true],
            0,
            Menu::Holding {
                slot: 2,
                progress: 100,
            },
            0,
            colors,
        )[2];
        assert_eq!(full, [150, 0, 0], "hold must end red");
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
        assert_eq!(c.update(12_900, 0).action, Some(Action::Select(1)));
    }

    #[test]
    fn held_slot_shifts_amber_to_red_as_the_clear_approaches() {
        let colors = [[255u8, 0, 216], [0, 216, 255], [255, 216, 0]];
        let at = |p: u8| {
            menu_colors(
                [false, false, false],
                0,
                Menu::Holding {
                    slot: 1,
                    progress: p,
                },
                0,
                colors,
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
        let mut prev = at(0);
        for p in 1..=100 {
            let cur = at(p);
            assert!(cur[0] >= prev[0], "red must not dip at {p}: {cur:?}");
            assert!(cur[1] <= prev[1], "green must not rise at {p}: {cur:?}");
            prev = cur;
        }
        // A non-held slot keeps its own status colour while another is held.
        assert_eq!(
            menu_colors(
                [false, false, true],
                0,
                Menu::Holding {
                    slot: 1,
                    progress: 100
                },
                100,
                colors
            )[2],
            scale(colors[2], MENU_BONDED_LEVEL)
        );
    }

    #[test]
    fn plus_key_fills_brighter_as_the_hold_builds() {
        let start = enter_fill(0);
        let end = enter_fill(100);
        assert!(
            end[0] > start[0],
            "must visibly brighten: {start:?} -> {end:?}"
        );
        assert!(end[1] > start[1]);
        assert!(start[0] > start[1] && end[0] > end[1], "amber, never white");
        assert_eq!(start[2], 0);
        assert_eq!(end[2], 0);
        assert!(start[0] > 0, "visible even at zero progress");
    }

    #[test]
    fn backlight_blanks_only_after_the_idle_timeout() {
        let t = LED_IDLE_OFF_MS;
        assert!(!backlight_blank(0, 0, Menu::Closed));
        assert!(!backlight_blank(t - 1, 0, Menu::Closed));
        assert!(backlight_blank(t, 0, Menu::Closed));
        assert!(backlight_blank(t * 10, 0, Menu::Closed));
        assert!(!backlight_blank(t, t / 2, Menu::Closed));
        assert!(backlight_blank(t + t / 2, t / 2, Menu::Closed));
    }

    #[test]
    fn an_open_menu_never_blanks_mid_gesture() {
        let t = LED_IDLE_OFF_MS + 9_999;
        assert!(!backlight_blank(t, 0, Menu::Open));
        assert!(!backlight_blank(t, 0, Menu::Entering(50)));
        assert!(!backlight_blank(
            t,
            0,
            Menu::Holding {
                slot: 1,
                progress: 100
            }
        ));
        assert!(backlight_blank(t, 0, Menu::Closed));
    }

    #[test]
    fn backlight_blank_saturates_on_a_clock_that_went_backwards() {
        assert!(!backlight_blank(1_000, 5_000, Menu::Closed));
        assert!(!backlight_blank(0, u64::MAX, Menu::Closed));
    }

    #[test]
    fn states_that_wait_on_a_release_report_no_deadline() {
        let mut c = Controls::new();
        assert_eq!(c.next_deadline(0), None); // Drain
        c.update(0, 0); // -> Ready
        assert_eq!(c.next_deadline(1), None);
    }

    #[test]
    fn plus_pending_reports_the_menu_hold_deadline() {
        let mut c = Controls::new();
        c.update(0, 0);
        c.update(100, PLUS); // -> PlusPending(100)
        assert_eq!(c.next_deadline(100), Some(MENU_HOLD_MS));
        assert_eq!(c.next_deadline(100 + MENU_HOLD_MS - 1), Some(1));
        assert_eq!(c.next_deadline(100 + MENU_HOLD_MS + 500), Some(0));
    }

    #[test]
    fn a_plus_tap_reports_the_reinject_deadline() {
        let mut c = Controls::new();
        c.update(0, 0);
        c.update(100, PLUS);
        c.update(101, 0); // released early -> PlusTap(101)
        assert_eq!(c.next_deadline(101), Some(TAP_HOLD_MS));
        assert_eq!(c.next_deadline(101 + TAP_HOLD_MS - 1), Some(1));
    }

    #[test]
    fn menu_and_slot_hold_report_their_own_deadlines() {
        let mut c = Controls::new();
        c.update(0, 0);
        c.update(1, PLUS);
        c.update(1 + MENU_HOLD_MS, PLUS); // -> MenuRelease
        assert_eq!(c.next_deadline(1 + MENU_HOLD_MS), None);
        let since = 1 + MENU_HOLD_MS + 10;
        c.update(since, 0); // -> Menu(since)
        assert_eq!(c.next_deadline(since), Some(MENU_TIMEOUT_MS));
        c.update(since + 100, SLOT_KEYS[0]); // -> Choosing{ slot: 0, .. }
        assert_eq!(c.next_deadline(since + 100), Some(CLEAR_HOLD_MS));
    }

    #[test]
    fn an_unbonded_slot_never_settles_its_advertisement() {
        assert!(advertise_fast(false, 0));
        assert!(advertise_fast(false, ADV_FAST_CYCLES));
        assert!(advertise_fast(false, u32::MAX));
    }

    #[test]
    fn a_bonded_slot_settles_only_after_the_fast_window() {
        assert!(advertise_fast(true, ADV_FAST_CYCLES - 1));
        assert!(!advertise_fast(true, ADV_FAST_CYCLES));
        assert!(!advertise_fast(true, ADV_FAST_CYCLES + 500));
    }
}
