//! Link power profiles: radio duty follows user activity, not the clock alone.
//!
//! Pre: the caller reports whether the backlight has blanked for idleness and
//! whether USB is driving the device.
//! Post: at most one parameter request is outstanding, requests never thrash,
//! and repeated refusals stop asking instead of hammering the host.

/// Connection parameters to request, in milliseconds. Kept as plain integers
/// rather than the stack's own type so the policy is testable off-target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub interval_min_ms: u32,
    pub interval_max_ms: u32,
    pub latency: u16,
    pub supervision_ms: u32,
}

/// Typing profile. 30 ms already carries a numpad burst, so the floor only
/// matters if the host wants to go faster.
pub const ACTIVE: Params = Params {
    interval_min_ms: 15,
    interval_max_ms: 30,
    latency: 0,
    supervision_ms: 2_000,
};

/// Idle profile. Roughly a quarter of the wake rate, paid for in first-keystroke
/// latency. Latency is deliberately zero: a slave skipped for L events saves
/// transmissions but still wakes to listen each interval, so the interval is
/// the lever and latency would only add delay without buying much.
pub const IDLE: Params = Params {
    interval_min_ms: 100,
    interval_max_ms: 150,
    latency: 0,
    supervision_ms: 2_000,
};

/// Do not send another request this soon after one, whatever the state says.
/// Absorbs a host that applies something different from what we asked for.
pub const REQUEST_COOLDOWN_MS: u64 = 10_000;

/// A request with no observed effect for this long is treated as not having
/// landed, so the policy can try again later instead of wedging.
pub const REQUEST_ACK_MS: u64 = 5_000;

/// Give up after this many refusals. A host that will not grant it is not going
/// to, and a request every cooldown is worse than no request at all.
pub const MAX_REFUSALS: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Active,
    Idle,
}

impl Profile {
    #[must_use]
    pub const fn params(self) -> Params {
        match self {
            Self::Active => ACTIVE,
            Self::Idle => IDLE,
        }
    }
}

impl Params {
    /// Mirror of the link layer's own validity rules, checked here so a bad
    /// profile fails a host test instead of an HCI command at runtime.
    /// Post: bounds on interval range and latency, and the supervision timeout
    /// long enough that a string of missed events is not read as a dropped link.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.interval_min_ms <= self.interval_max_ms
            && self.interval_min_ms >= 7
            && self.interval_max_ms <= 4_000
            && self.latency < 500
            && self.supervision_ms >= 100
            && self.supervision_ms <= 32_000
            && (self.supervision_ms as u64) * 1_000
                > 2 * (self.latency as u64 + 1) * (self.interval_max_ms as u64) * 1_000
    }
}

/// When to ask the host for which profile, and when to stop asking.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    applied: Option<Profile>,
    in_flight: Option<Profile>,
    last_request: Option<u64>,
    refusals: u8,
    muted: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self::new()
    }
}

impl Policy {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            applied: None,
            in_flight: None,
            last_request: None,
            refusals: 0,
            muted: false,
        }
    }

    /// The profile the current state calls for. On USB the target is always
    /// Active: VBUS is paying for the device, so trading keystroke latency for
    /// battery saves nothing.
    #[must_use]
    pub const fn target(blank: bool, usb: bool) -> Profile {
        if blank && !usb {
            Profile::Idle
        } else {
            Profile::Active
        }
    }

    /// Feed one observation. Post: `Some(profile)` means send that request now,
    /// and the request is recorded as in flight.
    pub fn update(&mut self, now: u64, blank: bool, usb: bool) -> Option<Profile> {
        if self.muted {
            return None;
        }
        // `Option` rather than a 0 sentinel: a request made at t=0 is a real
        // request, and reading it as "never asked" would skip the cooldown.
        let since = self.last_request.map(|at| now.saturating_sub(at));
        if self.in_flight.is_some() {
            match since {
                Some(elapsed) if elapsed < REQUEST_ACK_MS => return None,
                _ => self.in_flight = None,
            }
        }
        let want = Self::target(blank, usb);
        if self.applied == Some(want) {
            return None;
        }
        if since.is_some_and(|elapsed| elapsed < REQUEST_COOLDOWN_MS) {
            return None;
        }
        self.in_flight = Some(want);
        self.last_request = Some(now);
        Some(want)
    }

    /// The host moved the link. Post: the profile is considered applied even if
    /// the host picked different numbers, because the intent landed.
    pub fn confirmed(&mut self, profile: Profile) {
        self.applied = Some(profile);
        self.in_flight = None;
        self.refusals = 0;
    }

    /// The host rejected or ignored the request. Post: after `MAX_REFUSALS` the
    /// policy mutes itself permanently.
    pub fn refused(&mut self) {
        self.in_flight = None;
        self.refusals = self.refusals.saturating_add(1);
        if self.refusals >= MAX_REFUSALS {
            self.muted = true;
        }
    }

    /// Adopt a profile the host chose on its own, so we do not fight it.
    pub fn observe(&mut self, profile: Profile) {
        self.applied = Some(profile);
    }

    #[must_use]
    pub const fn applied(&self) -> Option<Profile> {
        self.applied
    }

    #[must_use]
    pub const fn is_muted(&self) -> bool {
        self.muted
    }
}

/// Classify parameters the host applied into a profile, so a host-driven change
/// is recognised instead of argued with. Post: `Idle` only when the interval is
/// clearly a sleeping one; anything tighter counts as Active.
#[must_use]
pub fn classify(params: Params) -> Profile {
    if params.interval_min_ms >= IDLE.interval_min_ms {
        Profile::Idle
    } else {
        Profile::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_profiles_satisfy_the_link_layers_own_bounds() {
        assert!(ACTIVE.is_valid(), "active profile is out of spec");
        assert!(IDLE.is_valid(), "idle profile is out of spec");
    }

    #[test]
    // The constants are the subject here: if someone retunes either profile so
    // idle stops being a real reduction in wake rate, this is what says so.
    #[allow(clippy::assertions_on_constants)]
    fn the_idle_profile_actually_saves_wakes_and_the_active_one_does_not() {
        assert!(
            IDLE.interval_min_ms >= 3 * ACTIVE.interval_max_ms,
            "idle must be a real reduction in wake rate, not a token one"
        );
    }

    #[test]
    fn an_interval_outside_the_bounds_is_caught_here_not_at_runtime() {
        assert!(
            !Params {
                interval_min_ms: 5,
                interval_max_ms: 30,
                latency: 0,
                supervision_ms: 2_000,
            }
            .is_valid()
        );
        assert!(
            !Params {
                interval_min_ms: 100,
                interval_max_ms: 9_000,
                latency: 0,
                supervision_ms: 2_000,
            }
            .is_valid()
        );
        // 2 * (latency+1) * interval must be under the supervision timeout or a
        // run of missed events drops a healthy link.
        assert!(
            !Params {
                interval_min_ms: 1_000,
                interval_max_ms: 1_000,
                latency: 4,
                supervision_ms: 9_000,
            }
            .is_valid()
        );
    }

    #[test]
    fn a_blank_backlight_asks_for_the_idle_profile() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        p.confirmed(Profile::Active);
        let t = super::REQUEST_COOLDOWN_MS + 1;
        assert_eq!(p.update(t, true, false), Some(Profile::Idle));
    }

    #[test]
    fn usb_never_asks_for_idle_because_vbus_is_already_paying() {
        let mut p = Policy::new();
        assert_eq!(Policy::target(true, true), Profile::Active);
        assert_eq!(Policy::target(true, false), Profile::Idle);
        assert_eq!(Policy::target(false, true), Profile::Active);
        // Once Active is in effect, a blanked backlight on USB asks for
        // nothing: there is no battery to save.
        assert_eq!(p.update(0, false, true), Some(Profile::Active));
        p.confirmed(Profile::Active);
        let t = super::REQUEST_COOLDOWN_MS + 1;
        assert_eq!(p.update(t, true, true), None);
    }

    #[test]
    fn it_does_not_re_ask_for_a_profile_that_is_already_applied() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        p.confirmed(Profile::Active);
        let t = super::REQUEST_COOLDOWN_MS + 1;
        assert_eq!(p.update(t, false, false), None);
    }

    #[test]
    fn requests_are_rate_limited_so_a_picky_host_cannot_cause_thrash() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        p.refused();
        // Still inside the cooldown: no second request even though the target
        // changed.
        assert_eq!(p.update(1_000, true, false), None);
        assert_eq!(p.update(super::REQUEST_COOLDOWN_MS - 1, true, false), None);
        assert_eq!(
            p.update(super::REQUEST_COOLDOWN_MS, true, false),
            Some(Profile::Idle)
        );
    }

    #[test]
    fn a_request_stays_in_flight_and_does_not_stack_another() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        assert_eq!(p.update(1, true, false), None);
        assert_eq!(p.update(super::REQUEST_ACK_MS - 1, true, false), None);
    }

    #[test]
    fn a_silent_request_eventually_releases_so_the_policy_cannot_wedge() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        // The ack window elapsed with no confirmation, so the next cooldown may
        // re-request rather than sitting on a request that never landed.
        let t = super::REQUEST_COOLDOWN_MS.max(super::REQUEST_ACK_MS) + 1;
        assert_eq!(p.update(t, true, false), Some(Profile::Idle));
    }

    #[test]
    fn repeated_refusals_mute_the_policy_instead_of_hammering_the_host() {
        let mut p = Policy::new();
        let mut asked = 0;
        let mut t = 0u64;
        while t < 100 * super::REQUEST_COOLDOWN_MS {
            if p.update(t, t.is_multiple_of(2), false).is_some() {
                asked += 1;
                p.refused();
            }
            t += super::REQUEST_COOLDOWN_MS;
        }
        assert!(p.is_muted(), "policy must give up after repeated refusals");
        assert!(
            asked <= super::MAX_REFUSALS as usize,
            "asked {asked} times, must stop at {}",
            super::MAX_REFUSALS
        );
        assert_eq!(p.update(t + super::REQUEST_COOLDOWN_MS, true, false), None);
    }

    #[test]
    fn a_host_applying_our_request_clears_the_refusal_counter() {
        let mut p = Policy::new();
        p.update(0, false, false);
        p.refused();
        p.refused();
        p.confirmed(Profile::Idle);
        assert!(!p.is_muted());
        // One more refusal from a clean slate must not mute on its own.
        p.update(super::REQUEST_COOLDOWN_MS + 1, false, false);
        p.refused();
        assert!(!p.is_muted());
    }

    #[test]
    fn a_host_picking_its_own_numbers_is_adopted_not_argued_with() {
        // macOS overruling us to 45 ms is not a failure to retry; it is close
        // enough to Active that re-asking would just fight the host forever.
        let mut p = Policy::new();
        p.update(0, false, false);
        p.observe(classify(Params {
            interval_min_ms: 45,
            interval_max_ms: 45,
            latency: 0,
            supervision_ms: 2_000,
        }));
        assert_eq!(p.applied(), Some(Profile::Active));
        let t = super::REQUEST_COOLDOWN_MS + 1;
        assert_eq!(p.update(t, false, false), None);
    }

    #[test]
    fn classify_separates_sleeping_intervals_from_awake_ones() {
        assert_eq!(classify(ACTIVE), Profile::Active);
        assert_eq!(classify(IDLE), Profile::Idle);
        // Straddling values count as awake, so a host that grants something
        // tidier than our idle floor is not treated as asleep.
        assert_eq!(
            classify(Params {
                interval_min_ms: IDLE.interval_min_ms - 1,
                ..IDLE
            }),
            Profile::Active
        );
    }

    #[test]
    fn going_active_is_never_delayed_by_the_idle_state() {
        // The first keypress after idle must go active immediately; a slow first
        // key is already paid for by the long interval.
        let mut p = Policy::new();
        p.update(0, false, false);
        p.confirmed(Profile::Active);
        let t = super::REQUEST_COOLDOWN_MS + 1;
        p.update(t, true, false);
        p.confirmed(Profile::Idle);
        let t2 = t + super::REQUEST_COOLDOWN_MS + 1;
        assert_eq!(p.update(t2, false, false), Some(Profile::Active));
    }
}
