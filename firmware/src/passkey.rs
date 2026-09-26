//! Passkey entry: during MITM-protected pairing the pad is the input device.
//!
//! Pre: the peer declared IO capabilities that select `PassKeyEntry` with the
//! peripheral inputting, so the SMP layer raises `PassKeyInput` and displays
//! the 6-digit code itself.
//! Post: six accumulated decimal digits are handed back for `pass_key_input`,
//! or the attempt is cancelled. Nothing here touches SMP.

use crate::config::DEFAULT_KEYMAP;

/// A BLE passkey is a decimal number in `0..=999_999`.
pub const PASSKEY_DIGITS: u8 = 6;

/// Positions on the numeric block.
const DIGIT_COUNT: u8 = 10;

/// Fail closed rather than hold the pad hostage: matches the SMP signalling
/// timeout, so the peer has not already given up on us.
pub const ENTRY_TIMEOUT_MS: u64 = 30_000;

pub const MARK_LEVEL: u8 = 60;
pub const FILL_LEVEL: u8 = 180;
pub const HIT_LEVEL: u8 = 255;

/// Physical key bit per digit, derived from the factory layout so the digits
/// always sit where the pad is printed regardless of how the user remapped it.
pub const DIGIT_KEYS: [u16; 10] = derived_keys();

/// Restart the entry without aborting pairing.
pub const CLEAR_KEY: u16 = derived_key(0x58); // Keypad Enter

/// Abort the pairing attempt.
pub const CANCEL_KEY: u16 = derived_key(0x57); // Keypad +

const fn derived_key(usage: u8) -> u16 {
    let mut i = 0usize;
    while i < DEFAULT_KEYMAP.len() {
        if DEFAULT_KEYMAP[i] == usage {
            return 1 << i;
        }
        i += 1;
    }
    panic!("usage missing from DEFAULT_KEYMAP")
}

const fn derived_keys() -> [u16; 10] {
    // Keypad 1..9 are usages 0x59..=0x61; Keypad 0 is 0x62.
    let mut out = [0u16; 10];
    let mut d: u8 = 0;
    while d < DIGIT_COUNT {
        let usage = if d == 0 { 0x62 } else { 0x58 + d };
        out[d as usize] = derived_key(usage);
        d += 1;
    }
    out
}

/// Which digit a mask refers to. Takes the lowest match, so callers that must
/// reject ambiguity check the popcount themselves.
fn digit_for(bits: u16) -> Option<u8> {
    let mut d: u8 = 0;
    while d < DIGIT_COUNT {
        if DIGIT_KEYS[d as usize] & bits != 0 {
            return Some(d);
        }
        d += 1;
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Digit { digit: u8, count: u8 },
    Commit(u32),
    Clear,
    Cancel,
    Timeout,
}

/// Decimal passkey accumulator. Driven by debounced press masks; acts on rising
/// edges only, so a held key never repeats a digit.
#[derive(Clone, Copy)]
pub struct Entry {
    active: bool,
    digits: u32,
    count: u8,
    last: Option<u8>,
    prev: u16,
    deadline: u64,
    outcome: Outcome,
}

impl Default for Entry {
    fn default() -> Self {
        Self::new()
    }
}

impl Entry {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            active: false,
            digits: 0,
            count: 0,
            last: None,
            prev: 0,
            deadline: 0,
            outcome: Outcome::None,
        }
    }

    /// Start collecting. `pressed` is the current mask, taken as already-down so
    /// a key resting under a finger when pairing begins is not read as a digit.
    pub fn begin(&mut self, now: u64, pressed: u16) {
        *self = Self {
            active: true,
            prev: pressed,
            deadline: now.saturating_add(ENTRY_TIMEOUT_MS),
            ..Self::new()
        };
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Milliseconds until the entry must be abandoned, if still active.
    #[must_use]
    pub fn deadline_in(&self, now: u64) -> Option<u64> {
        self.active.then(|| self.deadline.saturating_sub(now))
    }

    #[cfg(test)]
    #[must_use]
    pub const fn count(&self) -> u8 {
        self.count
    }

    #[cfg(test)]
    #[must_use]
    pub const fn last_digit(&self) -> Option<u8> {
        self.last
    }

    /// Take the pending outcome, leaving `None`.
    pub fn take_outcome(&mut self) -> Outcome {
        core::mem::replace(&mut self.outcome, Outcome::None)
    }

    pub fn end(&mut self) {
        *self = Self::new();
    }

    /// Feed one debounced mask. Pre: `pressed` is the debounced pressed mask.
    /// Post: at most one outcome is produced per call; a two-key chord is
    /// ambiguous and consumed with no effect, so a mistyped digit can only come
    /// from a key the user actually meant.
    pub fn update(&mut self, now: u64, pressed: u16) -> Outcome {
        if !self.active {
            return Outcome::None;
        }
        let outcome = self.step(now, pressed);
        self.prev = pressed;
        self.outcome = outcome;
        outcome
    }

    fn step(&mut self, now: u64, pressed: u16) -> Outcome {
        if now >= self.deadline {
            self.active = false;
            return Outcome::Timeout;
        }
        let fresh = pressed & !self.prev;
        if fresh == 0 {
            return Outcome::None;
        }
        if fresh & CANCEL_KEY != 0 {
            self.active = false;
            return Outcome::Cancel;
        }
        if fresh & CLEAR_KEY != 0 {
            self.digits = 0;
            self.count = 0;
            self.last = None;
            return Outcome::Clear;
        }
        // A chord of two digit keys is ambiguous; require a release instead of
        // guessing which digit the user meant.
        if (fresh & digit_mask()).count_ones() != 1 {
            return Outcome::None;
        }
        let Some(digit) = digit_for(fresh) else {
            return Outcome::None;
        };
        self.digits = self
            .digits
            .saturating_mul(10)
            .saturating_add(u32::from(digit));
        self.count += 1;
        self.last = Some(digit);
        if self.count >= PASSKEY_DIGITS {
            self.active = false;
            return Outcome::Commit(self.digits);
        }
        Outcome::Digit {
            digit,
            count: self.count,
        }
    }

    /// Frame for the LED field: digit keys stay marked so the user can see which
    /// keys are live, brighten as the passkey fills, and the last key pressed
    /// stands out. Non-digit keys go dark.
    #[must_use]
    pub fn frame(&self, color: [u8; 3]) -> [[u8; 3]; 16] {
        let span = u16::from(FILL_LEVEL - MARK_LEVEL);
        let level = (u16::from(MARK_LEVEL)
            + u16::from(self.count) * span / u16::from(PASSKEY_DIGITS))
        .min(255) as u8;
        core::array::from_fn(|i| {
            let bit = 1u16 << i;
            if DIGIT_KEYS.iter().any(|k| *k & bit != 0) {
                let lvl = if self.last.is_some_and(|d| DIGIT_KEYS[d as usize] & bit != 0) {
                    HIT_LEVEL
                } else {
                    level
                };
                crate::host_slots::scale(color, lvl)
            } else {
                [0, 0, 0]
            }
        })
    }
}

/// All ten digit bits, for callers that need the set rather than the mapping.
#[must_use]
pub const fn digit_mask() -> u16 {
    let mut m = 0u16;
    let mut d: u8 = 0;
    while d < DIGIT_COUNT {
        m |= DIGIT_KEYS[d as usize];
        d += 1;
    }
    m
}

#[cfg(test)]
#[path = "config.rs"]
mod config;

#[cfg(test)]
#[allow(dead_code)] // pulled in for `scale`; the rest of it is unused here
#[path = "host_slots.rs"]
mod host_slots;

#[cfg(test)]
mod tests {
    use super::*;

    fn press(e: &mut Entry, now: u64, digit: u8) -> Outcome {
        e.update(now, DIGIT_KEYS[digit as usize])
    }

    #[test]
    fn every_digit_maps_to_its_own_key_and_all_ten_are_distinct() {
        let mut seen = 0u16;
        for d in 0..10u8 {
            let k = DIGIT_KEYS[d as usize];
            assert_ne!(k, 0, "digit {d} has no key");
            assert_eq!(k.count_ones(), 1, "digit {d} maps to more than one bit");
            assert_eq!(k & seen, 0, "digit {d} shares a key");
            seen |= k;
        }
        assert_eq!(seen, digit_mask());
    }

    #[test]
    fn digits_land_on_the_factory_numpad_positions() {
        // Factory layout: 7 8 9 / | 4 5 6 * | 1 2 3 - | 0 . Enter +
        assert_eq!(DIGIT_KEYS[7], 1 << 0);
        assert_eq!(DIGIT_KEYS[8], 1 << 1);
        assert_eq!(DIGIT_KEYS[9], 1 << 2);
        assert_eq!(DIGIT_KEYS[4], 1 << 4);
        assert_eq!(DIGIT_KEYS[5], 1 << 5);
        assert_eq!(DIGIT_KEYS[6], 1 << 6);
        assert_eq!(DIGIT_KEYS[1], 1 << 8);
        assert_eq!(DIGIT_KEYS[2], 1 << 9);
        assert_eq!(DIGIT_KEYS[3], 1 << 10);
        assert_eq!(DIGIT_KEYS[0], 1 << 12);
        assert_eq!(CLEAR_KEY, 1 << 14);
        assert_eq!(CANCEL_KEY, 1 << 15);
    }

    #[test]
    fn six_digits_accumulate_decimal_and_commit() {
        let mut e = Entry::new();
        e.begin(0, 0);
        let mut n = 0u8;
        for d in [1u8, 2, 3, 4, 5] {
            n += 1;
            let t = u64::from(n) * 100;
            assert_eq!(press(&mut e, t, d), Outcome::Digit { digit: d, count: n });
            e.update(t + 50, 0);
        }
        assert_eq!(press(&mut e, 600, 6), Outcome::Commit(123_456));
        assert!(!e.is_active());
    }

    #[test]
    fn leading_zeros_are_part_of_the_number_not_bitflags() {
        // 000123 must accumulate to 123, not lose the leading zeros to a
        // numeric type that cannot represent them.
        let mut e = Entry::new();
        e.begin(0, 0);
        for d in [0u8, 0, 0, 1, 2] {
            press(&mut e, 10, d);
            e.update(20, 0);
        }
        assert_eq!(e.update(30, DIGIT_KEYS[3]), Outcome::Commit(123));
    }

    #[test]
    fn a_key_held_from_before_begin_is_not_read_as_a_digit() {
        let mut e = Entry::new();
        e.begin(0, DIGIT_KEYS[9]);
        assert_eq!(e.update(10, DIGIT_KEYS[9]), Outcome::None);
        assert_eq!(e.count(), 0);
    }

    #[test]
    fn holding_a_digit_does_not_repeat_it() {
        let mut e = Entry::new();
        e.begin(0, 0);
        press(&mut e, 10, 5);
        for t in 20..200 {
            assert_eq!(e.update(t, DIGIT_KEYS[5]), Outcome::None);
        }
        assert_eq!(e.count(), 1);
    }

    #[test]
    fn a_two_digit_chord_is_ambiguous_and_types_nothing() {
        let mut e = Entry::new();
        e.begin(0, 0);
        let both = DIGIT_KEYS[1] | DIGIT_KEYS[2];
        assert_eq!(e.update(10, both), Outcome::None);
        assert_eq!(e.count(), 0);
        // Released, then a single key still works.
        e.update(20, 0);
        assert_eq!(
            e.update(30, DIGIT_KEYS[1]),
            Outcome::Digit { digit: 1, count: 1 }
        );
    }

    #[test]
    fn clear_restarts_the_count_without_leaving_the_mode() {
        let mut e = Entry::new();
        e.begin(0, 0);
        press(&mut e, 10, 7);
        press(&mut e, 20, 8);
        e.update(25, 0);
        assert_eq!(e.update(30, CLEAR_KEY), Outcome::Clear);
        assert!(e.is_active());
        assert_eq!(e.count(), 0);
        assert_eq!(e.last_digit(), None);
        e.update(35, 0);
        // Restarted from zero, so six more digits are needed.
        for d in 1..6u8 {
            press(&mut e, 40 + u64::from(d) * 10, d);
            e.update(45 + u64::from(d) * 10, 0);
        }
        assert_eq!(press(&mut e, 100, 9), Outcome::Commit(123_459));
    }

    #[test]
    fn cancel_ends_the_attempt_and_swallows_later_keys() {
        let mut e = Entry::new();
        e.begin(0, 0);
        press(&mut e, 10, 1);
        e.update(15, 0);
        assert_eq!(e.update(20, CANCEL_KEY), Outcome::Cancel);
        assert!(!e.is_active());
        assert_eq!(e.update(30, DIGIT_KEYS[1]), Outcome::None);
        assert_eq!(e.deadline_in(30), None);
    }

    #[test]
    fn an_abandoned_entry_times_out_and_closes() {
        let mut e = Entry::new();
        e.begin(1_000, 0);
        assert_eq!(e.deadline_in(1_000), Some(ENTRY_TIMEOUT_MS));
        press(&mut e, 2_000, 4);
        e.update(2_010, 0);
        assert_eq!(e.update(1_000 + ENTRY_TIMEOUT_MS - 1, 0), Outcome::None);
        assert_eq!(e.update(1_000 + ENTRY_TIMEOUT_MS, 0), Outcome::Timeout);
        assert!(!e.is_active());
    }

    #[test]
    fn take_outcome_is_single_use() {
        let mut e = Entry::new();
        e.begin(0, 0);
        press(&mut e, 10, 3);
        assert_eq!(e.take_outcome(), Outcome::Digit { digit: 3, count: 1 });
        assert_eq!(e.take_outcome(), Outcome::None);
    }

    #[test]
    fn end_resets_so_a_later_attempt_starts_clean() {
        let mut e = Entry::new();
        e.begin(0, 0);
        press(&mut e, 10, 4);
        e.end();
        assert!(!e.is_active());
        assert_eq!(e.count(), 0);
        e.begin(5_000, 0);
        assert_eq!(e.count(), 0);
        assert_eq!(e.deadline_in(5_000), Some(ENTRY_TIMEOUT_MS));
    }

    #[test]
    fn the_frame_marks_digits_and_leaves_the_rest_dark() {
        let f = Entry::new().frame([255, 0, 216]);
        for (i, led) in f.iter().enumerate() {
            let bit = 1u16 << i;
            let is_digit = DIGIT_KEYS.iter().any(|k| *k & bit != 0);
            if is_digit {
                assert_ne!(*led, [0, 0, 0], "digit key {i} must be marked");
            } else {
                assert_eq!(*led, [0, 0, 0], "non-digit key {i} must be dark");
            }
        }
        assert_eq!(f[3], [0, 0, 0]); // '/'
        assert_eq!(f[15], [0, 0, 0]); // '+'
        assert_ne!(f[0], [0, 0, 0]); // '7'
    }

    #[test]
    fn the_frame_brightens_as_the_passkey_fills_and_highlights_the_last_key() {
        let color = [0, 216, 255];
        let empty = Entry::new().frame(color);
        let mut e = Entry::new();
        e.begin(0, 0);
        press(&mut e, 10, 5);
        let filled = e.frame(color);
        let marked: Vec<u16> = (0..16)
            .filter(|i| DIGIT_KEYS.iter().any(|k| *k & (1 << i) != 0))
            .map(|i| u16::from(filled[i][2]))
            .collect();
        assert!(
            marked.iter().all(|v| *v >= u16::from(empty[0][2])),
            "filling must not dim any digit key"
        );
        assert!(
            filled[5][2] > empty[5][2],
            "progress must raise the marked level"
        );
        assert_eq!(
            filled[5][2], HIT_LEVEL,
            "the last key pressed must stand out"
        );
    }
}
