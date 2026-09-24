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
}
