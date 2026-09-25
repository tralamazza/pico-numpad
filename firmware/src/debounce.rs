//! Per-key debounce shared by normal typing, the host menu, and recovery.

pub const DEBOUNCE_MS: u64 = 20;

#[derive(Default)]
pub struct Debouncer {
    initialised: bool,
    stable: u16,
    candidate: u16,
    since: [u64; 16],
}

impl Debouncer {
    /// Seed the initial physical state so controls can drain keys held at boot.
    /// Subsequent transitions require a stable candidate for `DEBOUNCE_MS`.
    pub fn update(&mut self, now: u64, raw: u16) -> u16 {
        if !self.initialised {
            self.initialised = true;
            self.stable = raw;
            self.candidate = raw;
            self.since.fill(now);
        }
        for (bit, since) in self.since.iter_mut().enumerate() {
            let mask = 1 << bit;
            if (raw ^ self.candidate) & mask != 0 {
                self.candidate = (self.candidate & !mask) | (raw & mask);
                *since = now;
            }
            if now.saturating_sub(*since) >= DEBOUNCE_MS {
                self.stable = (self.stable & !mask) | (self.candidate & mask);
            }
        }
        self.stable
    }

    /// Millis from `now` until a pending transition would settle, or `None` when
    /// every key is already stable.
    ///
    /// `update` accepts a transition only once its candidate has held for
    /// `DEBOUNCE_MS`, so it needs a sample *after* that point. A polling loop
    /// gets that sample for free; an interrupt-driven loop does not, because the
    /// expander goes quiet once the line stops moving and the read that cleared
    /// INT was taken too early to settle anything.
    ///
    /// Skipping this loses presses outright (the release overwrites the
    /// candidate before it ever stabilises) or reports them a whole safety
    /// timeout late and leaves the key stuck until the next one. Any loop that
    /// sleeps between reads must wake on this.
    #[must_use]
    pub fn next_settle(&self, now: u64) -> Option<u64> {
        if !self.initialised {
            return None;
        }
        let mut pending = false;
        let mut soonest = u64::MAX;
        for (bit, since) in self.since.iter().enumerate() {
            // Only a candidate that differs from stable represents a transition
            // still waiting to be accepted.
            if (self.candidate ^ self.stable) & (1 << bit) != 0 {
                pending = true;
                soonest = soonest.min(since.saturating_add(DEBOUNCE_MS));
            }
        }
        pending.then_some(soonest.saturating_sub(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_and_release_bounce_produce_one_edge_each() {
        let mut d = Debouncer::default();
        assert_eq!(d.update(0, 0), 0);
        for (time, raw) in [(5, 1), (10, 0), (15, 1), (34, 1)] {
            assert_eq!(d.update(time, raw), 0);
        }
        assert_eq!(d.update(35, 1), 1);
        for (time, raw) in [(40, 0), (45, 1), (50, 0), (69, 0)] {
            assert_eq!(d.update(time, raw), 1);
        }
        assert_eq!(d.update(70, 0), 0);
    }

    #[test]
    fn one_bouncing_key_does_not_delay_another() {
        let mut d = Debouncer::default();
        d.update(0, 0);
        d.update(5, 1);
        d.update(10, 3);
        d.update(15, 1);
        d.update(20, 3);
        assert_eq!(d.update(25, 1), 1);
        d.update(30, 3);
        assert_eq!(d.update(50, 3), 3);
    }

    #[test]
    fn keys_held_at_boot_are_visible_to_the_initial_release_guard() {
        let mut d = Debouncer::default();
        assert_eq!(d.update(0, 0x8000), 0x8000);
        assert_eq!(d.update(5, 0), 0x8000);
        assert_eq!(d.update(25, 0), 0);
    }

    // The sample sequence an interrupt-driven loop actually produces: wake on
    // INT, read, then wake again at the settle deadline because no further
    // interrupt arrives while the key is held steady. Without that second wake
    // the press never registers -- which is exactly how it came to lose keys and
    // leave them stuck.

    #[test]
    fn interrupt_driven_sampling_sees_a_press_and_its_release() {
        let mut d = Debouncer::default();
        d.update(0, 0);
        assert_eq!(d.next_settle(0), None, "idle has nothing pending");

        // Press: INT wakes us and we read the raw transition, too early to
        // accept.
        assert_eq!(d.update(100, 1), 0);
        let settle = d
            .next_settle(100)
            .expect("a pending transition must schedule a resample");
        assert_eq!(settle, DEBOUNCE_MS);
        // The second wake accepts the press.
        assert_eq!(d.update(100 + settle, 1), 1);
        assert_eq!(d.next_settle(120), None, "settled, nothing pending");

        // Release has the same shape, and must also produce its keyup.
        assert_eq!(d.update(500, 0), 1);
        let settle = d
            .next_settle(500)
            .expect("release must also schedule a resample");
        assert_eq!(d.update(500 + settle, 0), 0);
        assert_eq!(d.next_settle(520), None);
    }

    #[test]
    fn a_blip_shorter_than_the_debounce_window_still_produces_no_key() {
        // The opposite direction: fixing the lost press must not turn every
        // sub-20ms contact into a keystroke.
        let mut d = Debouncer::default();
        d.update(0, 0);
        d.update(100, 1);
        assert_eq!(d.update(110, 0), 0);
        assert_eq!(d.next_settle(110), None, "candidate matches stable again");
    }

    #[test]
    fn settle_reports_the_earliest_of_several_pending_keys() {
        let mut d = Debouncer::default();
        d.update(0, 0);
        d.update(100, 1); // key 0 pending, due 120
        d.update(110, 3); // key 1 pending, due 130
        assert_eq!(
            d.next_settle(110),
            Some(10),
            "must wake for the earlier one"
        );
    }
}
