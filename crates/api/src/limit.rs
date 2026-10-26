//! Small in-memory sliding-window rate limiter for the public, abusable routes (login,
//! register, contact-requests). Per process and best effort: real volumetric limits belong in
//! Caddy / Cloudflare in front of the API (see `SECURITY.md`).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

use crate::error::AppError;

const WINDOW: Duration = Duration::from_secs(60);
/// Distinct keys tracked at once; when full (after dropping idle keys) the longest-quiet key is
/// evicted, so a flood of made-up emails cannot lock real users out.
const MAX_KEYS: usize = 10_000;

pub struct Limiter {
    window: Duration,
    max_keys: usize,
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
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
        let live = |q: &mut VecDeque<Instant>| {
            while q
                .front()
                .is_some_and(|t| now.duration_since(*t) >= self.window)
            {
                q.pop_front();
            }
            !q.is_empty()
        };
        if !hits.contains_key(key) && hits.len() >= self.max_keys {
            hits.retain(|_, q| live(q));
            if hits.len() >= self.max_keys {
                let quietest = hits
                    .iter()
                    .min_by_key(|(_, q)| q.back().copied())
                    .map(|(k, _)| k.clone());
                if let Some(k) = quietest {
                    hits.remove(&k);
                }
            }
        }
        let q = hits.entry(key.to_string()).or_default();
        live(q);
        if q.len() >= max {
            return false;
        }
        q.push_back(now);
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
pub const CONTACT: Rule = Rule {
    route: "contact",
    per_ip: 10,
    per_email: 5,
};

/// Client address as set by the reverse proxy: the last `X-Forwarded-For` entry (the one our
/// own proxy appended). Without a proxy there is none and only the per-email limit applies.
fn client_ip(headers: &HeaderMap) -> Option<&str> {
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
    let email = email.trim().to_ascii_lowercase();
    let email_ok = l.allow(&format!("{route}:email:{email}"), rule.per_email);
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
