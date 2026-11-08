//! Application configuration loaded from environment variables.
//!
//! # Configuration Hierarchy
//!
//! All configuration is loaded from environment variables with sensible defaults
//! for development. In production, configure via environment variables or a `.env` file.
//!
//! # Security Configuration
//!
//! - `API_KEY`: When set, enables API key authentication for all endpoints except `/health`
//!   and `/ready`. Requires `TRUSTED_PROXIES`, or `ALLOW_UNVERIFIED_CLIENT_IPS=true` for
//!   local development: without verified client IPs the brute-force block cannot work
//! - `CORS_ALLOWED_ORIGINS`: Comma-separated list of allowed origins (default: `*` for dev)
//!
//! # Performance Tuning
//!
//! - `BATCH_MAX_SIZE`: Maximum messages per batch (default: 1000)
//! - `POLL_MAX_COUNT`: Maximum messages per poll (default: 100)
//! - `RATE_LIMIT_RPS`: Requests per second limit (default: 100)
//! - `RATE_LIMIT_BURST`: Burst capacity for rate limiter (default: 50)
//!
//! # Default Topic Options
//!
//! `IGGY_TOPIC_<OPTION>` sets one per-topic option of the default topic, e.g.
//! `IGGY_TOPIC_DURABILITY=persisted` or `IGGY_TOPIC_SEGMENT_SIZE="256 MiB"`.
//! The options and their forms are those of `POST /streams/{s}/topics`
//! ([`TopicOptionsInput`]); unset or empty leaves an option to the server.
//! They apply only when the gateway creates the topic: topic options are fixed
//! at creation, so for an existing topic the gateway logs each one that
//! differs and leaves the topic unchanged.

use std::env;
use std::time::Duration;

use crate::error::{AppError, AppResult};
use crate::models::{TopicOptions, TopicOptionsInput};

/// Application configuration loaded from environment variables.
///
/// # Example
///
/// ```rust,ignore
/// let config = Config::from_env()?;
/// println!("Server will listen on {}", config.server_addr());
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    // =========================================================================
    // Server Configuration
    // =========================================================================
    /// Server host address (default: "0.0.0.0")
    pub host: String,

    /// Server port (default: 8000; the Iggy server's HTTP API uses 3000)
    pub port: u16,

    // =========================================================================
    // Iggy Connection Configuration
    // =========================================================================
    /// Iggy server connection string
    /// Format: "iggy://user:pass@host:port"
    /// Default: "iggy://iggy:iggy@localhost:8090"
    pub iggy_connection_string: String,

    /// Default stream name for the application
    pub default_stream: String,

    /// Default topic name for the application
    pub default_topic: String,

    /// Number of partitions for the default topic
    pub topic_partitions: u32,

    /// Per-topic options for the default topic, from `IGGY_TOPIC_<OPTION>`.
    /// Applied only when the gateway creates the topic.
    pub default_topic_options: TopicOptions,

    // =========================================================================
    // Connection Resilience Configuration
    // =========================================================================
    /// Maximum reconnection attempts before giving up (0 = infinite)
    pub max_reconnect_attempts: u32,

    /// Base delay between reconnection attempts (exponential backoff applies)
    pub reconnect_base_delay: Duration,

    /// Maximum delay between reconnection attempts
    pub reconnect_max_delay: Duration,

    /// Interval for connection health checks
    pub health_check_interval: Duration,

    /// Timeout for individual Iggy client operations (default: 30 seconds)
    /// Prevents operations from hanging indefinitely on network issues
    pub operation_timeout: Duration,

    // =========================================================================
    // Circuit Breaker Configuration
    // =========================================================================
    /// Number of consecutive failures before opening the circuit (default: 5)
    pub circuit_breaker_failure_threshold: u32,

    /// Number of consecutive successes in half-open state to close circuit (default: 2)
    pub circuit_breaker_success_threshold: u32,

    /// How long the circuit stays open before transitioning to half-open (default: 30s)
    pub circuit_breaker_open_duration: Duration,

    // =========================================================================
    // Rate Limiting Configuration
    // =========================================================================
    /// Requests per second limit per client (default: 100)
    /// Set to 0 to disable rate limiting
    pub rate_limit_rps: u32,

    /// Burst capacity - allows temporary spikes above rps limit (default: 50)
    pub rate_limit_burst: u32,

    // =========================================================================
    // Message Limits Configuration
    // =========================================================================
    /// Maximum number of messages in a single batch sent (default: 1000)
    pub batch_max_size: usize,

    /// Maximum number of messages to return in a single poll (default: 100)
    pub poll_max_count: u32,

    /// Maximum request body size in bytes (default: 10MB)
    /// Prevents denial-of-service via large payloads
    pub max_request_body_size: usize,

    // =========================================================================
    // Security Configuration
    // =========================================================================
    /// API key for authentication (optional - when set, all endpoints require it)
    /// Pass via `X-API-Key` header or `api_key` query parameter
    pub api_key: Option<String>,

    /// Paths that bypass authentication (for health checks, monitoring).
    /// Default: ["/health", "/ready"]
    /// Security note: Only add paths that don't expose sensitive data.
    pub auth_bypass_paths: Vec<String>,

    /// Comma-separated list of allowed CORS origins
    /// Use "*" to allow all origins (not recommended for production)
    /// Example: `<https://app.example.com>,<https://admin.example.com>`
    pub cors_allowed_origins: Vec<String>,

    /// Trusted proxy CIDR ranges for IP spoofing mitigation.
    /// X-Forwarded-For headers will only be trusted if the connection
    /// originates from one of these networks.
    ///
    /// Format: Comma-separated CIDR notation (e.g., "10.0.0.0/8,172.16.0.0/12")
    /// Default: Empty (trust all sources - NOT recommended for production)
    ///
    /// Common values for cloud environments:
    /// - Private networks: "10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"
    /// - Kubernetes: "10.0.0.0/8" (pod network)
    /// - Docker: "172.17.0.0/16" (default bridge network)
    /// - Localhost: "127.0.0.0/8,::1/128"
    pub trusted_proxies: Vec<String>,

    /// Accept `API_KEY` without `TRUSTED_PROXIES` (default: false).
    ///
    /// Without trusted proxies the client IP comes from client-supplied
    /// `X-Forwarded-For`/`X-Real-IP`, so the auth brute-force limiter can
    /// throttle failures but never block an attacker, who rotates the header
    /// for a fresh budget. [`Config::from_env`] therefore refuses that
    /// combination unless this is `true` (`ALLOW_UNVERIFIED_CLIENT_IPS=true`),
    /// which is meant for local development only. It has no effect when
    /// `TRUSTED_PROXIES` is set or `API_KEY` is not.
    pub allow_unverified_client_ips: bool,

    // =========================================================================
    // Observability Configuration
    // =========================================================================
    /// Log level (e.g., "info", "debug", "trace")
    pub log_level: String,

    /// Interval for background stats cache refresh (default: 5 seconds)
    pub stats_cache_ttl: Duration,

    /// Port for Prometheus metrics endpoint (default: 9090, 0 = disabled)
    pub metrics_port: u16,
}

impl Config {
    /// Load configuration from environment variables with sensible defaults.
    ///
    /// # Errors
    ///
    /// Returns `AppError::ConfigError` if any required configuration is invalid
    /// (e.g., non-numeric PORT value, invalid delay ordering).
    pub fn from_env() -> AppResult<Self> {
        // Load an .env file if present (ignore errors if not found)
        let _ = dotenvy::dotenv();

        let topic_partitions = Self::parse_env("IGGY_PARTITIONS", 3)?;
        let default_topic_options =
            default_topic_options_from(|name| env::var(name).ok(), topic_partitions)?;

        let config = Self {
            // Server
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: Self::parse_env("PORT", 8000)?,

            // Iggy connection
            iggy_connection_string: env::var("IGGY_CONNECTION_STRING")
                .unwrap_or_else(|_| "iggy://iggy:iggy@localhost:8090".to_string()),
            default_stream: env::var("IGGY_STREAM").unwrap_or_else(|_| "sample-stream".to_string()),
            default_topic: env::var("IGGY_TOPIC").unwrap_or_else(|_| "events".to_string()),
            topic_partitions,
            default_topic_options,

            // Connection resilience
            max_reconnect_attempts: Self::parse_env("MAX_RECONNECT_ATTEMPTS", 0)?, // 0 = infinite
            reconnect_base_delay: Duration::from_millis(Self::parse_env(
                "RECONNECT_BASE_DELAY_MS",
                1000,
            )?),
            reconnect_max_delay: Duration::from_millis(Self::parse_env(
                "RECONNECT_MAX_DELAY_MS",
                30000,
            )?),
            health_check_interval: Duration::from_secs(Self::parse_env(
                "HEALTH_CHECK_INTERVAL_SECS",
                30,
            )?),
            operation_timeout: Duration::from_secs(Self::parse_env("OPERATION_TIMEOUT_SECS", 30)?),

            // Circuit breaker
            circuit_breaker_failure_threshold: Self::parse_env(
                "CIRCUIT_BREAKER_FAILURE_THRESHOLD",
                5,
            )?,
            circuit_breaker_success_threshold: Self::parse_env(
                "CIRCUIT_BREAKER_SUCCESS_THRESHOLD",
                2,
            )?,
            circuit_breaker_open_duration: Duration::from_secs(Self::parse_env(
                "CIRCUIT_BREAKER_OPEN_DURATION_SECS",
                30,
            )?),

            // Rate limiting
            rate_limit_rps: Self::parse_env("RATE_LIMIT_RPS", 100)?,
            rate_limit_burst: Self::parse_env("RATE_LIMIT_BURST", 50)?,

            // Message limits
            batch_max_size: Self::parse_env("BATCH_MAX_SIZE", 1000)?,
            poll_max_count: Self::parse_env("POLL_MAX_COUNT", 100)?,
            max_request_body_size: Self::parse_env("MAX_REQUEST_BODY_SIZE", 10 * 1024 * 1024)?, // 10MB

            // Security
            api_key: env::var("API_KEY").ok().filter(|k| !k.is_empty()),
            auth_bypass_paths: Self::parse_auth_bypass_paths(),
            cors_allowed_origins: Self::parse_cors_origins(),
            trusted_proxies: Self::parse_trusted_proxies(),
            allow_unverified_client_ips: Self::parse_flag("ALLOW_UNVERIFIED_CLIENT_IPS")?,

            // Observability
            log_level: env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
            stats_cache_ttl: Duration::from_secs(Self::parse_env("STATS_CACHE_TTL_SECS", 5)?),
            metrics_port: Self::parse_env("METRICS_PORT", 9090)?,
        };

        // Validate configuration before returning
        config.validate()?;

        Ok(config)
    }

    /// Validate configuration values for consistency and correctness.
    ///
    /// # Errors
    ///
    /// Returns `AppError::ConfigError` if validation fails.
    fn validate(&self) -> AppResult<()> {
        // Several comma-separated server addresses are seeds, tried in order
        // (`iggy_client::seeds`). The reason never repeats the string, which
        // holds credentials.
        crate::iggy_client::seeds::expand(&self.iggy_connection_string)
            .map_err(|reason| AppError::ConfigError(format!("IGGY_CONNECTION_STRING: {reason}")))?;

        // Validate delay ordering
        if self.reconnect_base_delay > self.reconnect_max_delay {
            return Err(AppError::ConfigError(format!(
                "RECONNECT_BASE_DELAY_MS ({:?}) must be <= RECONNECT_MAX_DELAY_MS ({:?})",
                self.reconnect_base_delay, self.reconnect_max_delay
            )));
        }

        // A zero max collapses the backoff floor to zero-delay retry spinning
        if self.reconnect_max_delay.is_zero() {
            return Err(AppError::ConfigError(
                "RECONNECT_MAX_DELAY_MS must be greater than 0".to_string(),
            ));
        }

        // Validate message limits are positive
        if self.batch_max_size == 0 {
            return Err(AppError::ConfigError(
                "BATCH_MAX_SIZE must be greater than 0".to_string(),
            ));
        }

        if self.poll_max_count == 0 {
            return Err(AppError::ConfigError(
                "POLL_MAX_COUNT must be greater than 0".to_string(),
            ));
        }

        // Validate max request body size is reasonable
        if self.max_request_body_size == 0 {
            return Err(AppError::ConfigError(
                "MAX_REQUEST_BODY_SIZE must be greater than 0".to_string(),
            ));
        }

        // A zero open window makes the breaker a no-op AND a log source: Open
        // never rejects because the window has always elapsed, and every
        // admission past the budget re-grants, bumping the probe generation so
        // each outstanding permit then returns as a stale discard. That is an
        // info plus a warn per request during the outage the breaker exists to
        // damp.
        if self.circuit_breaker_open_duration.is_zero() {
            return Err(AppError::ConfigError(
                "CIRCUIT_BREAKER_OPEN_DURATION_SECS must be greater than 0".to_string(),
            ));
        }

        // A zero operation timeout expires every request on its first poll,
        // and because that deadline equals the global one it counts as outage
        // evidence - so the breaker opens on a service that is perfectly
        // healthy and never closes.
        if self.operation_timeout.is_zero() {
            return Err(AppError::ConfigError(
                "OPERATION_TIMEOUT_SECS must be greater than 0".to_string(),
            ));
        }

        // Fail closed: an API key without verified client IPs looks protected
        // but is not. The brute-force block applies only to verified IPs
        // (`ip::client_ip_is_verified`); without TRUSTED_PROXIES every client
        // IP comes from a header the attacker controls, so guessing is
        // limited only by per-IP throttles that reset whenever the attacker
        // rotates that header.
        if self.auth_enabled()
            && !self.proxy_validation_enabled()
            && !self.allow_unverified_client_ips
        {
            return Err(AppError::ConfigError(
                "API_KEY is set but TRUSTED_PROXIES is empty: client IPs would come from \
                 spoofable X-Forwarded-For/X-Real-IP headers, so the brute-force block \
                 could never apply. Set TRUSTED_PROXIES to your reverse proxy's CIDR \
                 ranges, or set ALLOW_UNVERIFIED_CLIENT_IPS=true for local development"
                    .to_string(),
            ));
        }

        Ok(())
    }

    /// Get the full server address for binding.
    pub fn server_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Check if rate limiting is enabled.
    pub fn rate_limiting_enabled(&self) -> bool {
        self.rate_limit_rps > 0
    }

    /// Check if API key authentication is enabled.
    pub fn auth_enabled(&self) -> bool {
        self.api_key.is_some()
    }

    /// Check if trusted proxy validation is enabled.
    ///
    /// When enabled, X-Forwarded-For headers are only trusted if the request
    /// originates from a configured trusted proxy network.
    pub fn proxy_validation_enabled(&self) -> bool {
        !self.trusted_proxies.is_empty()
    }

    /// Check if Prometheus metrics export is enabled.
    pub fn metrics_enabled(&self) -> bool {
        self.metrics_port > 0
    }

    /// Get the metrics endpoint address (binds the same host as the API).
    ///
    /// Returns `None` if metrics are disabled (port = 0).
    /// This is the single source of truth for the metrics bind address;
    /// `main.rs` starts the exporter from it.
    pub fn metrics_addr(&self) -> Option<String> {
        if self.metrics_enabled() {
            Some(format!("{}:{}", self.host, self.metrics_port))
        } else {
            None
        }
    }

    /// Parse an environment variable into the specified type with a default value.
    fn parse_env<T>(name: &str, default: T) -> AppResult<T>
    where
        T: std::str::FromStr + ToString,
        T::Err: std::fmt::Display,
    {
        match env::var(name) {
            Ok(val) => val
                .parse()
                .map_err(|e| AppError::ConfigError(format!("Invalid {name}: {e}"))),
            Err(_) => Ok(default),
        }
    }

    /// Parse an opt-in boolean flag: unset or empty is `false`, like the
    /// other security variables treat empty as unset; otherwise `true` or
    /// `false` in any case. Anything else fails startup rather than being
    /// guessed at, since these flags relax security checks.
    fn parse_flag(name: &str) -> AppResult<bool> {
        parse_flag_value(name, env::var(name).ok().as_deref())
    }

    /// Parse CORS allowed origins from environment variable.
    fn parse_cors_origins() -> Vec<String> {
        env::var("CORS_ALLOWED_ORIGINS")
            .unwrap_or_else(|_| "*".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Parse auth bypass paths from environment variable.
    ///
    /// Default: "/health,/ready" (standard Kubernetes health endpoints)
    /// Security: Only paths that don't expose sensitive data should be added.
    fn parse_auth_bypass_paths() -> Vec<String> {
        env::var("AUTH_BYPASS_PATHS")
            .unwrap_or_else(|_| "/health,/ready".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s.starts_with('/'))
            .collect()
    }

    /// Parse trusted proxy CIDR ranges from environment variable.
    ///
    /// Format: Comma-separated CIDR notation (e.g., "10.0.0.0/8,172.16.0.0/12")
    /// Default: Empty (trust all sources - NOT recommended for production)
    ///
    /// When empty, all X-Forwarded-For headers are trusted, which allows IP spoofing.
    /// In production, configure this to your reverse proxy's IP ranges.
    fn parse_trusted_proxies() -> Vec<String> {
        env::var("TRUSTED_PROXIES")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The environment variable for one default-topic option:
/// `segment_size` reads `IGGY_TOPIC_SEGMENT_SIZE`.
fn topic_option_env(key: &str) -> String {
    format!("IGGY_TOPIC_{}", key.to_ascii_uppercase())
}

/// Read and validate the default topic's options through `lookup`, which
/// returns an environment variable's value if set. An empty value counts as
/// unset.
fn default_topic_options_from(
    lookup: impl Fn(&str) -> Option<String>,
    partitions: u32,
) -> AppResult<TopicOptions> {
    use iggy::prelude::topic_option_keys as keys;

    let text = |key: &str| {
        lookup(&topic_option_env(key))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let messages_required_to_save = text(keys::MESSAGES_REQUIRED_TO_SAVE)
        .map(|value| {
            value.parse::<u32>().map_err(|e| {
                AppError::ConfigError(format!(
                    "Invalid {}: {e}",
                    topic_option_env(keys::MESSAGES_REQUIRED_TO_SAVE)
                ))
            })
        })
        .transpose()?;
    let replication_factor = text(keys::REPLICATION_FACTOR)
        .map(|value| {
            value.parse::<u32>().map_err(|e| {
                AppError::ConfigError(format!(
                    "Invalid {}: {e}",
                    topic_option_env(keys::REPLICATION_FACTOR)
                ))
            })
        })
        .transpose()?;
    let preallocate_segments = text(keys::PREALLOCATE_SEGMENTS)
        .map(|value| parse_flag_value(&topic_option_env(keys::PREALLOCATE_SEGMENTS), Some(&value)))
        .transpose()?;

    let input = TopicOptionsInput {
        durability: text(keys::DURABILITY),
        consumer_offset_durability: text(keys::CONSUMER_OFFSET_DURABILITY),
        message_expiry: text(keys::MESSAGE_EXPIRY),
        max_topic_size: text(keys::MAX_TOPIC_SIZE),
        segment_size: text(keys::SEGMENT_SIZE),
        messages_required_to_save,
        size_of_messages_required_to_save: text(keys::SIZE_OF_MESSAGES_REQUIRED_TO_SAVE),
        preallocate_segments,
        replication_factor,
    };
    input
        .validate(partitions, topic_option_env)
        .map_err(|error| AppError::ConfigError(format!("Invalid {error}")))
}

/// The pure half of [`Config::parse_flag`], split out so tests need not set
/// process environment variables.
fn parse_flag_value(name: &str, raw: Option<&str>) -> AppResult<bool> {
    let value = raw.map(str::trim).unwrap_or_default();
    if value.is_empty() || value.eq_ignore_ascii_case("false") {
        Ok(false)
    } else if value.eq_ignore_ascii_case("true") {
        Ok(true)
    } else {
        Err(AppError::ConfigError(format!(
            "Invalid {name}: expected `true` or `false`, got `{value}`"
        )))
    }
}

/// Default configuration for testing and development.
///
/// Production deployments should use `Config::from_env()` instead.
impl Default for Config {
    fn default() -> Self {
        Self {
            // Server
            host: "0.0.0.0".to_string(),
            port: 8000,
            // Iggy connection
            iggy_connection_string: "iggy://iggy:iggy@localhost:8090".to_string(),
            default_stream: "sample-stream".to_string(),
            default_topic: "events".to_string(),
            topic_partitions: 3,
            default_topic_options: TopicOptions::default(),
            // Connection resilience
            max_reconnect_attempts: 0, // infinite
            reconnect_base_delay: Duration::from_secs(1),
            reconnect_max_delay: Duration::from_secs(30),
            health_check_interval: Duration::from_secs(30),
            operation_timeout: Duration::from_secs(30),
            // Circuit breaker
            circuit_breaker_failure_threshold: 5,
            circuit_breaker_success_threshold: 2,
            circuit_breaker_open_duration: Duration::from_secs(30),
            // Rate limiting
            rate_limit_rps: 100,
            rate_limit_burst: 50,
            // Message limits
            batch_max_size: 1000,
            poll_max_count: 100,
            max_request_body_size: 10 * 1024 * 1024, // 10MB
            // Security
            api_key: None,
            auth_bypass_paths: vec!["/health".to_string(), "/ready".to_string()],
            cors_allowed_origins: vec!["*".to_string()],
            trusted_proxies: vec![], // Empty = trust all (dev mode)
            allow_unverified_client_ips: false,
            // Observability
            log_level: "info".to_string(),
            stats_cache_ttl: Duration::from_secs(5),
            metrics_port: 9090,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_values() {
        let config = Config::default();

        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8000);
        assert_eq!(config.rate_limit_rps, 100);
        assert_eq!(config.batch_max_size, 1000);
        assert_eq!(config.max_request_body_size, 10 * 1024 * 1024);
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_server_addr_format() {
        let config = Config {
            host: "localhost".to_string(),
            port: 3000,
            ..Config::default()
        };

        assert_eq!(config.server_addr(), "localhost:3000");
    }

    #[test]
    fn test_server_addr_format_with_ip() {
        let config = Config {
            host: "192.168.1.1".to_string(),
            port: 8080,
            ..Config::default()
        };

        assert_eq!(config.server_addr(), "192.168.1.1:8080");
    }

    #[test]
    fn test_rate_limiting_enabled() {
        let config = Config::default();
        assert!(config.rate_limiting_enabled());

        let config = Config {
            rate_limit_rps: 0,
            ..Config::default()
        };
        assert!(!config.rate_limiting_enabled());
    }

    #[test]
    fn test_auth_enabled() {
        let config = Config::default();
        assert!(!config.auth_enabled());

        let config = Config {
            api_key: Some("secret-key".to_string()),
            ..Config::default()
        };
        assert!(config.auth_enabled());
    }

    #[test]
    fn test_validate_delay_ordering() {
        let config = Config {
            reconnect_base_delay: Duration::from_secs(60),
            reconnect_max_delay: Duration::from_secs(30),
            ..Config::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("RECONNECT_BASE_DELAY_MS")
        );
    }

    #[test]
    fn test_validate_batch_max_size_zero() {
        let config = Config {
            batch_max_size: 0,
            ..Config::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("BATCH_MAX_SIZE"));
    }

    #[test]
    fn test_validate_circuit_breaker_open_duration_zero() {
        // A zero window disables the breaker and turns every request during an
        // outage into an info + a warn.
        let config = Config {
            circuit_breaker_open_duration: Duration::ZERO,
            ..Config::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("CIRCUIT_BREAKER_OPEN_DURATION_SECS")
        );
    }

    #[test]
    fn test_validate_operation_timeout_zero() {
        // A zero deadline opens the breaker on a healthy service: every
        // request expires on first poll, at the global deadline, which counts
        // as outage evidence.
        let config = Config {
            operation_timeout: Duration::ZERO,
            ..Config::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("OPERATION_TIMEOUT_SECS")
        );
    }

    #[test]
    fn test_validate_poll_max_count_zero() {
        let config = Config {
            poll_max_count: 0,
            ..Config::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("POLL_MAX_COUNT"));
    }

    #[test]
    fn test_validate_valid_config() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_rejects_api_key_without_trusted_proxies() {
        let config = Config {
            api_key: Some("secret-key".to_string()),
            ..Config::default()
        };

        let message = config.validate().unwrap_err().to_string();
        // The error must name both variables that fix it.
        assert!(message.contains("TRUSTED_PROXIES"), "{message}");
        assert!(message.contains("ALLOW_UNVERIFIED_CLIENT_IPS"), "{message}");
    }

    #[test]
    fn test_validate_accepts_api_key_with_trusted_proxies() {
        let config = Config {
            api_key: Some("secret-key".to_string()),
            trusted_proxies: vec!["10.0.0.0/8".to_string()],
            ..Config::default()
        };

        assert!(config.validate().is_ok());
    }

    fn topic_env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        move |name| vars.get(name).cloned()
    }

    #[test]
    fn test_default_topic_options_unset_leave_everything_to_the_server() {
        let options = default_topic_options_from(topic_env(&[]), 3).unwrap();
        assert_eq!(options, TopicOptions::default());

        let empty = topic_env(&[
            ("IGGY_TOPIC_DURABILITY", " "),
            ("IGGY_TOPIC_PREALLOCATE_SEGMENTS", ""),
        ]);
        assert_eq!(
            default_topic_options_from(empty, 3).unwrap(),
            TopicOptions::default()
        );
    }

    #[test]
    fn test_default_topic_options_read_each_variable() {
        let env = topic_env(&[
            ("IGGY_TOPIC_DURABILITY", "persisted"),
            ("IGGY_TOPIC_CONSUMER_OFFSET_DURABILITY", "persisted"),
            ("IGGY_TOPIC_MESSAGE_EXPIRY", "7days"),
            ("IGGY_TOPIC_MAX_TOPIC_SIZE", "unlimited"),
            ("IGGY_TOPIC_SEGMENT_SIZE", "256 MiB"),
            ("IGGY_TOPIC_MESSAGES_REQUIRED_TO_SAVE", "1"),
            ("IGGY_TOPIC_SIZE_OF_MESSAGES_REQUIRED_TO_SAVE", "4 KiB"),
            ("IGGY_TOPIC_PREALLOCATE_SEGMENTS", "TRUE"),
            ("IGGY_TOPIC_REPLICATION_FACTOR", "2"),
        ]);
        let options = default_topic_options_from(env, 3).unwrap();

        assert_eq!(
            options.durability,
            Some(iggy::prelude::Durability::Persisted)
        );
        assert_eq!(
            options.consumer_offset_durability,
            Some(iggy::prelude::Durability::Persisted)
        );
        assert!(options.message_expiry.is_some());
        assert_eq!(
            options.max_topic_size,
            Some(iggy::prelude::MaxTopicSize::Unlimited)
        );
        assert_eq!(
            options.segment_size.map(|size| size.as_bytes_u64()),
            Some(256 << 20)
        );
        assert_eq!(options.messages_required_to_save, Some(1));
        assert_eq!(
            options
                .size_of_messages_required_to_save
                .map(|size| size.as_bytes_u64()),
            Some(4096)
        );
        assert_eq!(options.preallocate_segments, Some(true));
        assert_eq!(options.replication_factor, Some(2));
    }

    #[test]
    fn test_default_topic_options_name_the_variable_on_error() {
        let cases = [
            ("IGGY_TOPIC_DURABILITY", "fsync"),
            ("IGGY_TOPIC_SEGMENT_SIZE", "100 KiB"),
            ("IGGY_TOPIC_MESSAGES_REQUIRED_TO_SAVE", "many"),
            ("IGGY_TOPIC_PREALLOCATE_SEGMENTS", "yes"),
            ("IGGY_TOPIC_REPLICATION_FACTOR", "0"),
            ("IGGY_TOPIC_REPLICATION_FACTOR", "all"),
        ];
        for (name, value) in cases {
            let error = default_topic_options_from(topic_env(&[(name, value)]), 3)
                .expect_err("invalid value must fail startup");
            assert!(
                matches!(&error, AppError::ConfigError(message) if message.contains(name)),
                "{name}={value}: got {error:?}"
            );
        }
    }

    #[test]
    fn test_default_topic_preallocation_is_checked_against_the_partition_count() {
        let env = topic_env(&[("IGGY_TOPIC_PREALLOCATE_SEGMENTS", "true")]);
        let error = default_topic_options_from(env, 100).expect_err("100 GiB reserved");
        assert!(
            matches!(error, AppError::ConfigError(message) if message.contains("IGGY_TOPIC_PREALLOCATE_SEGMENTS"))
        );
    }

    #[test]
    fn test_parse_flag_value_treats_unset_and_empty_as_false() {
        assert!(!parse_flag_value("FLAG", None).unwrap());
        assert!(!parse_flag_value("FLAG", Some("")).unwrap());
        assert!(!parse_flag_value("FLAG", Some("  ")).unwrap());
    }

    #[test]
    fn test_parse_flag_value_accepts_true_and_false_in_any_case() {
        for raw in ["true", "TRUE", "True", " true "] {
            assert!(parse_flag_value("FLAG", Some(raw)).unwrap(), "{raw:?}");
        }
        for raw in ["false", "FALSE", "False"] {
            assert!(!parse_flag_value("FLAG", Some(raw)).unwrap(), "{raw:?}");
        }
    }

    #[test]
    fn test_parse_flag_value_rejects_anything_else() {
        // A security-relaxing flag must not guess: "1" or "yes" fail startup.
        for raw in ["1", "yes", "on", "tru"] {
            let message = parse_flag_value("FLAG", Some(raw)).unwrap_err().to_string();
            assert!(message.contains("FLAG"), "{message}");
        }
    }

    #[test]
    fn test_validate_accepts_api_key_without_trusted_proxies_when_explicitly_allowed() {
        let config = Config {
            api_key: Some("secret-key".to_string()),
            allow_unverified_client_ips: true,
            ..Config::default()
        };

        assert!(config.validate().is_ok());
    }
}
