//! Link power profiles: radio duty follows user activity, not the clock alone.
//!
//! Pre: the caller reports whether the backlight has blanked for idleness and
//! whether USB is driving the device.
//! Post: at most one parameter request is outstanding, requests never thrash,
//! and a host that will not grant the idle profile is detected rather than
//! asked forever.
//!
//! Two numbers matter and they do different jobs. The interval governs latency
//! while the pad has data, because a peripheral with something to send answers
//! at every event. The latency governs how deeply it sleeps while it has
//! nothing, because it may stay away for that many events. What the user feels
//! and what the battery pays is the product of the two.

/// Units of 1.25 ms, the link layer's own granularity. Deliberately not
/// milliseconds: the BLE floor of 7.5 ms is not an integer number of
/// milliseconds, and storing ms would silently round the floor to 6.25 ms and
/// get the request rejected.
pub const UNIT_US: u32 = 1_250;

/// The BLE minimum connection interval, in units.
pub const MIN_INTERVAL_UNITS: u16 = 6;

/// Connection parameters to request. Kept as plain integers rather than the
/// stack's own type so the policy is testable off-target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub interval_min_units: u16,
    pub interval_max_units: u16,
    pub latency: u16,
    pub supervision_ms: u32,
}

/// Typing profile: the floor interval, no latency. Every event answered, so a
/// keystroke waits at most one interval.
pub const ACTIVE: Params = Params {
    interval_min_units: MIN_INTERVAL_UNITS,
    interval_max_units: 8,
    latency: 0,
    supervision_ms: 2_000,
};

/// Idle profile: latency doing all the work. 15 ms was measured as the value
/// macOS itself picks at connect; 7.5 ms was rejected outright, so this is the
/// lowest interval the host we target will actually grant. 15 x 40 events puts
/// the effective period at 600 ms.
pub const IDLE: Params = Params {
    interval_min_units: 12,
    interval_max_units: 12,
    latency: 39,
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
    /// The gap between points at which the peripheral is obliged to be awake.
    /// This single number is both the idle power budget and the worst-case
    /// delay on the first keystroke after idle, which is why the interval and
    /// the latency cannot both be optimised independently.
    #[must_use]
    pub const fn effective_ms(&self) -> u32 {
        self.interval_max_units as u32 * (self.latency as u32 + 1) * 5 / 4
    }

    /// Mirror of the link layer's own validity rules, checked here so a bad
    /// profile fails a host test instead of an HCI command at runtime.
    /// Post: bounds on interval range and latency, and a supervision timeout
    /// long enough that a full run of skipped events is not read as a drop.
    #[cfg(test)]
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.interval_min_units <= self.interval_max_units
            && self.interval_min_units >= MIN_INTERVAL_UNITS
            && self.interval_max_units <= 3_200
            && self.latency < 500
            && self.supervision_ms >= 100
            && self.supervision_ms <= 32_000
            && (self.supervision_ms as u64)
                > 2 * (self.latency as u64 + 1) * self.interval_max_units as u64 * 5 / 4
    }
}

/// Convert a measured interval to units. Dividing microseconds by 1250 is
/// exact for every legal interval, so no rounding drift creeps in through the
/// millisecond truncation that `as_millis` would apply.
/// Post: saturates at `u16::MAX`, so an absurd measurement cannot wrap back
/// down into a small interval that looks legal.
#[must_use]
#[allow(clippy::cast_possible_truncation)] // bounded by the saturation guard
pub const fn units_from_us(us: u64) -> u16 {
    let units = us / UNIT_US as u64;
    if units > u16::MAX as u64 {
        u16::MAX
    } else {
        units as u16
    }
}

/// When to ask the host for which profile, and when to stop asking.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    applied: Option<Profile>,
    in_flight: Option<Profile>,
    last_request: Option<u64>,
    refusals: u8,
    idle_abandoned: bool,
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
            idle_abandoned: false,
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
    ///
    /// Giving up is per-direction. Abandoning Idle must never suppress Active:
    /// a pad that cannot ask for the fast link back is broken for the rest of
    /// the connection, whereas a battery optimization that keeps failing only
    /// costs battery.
    pub fn update(&mut self, now: u64, blank: bool, usb: bool) -> Option<Profile> {
        // `Option` rather than a 0 sentinel: a request made at t=0 is a real
        // request, and reading it as "never asked" would skip the cooldown.
        let since = self.last_request.map(|at| now.saturating_sub(at));
        if self.in_flight.is_some() {
            match since {
                Some(elapsed) if elapsed < REQUEST_ACK_MS => return None,
                _ => {
                    // No answering `ConnectionParamsUpdated` ever arrived. A
                    // rejected parameter update is logged inside trouble and
                    // returned as `Ok`, so this silence is the only signal we
                    // were refused, and counting it is what stops us asking on
                    // every cooldown forever.
                    if self.in_flight == Some(Profile::Idle) {
                        self.count_failure();
                    }
                    self.in_flight = None;
                }
            }
        }
        let want = Self::target(blank, usb);
        if self.applied == Some(want) {
            return None;
        }
        if want == Profile::Idle && self.idle_abandoned {
            return None;
        }
        if since.is_some_and(|elapsed| elapsed < REQUEST_COOLDOWN_MS) {
            return None;
        }
        self.in_flight = Some(want);
        self.last_request = Some(now);
        Some(want)
    }

    /// What the host actually settled on is the only truth about the link. The
    /// applied state follows the observation, never our own request, so a
    /// request the host quietly ignored cannot leave us believing we are asleep.
    /// Post: asking for Idle and being kept awake counts against it; a granted
    /// Idle proves the host can do it and clears the ledger.
    pub fn observe(&mut self, observed: Profile) {
        let denied_idle = self.in_flight == Some(Profile::Idle) && observed != Profile::Idle;
        self.applied = Some(observed);
        self.in_flight = None;
        if denied_idle {
            self.count_failure();
        } else {
            self.refusals = 0;
            self.idle_abandoned = false;
        }
    }

    /// The host rejected the request outright, for the cases where the stack
    /// actually propagates it.
    pub fn refused(&mut self) {
        if self.in_flight == Some(Profile::Idle) {
            self.count_failure();
        }
        self.in_flight = None;
    }

    fn count_failure(&mut self) {
        if self.idle_abandoned {
            return;
        }
        self.refusals = self.refusals.saturating_add(1);
        if self.refusals >= MAX_REFUSALS {
            self.idle_abandoned = true;
        }
    }

    /// The profile the policy last saw applied, test-visible only.
    #[cfg(test)]
    #[must_use]
    pub const fn applied(&self) -> Option<Profile> {
        self.applied
    }

    #[cfg(test)]
    #[must_use]
    pub const fn has_given_up_on_idle(&self) -> bool {
        self.idle_abandoned
    }
}

/// Classify by effective wake period, never by raw interval. A host can pair a
/// short interval with a large latency and be asleep, or a long interval with
/// zero latency and be awake; comparing intervals alone gets both of those
/// backwards, which is precisely the mistake this profile shape makes easy.
#[must_use]
pub fn classify(params: Params) -> Profile {
    if params.effective_ms() >= IDLE.effective_ms() {
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
    fn the_supervision_timeout_covers_a_full_run_of_skipped_events() {
        // Latency 79 means 80 intervals can pass without contact, and the
        // effective period already is that stretch. The link layer wants the
        // timeout above twice it, so a healthy link is not declared dead
        // during a normal idle run.
        let window_ms = u64::from(IDLE.effective_ms());
        assert!(
            u64::from(IDLE.supervision_ms) > 2 * window_ms,
            "timeout {} ms must exceed {} ms of skipped events",
            IDLE.supervision_ms,
            window_ms
        );
    }

    #[test]
    fn effective_period_is_the_product_not_the_interval() {
        assert_eq!(
            Params {
                interval_min_units: 6,
                interval_max_units: 6,
                latency: 0,
                supervision_ms: 2_000,
            }
            .effective_ms(),
            7
        ); // 7.5 ms truncates to 7 in integer ms; the units hold the truth
        assert_eq!(
            Params {
                interval_min_units: 6,
                interval_max_units: 6,
                latency: 79,
                supervision_ms: 2_000,
            }
            .effective_ms(),
            600
        );
        assert_eq!(ACTIVE.effective_ms(), 10, "8 units = 10 ms, no latency");
        assert_eq!(IDLE.effective_ms(), 600);
    }

    #[test]
    fn the_idle_profile_is_a_real_reduction_in_wake_rate() {
        // 600 ms against 10 ms is not a token saving, and against the 345 ms
        // macOS was measured using at connect it is still better.
        assert!(IDLE.effective_ms() >= 20 * ACTIVE.effective_ms());
        assert!(IDLE.effective_ms() >= 345);
    }

    #[test]
    fn the_floor_interval_is_expressible_which_is_why_units_exist() {
        assert_eq!(units_from_us(7_500), 6);
        assert_eq!(units_from_us(15_000), 12);
        assert_eq!(units_from_us(11_250), 9);
        assert_eq!(MIN_INTERVAL_UNITS * 1_250, 7_500);
        assert_eq!(units_from_us(0), 0);
        assert_eq!(units_from_us(u64::MAX), u16::MAX, "saturates, never wraps");
    }

    #[test]
    fn an_interval_below_the_ble_floor_is_caught_here_not_at_runtime() {
        assert!(
            !Params {
                interval_min_units: 5,
                interval_max_units: 8,
                latency: 0,
                supervision_ms: 2_000,
            }
            .is_valid()
        );
        assert!(
            !Params {
                interval_min_units: 6,
                interval_max_units: 3_201,
                latency: 0,
                supervision_ms: 2_000,
            }
            .is_valid()
        );
    }

    #[test]
    fn a_timeout_that_does_not_cover_the_latency_is_rejected() {
        // 6 units with latency 79 skips 600 ms; a 1000 ms timeout would let a
        // healthy link be declared dead during a normal idle stretch.
        assert!(
            !Params {
                interval_min_units: 6,
                interval_max_units: 6,
                latency: 79,
                supervision_ms: 1_000,
            }
            .is_valid()
        );
    }

    #[test]
    fn a_blank_backlight_asks_for_the_idle_profile() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        p.observe(Profile::Active);
        let t = REQUEST_COOLDOWN_MS + 1;
        assert_eq!(p.update(t, true, false), Some(Profile::Idle));
    }

    #[test]
    fn usb_never_asks_for_idle_because_vbus_is_already_paying() {
        assert_eq!(Policy::target(true, true), Profile::Active);
        assert_eq!(Policy::target(true, false), Profile::Idle);
        assert_eq!(Policy::target(false, true), Profile::Active);
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, true), Some(Profile::Active));
        p.observe(Profile::Active);
        assert_eq!(p.update(REQUEST_COOLDOWN_MS + 1, true, true), None);
    }

    #[test]
    fn it_does_not_re_ask_for_a_profile_that_is_already_applied() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        p.observe(Profile::Active);
        assert_eq!(p.update(REQUEST_COOLDOWN_MS + 1, false, false), None);
    }

    #[test]
    fn requests_are_rate_limited_so_a_picky_host_cannot_cause_thrash() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        p.refused();
        assert_eq!(p.update(1_000, true, false), None);
        assert_eq!(p.update(REQUEST_COOLDOWN_MS - 1, true, false), None);
        assert_eq!(
            p.update(REQUEST_COOLDOWN_MS, true, false),
            Some(Profile::Idle)
        );
    }

    #[test]
    fn a_request_stays_in_flight_and_does_not_stack_another() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        assert_eq!(p.update(1, true, false), None);
        assert_eq!(p.update(REQUEST_ACK_MS - 1, true, false), None);
    }

    #[test]
    fn a_silent_request_eventually_releases_so_the_policy_cannot_wedge() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, false, false), Some(Profile::Active));
        let t = REQUEST_COOLDOWN_MS.max(REQUEST_ACK_MS) + 1;
        assert_eq!(p.update(t, true, false), Some(Profile::Idle));
    }

    #[test]
    fn being_kept_awake_when_we_asked_to_sleep_counts_against_it() {
        // The dangerous case: the request succeeds, so nothing errors, but the
        // host never lets us sleep. Counting this is what stops the policy
        // asking again on every cooldown for ever.
        let mut p = Policy::new();
        let mut t = 0;
        for attempt in 1..=MAX_REFUSALS {
            assert_eq!(
                p.update(t, true, false),
                Some(Profile::Idle),
                "still trying at refusal {attempt}"
            );
            p.observe(classify(ACTIVE));
            assert_eq!(p.applied(), Some(Profile::Active));
            t += REQUEST_COOLDOWN_MS;
        }
        assert!(
            p.has_given_up_on_idle(),
            "a host that never lets us sleep must be given up on"
        );
        assert_eq!(p.update(t, true, false), None);
    }

    #[test]
    fn a_silent_idle_request_counts_because_the_error_is_swallowed_upstream() {
        // trouble logs a rejected parameter update and returns Ok, so the only
        // sign of refusal is that no `ConnectionParamsUpdated` ever arrives.
        // Measured against macOS, which rejected the profile 14 times without
        // our `Err` arm firing once.
        let mut p = Policy::new();
        assert_eq!(p.update(0, true, false), Some(Profile::Idle));
        for attempt in 2..=MAX_REFUSALS {
            let t = u64::from(attempt) * (REQUEST_COOLDOWN_MS + 1);
            assert_eq!(
                p.update(t, true, false),
                Some(Profile::Idle),
                "still trying at attempt {attempt}"
            );
        }
        let t = u64::from(MAX_REFUSALS + 1) * (REQUEST_COOLDOWN_MS + 1);
        assert_eq!(p.update(t, true, false), None, "silence must add up");
        assert!(p.has_given_up_on_idle());
    }

    #[test]
    fn giving_up_on_idle_never_blocks_the_way_back_to_active() {
        // The reason the mute is per-direction. If abandoning the battery
        // optimization also silenced Active, a few refusals early in a
        // connection would strand the pad on a slow link forever.
        let mut p = Policy::new();
        assert_eq!(p.update(0, true, false), Some(Profile::Idle));
        for attempt in 2..=MAX_REFUSALS {
            assert_eq!(
                p.update(u64::from(attempt) * (REQUEST_COOLDOWN_MS + 1), true, false),
                Some(Profile::Idle)
            );
        }
        let mut t = u64::from(MAX_REFUSALS + 1) * (REQUEST_COOLDOWN_MS + 1);
        assert_eq!(p.update(t, true, false), None);
        assert!(p.has_given_up_on_idle());
        t += REQUEST_COOLDOWN_MS + 1;
        assert_eq!(p.update(t, false, false), Some(Profile::Active));
    }

    #[test]
    fn a_granted_idle_proves_the_host_can_do_it_and_clears_the_ledger() {
        let mut p = Policy::new();
        assert_eq!(p.update(0, true, false), Some(Profile::Idle));
        p.observe(classify(ACTIVE));
        assert_eq!(
            p.update(REQUEST_COOLDOWN_MS, true, false),
            Some(Profile::Idle)
        );
        p.observe(classify(ACTIVE));
        assert_eq!(
            p.update(2 * REQUEST_COOLDOWN_MS, true, false),
            Some(Profile::Idle)
        );
        p.observe(classify(IDLE));
        assert!(!p.has_given_up_on_idle());
        assert_eq!(p.applied(), Some(Profile::Idle));
    }

    #[test]
    fn classify_uses_the_effective_period_not_the_raw_interval() {
        assert_eq!(classify(ACTIVE), Profile::Active);
        assert_eq!(classify(IDLE), Profile::Idle);
        // A short interval with a big latency IS asleep. Reading the interval
        // alone would call this Active and never let the pad sleep.
        assert_eq!(
            classify(Params {
                interval_min_units: MIN_INTERVAL_UNITS,
                interval_max_units: MIN_INTERVAL_UNITS,
                latency: 80,
                supervision_ms: 2_000,
            }),
            Profile::Idle
        );
        // A long interval with no latency is NOT asleep. Reading the interval
        // alone would call this Idle and stop asking for a fast link.
        assert_eq!(
            classify(Params {
                interval_min_units: 400,
                interval_max_units: 400,
                latency: 0,
                supervision_ms: 4_000,
            }),
            Profile::Active
        );
    }

    #[test]
    fn the_measured_macos_connect_profile_reads_as_active_not_idle() {
        // Observed on hardware: 15 ms / latency 22 / 2000 ms = 345 ms
        // effective. Below our 600 ms idle floor, so correctly awake.
        let measured = Params {
            interval_min_units: units_from_us(15_000),
            interval_max_units: units_from_us(15_000),
            latency: 22,
            supervision_ms: 2_000,
        };
        assert_eq!(measured.effective_ms(), 345);
        assert_eq!(classify(measured), Profile::Active);
    }

    #[test]
    fn going_active_is_never_delayed_by_the_idle_state() {
        let mut p = Policy::new();
        p.update(0, false, false);
        p.observe(Profile::Active);
        let t = REQUEST_COOLDOWN_MS + 1;
        p.update(t, true, false);
        p.observe(Profile::Idle);
        let t2 = t + REQUEST_COOLDOWN_MS + 1;
        assert_eq!(p.update(t2, false, false), Some(Profile::Active));
    }
}
