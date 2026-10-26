pub mod metrics;

use std::fmt;
use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub database_url: String,
    pub api_port: u16,
    pub tracker_port: u16,
    pub jwt_secret: String,
    pub spool_dir: PathBuf,
}

/// Shortest `JWT_SECRET` accepted (bytes).
const MIN_JWT_SECRET_LEN: usize = 32;

/// Secrets that ship in `.env.example`, the deploy compose files and the docs. Anyone can sign
/// a `super_admin` token with these, so they are never accepted.
const KNOWN_JWT_SECRETS: [&str; 3] = [
    "change-me-to-a-secure-random-secret-key-at-least-32-chars",
    "insecure-default-jwt-secret-replace-me",
    "your-secure-random-secret-key-at-least-32-chars",
];

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing(&'static str),
    Invalid(&'static str),
    /// Present but unsafe; the text says what to do.
    Weak(&'static str, &'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(key) => write!(f, "missing env var {key}"),
            Self::Invalid(key) => write!(f, "invalid value for env var {key}"),
            Self::Weak(key, why) => write!(f, "unsafe value for env var {key}: {why}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let required = |key: &'static str| get(key).ok_or(ConfigError::Missing(key));
        let port = |key: &'static str| {
            required(key)?
                .parse()
                .map_err(|_| ConfigError::Invalid(key))
        };
        let jwt_secret = required("JWT_SECRET")?;
        if jwt_secret.len() < MIN_JWT_SECRET_LEN {
            return Err(ConfigError::Weak(
                "JWT_SECRET",
                "must be at least 32 bytes; generate one with `openssl rand -hex 32`",
            ));
        }
        if KNOWN_JWT_SECRETS.contains(&jwt_secret.as_str()) {
            return Err(ConfigError::Weak(
                "JWT_SECRET",
                "is a published placeholder; generate one with `openssl rand -hex 32`",
            ));
        }
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            api_port: port("API_PORT")?,
            tracker_port: port("TRACKER_PORT")?,
            jwt_secret,
            spool_dir: required("SPOOL_DIR")?.into(),
        })
    }
}

/// Installs the global tracing subscriber; `RUST_LOG` overrides the `info` default.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(key: &str) -> Option<String> {
        match key {
            "DATABASE_URL" => Some("postgres://localhost/bf".into()),
            "API_PORT" => Some("8080".into()),
            "TRACKER_PORT" => Some("8081".into()),
            "JWT_SECRET" => Some("0123456789abcdef0123456789abcdef".into()),
            "SPOOL_DIR" => Some("/tmp/spool".into()),
            _ => None,
        }
    }

    #[test]
    fn parses_config() {
        let cfg = Config::from_lookup(lookup).unwrap();
        assert_eq!(cfg.api_port, 8080);
        assert_eq!(cfg.tracker_port, 8081);
        assert_eq!(cfg.spool_dir, PathBuf::from("/tmp/spool"));
    }

    #[test]
    fn reports_missing_key() {
        let err = Config::from_lookup(|k| lookup(k).filter(|_| k != "JWT_SECRET")).unwrap_err();
        assert_eq!(err, ConfigError::Missing("JWT_SECRET"));
    }

    #[test]
    fn rejects_short_jwt_secret() {
        let short = |k: &str| {
            (k == "JWT_SECRET")
                .then(|| "x".repeat(31))
                .or_else(|| lookup(k))
        };
        let err = Config::from_lookup(short).unwrap_err();
        assert!(matches!(err, ConfigError::Weak("JWT_SECRET", _)));
        assert!(err.to_string().contains("at least 32 bytes"));
    }

    #[test]
    fn rejects_published_placeholder_secrets() {
        for known in KNOWN_JWT_SECRETS {
            let get = |k: &str| {
                (k == "JWT_SECRET")
                    .then(|| known.to_string())
                    .or_else(|| lookup(k))
            };
            let err = Config::from_lookup(get).unwrap_err();
            assert!(matches!(err, ConfigError::Weak("JWT_SECRET", _)), "{known}");
        }
    }
}
