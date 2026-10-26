//! Which request headers may name the client. By default none: the client is the TCP peer.
//! Behind a reverse proxy the operator lists it in `BF_TRUSTED_PROXIES` and the tracker reads
//! `X-Forwarded-For`; behind Cloudflare `BF_TRUST_CF=1` also trusts `CF-Connecting-IP` and
//! `CF-IPCountry`. Anything else a client sends about itself is ignored, otherwise one header
//! defeats the IP-based fraud rules.

use std::net::IpAddr;
use std::str::FromStr;

use axum::http::HeaderMap;

/// An address range, `10.0.0.0/8` or a single address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: IpAddr,
    bits: u8,
}

impl FromStr for Cidr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (ip, bits) = match s.split_once('/') {
            Some((ip, bits)) => (ip, Some(bits)),
            None => (s, None),
        };
        let net: IpAddr = ip
            .trim()
            .parse()
            .map_err(|_| format!("bad address in {s:?}"))?;
        let max = if net.is_ipv4() { 32 } else { 128 };
        let bits = match bits {
            Some(b) => b
                .trim()
                .parse::<u8>()
                .map_err(|_| format!("bad prefix in {s:?}"))?,
            None => max,
        };
        if bits > max {
            return Err(format!("prefix too long in {s:?}"));
        }
        Ok(Self { net, bits })
    }
}

impl Cidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        // An IPv4 peer may show up as an IPv4-mapped IPv6 address.
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 => v4,
        };
        match (self.net, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => prefix_eq(&n.octets(), &a.octets(), self.bits),
            (IpAddr::V6(n), IpAddr::V6(a)) => prefix_eq(&n.octets(), &a.octets(), self.bits),
            _ => false,
        }
    }
}

fn prefix_eq(a: &[u8], b: &[u8], bits: u8) -> bool {
    let (full, rem) = (usize::from(bits / 8), bits % 8);
    a[..full] == b[..full] && (rem == 0 || (a[full] ^ b[full]) >> (8 - rem) == 0)
}

#[derive(Debug, Clone, Default)]
pub struct Trust {
    /// Believe `CF-Connecting-IP` and `CF-IPCountry`. Only safe when every request comes
    /// through Cloudflare, which overwrites them.
    pub cf: bool,
    /// Peers whose `X-Forwarded-For` is believed.
    pub proxies: Vec<Cidr>,
}

impl Trust {
    /// `BF_TRUST_CF` (`1`/`true`) and `BF_TRUSTED_PROXIES` (comma separated addresses or CIDRs).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let cf = get("BF_TRUST_CF").is_some_and(|v| matches!(v.trim(), "1" | "true"));
        let proxies = get("BF_TRUSTED_PROXIES")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|e| format!("BF_TRUSTED_PROXIES: {e}"))?;
        Ok(Self { cf, proxies })
    }

    fn trusted(&self, ip: IpAddr) -> bool {
        self.proxies.iter().any(|c| c.contains(ip))
    }

    /// The client address: Cloudflare's header if trusted, else the nearest `X-Forwarded-For`
    /// entry that is not itself a trusted proxy (when the peer is one), else the peer.
    pub fn client_ip(&self, headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
        if self.cf
            && let Some(ip) =
                header(headers, "cf-connecting-ip").and_then(|v| v.trim().parse().ok())
        {
            return Some(ip);
        }
        let peer = peer?;
        if !self.trusted(peer) {
            return Some(peer);
        }
        let xff = header(headers, "x-forwarded-for").unwrap_or("");
        let hops = xff.rsplit(',').map(|h| h.trim().parse::<IpAddr>());
        for hop in hops {
            match hop {
                Ok(ip) if self.trusted(ip) => continue,
                Ok(ip) => return Some(ip),
                Err(_) => break, // junk: do not look further left
            }
        }
        Some(peer)
    }

    /// `CF-IPCountry`, only when Cloudflare is trusted.
    pub fn country<'a>(&self, headers: &'a HeaderMap) -> Option<&'a str> {
        self.cf.then(|| header(headers, "cf-ipcountry")).flatten()
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).parse().unwrap(), v.parse().unwrap()))
            .collect()
    }

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    fn trust(cf: bool, proxies: &str) -> Trust {
        let env = [
            ("BF_TRUST_CF", if cf { "1" } else { "0" }),
            ("BF_TRUSTED_PROXIES", proxies),
        ];
        Trust::from_lookup(|k| {
            env.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_string())
        })
        .unwrap()
    }

    #[test]
    fn proxy_cidr_matching() {
        let c: Cidr = "10.0.0.0/8".parse().unwrap();
        assert!(c.contains("10.255.1.1".parse().unwrap()));
        assert!(!c.contains("11.0.0.1".parse().unwrap()));
        let c: Cidr = "172.16.0.0/12".parse().unwrap();
        assert!(c.contains("172.31.9.9".parse().unwrap()));
        assert!(!c.contains("172.32.0.1".parse().unwrap()));
        let one: Cidr = "203.0.113.7".parse().unwrap();
        assert!(
            one.contains("203.0.113.7".parse().unwrap())
                && !one.contains("203.0.113.8".parse().unwrap())
        );
        let v6: Cidr = "fd00::/8".parse().unwrap();
        assert!(
            v6.contains("fd12::1".parse().unwrap()) && !v6.contains("fe80::1".parse().unwrap())
        );
        assert!(
            c.contains("::ffff:172.16.0.5".parse().unwrap()),
            "v4-mapped peer"
        );
        assert!("10.0.0.0/33".parse::<Cidr>().is_err());
        assert!("nope".parse::<Cidr>().is_err());
    }

    #[test]
    fn proxy_default_trusts_nothing_but_the_peer() {
        let t = Trust::default();
        let spoof = h(&[
            ("cf-connecting-ip", "6.6.6.6"),
            ("x-forwarded-for", "7.7.7.7"),
            ("cf-ipcountry", "XX"),
        ]);
        assert_eq!(t.client_ip(&spoof, ip("198.51.100.4")), ip("198.51.100.4"));
        assert_eq!(t.country(&spoof), None);
        assert_eq!(t.client_ip(&spoof, None), None);
    }

    #[test]
    fn proxy_cloudflare_headers_only_when_trusted() {
        let t = trust(true, "");
        let req = h(&[("cf-connecting-ip", "6.6.6.6"), ("cf-ipcountry", "DE")]);
        assert_eq!(t.client_ip(&req, ip("198.51.100.4")), ip("6.6.6.6"));
        assert_eq!(t.country(&req), Some("DE"));
        assert_eq!(
            t.client_ip(&h(&[("cf-connecting-ip", "junk")]), ip("1.2.3.4")),
            ip("1.2.3.4")
        );
    }

    #[test]
    fn proxy_forwarded_for_only_from_a_trusted_peer() {
        let t = trust(false, "10.0.0.0/8, 172.16.0.0/12");
        let req = h(&[("x-forwarded-for", "9.9.9.9, 1.1.1.1, 10.1.1.1")]);
        // The peer is a trusted proxy: walk back past trusted hops to the first outside one.
        assert_eq!(t.client_ip(&req, ip("172.18.0.2")), ip("1.1.1.1"));
        // An untrusted peer cannot make itself someone else.
        assert_eq!(t.client_ip(&req, ip("198.51.100.4")), ip("198.51.100.4"));
        // A forged leftmost entry does not win over what the proxy appended.
        let forged = h(&[("x-forwarded-for", "6.6.6.6, 203.0.113.9")]);
        assert_eq!(t.client_ip(&forged, ip("10.0.0.2")), ip("203.0.113.9"));
        assert_eq!(
            t.client_ip(&h(&[("x-forwarded-for", "junk")]), ip("10.0.0.2")),
            ip("10.0.0.2")
        );
        assert_eq!(
            t.client_ip(&HeaderMap::new(), ip("10.0.0.2")),
            ip("10.0.0.2")
        );
    }

    #[test]
    fn proxy_config_errors_are_reported() {
        let bad = |v: &str| {
            Trust::from_lookup(|k| (k == "BF_TRUSTED_PROXIES").then(|| v.to_string())).unwrap_err()
        };
        assert!(bad("10.0.0.0/8,oops").contains("BF_TRUSTED_PROXIES"));
    }
}
