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
//! - `CORS_ALLOWED_ORIGINS`: Comma-separated list of allowed origins (default: `*` for dev)
//!
//! # Performance Tuning
//!
//! - `BATCH_MAX_SIZE`: Maximum messages per batch (default: 1000)
//! - `POLL_MAX_COUNT`: Maximum messages per poll (default: 100)
//! - `RATE_LIMIT_RPS`: Requests per second limit (default: 100)
//! - `RATE_LIMIT_BURST`: Burst capacity for rate limiter (default: 50)

use std::env;
use std::time::Duration;

use crate::error::{AppError, AppResult};

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

    /// Server port (default: 3000)
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

        let config = Self {
            // Server
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: Self::parse_env("PORT", 3000)?,

            // Iggy connection
            iggy_connection_string: env::var("IGGY_CONNECTION_STRING")
                .unwrap_or_else(|_| "iggy://iggy:iggy@localhost:8090".to_string()),
            default_stream: env::var("IGGY_STREAM").unwrap_or_else(|_| "sample-stream".to_string()),
            default_topic: env::var("IGGY_TOPIC").unwrap_or_else(|_| "events".to_string()),
            topic_partitions: Self::parse_env("IGGY_PARTITIONS", 3)?,

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
        // Validate delay ordering
        if self.reconnect_base_delay > self.reconnect_max_delay {
            return Err(AppError::ConfigError(format!(
                "RECONNECT_BASE_DELAY_MS ({:?}) must be <= RECONNECT_MAX_DELAY_MS ({:?})",
                self.reconnect_base_delay, self.reconnect_max_delay
            )));
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

    /// Get the metrics endpoint address.
    ///
    /// Returns `None` if metrics are disabled (port = 0).
    pub fn metrics_addr(&self) -> Option<std::net::SocketAddr> {
        if self.metrics_enabled() {
            Some(std::net::SocketAddr::from((
                [0, 0, 0, 0],
                self.metrics_port,
            )))
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

/// Default configuration for testing and development.
///
/// Production deployments should use `Config::from_env()` instead.
impl Default for Config {
    fn default() -> Self {
        Self {
            // Server
            host: "0.0.0.0".to_string(),
            port: 3000,
            // Iggy connection
            iggy_connection_string: "iggy://iggy:iggy@localhost:8090".to_string(),
            default_stream: "sample-stream".to_string(),
            default_topic: "events".to_string(),
            topic_partitions: 3,
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
        assert_eq!(config.port, 3000);
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
}
