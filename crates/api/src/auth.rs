//! Passwords, access JWTs, refresh tokens, the `Auth<R>` extractor and the Origin check.

use std::marker::PhantomData;

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

/// 256 random bits, hex encoded.
pub fn new_refresh_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
}

/// HS256 JWT. Hand-rolled over `hmac` to avoid the RSA stack that `jsonwebtoken` drags in.
const JWT_HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

fn jwt_mac(secret: &str) -> Hmac<Sha256> {
    Hmac::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length")
}

pub fn issue_access(secret: &str, p: &Principal) -> Result<String, AppError> {
    let claims = Claims {
        sub: p.user_id,
        role: p.role,
        ws: p.workspace_id,
        exp: chrono::Utc::now().timestamp() + ACCESS_TTL_SECS,
    };
    let claims = serde_json::to_vec(&claims).map_err(|_| AppError::Internal)?;
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(JWT_HEADER),
        URL_SAFE_NO_PAD.encode(claims)
    );
    let mut mac = jwt_mac(secret);
    mac.update(signing_input.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{signing_input}.{sig}"))
}

fn verify_access(secret: &str, token: &str) -> Result<Principal, AppError> {
    let invalid = || AppError::Unauthorized("invalid or expired access token");
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(sig), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    // Only our own header is accepted: no `alg` negotiation.
    if header != URL_SAFE_NO_PAD.encode(JWT_HEADER) {
        return Err(invalid());
    }
    let sig = URL_SAFE_NO_PAD.decode(sig).map_err(|_| invalid())?;
    let mut mac = jwt_mac(secret);
    mac.update(format!("{header}.{payload}").as_bytes());
    mac.verify_slice(&sig).map_err(|_| invalid())?;
    let claims: Claims = URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(invalid)?;
    if claims.exp <= chrono::Utc::now().timestamp() {
        return Err(invalid());
    }
    Ok(Principal {
        user_id: claims.sub,
        role: claims.role,
        workspace_id: claims.ws,
    })
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
        let principal = verify_access(&state.jwt_secret, token)?;
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

    #[test]
    fn auth_jwt_roundtrip_and_tamper() {
        let p = principal(UserRole::Admin, Some(Uuid::now_v7()));
        let t = issue_access("s3cret", &p).unwrap();
        assert_eq!(verify_access("s3cret", &t).unwrap(), p);
        assert!(verify_access("other", &t).is_err());
        // swapped payload keeps the old signature
        let forged = issue_access("s3cret", &principal(UserRole::SuperAdmin, None)).unwrap();
        let swapped = format!(
            "{}.{}.{}",
            t.split('.').next().unwrap(),
            forged.split('.').nth(1).unwrap(),
            t.split('.').nth(2).unwrap()
        );
        assert!(verify_access("s3cret", &swapped).is_err());
        assert!(verify_access("s3cret", "a.b").is_err());
    }

    #[test]
    fn auth_jwt_expired_rejected() {
        let claims = Claims {
            sub: Uuid::now_v7(),
            role: UserRole::Admin,
            ws: None,
            exp: chrono::Utc::now().timestamp() - 1,
        };
        let input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(JWT_HEADER),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        );
        let mut mac = jwt_mac("s3cret");
        mac.update(input.as_bytes());
        let token = format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        );
        assert!(verify_access("s3cret", &token).is_err());
    }
}
