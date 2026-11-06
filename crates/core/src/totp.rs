//! RFC 6238 TOTP (SHA-1, 30 s, 6 digits) and single-use recovery codes.
//!
//! Recovery codes are stored as plain SHA-256 of the normalised code. That is
//! acceptable only because each code carries ~50 random bits, is single use,
//! and the reset endpoint is rate limited and locks out per user.

use hmac::{Hmac, Mac};
use rand::RngCore;
use rand::rngs::OsRng;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const PERIOD: i64 = 30;
const RECOVERY_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

pub fn generate_secret() -> [u8; 20] {
    let mut s = [0u8; 20];
    OsRng.fill_bytes(&mut s);
    s
}

/// Base32 without padding, uppercase.
pub fn base32(secret: &[u8]) -> String {
    data_encoding::BASE32_NOPAD.encode(secret)
}

fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period=30",
        pct(issuer),
        pct(account),
        base32(secret),
        pct(issuer)
    )
}

fn hotp(secret: &[u8], counter: u64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = (h[19] & 0x0f) as usize;
    let bin = u32::from_be_bytes([h[off] & 0x7f, h[off + 1], h[off + 2], h[off + 3]]);
    bin % 1_000_000
}

fn step_of(unix_secs: i64) -> i64 {
    unix_secs.div_euclid(PERIOD)
}

pub fn code_at(secret: &[u8], unix_secs: i64) -> u32 {
    hotp(secret, step_of(unix_secs).max(0) as u64)
}

/// Check `code` against steps `now-1..=now+1`. Returns the matched step, or
/// `None` when the code is wrong, malformed, or its step is not newer than
/// `last_used_step` (replay).
pub fn verify(secret: &[u8], code: &str, now: i64, last_used_step: Option<i64>) -> Option<i64> {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let cur = step_of(now);
    let mut found = None;
    // Check every step so timing does not reveal which one matched.
    for step in (cur - 1)..=(cur + 1) {
        if step < 0 {
            continue;
        }
        let want = format!("{:06}", hotp(secret, step as u64));
        if bool::from(want.as_bytes().ct_eq(code.as_bytes())) && found.is_none() {
            found = Some(step);
        }
    }
    let step = found?;
    match last_used_step {
        Some(last) if step <= last => None,
        _ => Some(step),
    }
}

/// `n` codes shaped `xxxxx-xxxxx`, ~49.6 bits each.
pub fn new_recovery_codes(n: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let mut chars = String::with_capacity(11);
        while chars.len() < 11 {
            let mut b = [0u8; 1];
            OsRng.fill_bytes(&mut b);
            // Rejection sampling keeps the alphabet unbiased.
            if (b[0] as usize) < (256 / RECOVERY_ALPHABET.len()) * RECOVERY_ALPHABET.len() {
                if chars.len() == 5 {
                    chars.push('-');
                }
                chars.push(RECOVERY_ALPHABET[b[0] as usize % RECOVERY_ALPHABET.len()] as char);
            }
        }
        if !out.contains(&chars) {
            out.push(chars);
        }
    }
    out
}

pub fn normalize_recovery(s: &str) -> String {
    s.trim().to_lowercase().replace(' ', "")
}

/// SHA-256 of the normalised code, as stored.
pub fn recovery_hash(code: &str) -> [u8; 32] {
    Sha256::digest(normalize_recovery(code).as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const RFC: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc6238_vectors() {
        // RFC 6238 Appendix B, SHA1; RFC lists 8 digits, we use the low 6.
        for (t, full) in [
            (59i64, 94287082u32),
            (1111111109, 7081804),
            (1111111111, 14050471),
            (1234567890, 89005924),
            (2000000000, 69279037),
            (20000000000, 65353130),
        ] {
            assert_eq!(code_at(RFC, t), full % 1_000_000, "t={t}");
        }
        assert_eq!(code_at(RFC, 1111111109), 81804);
        assert_eq!(code_at(RFC, 1111111111), 50471);
        assert_eq!(code_at(RFC, 2000000000), 279037);
    }

    #[test]
    fn skew_window() {
        let now = 1_700_000_000;
        let f = |t: i64| format!("{:06}", code_at(RFC, t));
        assert!(verify(RFC, &f(now - 30), now, None).is_some());
        assert!(verify(RFC, &f(now + 30), now, None).is_some());
        assert!(verify(RFC, &f(now), now, None).is_some());
        assert!(verify(RFC, &f(now - 60), now, None).is_none());
        assert!(verify(RFC, &f(now + 60), now, None).is_none());
    }

    #[test]
    fn replay_rejected() {
        let now = 1_700_000_000;
        let c = format!("{:06}", code_at(RFC, now));
        let step = verify(RFC, &c, now, None).unwrap();
        assert_eq!(verify(RFC, &c, now, Some(step)), None);
        assert!(verify(RFC, &c, now, Some(step - 1)).is_some());
    }

    #[test]
    fn rejects_non_digits_and_wrong_len() {
        for bad in [
            "12345",
            "1234567",
            "12345a",
            " 123456",
            "",
            "+12345",
            "１２３４５６",
        ] {
            assert_eq!(verify(RFC, bad, 59, None), None, "{bad:?}");
        }
    }

    #[test]
    fn otpauth_uri_shape() {
        let s = [1u8; 20];
        let u = otpauth_uri("Acme Inc", "a@b.c", &s);
        assert!(u.starts_with("otpauth://totp/Acme%20Inc:a%40b.c?"));
        assert!(u.contains(&format!("secret={}", base32(&s))));
        assert!(u.contains("issuer=Acme%20Inc"));
        assert!(u.contains("algorithm=SHA1&digits=6&period=30"));
    }

    #[test]
    fn recovery_codes_unique_and_formatted() {
        let codes = new_recovery_codes(10);
        assert_eq!(codes.len(), 10);
        for c in &codes {
            let b = c.as_bytes();
            assert_eq!(b.len(), 11, "{c}");
            assert_eq!(b[5], b'-');
            assert!(
                b.iter()
                    .enumerate()
                    .all(|(i, x)| i == 5 || x.is_ascii_lowercase() || (b'2'..=b'9').contains(x)),
                "{c}"
            );
        }
        let mut d = codes.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), 10);
    }

    #[test]
    fn recovery_normalisation_and_hash() {
        assert_eq!(normalize_recovery("  ABCDE-Fghjk "), "abcde-fghjk");
        assert_eq!(recovery_hash("ABCDE FGHJK"), recovery_hash("abcdefghjk"));
        assert_ne!(recovery_hash("aaaaa-aaaaa"), recovery_hash("aaaaa-aaaab"));
    }

    proptest! {
        #[test]
        fn code_always_6_digits(secret in proptest::collection::vec(any::<u8>(), 1..40), t in 0i64..4_000_000_000) {
            let c = code_at(&secret, t);
            prop_assert!(c <= 999_999);
            prop_assert_eq!(format!("{c:06}").len(), 6);
        }

        #[test]
        fn verify_accepts_own_code(secret in proptest::collection::vec(any::<u8>(), 1..40), now in 0i64..4_000_000_000) {
            let c = format!("{:06}", code_at(&secret, now));
            prop_assert!(verify(&secret, &c, now, None).is_some());
        }
    }
}
