//! Click fraud rules: cheap in-memory checks the tracker runs per click.
//!
//! A flagged click is still stored and counted; it only carries a reason so the worker can
//! hold its conversions for review. Rules never touch IO and are O(1) per click.

use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, RandomState};
use std::mem;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

pub const IP_VELOCITY: &str = "ip_velocity";
pub const BOT_UA: &str = "bot_ua";
pub const DUPLICATE: &str = "duplicate";

/// What a rule needs to know about one click. `device` is the woothee category the
/// tracker already computed (`crawler` for bots), so the user agent is parsed once.
pub struct ClickFacts<'a> {
    pub ip_hash: Option<&'a [u8]>,
    pub ua: &'a str,
    pub device: Option<&'a str>,
    pub code: &'a str,
    pub ts_ms: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Flag(&'static str),
}

pub trait FraudRule: Send + Sync {
    fn check(&self, click: &ClickFacts<'_>) -> Verdict;
}

/// Run every rule (stateful ones must see each click) and return the first reason.
pub fn first_flag(rules: &[Box<dyn FraudRule>], click: &ClickFacts<'_>) -> Option<&'static str> {
    rules.iter().fold(None, |acc, r| match r.check(click) {
        Verdict::Flag(why) => acc.or(Some(why)),
        Verdict::Ok => acc,
    })
}

/// Tracker defaults: 30 clicks per ip per minute, a repeat of the same ip+ua+code within 5s.
pub fn default_rules() -> Vec<Box<dyn FraudRule>> {
    vec![
        Box::new(IpVelocity::new(30, Duration::from_secs(60), 50_000)),
        Box::new(BotUa),
        Box::new(Dedupe::new(Duration::from_secs(5), 50_000)),
    ]
}

/// Last `limit` hit times per key within `window_ms`. Keys live in two generations: when
/// the current one holds `cap` keys it becomes the old one and the previous old one is
/// dropped, so memory stays under `2 * cap * limit` timestamps with no scan on the hot path.
struct Hits {
    limit: usize,
    window_ms: i64,
    cap: usize,
    cur: HashMap<u64, VecDeque<i64>>,
    old: HashMap<u64, VecDeque<i64>>,
}

impl Hits {
    fn new(limit: usize, window: Duration, cap: usize) -> Self {
        Self {
            limit,
            window_ms: i64::try_from(window.as_millis()).unwrap_or(i64::MAX),
            cap,
            cur: HashMap::new(),
            old: HashMap::new(),
        }
    }

    /// Record a hit; true when `limit` earlier hits are still inside the window.
    fn hit(&mut self, key: u64, ts_ms: i64) -> bool {
        let mut q = match self.cur.remove(&key) {
            Some(q) => q,
            None => self.old.remove(&key).unwrap_or_default(),
        };
        if self.cur.len() >= self.cap {
            self.old = mem::take(&mut self.cur);
        }
        let floor = ts_ms.saturating_sub(self.window_ms);
        while q.front().is_some_and(|&t| t <= floor) {
            q.pop_front();
        }
        let over = q.len() >= self.limit;
        q.push_back(ts_ms);
        if q.len() > self.limit {
            q.pop_front();
        }
        self.cur.insert(key, q);
        over
    }

    #[cfg(test)]
    fn keys(&self) -> usize {
        self.cur.len() + self.old.len()
    }
}

/// Hits behind a lock plus a per-process random hasher, so keys cannot be forged offline.
struct Counter {
    hits: Mutex<Hits>,
    hasher: RandomState,
}

impl Counter {
    fn new(limit: usize, window: Duration, cap: usize) -> Self {
        Self {
            hits: Mutex::new(Hits::new(limit, window, cap)),
            hasher: RandomState::new(),
        }
    }

    fn hit(&self, key: impl std::hash::Hash, ts_ms: i64) -> bool {
        let key = self.hasher.hash_one(key);
        self.hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .hit(key, ts_ms)
    }
}

/// More than `max_clicks` clicks from one ip within `window`.
pub struct IpVelocity(Counter);

impl IpVelocity {
    /// Tracks at most about `2 * max_ips` ips.
    pub fn new(max_clicks: usize, window: Duration, max_ips: usize) -> Self {
        Self(Counter::new(max_clicks, window, max_ips))
    }
}

impl FraudRule for IpVelocity {
    fn check(&self, c: &ClickFacts<'_>) -> Verdict {
        match c.ip_hash {
            Some(ip) if self.0.hit(ip, c.ts_ms) => Verdict::Flag(IP_VELOCITY),
            _ => Verdict::Ok,
        }
    }
}

/// Obvious crawlers: woothee category `crawler`.
pub struct BotUa;

impl FraudRule for BotUa {
    fn check(&self, c: &ClickFacts<'_>) -> Verdict {
        if c.device == Some("crawler") {
            Verdict::Flag(BOT_UA)
        } else {
            Verdict::Ok
        }
    }
}

/// The same ip + user agent + code again within `window`.
pub struct Dedupe(Counter);

impl Dedupe {
    /// Tracks at most about `2 * max_keys` distinct ip+ua+code triples.
    pub fn new(window: Duration, max_keys: usize) -> Self {
        Self(Counter::new(1, window, max_keys))
    }
}

impl FraudRule for Dedupe {
    fn check(&self, c: &ClickFacts<'_>) -> Verdict {
        match c.ip_hash {
            Some(ip) if self.0.hit((ip, c.ua, c.code), c.ts_ms) => Verdict::Flag(DUPLICATE),
            _ => Verdict::Ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: i64 = 1_000;

    fn click<'a>(ip: &'a [u8], ua: &'a str, code: &'a str, ts_ms: i64) -> ClickFacts<'a> {
        ClickFacts {
            ip_hash: Some(ip),
            ua,
            device: Some("pc"),
            code,
            ts_ms,
        }
    }

    #[test]
    fn velocity_flags_past_the_limit_and_expires() {
        let r = IpVelocity::new(3, Duration::from_secs(10), 100);
        let at = |ip: &[u8], t| r.check(&click(ip, "ua", "c", t));
        for i in 0..3 {
            assert_eq!(at(b"a", i * SEC), Verdict::Ok, "click {i}");
        }
        assert_eq!(at(b"a", 3 * SEC), Verdict::Flag(IP_VELOCITY));
        assert_eq!(at(b"a", 4 * SEC), Verdict::Flag(IP_VELOCITY));
        // Another ip has its own budget.
        assert_eq!(at(b"b", 4 * SEC), Verdict::Ok);
        // The window slides: the last three hits (3s, 4s, and the flagged ones) age out.
        assert_eq!(at(b"a", 15 * SEC), Verdict::Ok);
    }

    #[test]
    fn velocity_window_is_sliding_not_fixed() {
        let r = IpVelocity::new(2, Duration::from_secs(10), 100);
        let at = |t| r.check(&click(b"a", "ua", "c", t));
        assert_eq!(at(0), Verdict::Ok);
        assert_eq!(at(9 * SEC), Verdict::Ok);
        assert_eq!(
            at(10 * SEC),
            Verdict::Ok,
            "the click at 0 just left the window"
        );
        assert_eq!(at(11 * SEC), Verdict::Flag(IP_VELOCITY));
    }

    #[test]
    fn velocity_ignores_clicks_without_ip() {
        let r = IpVelocity::new(1, Duration::from_secs(10), 100);
        let c = ClickFacts {
            ip_hash: None,
            ..click(b"", "ua", "c", 0)
        };
        for _ in 0..5 {
            assert_eq!(r.check(&c), Verdict::Ok);
        }
    }

    #[test]
    fn bot_ua_flags_crawlers_only() {
        let facts = |device| ClickFacts {
            device,
            ..click(b"a", "ua", "c", 0)
        };
        assert_eq!(BotUa.check(&facts(Some("crawler"))), Verdict::Flag(BOT_UA));
        assert_eq!(BotUa.check(&facts(Some("pc"))), Verdict::Ok);
        assert_eq!(BotUa.check(&facts(None)), Verdict::Ok);
    }

    #[test]
    fn dedupe_flags_same_triple_within_window() {
        let r = Dedupe::new(Duration::from_secs(5), 100);
        assert_eq!(r.check(&click(b"a", "ua", "c", 0)), Verdict::Ok);
        assert_eq!(
            r.check(&click(b"a", "ua", "c", 2 * SEC)),
            Verdict::Flag(DUPLICATE)
        );
        // Any differing part is a different click.
        assert_eq!(r.check(&click(b"b", "ua", "c", 2 * SEC)), Verdict::Ok);
        assert_eq!(r.check(&click(b"a", "other", "c", 2 * SEC)), Verdict::Ok);
        assert_eq!(r.check(&click(b"a", "ua", "d", 2 * SEC)), Verdict::Ok);
    }

    #[test]
    fn dedupe_window_expires() {
        let r = Dedupe::new(Duration::from_secs(5), 100);
        assert_eq!(r.check(&click(b"a", "ua", "c", 0)), Verdict::Ok);
        assert_eq!(r.check(&click(b"a", "ua", "c", 6 * SEC)), Verdict::Ok);
        assert_eq!(
            r.check(&click(b"a", "ua", "c", 7 * SEC)),
            Verdict::Flag(DUPLICATE)
        );
    }

    #[test]
    fn memory_is_bounded() {
        let mut h = Hits::new(2, Duration::from_secs(60), 10);
        for key in 0..1_000u64 {
            h.hit(key, 0);
            assert!(h.keys() <= 20, "{} keys after {key}", h.keys());
        }
        // A key from the previous generation keeps its history across a rotation.
        let mut h = Hits::new(1, Duration::from_secs(60), 2);
        assert!(!h.hit(1, 0));
        h.hit(2, 0);
        h.hit(3, 0); // rotates: 1 and 2 move to the old generation
        assert!(h.hit(1, SEC), "history survived one rotation");
        // Per-key history is capped at `limit` timestamps.
        let mut h = Hits::new(3, Duration::from_secs(60), 10);
        for t in 0..100 {
            h.hit(7, t);
        }
        assert_eq!(h.cur[&7].len(), 3);
    }

    #[test]
    fn first_flag_runs_every_rule() {
        let rules: Vec<Box<dyn FraudRule>> = vec![
            Box::new(BotUa),
            Box::new(Dedupe::new(Duration::from_secs(5), 100)),
        ];
        let bot = ClickFacts {
            device: Some("crawler"),
            ..click(b"a", "ua", "c", 0)
        };
        assert_eq!(first_flag(&rules, &bot), Some(BOT_UA));
        // Dedupe saw the bot click too, so the repeat is a duplicate.
        assert_eq!(
            first_flag(&rules, &click(b"a", "ua", "c", SEC)),
            Some(DUPLICATE)
        );
        assert_eq!(first_flag(&rules, &click(b"z", "ua", "c", SEC)), None);
    }
}
