//! Startup configuration, read from the environment and validated eagerly.
//!
//! **There are no defaults and no fallbacks.** Every value is required, and a missing or malformed
//! one aborts the process before it accepts a request.
//!
//! That is deliberate and costs a little convenience. A POS server that quietly starts with a
//! plausible-but-wrong replay window, or points at the wrong database because a variable was
//! misspelled, does not fail visibly — it fails as a wrong number in a merchant's monthly report
//! weeks later. Loud and immediate at boot is the only failure mode worth having.

use std::env::VarError;
use std::net::SocketAddr;
use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    #[error("required environment variable {name} is not set")]
    Missing { name: &'static str },

    #[error("environment variable {name} is not valid UTF-8")]
    NotUnicode { name: &'static str },

    #[error("environment variable {name} is invalid: {reason}")]
    Invalid { name: &'static str, reason: String },
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Postgres connection string. The role **must not** hold `BYPASSRLS`, or every row-level
    /// security policy in the schema becomes decorative.
    pub database_url: String,
    /// Maximum pooled connections.
    pub database_max_connections: u32,
    /// Address to bind the HTTP listener to.
    pub bind_address: SocketAddr,
    /// How long a device-enrollment token stays usable. Short by design — it is a bearer credential
    /// that an owner reads off a screen and types into a terminal.
    pub enrollment_token_ttl: Duration,
    /// How far a signed request's timestamp may drift from server time before it is rejected as a
    /// replay. Shop terminals genuinely have bad clocks, so this cannot be seconds; it also cannot
    /// be hours, or a captured request stays usable all day.
    pub signature_max_skew: Duration,
}

impl Config {
    /// Read and validate the whole configuration.
    ///
    /// # Errors
    /// The first [`ConfigError`] encountered. Startup should print it and exit non-zero.
    pub fn from_env() -> Result<Self, ConfigError> {
        let config = Self {
            database_url: required("DATABASE_URL")?,
            database_max_connections: required_parsed("SAHL_DB_MAX_CONNECTIONS")?,
            bind_address: required_parsed("SAHL_BIND_ADDRESS")?,
            enrollment_token_ttl: Duration::from_secs(required_parsed(
                "SAHL_ENROLLMENT_TOKEN_TTL_SECONDS",
            )?),
            signature_max_skew: Duration::from_secs(required_parsed(
                "SAHL_SIGNATURE_MAX_SKEW_SECONDS",
            )?),
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if !self.database_url.starts_with("postgres://")
            && !self.database_url.starts_with("postgresql://")
        {
            return Err(ConfigError::Invalid {
                name: "DATABASE_URL",
                reason: "must be a postgres:// connection string".to_owned(),
            });
        }
        if self.database_max_connections == 0 {
            return Err(ConfigError::Invalid {
                name: "SAHL_DB_MAX_CONNECTIONS",
                reason: "must be at least 1".to_owned(),
            });
        }
        if self.enrollment_token_ttl.is_zero() {
            return Err(ConfigError::Invalid {
                name: "SAHL_ENROLLMENT_TOKEN_TTL_SECONDS",
                reason: "must be greater than zero".to_owned(),
            });
        }
        // An unbounded skew window would accept a replayed request forever.
        if self.signature_max_skew.is_zero() || self.signature_max_skew > Duration::from_secs(3_600)
        {
            return Err(ConfigError::Invalid {
                name: "SAHL_SIGNATURE_MAX_SKEW_SECONDS",
                reason: "must be between 1 and 3600 seconds".to_owned(),
            });
        }
        Ok(())
    }
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => Err(ConfigError::Missing { name }),
        Ok(value) => Ok(value),
        Err(VarError::NotPresent) => Err(ConfigError::Missing { name }),
        Err(VarError::NotUnicode(_)) => Err(ConfigError::NotUnicode { name }),
    }
}

fn required_parsed<T>(name: &'static str) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let raw = required(name)?;
    raw.trim()
        .parse::<T>()
        .map_err(|error| ConfigError::Invalid {
            name,
            reason: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_variable_is_named_in_the_error() {
        // Operators debug these at 3am; the message has to say which one.
        let error = required("SAHL_DEFINITELY_NOT_SET_XYZ").unwrap_err();
        assert_eq!(
            error,
            ConfigError::Missing {
                name: "SAHL_DEFINITELY_NOT_SET_XYZ"
            }
        );
        assert!(error.to_string().contains("SAHL_DEFINITELY_NOT_SET_XYZ"));
    }

    fn valid() -> Config {
        Config {
            database_url: "postgres://localhost/sahl".to_owned(),
            database_max_connections: 5,
            bind_address: "127.0.0.1:8080".parse().expect("valid address"),
            enrollment_token_ttl: Duration::from_secs(900),
            signature_max_skew: Duration::from_secs(300),
        }
    }

    #[test]
    fn a_valid_configuration_passes() {
        assert_eq!(valid().validate(), Ok(()));
    }

    #[test]
    fn a_non_postgres_url_is_rejected() {
        let mut config = valid();
        config.database_url = "mysql://localhost/sahl".to_owned();
        assert!(config.validate().is_err());
    }

    #[test]
    fn a_zero_connection_pool_is_rejected() {
        let mut config = valid();
        config.database_max_connections = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn an_unbounded_skew_window_is_rejected() {
        // A wide window means a captured request stays replayable for as long as it lasts.
        let mut config = valid();
        config.signature_max_skew = Duration::from_secs(86_400);
        assert!(config.validate().is_err());

        config.signature_max_skew = Duration::ZERO;
        assert!(config.validate().is_err());
    }
}
