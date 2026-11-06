//! Envelope encryption for secrets at rest.
//!
//! Each value gets a random 32-byte data key (DEK) and a random 24-byte nonce;
//! the value is sealed with XChaCha20-Poly1305 under the DEK. The DEK is then
//! wrapped under a master key (KEK) identified by a key id. Rotating the KEK
//! only re-wraps the DEK; the value ciphertext is untouched.
//!
//! Sealed form: `bfenc1:<kid>:<b64url wrapped_dek>:<b64url nonce||ct>`.
//! The AAD (for example `programs.webhook_secret.<row uuid>`) binds a
//! ciphertext to its row, so copying it to another row fails to open.

use std::collections::HashMap;
use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use rand::rngs::OsRng;
use zeroize::Zeroize;

const PREFIX: &str = "bfenc1:";
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum SecretsError {
    /// Bad key configuration.
    Config(String),
    /// Authentication failed: wrong key, wrong AAD or tampered data.
    Decrypt,
    /// The sealed value names a key id this ring does not hold.
    UnknownKey(String),
    /// Not a well-formed sealed value.
    Malformed,
}

impl fmt::Display for SecretsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretsError::Config(m) => write!(f, "secrets config: {m}"),
            SecretsError::Decrypt => write!(f, "secret decrypt failed"),
            SecretsError::UnknownKey(k) => write!(f, "unknown master key id {k}"),
            SecretsError::Malformed => write!(f, "malformed sealed secret"),
        }
    }
}

impl std::error::Error for SecretsError {}

/// Master keys. The active key seals and wraps; every key can open.
pub struct KeyRing {
    active: String,
    keys: HashMap<String, [u8; KEY_LEN]>,
}

impl fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ids: Vec<&String> = self.keys.keys().collect();
        ids.sort();
        f.debug_struct("KeyRing")
            .field("active", &self.active)
            .field("kids", &ids)
            .finish_non_exhaustive()
    }
}

impl Drop for KeyRing {
    fn drop(&mut self) {
        for k in self.keys.values_mut() {
            k.zeroize();
        }
    }
}

fn valid_kid(kid: &str) -> bool {
    !kid.is_empty()
        && kid.len() <= 32
        && kid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

fn aead_seal(key: &[u8; KEY_LEN], plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
    let nonce = random::<NONCE_LEN>();
    let c = XChaCha20Poly1305::new(key.into());
    let ct = c
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("xchacha20poly1305 encrypt");
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

fn aead_open(key: &[u8; KEY_LEN], blob: &[u8], aad: &[u8]) -> Result<Vec<u8>, SecretsError> {
    if blob.len() < NONCE_LEN + 16 {
        return Err(SecretsError::Decrypt);
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| SecretsError::Decrypt)
}

struct Parsed<'a> {
    kid: &'a str,
    wrapped: Vec<u8>,
    body: Vec<u8>,
}

fn parse(sealed: &str) -> Result<Parsed<'_>, SecretsError> {
    let rest = sealed.strip_prefix(PREFIX).ok_or(SecretsError::Malformed)?;
    let mut it = rest.split(':');
    let (kid, w, b) = match (it.next(), it.next(), it.next(), it.next()) {
        (Some(k), Some(w), Some(b), None) => (k, w, b),
        _ => return Err(SecretsError::Malformed),
    };
    if !valid_kid(kid) {
        return Err(SecretsError::Malformed);
    }
    let wrapped = URL_SAFE_NO_PAD
        .decode(w)
        .map_err(|_| SecretsError::Malformed)?;
    let body = URL_SAFE_NO_PAD
        .decode(b)
        .map_err(|_| SecretsError::Malformed)?;
    Ok(Parsed { kid, wrapped, body })
}

impl KeyRing {
    /// Load from `BF_MASTER_KEYS` or `BF_MASTER_KEY_FILE` (never both).
    pub fn from_env() -> Result<Self, SecretsError> {
        let inline = std::env::var("BF_MASTER_KEYS")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let file = std::env::var("BF_MASTER_KEY_FILE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        match (inline, file) {
            (Some(_), Some(_)) => Err(SecretsError::Config(
                "set only one of BF_MASTER_KEYS and BF_MASTER_KEY_FILE".into(),
            )),
            (Some(s), None) => Self::from_str_config(&s),
            (None, Some(p)) => {
                let s = std::fs::read_to_string(&p)
                    .map_err(|e| SecretsError::Config(format!("read {p}: {e}")))?;
                Self::from_str_config(&s)
            }
            (None, None) => Err(SecretsError::Config(
                "BF_MASTER_KEYS or BF_MASTER_KEY_FILE is required".into(),
            )),
        }
    }

    /// Parse `kid:<b64 32 bytes>` entries separated by commas or newlines.
    /// The first entry is the active key.
    pub fn from_str_config(s: &str) -> Result<Self, SecretsError> {
        let mut active: Option<String> = None;
        let mut keys: HashMap<String, [u8; KEY_LEN]> = HashMap::new();
        for entry in s
            .split([',', '\n'])
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            let (kid, b64) = entry
                .split_once(':')
                .ok_or_else(|| SecretsError::Config("entry must be kid:base64".into()))?;
            if !valid_kid(kid) {
                return Err(SecretsError::Config(format!("bad key id {kid:?}")));
            }
            let raw = STANDARD
                .decode(b64.trim())
                .map_err(|_| SecretsError::Config(format!("key {kid}: bad base64")))?;
            let key: [u8; KEY_LEN] = raw
                .try_into()
                .map_err(|_| SecretsError::Config(format!("key {kid}: must be 32 bytes")))?;
            if keys.insert(kid.to_string(), key).is_some() {
                return Err(SecretsError::Config(format!("duplicate key id {kid}")));
            }
            active.get_or_insert_with(|| kid.to_string());
        }
        match active {
            Some(active) => Ok(Self { active, keys }),
            None => Err(SecretsError::Config("no keys configured".into())),
        }
    }

    pub fn from_pairs(active: &str, pairs: &[(&str, [u8; KEY_LEN])]) -> Result<Self, SecretsError> {
        let mut keys = HashMap::new();
        for (k, v) in pairs {
            if !valid_kid(k) {
                return Err(SecretsError::Config(format!("bad key id {k:?}")));
            }
            if keys.insert((*k).to_string(), *v).is_some() {
                return Err(SecretsError::Config(format!("duplicate key id {k}")));
            }
        }
        if !keys.contains_key(active) {
            return Err(SecretsError::Config("active key id not in ring".into()));
        }
        Ok(Self {
            active: active.to_string(),
            keys,
        })
    }

    pub fn active_kid(&self) -> &str {
        &self.active
    }

    pub fn is_sealed(s: &str) -> bool {
        s.starts_with(PREFIX)
    }

    fn kek(&self, kid: &str) -> Result<&[u8; KEY_LEN], SecretsError> {
        self.keys
            .get(kid)
            .ok_or_else(|| SecretsError::UnknownKey(kid.to_string()))
    }

    fn assemble(kid: &str, wrapped: &[u8], body: &[u8]) -> String {
        format!(
            "{PREFIX}{kid}:{}:{}",
            URL_SAFE_NO_PAD.encode(wrapped),
            URL_SAFE_NO_PAD.encode(body)
        )
    }

    pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> String {
        let mut dek = random::<KEY_LEN>();
        let body = aead_seal(&dek, plaintext, aad);
        let kek = self.keys.get(&self.active).expect("active key present");
        let wrapped = aead_seal(kek, &dek, aad);
        dek.zeroize();
        Self::assemble(&self.active, &wrapped, &body)
    }

    fn unwrap_dek(&self, p: &Parsed<'_>, aad: &[u8]) -> Result<[u8; KEY_LEN], SecretsError> {
        let kek = self.kek(p.kid)?;
        let mut raw = aead_open(kek, &p.wrapped, aad)?;
        let dek: [u8; KEY_LEN] = raw
            .as_slice()
            .try_into()
            .map_err(|_| SecretsError::Decrypt)?;
        raw.zeroize();
        Ok(dek)
    }

    pub fn open(&self, sealed: &str, aad: &[u8]) -> Result<Vec<u8>, SecretsError> {
        let p = parse(sealed)?;
        let mut dek = self.unwrap_dek(&p, aad)?;
        let out = aead_open(&dek, &p.body, aad);
        dek.zeroize();
        out
    }

    /// True when the value is plaintext or sealed under a non-active key.
    pub fn needs_rewrap(&self, sealed: &str) -> bool {
        match parse(sealed) {
            Ok(p) => p.kid != self.active,
            Err(_) => true,
        }
    }

    /// Re-wrap the DEK under the active key; the value ciphertext is kept.
    pub fn rewrap(&self, sealed: &str, aad: &[u8]) -> Result<String, SecretsError> {
        let p = parse(sealed)?;
        let mut dek = self.unwrap_dek(&p, aad)?;
        let kek = self.kek(&self.active)?;
        let wrapped = aead_seal(kek, &dek, aad);
        dek.zeroize();
        Ok(Self::assemble(&self.active, &wrapped, &p.body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ring(active: &str, ids: &[&str]) -> KeyRing {
        let pairs: Vec<(&str, [u8; 32])> =
            ids.iter().map(|i| (*i, [i.as_bytes()[i.len() - 1]; 32])).collect();
        KeyRing::from_pairs(active, &pairs).unwrap()
    }

    #[test]
    fn seal_open_roundtrip() {
        let r = ring("k1", &["k1"]);
        for p in [vec![], b"hello".to_vec(), vec![7u8; 10 * 1024]] {
            let s = r.seal(&p, b"t.c.1");
            assert!(KeyRing::is_sealed(&s));
            assert_eq!(r.open(&s, b"t.c.1").unwrap(), p);
        }
    }

    #[test]
    fn wrong_aad_fails() {
        let r = ring("k1", &["k1"]);
        let s = r.seal(b"x", b"a");
        assert_eq!(r.open(&s, b"b"), Err(SecretsError::Decrypt));
    }

    #[test]
    fn tamper_fails() {
        let r = ring("k1", &["k1"]);
        let s = r.seal(b"secret", b"a");
        let p = parse(&s).unwrap();
        // wrapped dek
        let mut w = p.wrapped.clone();
        w[30] ^= 1;
        assert!(r.open(&KeyRing::assemble("k1", &w, &p.body), b"a").is_err());
        // body ciphertext
        let mut b = p.body.clone();
        let last = b.len() - 1;
        b[last] ^= 1;
        assert!(
            r.open(&KeyRing::assemble("k1", &p.wrapped, &b), b"a")
                .is_err()
        );
        // nonce
        let mut b = p.body.clone();
        b[0] ^= 1;
        assert!(
            r.open(&KeyRing::assemble("k1", &p.wrapped, &b), b"a")
                .is_err()
        );
        // prefix
        assert!(r.open(&s.replacen("bfenc1", "bfenc2", 1), b"a").is_err());
        // kid
        let other = ring("k1", &["k1", "k2"]);
        assert!(other.open(&s.replacen(":k1:", ":k2:", 1), b"a").is_err());
        // truncated
        assert!(r.open(&s[..s.len() - 4], b"a").is_err());
    }

    #[test]
    fn unknown_kid_errors() {
        let s = ring("k1", &["k1"]).seal(b"x", b"a");
        let r2 = ring("k2", &["k2"]);
        assert_eq!(
            r2.open(&s, b"a"),
            Err(SecretsError::UnknownKey("k1".into()))
        );
    }

    #[test]
    fn rotation() {
        let r1 = ring("k1", &["k1"]);
        let s = r1.seal(b"plain", b"a");
        let r12 = ring("k2", &["k1", "k2"]);
        assert_eq!(r12.open(&s, b"a").unwrap(), b"plain");
        assert!(r12.needs_rewrap(&s));
        let s2 = r12.rewrap(&s, b"a").unwrap();
        assert!(s2.starts_with("bfenc1:k2:"));
        assert!(!r12.needs_rewrap(&s2));
        assert_eq!(r12.open(&s2, b"a").unwrap(), b"plain");
        let r2 = ring("k2", &["k2"]);
        assert_eq!(r2.open(&s2, b"a").unwrap(), b"plain");
        assert!(r12.needs_rewrap("legacy-plaintext"));
    }

    #[test]
    fn two_seals_differ() {
        let r = ring("k1", &["k1"]);
        assert_ne!(r.seal(b"x", b"a"), r.seal(b"x", b"a"));
    }

    #[test]
    fn env_parsing() {
        let k = |b: u8| STANDARD.encode([b; 32]);
        let ok = format!("k1:{},k2:{}", k(1), k(2));
        let r = KeyRing::from_str_config(&ok).unwrap();
        assert_eq!(r.active_kid(), "k1");
        assert!(KeyRing::from_str_config(&format!("k1:{}\nk2:{}", k(1), k(2))).is_ok());
        assert!(KeyRing::from_str_config("k1:!!!").is_err());
        assert!(KeyRing::from_str_config(&format!("k1:{}", STANDARD.encode([1u8; 16]))).is_err());
        assert!(KeyRing::from_str_config(&format!("k1:{},k1:{}", k(1), k(2))).is_err());
        assert!(KeyRing::from_str_config("").is_err());
        assert!(KeyRing::from_str_config(&format!("bad kid:{}", k(1))).is_err());
        assert!(KeyRing::from_str_config("nocolon").is_err());
    }

    #[test]
    fn debug_redacts() {
        let r = ring("k1", &["k1"]);
        let d = format!("{r:?}");
        assert!(d.contains("k1"));
        assert!(!d.contains("49"), "{d}");
    }

    proptest! {
        #[test]
        fn roundtrip_any_bytes(p in proptest::collection::vec(any::<u8>(), 0..4096), aad in proptest::collection::vec(any::<u8>(), 0..64)) {
            let r = ring("k1", &["k1"]);
            let s = r.seal(&p, &aad);
            prop_assert_eq!(r.open(&s, &aad).unwrap(), p);
        }

        #[test]
        fn bitflip_always_detected(bit in 0usize..4000, p in proptest::collection::vec(any::<u8>(), 1..64)) {
            let r = ring("k1", &["k1"]);
            let s = r.seal(&p, b"a");
            let parsed = parse(&s).unwrap();
            let mut body = parsed.body.clone();
            let i = bit % (body.len() * 8);
            body[i / 8] ^= 1 << (i % 8);
            let mutated = KeyRing::assemble("k1", &parsed.wrapped, &body);
            prop_assert!(r.open(&mutated, b"a").is_err());
        }
    }
}
