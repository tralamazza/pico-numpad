//! Explicit gestures used only when saved host data cannot be loaded.

const RETRY_KEY: u16 = 1 << 15;
const RESET_KEYS: u16 = (1 << 8) | (1 << 9) | (1 << 10);
const RETRY_MS: u64 = 3_000;
const RESET_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Retry,
    ResetBonds,
}

pub struct Controls {
    draining: bool,
    candidate: u16,
    since: u64,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            draining: true,
            candidate: 0,
            since: 0,
        }
    }
}

impl Controls {
    /// Input must already be debounced. A full release is required after boot
    /// and after every action, so a failed write never retriggers a held reset.
    pub fn update(&mut self, now: u64, keys: u16) -> Option<Action> {
        if self.draining {
            if keys == 0 {
                self.draining = false;
                self.candidate = 0;
                self.since = now;
            }
            return None;
        }
        if keys != self.candidate {
            self.candidate = keys;
            self.since = now;
        }
        let action = match keys {
            RETRY_KEY if now.saturating_sub(self.since) >= RETRY_MS => Action::Retry,
            RESET_KEYS if now.saturating_sub(self.since) >= RESET_MS => Action::ResetBonds,
            _ => return None,
        };
        self.draining = true;
        Some(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_does_not_request_a_reset() {
        let mut c = Controls::default();
        c.update(0, 0);
        assert_eq!(c.update(1, RETRY_KEY), None);
        assert_eq!(c.update(3_000, RETRY_KEY), None);
        assert_eq!(c.update(3_001, RETRY_KEY), Some(Action::Retry));
        assert_eq!(c.update(9_000, RETRY_KEY), None);
    }

    #[test]
    fn reset_requires_exact_chord_for_five_seconds_and_fires_once() {
        let mut c = Controls::default();
        c.update(0, 0);
        c.update(1, RESET_KEYS);
        assert_eq!(c.update(5_000, RESET_KEYS), None);
        assert_eq!(c.update(5_001, RESET_KEYS), Some(Action::ResetBonds));
        assert_eq!(c.update(15_000, RESET_KEYS), None);
        c.update(15_001, 0);
        assert_eq!(c.update(15_002, RESET_KEYS), None);
        assert_eq!(c.update(20_001, RESET_KEYS), None);
        assert_eq!(c.update(20_002, RESET_KEYS), Some(Action::ResetBonds));
    }

    #[test]
    fn boot_held_keys_and_interrupted_chords_never_reset() {
        let mut c = Controls::default();
        assert_eq!(c.update(0, RESET_KEYS), None);
        assert_eq!(c.update(10_000, RESET_KEYS), None);
        c.update(10_001, 0);
        c.update(10_002, RESET_KEYS);
        c.update(14_000, 1 << 8);
        assert_eq!(c.update(20_000, 1 << 8), None);
        assert_eq!(c.update(20_001, RESET_KEYS), None);
        assert_eq!(c.update(24_000, RESET_KEYS), None);
    }
}
