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

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing(&'static str),
    Invalid(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(key) => write!(f, "missing env var {key}"),
            Self::Invalid(key) => write!(f, "invalid value for env var {key}"),
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
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            api_port: port("API_PORT")?,
            tracker_port: port("TRACKER_PORT")?,
            jwt_secret: required("JWT_SECRET")?,
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
            "JWT_SECRET" => Some("secret".into()),
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
}
