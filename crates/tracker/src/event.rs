//! Request normalization: UA, referrer, country, ip hash. Pure functions.

use std::sync::LazyLock;

use sha2::{Digest, Sha256};
use woothee::parser::Parser;

static PARSER: LazyLock<Parser> = LazyLock::new(Parser::new);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct NormalizedUa {
    pub device: Option<String>,
    pub os: Option<String>,
    pub browser: Option<String>,
}

pub fn normalize_ua(ua: &str) -> NormalizedUa {
    let Some(r) = PARSER.parse(ua) else {
        return NormalizedUa::default();
    };
    let known = |s: &str| (!s.is_empty() && s != "UNKNOWN").then(|| s.to_owned());
    NormalizedUa {
        device: known(r.category),
        os: known(r.os),
        browser: known(r.name),
    }
}

/// Host of a referrer URL or origin: lowercase, no scheme, userinfo, port or path.
pub fn referrer_host(referrer: &str) -> Option<String> {
    let rest = referrer.split_once("://").map_or(referrer, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = match host.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Path of an absolute URL (or of a bare path); never the query or fragment.
pub fn url_path(url: &str) -> String {
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.find(['/', '?', '#']).map_or("", |i| &rest[i..]),
        None => url,
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    if path.is_empty() { "/" } else { path }.to_owned()
}

/// `CF-IPCountry` value as an upper-case ISO alpha-2 code; `XX`/`T1` and junk give `None`.
pub fn country(header: &str) -> Option<String> {
    let c = header.trim();
    (c.len() == 2 && c.bytes().all(|b| b.is_ascii_alphabetic()) && !c.eq_ignore_ascii_case("xx"))
        .then(|| c.to_ascii_uppercase())
}

/// Salted SHA-256 of the client ip. The raw ip is never stored.
pub fn ip_hash(salt: &str, ip: &str) -> Vec<u8> {
    Sha256::new()
        .chain_update(b"bf-ip\0")
        .chain_update(salt.as_bytes())
        .chain_update(ip.as_bytes())
        .finalize()
        .to_vec()
}

/// Client text for storage: control chars dropped (Postgres `text` rejects NUL, which
/// would fail a whole batch), at most `max_chars` kept.
pub fn clip(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect()
}

/// Append `bf_click=<id>` to a landing url, before any fragment.
pub fn with_click(url: &str, click_id: &str) -> String {
    let (base, frag) = url.split_once('#').map_or((url, ""), |(b, f)| (b, f));
    let sep = if base.contains('?') { '&' } else { '?' };
    let frag = if frag.is_empty() {
        String::new()
    } else {
        format!("#{frag}")
    };
    format!("{base}{sep}bf_click={click_id}{frag}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ua_is_normalized() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
        let n = normalize_ua(ua);
        assert_eq!(n.device.as_deref(), Some("smartphone"));
        assert_eq!(n.os.as_deref(), Some("iPhone"));
        assert_eq!(n.browser.as_deref(), Some("Safari"));
        assert_eq!(normalize_ua("???"), NormalizedUa::default());
    }

    #[test]
    fn referrer_reduced_to_host() {
        assert_eq!(
            referrer_host("https://User@Blog.Example.com:8443/a?b=1#c").as_deref(),
            Some("blog.example.com")
        );
        assert_eq!(referrer_host("https://x.io").as_deref(), Some("x.io"));
        assert_eq!(referrer_host(""), None);
    }

    #[test]
    fn paths_and_country() {
        assert_eq!(url_path("https://a.com/pricing?x=1#y"), "/pricing");
        assert_eq!(url_path("https://a.com"), "/");
        assert_eq!(url_path("https://a.com?x=1"), "/");
        assert_eq!(country("us").as_deref(), Some("US"));
        assert_eq!(country("XX"), None);
        assert_eq!(country("T1"), None);
        assert_eq!(country("USA"), None);
    }

    #[test]
    fn clip_drops_control_chars() {
        assert_eq!(clip("a\u{0}b\nc", 10), "abc");
        assert_eq!(clip("abcdef", 3), "abc");
    }

    #[test]
    fn click_id_appended() {
        assert_eq!(
            with_click("https://a.com/p", "1"),
            "https://a.com/p?bf_click=1"
        );
        assert_eq!(
            with_click("https://a.com/p?x=1#top", "1"),
            "https://a.com/p?x=1&bf_click=1#top"
        );
    }

    #[test]
    fn ip_hash_is_salted_and_not_raw() {
        let h = ip_hash("s", "1.2.3.4");
        assert_eq!(h.len(), 32);
        assert_ne!(h, ip_hash("t", "1.2.3.4"));
        assert_ne!(h, b"1.2.3.4".to_vec());
    }
}
