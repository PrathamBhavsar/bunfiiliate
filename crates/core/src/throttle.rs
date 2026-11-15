//! Failure throttling for auth routes. Pure logic: the caller supplies `now` and persists the state.

use chrono::{DateTime, Duration, Utc};

/// How a scope counts failures and locks.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Failures inside `window` that trigger a lock.
    pub threshold: i32,
    pub window: Duration,
    /// First lock length; each further lock doubles it, up to `cap`.
    pub base: Duration,
    pub cap: Duration,
}

/// Reset codes: 5 failures in 15 minutes lock the user for 15 minutes, doubling to 24 hours.
pub const RESET: Policy = Policy {
    threshold: 5,
    window: Duration::minutes(15),
    base: Duration::minutes(15),
    cap: Duration::hours(24),
};

/// Login, keyed by (email, ip): a short backoff that grows to 15 minutes. It only ever slows the
/// one address guessing; the account itself is never locked.
pub const LOGIN: Policy = Policy {
    threshold: 5,
    window: Duration::minutes(15),
    base: Duration::seconds(2),
    cap: Duration::minutes(15),
};

/// Lock length for the `n`th lock (1-based): `base * 2^(n-1)`, capped.
pub fn lock_duration(base: Duration, cap: Duration, n: u32) -> Duration {
    let shift = n.saturating_sub(1).min(32);
    let secs = base.num_seconds().saturating_mul(1i64 << shift);
    Duration::seconds(secs.min(cap.num_seconds()))
}

/// Reset-throttle lock length for the `n`th lock: 15m, 30m, 60m ... capped at 24h.
pub fn next_lock_duration(n: u32) -> Duration {
    lock_duration(RESET.base, RESET.cap, n)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub failures: i32,
    pub window_started_at: DateTime<Utc>,
    pub locked_until: Option<DateTime<Utc>>,
    pub lock_count: i32,
}

impl State {
    pub fn fresh(now: DateTime<Utc>) -> Self {
        Self {
            failures: 0,
            window_started_at: now,
            locked_until: None,
            lock_count: 0,
        }
    }

    pub fn is_locked(&self, now: DateTime<Utc>) -> bool {
        self.locked_until.is_some_and(|t| t > now)
    }

    /// State after one more failure at `now`. A failure while locked changes nothing, so
    /// hammering a locked key cannot extend its lock.
    pub fn failed(mut self, now: DateTime<Utc>, p: &Policy) -> Self {
        if self.is_locked(now) {
            return self;
        }
        // A lock that expired long ago no longer counts towards the doubling.
        if self.locked_until.is_some_and(|t| now - t > p.cap) {
            self.lock_count = 0;
        }
        if now - self.window_started_at >= p.window {
            self.failures = 0;
            self.window_started_at = now;
        }
        self.failures = self.failures.saturating_add(1);
        if self.failures >= p.threshold {
            self.lock_count = self.lock_count.saturating_add(1);
            let n = u32::try_from(self.lock_count).unwrap_or(u32::MAX);
            self.locked_until = Some(now + lock_duration(p.base, p.cap, n));
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_lock_duration_doubles_and_caps() {
        let m = |n| next_lock_duration(n).num_minutes();
        assert_eq!(m(1), 15);
        assert_eq!(m(2), 30);
        assert_eq!(m(3), 60);
        assert_eq!(m(4), 120);
        assert_eq!(m(7), 960);
        assert_eq!(m(8), 24 * 60, "capped at 24h");
        assert_eq!(m(500), 24 * 60, "no overflow far past the cap");
        assert_eq!(m(0), 15);
    }

    #[test]
    fn locks_at_threshold_and_doubles_on_repeat() {
        let t0 = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut s = State::fresh(t0);
        for _ in 0..4 {
            s = s.failed(t0, &RESET);
            assert!(!s.is_locked(t0));
        }
        s = s.failed(t0, &RESET);
        assert!(s.is_locked(t0));
        assert_eq!(s.locked_until, Some(t0 + Duration::minutes(15)));
        // Failures while locked do not extend the lock.
        let same = s.clone().failed(t0 + Duration::minutes(1), &RESET);
        assert_eq!(same, s);
        // After it expires the next window needs five fresh failures and locks for twice as long.
        let t1 = t0 + Duration::minutes(16);
        assert!(!s.is_locked(t1));
        for _ in 0..5 {
            s = s.failed(t1, &RESET);
        }
        assert_eq!(s.locked_until, Some(t1 + Duration::minutes(30)));
    }

    #[test]
    fn failures_outside_the_window_do_not_add_up() {
        let t0 = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut s = State::fresh(t0);
        for i in 0..10 {
            s = s.failed(t0 + Duration::minutes(16 * i), &RESET);
        }
        assert!(!s.is_locked(t0 + Duration::minutes(16 * 10)));
    }

    #[test]
    fn login_backoff_is_short_and_capped() {
        assert_eq!(lock_duration(LOGIN.base, LOGIN.cap, 1).num_seconds(), 2);
        assert_eq!(lock_duration(LOGIN.base, LOGIN.cap, 3).num_seconds(), 8);
        assert_eq!(lock_duration(LOGIN.base, LOGIN.cap, 40), LOGIN.cap);
    }
}
