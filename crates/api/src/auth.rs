//! Passwords, access JWTs, refresh tokens, the `Auth<R>` extractor and the Origin check.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::extract::{FromRequestParts, Request};
use axum::http::header::{COOKIE, HOST, ORIGIN};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bf_db::auth::{User, UserRole};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::AppState;
use crate::error::AppError;

pub const ACCESS_COOKIE: &str = "bf_at";
pub const REFRESH_COOKIE: &str = "bf_rt";
pub const ACCESS_TTL_SECS: i64 = 15 * 60;
pub const REFRESH_TTL_SECS: i64 = 30 * 24 * 60 * 60;

pub fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| {
            tracing::error!(error = %e, "password hash failed");
            AppError::Internal
        })
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| {
        Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

/// Argon2 uses ~19 MiB per run, so only this many hash/verify jobs run at once; the rest wait.
const MAX_CONCURRENT_HASHES: usize = 4;
static HASHING: Semaphore = Semaphore::const_new(MAX_CONCURRENT_HASHES);

/// [`hash_password`] on the blocking pool, bounded by [`HASHING`].
pub async fn hash_password_bounded(password: String) -> Result<String, AppError> {
    let _permit = HASHING.acquire().await.map_err(|_| AppError::Internal)?;
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| AppError::Internal)?
}

/// [`verify_password`] on the blocking pool, bounded by [`HASHING`].
pub async fn verify_password_bounded(password: String, hash: String) -> bool {
    let Ok(_permit) = HASHING.acquire().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .unwrap_or(false)
}

/// A valid hash nobody knows the password of: login verifies against it when the email is
/// unknown, so unknown and wrong-password cost the same.
pub fn dummy_hash() -> &'static str {
    static HASH: LazyLock<String> =
        LazyLock::new(|| hash_password(&random_hex(16)).expect("argon2 hashes"));
    &HASH
}

/// `n` random bytes, hex encoded.
pub fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 256 random bits, hex encoded.
pub fn new_refresh_token() -> String {
    random_hex(32)
}

/// Only the SHA-256 of a refresh token is stored.
pub fn hash_refresh_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub user_id: Uuid,
    pub role: UserRole,
    pub workspace_id: Option<Uuid>,
}

impl From<&User> for Principal {
    fn from(u: &User) -> Self {
        Self {
            user_id: u.id,
            role: u.role,
            workspace_id: u.workspace_id,
        }
    }
}

impl Principal {
    /// Workspace a query must be scoped to. Super admins may pick any workspace
    /// (`requested`, else their own); everyone else only gets their own.
    pub fn workspace_scope(&self, requested: Option<Uuid>) -> Result<Uuid, AppError> {
        if self.role == UserRole::SuperAdmin {
            return requested
                .or(self.workspace_id)
                .ok_or_else(|| AppError::BadRequest("workspace_id required".into()));
        }
        match (self.workspace_id, requested) {
            (Some(own), None) => Ok(own),
            (Some(own), Some(r)) if r == own => Ok(own),
            _ => Err(AppError::Forbidden("workspace not accessible")),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: Uuid,
    role: UserRole,
    ws: Option<Uuid>,
    exp: i64,
    /// `users.token_version` at issue time; absent (pre-upgrade tokens) counts as 0.
    #[serde(default)]
    ver: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jti: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nbf: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    iat: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    typ: Option<String>,
    kid: Option<String>,
}

const MIN_JWT_SECRET_LEN: usize = 32;
const MAX_TOKEN_LEN: usize = 4096;
const MAX_HEADER_LEN: usize = 256;
const MAX_IAT_SKEW_SECS: i64 = 60;
/// Key id given to the legacy `JWT_SECRET`, and assumed for tokens without a `kid`.
pub const LEGACY_KID: &str = "legacy";

/// HS256 signing keys. `active` signs; every key verifies (rotation: add the new key first,
/// drop the old one once its tokens have expired). Hand-rolled over `hmac` to avoid the RSA
/// stack that `jsonwebtoken` drags in (ADR jwt-keep-handrolled).
#[derive(Debug)]
pub struct JwtKeys {
    active: String,
    keys: HashMap<String, Vec<u8>>,
}

impl JwtKeys {
    /// One key under the legacy kid. No length check: for tests and [`issue_access`].
    pub fn single(secret: &str) -> Self {
        Self {
            active: LEGACY_KID.into(),
            keys: HashMap::from([(LEGACY_KID.into(), secret.as_bytes().to_vec())]),
        }
    }

    /// `spec` is `BF_JWT_SECRETS` (`kid:secret,kid:secret`, first is active); `legacy` is
    /// `JWT_SECRET`, added as kid `legacy` (active only when `spec` is absent). Every secret
    /// must be at least 32 bytes.
    pub fn parse(spec: Option<&str>, legacy: Option<&str>) -> Result<Self, String> {
        let mut keys = HashMap::new();
        let mut active = None;
        for entry in spec.into_iter().flat_map(|s| s.split(',')) {
            let (kid, secret) = entry
                .trim()
                .split_once(':')
                .ok_or("BF_JWT_SECRETS entries must be kid:secret")?;
            let kid_ok = (1..=32).contains(&kid.len())
                && kid
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if !kid_ok {
                return Err("BF_JWT_SECRETS kid must match [A-Za-z0-9_-]{1,32}".into());
            }
            if secret.len() < MIN_JWT_SECRET_LEN {
                return Err(format!("JWT secret for kid {kid} is shorter than 32 bytes"));
            }
            if keys
                .insert(kid.to_string(), secret.as_bytes().to_vec())
                .is_some()
            {
                return Err(format!("duplicate JWT kid {kid}"));
            }
            active.get_or_insert_with(|| kid.to_string());
        }
        if let Some(secret) = legacy {
            if secret.len() < MIN_JWT_SECRET_LEN {
                return Err("JWT_SECRET is shorter than 32 bytes".into());
            }
            if keys.contains_key(LEGACY_KID) {
                return Err("kid `legacy` is reserved for JWT_SECRET".into());
            }
            keys.insert(LEGACY_KID.into(), secret.as_bytes().to_vec());
            active.get_or_insert_with(|| LEGACY_KID.to_string());
        }
        let active = active.ok_or("set BF_JWT_SECRETS or JWT_SECRET")?;
        Ok(Self { active, keys })
    }

    fn mac(secret: &[u8]) -> Hmac<Sha256> {
        Hmac::new_from_slice(secret).expect("hmac accepts any key length")
    }

    /// Sign an access token for `p` carrying the user's current `token_version`.
    pub fn issue(&self, p: &Principal, ver: i32) -> Result<String, AppError> {
        self.issue_at(p, ver, chrono::Utc::now().timestamp())
    }

    fn issue_at(&self, p: &Principal, ver: i32, now: i64) -> Result<String, AppError> {
        let claims = Claims {
            sub: p.user_id,
            role: p.role,
            ws: p.workspace_id,
            exp: now + ACCESS_TTL_SECS,
            ver,
            jti: Some(random_hex(8)),
            nbf: None,
            iat: Some(now),
        };
        let header = serde_json::json!({"alg": "HS256", "typ": "JWT", "kid": self.active});
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).map_err(|_| AppError::Internal)?),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| AppError::Internal)?)
        );
        let mut mac = Self::mac(&self.keys[&self.active]);
        mac.update(signing_input.as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        Ok(format!("{signing_input}.{sig}"))
    }

    /// Check structure, header, MAC and time claims. Returns the principal and the token's
    /// `ver`; the caller still compares `ver` with the user's current `token_version`.
    pub fn verify(&self, token: &str) -> Result<(Principal, i32), AppError> {
        self.verify_at(token, chrono::Utc::now().timestamp())
    }

    fn verify_at(&self, token: &str, now: i64) -> Result<(Principal, i32), AppError> {
        let invalid = || AppError::Unauthorized("invalid or expired access token");
        if token.len() > MAX_TOKEN_LEN {
            return Err(invalid());
        }
        let mut parts = token.split('.');
        let (Some(header), Some(payload), Some(sig), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        if header.len() > MAX_HEADER_LEN {
            return Err(invalid());
        }
        let signing_input = &token[..header.len() + 1 + payload.len()];
        let decode = |s: &str| {
            let bytes = URL_SAFE_NO_PAD.decode(s).ok()?;
            (URL_SAFE_NO_PAD.encode(&bytes) == s).then_some(bytes)
        };
        let header_json = decode(header).ok_or_else(invalid)?;
        let sig = decode(sig).ok_or_else(invalid)?;
        let payload = decode(payload).ok_or_else(invalid)?;
        let h: Header = serde_json::from_slice(&header_json).map_err(|_| invalid())?;
        if h.alg != "HS256" || h.typ.as_deref().is_some_and(|t| t != "JWT") {
            return Err(invalid());
        }
        let kid = h.kid.as_deref().unwrap_or(LEGACY_KID);
        let secret = self.keys.get(kid).ok_or_else(invalid)?;
        let mut mac = Self::mac(secret);
        mac.update(signing_input.as_bytes());
        mac.verify_slice(&sig).map_err(|_| invalid())?;
        let claims: Claims = serde_json::from_slice(&payload).map_err(|_| invalid())?;
        if claims.exp <= now
            || claims.nbf.is_some_and(|nbf| nbf > now)
            || claims.iat.is_some_and(|iat| iat > now + MAX_IAT_SKEW_SECS)
        {
            return Err(invalid());
        }
        Ok((
            Principal {
                user_id: claims.sub,
                role: claims.role,
                workspace_id: claims.ws,
            },
            claims.ver,
        ))
    }
}

/// Sign with a single legacy key and version 0. Convenience for tests and tooling.
pub fn issue_access(secret: &str, p: &Principal) -> Result<String, AppError> {
    JwtKeys::single(secret).issue(p, 0)
}

/// How long a user's `token_version` is cached per instance. This is the revocation bound
/// across instances: a bump on one instance reaches the others within this time.
pub const VERSION_CACHE_TTL: Duration = Duration::from_secs(5);
const VERSION_CACHE_MAX: usize = 10_000;

/// Per-instance cache of `users.token_version`, so a request does not always hit the DB.
#[derive(Default)]
pub struct VersionCache {
    map: Mutex<HashMap<Uuid, (i32, Instant)>>,
}

impl VersionCache {
    fn get(&self, user: Uuid) -> Option<i32> {
        let map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&user)
            .filter(|(_, at)| at.elapsed() < VERSION_CACHE_TTL)
            .map(|(v, _)| *v)
    }

    fn put(&self, user: Uuid, ver: i32) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= VERSION_CACHE_MAX {
            map.retain(|_, (_, at)| at.elapsed() < VERSION_CACHE_TTL);
            if map.len() >= VERSION_CACHE_MAX {
                map.clear();
            }
        }
        map.insert(user, (ver, Instant::now()));
    }

    /// Forget a user's cached version (this instance only).
    pub fn evict(&self, user: Uuid) {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&user);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

/// Revoke every access token of `user`: bump `token_version` and evict this instance's cache.
/// Other instances catch up within [`VERSION_CACHE_TTL`].
pub async fn revoke_access_tokens(s: &AppState, user: Uuid) -> Result<(), AppError> {
    bf_db::auth::bump_token_version(&s.pool, user).await?;
    s.version_cache.evict(user);
    Ok(())
}

/// Verify the token, then require its `ver` to equal the user's current `token_version`
/// (a deleted user is rejected).
async fn authenticate(s: &AppState, token: &str) -> Result<Principal, AppError> {
    let (principal, ver) = s.jwt_keys.verify(token)?;
    let current = match s.version_cache.get(principal.user_id) {
        Some(v) => v,
        None => {
            let v = bf_db::auth::token_version(&s.pool, principal.user_id)
                .await?
                .ok_or(AppError::Unauthorized("invalid or expired access token"))?;
            s.version_cache.put(principal.user_id, v);
            v
        }
    };
    if ver != current {
        return Err(AppError::Unauthorized("invalid or expired access token"));
    }
    Ok(principal)
}

pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| kv.trim().strip_prefix(name)?.strip_prefix('='))
}

/// Which roles a route accepts.
pub trait Role {
    fn allows(role: UserRole) -> bool;
}

/// Any signed-in user.
pub struct Any;
/// `admin` or `super_admin`.
pub struct Admin;
pub struct SuperAdmin;
pub struct Affiliate;

impl Role for Any {
    fn allows(_: UserRole) -> bool {
        true
    }
}
impl Role for Admin {
    fn allows(role: UserRole) -> bool {
        matches!(role, UserRole::Admin | UserRole::SuperAdmin)
    }
}
impl Role for SuperAdmin {
    fn allows(role: UserRole) -> bool {
        role == UserRole::SuperAdmin
    }
}
impl Role for Affiliate {
    fn allows(role: UserRole) -> bool {
        role == UserRole::Affiliate
    }
}

/// Extractor: valid `bf_at` cookie whose role satisfies `R`, else 401/403.
pub struct Auth<R> {
    pub principal: Principal,
    _role: PhantomData<R>,
}

impl<R: Role> FromRequestParts<AppState> for Auth<R> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let token = cookie(&parts.headers, ACCESS_COOKIE)
            .ok_or(AppError::Unauthorized("authentication required"))?;
        let principal = authenticate(state, token).await?;
        if !R::allows(principal.role) {
            return Err(AppError::Forbidden("insufficient role"));
        }
        Ok(Self {
            principal,
            _role: PhantomData,
        })
    }
}

/// Middleware: mutating requests must carry an `Origin` equal to the request's own host
/// (same-origin deployment behind Caddy). Missing or mismatched Origin is 403.
pub async fn check_origin(req: Request, next: Next) -> Result<Response, AppError> {
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        let origin = req.headers().get(ORIGIN).and_then(|v| v.to_str().ok());
        let host = req
            .headers()
            .get(HOST)
            .and_then(|v| v.to_str().ok())
            .or_else(|| req.uri().authority().map(|a| a.as_str()));
        let same = match (origin.and_then(|o| o.split_once("://")), host) {
            (Some((_, authority)), Some(host)) => authority == host,
            _ => false,
        };
        if !same {
            return Err(AppError::Forbidden("origin mismatch"));
        }
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(role: UserRole, ws: Option<Uuid>) -> Principal {
        Principal {
            user_id: Uuid::now_v7(),
            role,
            workspace_id: ws,
        }
    }

    #[test]
    fn auth_workspace_scope_rules() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let admin = principal(UserRole::Admin, Some(a));
        assert_eq!(admin.workspace_scope(None).unwrap(), a);
        assert_eq!(admin.workspace_scope(Some(a)).unwrap(), a);
        assert!(matches!(
            admin.workspace_scope(Some(b)),
            Err(AppError::Forbidden(_))
        ));
        let orphan = principal(UserRole::Affiliate, None);
        assert!(orphan.workspace_scope(None).is_err());
        let root = principal(UserRole::SuperAdmin, Some(a));
        assert_eq!(root.workspace_scope(Some(b)).unwrap(), b);
        assert_eq!(root.workspace_scope(None).unwrap(), a);
        assert!(
            principal(UserRole::SuperAdmin, None)
                .workspace_scope(None)
                .is_err()
        );
    }

    #[test]
    fn auth_cookie_lookup() {
        let mut h = HeaderMap::new();
        h.insert(COOKIE, "a=1; bf_at=tok; bf_atx=no".parse().unwrap());
        assert_eq!(cookie(&h, "bf_at"), Some("tok"));
        assert_eq!(cookie(&h, "bf_rt"), None);
    }

    const S1: &str = "0123456789abcdef0123456789abcdef-one";
    const S2: &str = "fedcba9876543210fedcba9876543210-two";

    fn keys() -> JwtKeys {
        JwtKeys::parse(Some(&format!("k1:{S1}")), None).unwrap()
    }

    fn b64(v: &serde_json::Value) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
    }

    /// Hand-signed token with arbitrary header and claims.
    fn forge(secret: &str, header: serde_json::Value, claims: serde_json::Value) -> String {
        let input = format!("{}.{}", b64(&header), b64(&claims));
        let mut mac = JwtKeys::mac(secret.as_bytes());
        mac.update(input.as_bytes());
        format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        )
    }

    fn hdr() -> serde_json::Value {
        serde_json::json!({"alg": "HS256", "typ": "JWT", "kid": "k1"})
    }

    fn claims(exp_off: i64) -> serde_json::Value {
        let now = chrono::Utc::now().timestamp();
        serde_json::json!({"sub": Uuid::now_v7(), "role": "admin", "ws": null, "exp": now + exp_off})
    }

    fn ok(t: &str) -> bool {
        keys().verify(t).is_ok()
    }

    #[test]
    fn jwt_roundtrip_and_header() {
        let p = principal(UserRole::Admin, Some(Uuid::now_v7()));
        let t = keys().issue(&p, 3).unwrap();
        assert_eq!(keys().verify(&t).unwrap(), (p, 3));
        let h: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(t.split('.').next().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(h, hdr());
    }

    #[test]
    fn jwt_alg_none_rejected() {
        let t = forge(
            S1,
            serde_json::json!({"alg": "none", "typ": "JWT", "kid": "k1"}),
            claims(60),
        );
        assert!(!ok(&t));
        let unsigned = format!(
            "{}.{}.",
            b64(&serde_json::json!({"alg":"none","kid":"k1"})),
            b64(&claims(60))
        );
        assert!(!ok(&unsigned));
    }

    #[test]
    fn jwt_other_algs_rejected() {
        for alg in ["HS512", "HS384", "RS256", "hs256", "ES256"] {
            let t = forge(
                S1,
                serde_json::json!({"alg": alg, "typ": "JWT", "kid": "k1"}),
                claims(60),
            );
            assert!(!ok(&t), "{alg}");
        }
    }

    #[test]
    fn jwt_alg_rs256_with_hmac_of_public_key_rejected() {
        // classic confusion: attacker MACs with a PEM public key and claims RS256
        let pem =
            "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOC\n-----END PUBLIC KEY-----";
        let t = forge(
            pem,
            serde_json::json!({"alg": "RS256", "typ": "JWT", "kid": "k1"}),
            claims(60),
        );
        assert!(!ok(&t));
    }

    #[test]
    fn jwt_typ_and_extra_header_fields() {
        assert!(!ok(&forge(
            S1,
            serde_json::json!({"alg":"HS256","typ":"JWS","kid":"k1"}),
            claims(60)
        )));
        assert!(!ok(&forge(
            S1,
            serde_json::json!({"alg":"HS256","kid":"k1","jku":"http://x"}),
            claims(60)
        )));
        // typ may be absent
        assert!(ok(&forge(
            S1,
            serde_json::json!({"alg":"HS256","kid":"k1"}),
            claims(60)
        )));
    }

    #[test]
    fn jwt_time_claims() {
        assert!(ok(&forge(S1, hdr(), claims(60))));
        assert!(!ok(&forge(S1, hdr(), claims(-1))), "expired");
        assert!(!ok(&forge(S1, hdr(), claims(0))), "exp == now");
        let mut c = claims(60);
        c.as_object_mut().unwrap().remove("exp");
        assert!(!ok(&forge(S1, hdr(), c)), "exp missing");
        let now = chrono::Utc::now().timestamp();
        let mut c = claims(60);
        c["nbf"] = (now + 120).into();
        assert!(!ok(&forge(S1, hdr(), c)), "nbf future");
        let mut c = claims(60);
        c["nbf"] = (now - 5).into();
        assert!(ok(&forge(S1, hdr(), c)), "nbf past");
        let mut c = claims(60);
        c["iat"] = (now + 3600).into();
        assert!(!ok(&forge(S1, hdr(), c)), "iat far future");
        let mut c = claims(60);
        c["iat"] = (now + 30).into();
        assert!(ok(&forge(S1, hdr(), c)), "iat small skew");
    }

    #[test]
    fn jwt_signature_and_structure() {
        let p = principal(UserRole::Admin, None);
        let t = keys().issue(&p, 0).unwrap();
        assert!(!ok(&format!("{}x", t)), "bad signature");
        assert!(!ok(&t[..t.len() - 4]), "truncated signature");
        assert!(!ok(&format!("{t}.extra")), "extra segment");
        assert!(!ok(&format!("{t}=")), "padding");
        assert!(!ok("a.b"));
        assert!(!ok(""));
        assert!(!ok(&format!("{}.{}", t, "A".repeat(5000))), "oversize");
        let big = forge(
            S1,
            serde_json::json!({"alg":"HS256","kid":"k1","pad":"x".repeat(300)}),
            claims(60),
        );
        assert!(!ok(&big), "header too long");
        let parts: Vec<_> = t.split('.').collect();
        let forged = format!("{}.{}.{}", parts[0], b64(&claims(60)), parts[2]);
        assert!(!ok(&forged), "payload swapped, signature kept");
    }

    #[test]
    fn jwt_role_tamper_rejected() {
        let t = keys()
            .issue(&principal(UserRole::Affiliate, None), 0)
            .unwrap();
        let parts: Vec<_> = t.split('.').collect();
        let mut c: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        c["role"] = "super_admin".into();
        assert!(!ok(&format!("{}.{}.{}", parts[0], b64(&c), parts[2])));
    }

    #[test]
    fn jwt_kid_handling() {
        assert!(!ok(&forge(
            S1,
            serde_json::json!({"alg":"HS256","kid":"nope"}),
            claims(60)
        )));
        // no kid: legacy only if configured
        let no_kid = forge(
            S1,
            serde_json::json!({"alg":"HS256","typ":"JWT"}),
            claims(60),
        );
        assert!(!ok(&no_kid));
        let with_legacy = JwtKeys::parse(Some(&format!("k1:{S2}")), Some(S1)).unwrap();
        assert!(with_legacy.verify(&no_kid).is_ok());
    }

    #[test]
    fn jwt_rotation() {
        let p = principal(UserRole::Admin, None);
        let old = keys().issue(&p, 0).unwrap();
        let ring = JwtKeys::parse(Some(&format!("k2:{S2},k1:{S1}")), None).unwrap();
        assert!(ring.verify(&old).is_ok(), "old token still verifies");
        let new = ring.issue(&p, 0).unwrap();
        let h: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(new.split('.').next().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(h["kid"], "k2");
        let only_k2 = JwtKeys::parse(Some(&format!("k2:{S2}")), None).unwrap();
        assert!(only_k2.verify(&old).is_err(), "k1 removed");
        assert!(only_k2.verify(&new).is_ok());
    }

    #[test]
    fn jwt_short_secret_rejected_at_startup() {
        assert!(JwtKeys::parse(Some("k1:short"), None).is_err());
        assert!(JwtKeys::parse(None, Some("short")).is_err());
        assert!(JwtKeys::parse(None, None).is_err());
        assert!(JwtKeys::parse(Some(&format!("k1:{S1},k1:{S2}")), None).is_err());
        assert!(JwtKeys::parse(Some(&format!("bad kid:{S1}")), None).is_err());
        // legacy only: signs under kid legacy
        let k = JwtKeys::parse(None, Some(S1)).unwrap();
        assert_eq!(k.active, LEGACY_KID);
    }

    #[test]
    fn jwt_missing_ver_counts_as_zero() {
        let t = forge(S1, hdr(), claims(60));
        assert_eq!(keys().verify(&t).unwrap().1, 0);
    }

    #[test]
    fn version_cache_ttl_bound_and_evict() {
        let c = VersionCache::default();
        let u = Uuid::now_v7();
        assert_eq!(c.get(u), None);
        c.put(u, 4);
        assert_eq!(c.get(u), Some(4));
        c.evict(u);
        assert_eq!(c.get(u), None);
        for _ in 0..VERSION_CACHE_MAX + 5 {
            c.put(Uuid::now_v7(), 0);
        }
        assert!(c.len() <= VERSION_CACHE_MAX);
        assert_eq!(VERSION_CACHE_TTL, Duration::from_secs(5));
    }

    proptest::proptest! {
        #[test]
        fn jwt_verify_never_panics(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..600)) {
            let s = String::from_utf8_lossy(&bytes);
            proptest::prop_assert!(keys().verify(&s).is_err());
        }

        #[test]
        fn jwt_verify_never_panics_dotted(a in "[A-Za-z0-9_=-]{0,80}", b in "[A-Za-z0-9_=-]{0,80}", c in "[A-Za-z0-9_=-]{0,80}") {
            let _ = keys().verify(&format!("{a}.{b}.{c}"));
        }

        #[test]
        fn jwt_roundtrip(role in 0usize..3, has_ws: bool, ver in 0i32..1000) {
            let role = [UserRole::Admin, UserRole::SuperAdmin, UserRole::Affiliate][role];
            let p = principal(role, has_ws.then(Uuid::now_v7));
            let t = keys().issue(&p, ver).unwrap();
            proptest::prop_assert_eq!(keys().verify(&t).unwrap(), (p, ver));
        }
    }
}
