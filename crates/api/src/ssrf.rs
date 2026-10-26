//! Keeps outbound webhooks away from internal networks. A tenant picks the URL, so without this
//! a webhook endpoint is a way to make the server call loopback, the cloud metadata service or
//! anything else on its private network.
//!
//! Checked twice: at registration ([`check_url`], resolves the host once) and at connect time
//! ([`PublicOnly`], the client's resolver, which defeats DNS rebinding because the address that
//! was checked is the address that is dialled). `BF_ALLOW_PRIVATE_WEBHOOKS=1` switches both off,
//! for local development and tests against a receiver on localhost.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::error::AppError;

/// Escape hatch: `1` or `true` lets webhooks target private addresses.
pub const ALLOW_ENV: &str = "BF_ALLOW_PRIVATE_WEBHOOKS";

pub fn allow_private() -> bool {
    std::env::var(ALLOW_ENV).is_ok_and(|v| matches!(v.trim(), "1" | "true"))
}

fn v4_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local() // 169.254/16, includes the cloud metadata address
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (64..128).contains(&o[1])) // 100.64/10 carrier-grade NAT
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0/24 protocol assignments
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // 198.18/15 benchmarking
        || o[0] >= 240) // reserved
}

fn v6_public(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return v4_public(v4);
    }
    let s = ip.segments();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
        || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link local
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        || (s[0] == 0x0064 && s[1] == 0xff9b) // NAT64 can reach any v4 address
        || s[0] == 0x2002 // 6to4 embeds an arbitrary v4 address
        || (s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0)) // ::/96
}

/// Whether a webhook may connect to `ip`.
pub fn ip_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_public(v4),
        IpAddr::V6(v6) => v6_public(v6),
    }
}

/// The client's DNS resolver: resolves normally, then drops every non-public address.
pub struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let public: Vec<SocketAddr> = found.filter(|a| ip_public(a.ip())).collect();
            if public.is_empty() {
                return Err(format!("{host} resolves to no public address").into());
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// The host of `url` as an IP when it is a literal (`[::1]` loses its brackets).
fn literal_host(url: &reqwest::Url) -> Option<IpAddr> {
    url.host_str()?
        .trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .ok()
}

/// Registration-time check: reject a URL whose host is, or resolves to, a non-public address.
pub async fn check_url(url: &str) -> Result<(), AppError> {
    check_url_with(url, allow_private()).await
}

async fn check_url_with(url: &str, allow: bool) -> Result<(), AppError> {
    if allow {
        return Ok(());
    }
    let bad = |m: &str| AppError::BadRequest(m.into());
    let url = reqwest::Url::parse(url).map_err(|_| bad("url must be an absolute http(s) url"))?;
    if let Some(ip) = literal_host(&url) {
        return literal(ip);
    }
    let host = url.host_str().ok_or_else(|| bad("url has no host"))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| bad("url host does not resolve"))?
        .collect();
    if addrs.is_empty() || addrs.iter().any(|a| !ip_public(a.ip())) {
        return Err(bad("url must not point at a private or internal address"));
    }
    Ok(())
}

/// Connect-time check for IP-literal hosts, which skip the resolver.
pub fn check_literal_host(url: &str) -> Result<(), AppError> {
    if allow_private() {
        return Ok(());
    }
    let url = reqwest::Url::parse(url).map_err(|_| AppError::Internal)?;
    literal_host(&url).map_or(Ok(()), literal)
}

fn literal(ip: IpAddr) -> Result<(), AppError> {
    if ip_public(ip) {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "url must not point at a private or internal address".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(s: &str) -> bool {
        ip_public(s.parse().unwrap())
    }

    #[test]
    fn ssrf_blocks_internal_ranges() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "192.0.0.8",
            "198.18.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "fc00::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "2001:db8::1",
        ] {
            assert!(!public(ip), "{ip} must be blocked");
        }
    }

    #[test]
    fn ssrf_allows_public_addresses() {
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",
            "100.128.0.1",
            "2606:4700:4700::1111",
        ] {
            assert!(public(ip), "{ip} must be allowed");
        }
    }

    #[tokio::test]
    async fn ssrf_registration_rejects_literals_and_localhost() {
        for url in [
            "http://127.0.0.1:9/h",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]/h",
            "http://localhost/h",
            "https://10.0.0.5/h",
        ] {
            assert!(
                check_url_with(url, false).await.is_err(),
                "{url} must be rejected"
            );
        }
    }
}
