//! Small in-memory sliding-window rate limiter for the public, abusable routes (login,
//! register, contact-requests). Per process and best effort: real volumetric limits belong in
//! Caddy / Cloudflare in front of the API (see `SECURITY.md`).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

use crate::error::AppError;

const WINDOW: Duration = Duration::from_secs(60);
/// Distinct keys tracked at once. When full, idle keys go first, then the longest-quiet key that
/// is not currently blocked; a blocked key is evicted only if every key is blocked. So a flood
/// of made-up emails can neither lock real users out nor lift a block that is in force.
const MAX_KEYS: usize = 10_000;

#[derive(Default)]
struct Entry {
    /// The limit this key was last checked against.
    max: usize,
    hits: VecDeque<Instant>,
}

pub struct Limiter {
    window: Duration,
    max_keys: usize,
    hits: Mutex<HashMap<String, Entry>>,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new(WINDOW, MAX_KEYS)
    }
}

impl Limiter {
    pub fn new(window: Duration, max_keys: usize) -> Self {
        Self {
            window,
            max_keys,
            hits: Mutex::default(),
        }
    }

    /// Record a hit for `key`; `false` when it already made `max` hits inside the window.
    pub fn allow(&self, key: &str, max: usize) -> bool {
        self.allow_at(key, max, Instant::now())
    }

    fn allow_at(&self, key: &str, max: usize, now: Instant) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let live = |e: &mut Entry| {
            while e
                .hits
                .front()
                .is_some_and(|t| now.duration_since(*t) >= self.window)
            {
                e.hits.pop_front();
            }
            !e.hits.is_empty()
        };
        if !hits.contains_key(key) && hits.len() >= self.max_keys {
            hits.retain(|_, e| live(e));
            if hits.len() >= self.max_keys {
                let quietest = |blocked: bool| {
                    hits.iter()
                        .filter(|(_, e)| (e.hits.len() >= e.max) == blocked)
                        .min_by_key(|(_, e)| e.hits.back().copied())
                        .map(|(k, _)| k.clone())
                };
                if let Some(k) = quietest(false).or_else(|| quietest(true)) {
                    hits.remove(&k);
                }
            }
        }
        let e = hits.entry(key.to_string()).or_default();
        live(e);
        e.max = max;
        if e.hits.len() >= max {
            return false;
        }
        e.hits.push_back(now);
        true
    }
}

/// Hits allowed per minute for one client IP / one email, per route.
pub struct Rule {
    pub route: &'static str,
    pub per_ip: usize,
    pub per_email: usize,
}

pub const LOGIN: Rule = Rule {
    route: "login",
    per_ip: 30,
    per_email: 10,
};
pub const REGISTER: Rule = Rule {
    route: "register",
    per_ip: 10,
    per_email: 5,
};
pub const SIGNUP: Rule = Rule {
    route: "signup",
    per_ip: 10,
    per_email: 5,
};
pub const RESET: Rule = Rule {
    route: "reset",
    per_ip: 10,
    per_email: 5,
};
pub const CONTACT: Rule = Rule {
    route: "contact",
    per_ip: 10,
    per_email: 5,
};

/// Client address as set by the reverse proxy: the last `X-Forwarded-For` entry (the one our
/// own proxy appended). Without a proxy there is none and only the per-email limit applies.
pub fn client_ip(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get("x-forwarded-for")?.to_str().ok()?;
    v.rsplit(',')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Count one request against `rule` for this client and email; 429 when over either limit.
pub fn check(l: &Limiter, rule: &Rule, headers: &HeaderMap, email: &str) -> Result<(), AppError> {
    let route = rule.route;
    let ip_ok =
        client_ip(headers).is_none_or(|ip| l.allow(&format!("{route}:ip:{ip}"), rule.per_ip));
    // An address already over its limit does not also spend the email's allowance (or a slot).
    let email = email.trim().to_ascii_lowercase();
    let email_ok = ip_ok && l.allow(&format!("{route}:email:{email}"), rule.per_email);
    if ip_ok && email_ok {
        Ok(())
    } else {
        Err(AppError::TooManyRequests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_window_slides() {
        let l = Limiter::new(Duration::from_secs(60), 100);
        let t0 = Instant::now();
        assert!(l.allow_at("k", 2, t0));
        assert!(l.allow_at("k", 2, t0 + Duration::from_secs(30)));
        assert!(!l.allow_at("k", 2, t0 + Duration::from_secs(59)));
        assert!(l.allow_at("other", 2, t0 + Duration::from_secs(59)));
        // The first hit has left the window; the second has not.
        assert!(l.allow_at("k", 2, t0 + Duration::from_secs(61)));
        assert!(!l.allow_at("k", 2, t0 + Duration::from_secs(62)));
    }

    #[test]
    fn limit_map_is_bounded() {
        let l = Limiter::new(Duration::from_secs(60), 2);
        let t0 = Instant::now();
        assert!(l.allow_at("a", 5, t0));
        assert!(l.allow_at("b", 5, t0 + Duration::from_secs(1)));
        assert!(
            l.allow_at("c", 5, t0 + Duration::from_secs(2)),
            "full: the quietest key is evicted, the new one is not refused"
        );
        assert_eq!(l.hits.lock().unwrap().len(), 2);
        assert!(
            !l.hits.lock().unwrap().contains_key("a"),
            "oldest went first"
        );
        assert!(
            l.allow_at("d", 5, t0 + Duration::from_secs(61)),
            "idle keys evicted"
        );
        assert!(l.hits.lock().unwrap().len() <= 2);
    }

    #[test]
    fn limit_flood_of_new_keys_does_not_lift_a_block() {
        let l = Limiter::new(Duration::from_secs(60), 3);
        let t0 = Instant::now();
        assert!(l.allow_at("victim", 1, t0));
        assert!(!l.allow_at("victim", 1, t0), "blocked");
        // An attacker rotates through fresh keys; the blocked key is the oldest of them all.
        for i in 0..20 {
            assert!(l.allow_at(&format!("fresh{i}"), 5, t0 + Duration::from_secs(1 + i)));
        }
        assert!(
            !l.allow_at("victim", 1, t0 + Duration::from_secs(30)),
            "still blocked after the flood"
        );
    }

    #[test]
    fn limit_keys_by_ip_and_normalised_email() {
        let l = Limiter::new(Duration::from_secs(60), 100);
        let rule = Rule {
            route: "t",
            per_ip: 2,
            per_email: 3,
        };
        let from = |ip: &'static str| {
            let mut h = HeaderMap::new();
            h.insert("x-forwarded-for", format!("1.1.1.1, {ip}").parse().unwrap());
            h
        };
        assert!(check(&l, &rule, &from("9.9.9.9"), "a@x.test").is_ok());
        assert!(check(&l, &rule, &from("9.9.9.9"), "b@x.test").is_ok());
        assert!(
            check(&l, &rule, &from("9.9.9.9"), "c@x.test").is_err(),
            "ip over"
        );
        assert!(check(&l, &rule, &from("8.8.8.8"), " A@X.test").is_ok());
        assert!(check(&l, &rule, &from("7.7.7.7"), "a@x.test").is_ok());
        assert!(
            check(&l, &rule, &from("6.6.6.6"), "a@x.TEST").is_err(),
            "email over"
        );
        assert!(
            check(&l, &rule, &HeaderMap::new(), "z@x.test").is_ok(),
            "no ip: email only"
        );
    }
}
