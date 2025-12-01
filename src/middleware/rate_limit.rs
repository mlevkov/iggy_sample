//! Rate limiting middleware using the token bucket algorithm.
//!
//! # Algorithm
//!
//! Uses the Governor crate which implements a Generic Cell Rate Algorithm (GCRA),
//! also known as a "leaky bucket as a meter". This provides:
//!
//! - Smooth rate limiting (no sudden bursts followed by long waits)
//! - Per-IP rate limiting to prevent single-client abuse
//! - Memory efficient with automatic cleanup of old entries
//! - Thread-safe
//!
//! # Configuration
//!
//! - `rate_limit_rps`: Sustained requests per second per IP
//! - `rate_limit_burst`: Additional burst capacity above RPS
//! - `trusted_proxies`: CIDR ranges of trusted reverse proxies
//!
//! # Response Headers
//!
//! On rate limit exceeded (429):
//! - `Retry-After`: Seconds until the next request will be accepted
//! - `X-RateLimit-Limit`: Configured RPS limit
//! - `X-RateLimit-Remaining`: Remaining requests in current window
//!
//! # IP Spoofing Mitigation
//!
//! Per-IP rate limiting relies on headers like `X-Forwarded-For` when behind
//! a reverse proxy. To prevent IP spoofing attacks:
//!
//! 1. Configure `TRUSTED_PROXIES` with your reverse proxy's IP/CIDR ranges
//! 2. Ensure your proxy overwrites (not appends) client IP headers
//! 3. Block direct access to this service from the internet
//!
//! When `trusted_proxies` is configured, the middleware logs warnings if
//! X-Forwarded-For is received from an untrusted source.

use std::fmt;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use tower::{Layer, Service};

/// Error type for rate limit layer configuration.
///
/// This is a simple enum with no data, so it derives `Copy` for efficient
/// pass-by-value semantics without cloning overhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitError {
    /// RPS value cannot be zero.
    ZeroRps,
}

impl fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RateLimitError::ZeroRps => {
                write!(
                    f,
                    "RPS must be greater than 0; use disabled() for no limiting"
                )
            }
        }
    }
}

impl std::error::Error for RateLimitError {}

use tracing::{debug, warn};

use super::ip::extract_client_ip_with_validation;

/// Type alias for per-IP rate limiter.
///
/// Uses `String` keys (IP addresses) with the default DashMap-based state store.
type KeyedLimiter = RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>;

// =============================================================================
// Trusted Proxy CIDR Matching
// =============================================================================

/// Parsed CIDR network range for trusted proxy validation.
#[derive(Debug, Clone)]
pub struct CidrRange {
    /// Network address
    network: IpAddr,
    /// Prefix length (e.g., 24 for /24)
    prefix_len: u8,
}

impl CidrRange {
    /// Parse a CIDR notation string (e.g., "10.0.0.0/8" or "::1/128").
    ///
    /// Returns `None` if the format is invalid.
    pub fn parse(cidr: &str) -> Option<Self> {
        let parts: Vec<&str> = cidr.trim().split('/').collect();

        if parts.len() != 2 {
            // Try parsing as a single IP (implicit /32 or /128)
            if let Ok(ip) = parts.first()?.parse::<IpAddr>() {
                let prefix_len = match ip {
                    IpAddr::V4(_) => 32,
                    IpAddr::V6(_) => 128,
                };
                return Some(Self {
                    network: ip,
                    prefix_len,
                });
            }
            return None;
        }

        let ip: IpAddr = parts.first()?.parse().ok()?;
        let prefix_len: u8 = parts.get(1)?.parse().ok()?;

        // Validate prefix length
        let max_prefix = match ip {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };

        if prefix_len > max_prefix {
            return None;
        }

        Some(Self {
            network: ip,
            prefix_len,
        })
    }

    /// Check if an IP address is contained within this CIDR range.
    pub fn contains(&self, ip: &IpAddr) -> bool {
        match (&self.network, ip) {
            (IpAddr::V4(net), IpAddr::V4(addr)) => {
                let net_bits = u32::from(*net);
                let addr_bits = u32::from(*addr);
                let mask = if self.prefix_len == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix_len)
                };
                (net_bits & mask) == (addr_bits & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(addr)) => {
                let net_bits = u128::from(*net);
                let addr_bits = u128::from(*addr);
                let mask = if self.prefix_len == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix_len)
                };
                (net_bits & mask) == (addr_bits & mask)
            }
            // IPv4 and IPv6 don't match
            _ => false,
        }
    }
}

/// Configuration for trusted proxy validation.
///
/// When configured, X-Forwarded-For headers are only trusted from
/// requests originating within the specified CIDR ranges.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxyConfig {
    /// Parsed CIDR ranges for trusted proxies
    ranges: Vec<CidrRange>,
}

impl TrustedProxyConfig {
    /// Create a new trusted proxy configuration from CIDR strings.
    ///
    /// Invalid CIDR strings are logged as warnings and skipped.
    pub fn new(cidrs: &[String]) -> Self {
        let ranges: Vec<CidrRange> = cidrs
            .iter()
            .filter_map(|cidr| {
                let parsed = CidrRange::parse(cidr);
                if parsed.is_none() {
                    warn!(cidr = %cidr, "Invalid CIDR range in TRUSTED_PROXIES, skipping");
                }
                parsed
            })
            .collect();

        if !cidrs.is_empty() && !ranges.is_empty() {
            debug!(
                count = ranges.len(),
                "Trusted proxy validation enabled with {} CIDR ranges",
                ranges.len()
            );
        }

        Self { ranges }
    }

    /// Check if trusted proxy validation is enabled (any ranges configured).
    pub fn is_enabled(&self) -> bool {
        !self.ranges.is_empty()
    }

    /// Check if an IP address is from a trusted proxy.
    ///
    /// Returns `true` if the IP matches any configured CIDR range,
    /// or if no ranges are configured (trust all mode).
    pub fn is_trusted(&self, ip_str: &str) -> bool {
        // If no trusted proxies configured, trust all (backwards compatible)
        if self.ranges.is_empty() {
            return true;
        }

        // Parse the IP address
        let ip: IpAddr = match ip_str.parse() {
            Ok(ip) => ip,
            Err(_) => {
                // Can't parse as IP, not trusted
                return false;
            }
        };

        // Check against all configured ranges
        self.ranges.iter().any(|range| range.contains(&ip))
    }
}

/// Rate limiting layer for Tower middleware stack.
///
/// Uses per-IP rate limiting to prevent abuse from individual clients
/// while allowing legitimate traffic from others to continue.
///
/// # Example
///
/// ```rust,ignore
/// let layer = RateLimitLayer::new(100, 50); // 100 RPS per IP, 50 burst
/// let app = Router::new()
///     .route("/api", get(handler))
///     .layer(layer);
/// ```
#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: Arc<KeyedLimiter>,
    /// Configured RPS limit (for headers)
    limit: u32,
    /// Trusted proxy configuration for IP spoofing mitigation
    trusted_proxies: Arc<TrustedProxyConfig>,
}

impl RateLimitLayer {
    /// Create a new per-IP rate limit layer.
    ///
    /// # Arguments
    ///
    /// * `rps` - Requests per second limit per IP (sustained rate)
    /// * `burst` - Additional burst capacity per IP
    ///
    /// # Errors
    ///
    /// Returns `RateLimitError::ZeroRps` if `rps` is 0.
    /// Use `RateLimitLayer::disabled()` for no limiting.
    pub fn new(rps: u32, burst: u32) -> Result<Self, RateLimitError> {
        Self::with_trusted_proxies(rps, burst, &[])
    }

    /// Create a new per-IP rate limit layer with trusted proxy configuration.
    ///
    /// # Arguments
    ///
    /// * `rps` - Requests per second limit per IP (sustained rate)
    /// * `burst` - Additional burst capacity per IP
    /// * `trusted_proxies` - CIDR ranges for trusted reverse proxies
    ///
    /// # Errors
    ///
    /// Returns `RateLimitError::ZeroRps` if `rps` is 0.
    /// Use `RateLimitLayer::disabled()` for no limiting.
    pub fn with_trusted_proxies(
        rps: u32,
        burst: u32,
        trusted_proxies: &[String],
    ) -> Result<Self, RateLimitError> {
        // Validate rps is non-zero
        let rps_nonzero = NonZeroU32::new(rps).ok_or(RateLimitError::ZeroRps)?;

        // burst.max(1) is always >= 1, so this is infallible
        // We use a const NonZeroU32 for the minimum burst value
        const MIN_BURST: NonZeroU32 = NonZeroU32::new(1).unwrap();
        let burst_nonzero = NonZeroU32::new(burst).unwrap_or(MIN_BURST);

        // Create quota: burst capacity refilled at `rps` per second
        let quota = Quota::per_second(rps_nonzero).allow_burst(burst_nonzero);

        // Create keyed rate limiter with custom hasher for efficiency
        let limiter = RateLimiter::keyed(quota);

        // Parse trusted proxy configuration
        let proxy_config = TrustedProxyConfig::new(trusted_proxies);

        Ok(Self {
            limiter: Arc::new(limiter),
            limit: rps,
            trusted_proxies: Arc::new(proxy_config),
        })
    }

    /// Create a disabled rate limiter (allows all requests).
    ///
    /// Use this when rate limiting is configured to be disabled.
    pub fn disabled() -> Option<Self> {
        None
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: self.limiter.clone(),
            limit: self.limit,
            trusted_proxies: self.trusted_proxies.clone(),
        }
    }
}

/// Rate limiting service wrapper.
#[derive(Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: Arc<KeyedLimiter>,
    limit: u32,
    trusted_proxies: Arc<TrustedProxyConfig>,
}

impl<S> Service<Request<Body>> for RateLimitService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let limiter = self.limiter.clone();
        let limit = self.limit;
        let trusted_proxies = self.trusted_proxies.clone();
        let mut inner = self.inner.clone();

        // Extract client IP before moving req
        // Convert Cow to String for the limiter key (required by governor's keyed rate limiter)
        let client_ip_cow = extract_client_ip_with_validation(&req, &trusted_proxies);
        let client_ip = client_ip_cow.into_owned();

        Box::pin(async move {
            // Check rate limit for this specific client IP
            match limiter.check_key(&client_ip) {
                Ok(_) => {
                    // Request allowed - forward to inner service
                    inner.call(req).await
                }
                Err(not_until) => {
                    // Rate limit exceeded for this IP
                    // Only extract path for logging (lazy evaluation)
                    let path = req.uri().path();
                    let wait_time =
                        not_until.wait_time_from(governor::clock::DefaultClock::default().now());
                    let retry_after = wait_time.as_secs().max(1);

                    warn!(
                        client_ip = %client_ip,
                        path = %path,
                        retry_after_secs = retry_after,
                        "Rate limit exceeded for IP"
                    );

                    // Build 429 response with rate limit headers
                    let response = (
                        StatusCode::TOO_MANY_REQUESTS,
                        [
                            ("Retry-After", retry_after.to_string()),
                            ("X-RateLimit-Limit", limit.to_string()),
                            ("X-RateLimit-Remaining", "0".to_string()),
                        ],
                        "Rate limit exceeded. Please retry later.",
                    )
                        .into_response();

                    Ok(response)
                }
            }
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_layer_creation() {
        let layer = RateLimitLayer::new(100, 50).unwrap();
        assert_eq!(layer.limit, 100);
    }

    #[test]
    fn test_rate_limit_zero_rps_returns_error() {
        let result = RateLimitLayer::new(0, 50);
        assert!(matches!(result, Err(RateLimitError::ZeroRps)));
    }

    // ==========================================================================
    // CIDR Range Tests
    // ==========================================================================
    // Note: IP extraction tests are in middleware/ip.rs

    #[test]
    fn test_cidr_parse_ipv4() {
        let cidr = CidrRange::parse("10.0.0.0/8").unwrap();
        assert_eq!(cidr.prefix_len, 8);
    }

    #[test]
    fn test_cidr_parse_ipv6() {
        let cidr = CidrRange::parse("::1/128").unwrap();
        assert_eq!(cidr.prefix_len, 128);
    }

    #[test]
    fn test_cidr_parse_single_ip() {
        let cidr = CidrRange::parse("192.168.1.1").unwrap();
        assert_eq!(cidr.prefix_len, 32);
    }

    #[test]
    fn test_cidr_parse_invalid() {
        assert!(CidrRange::parse("not-an-ip").is_none());
        assert!(CidrRange::parse("10.0.0.0/33").is_none()); // Invalid prefix
    }

    #[test]
    fn test_cidr_contains_ipv4() {
        let cidr = CidrRange::parse("10.0.0.0/8").unwrap();

        assert!(cidr.contains(&"10.0.0.1".parse().unwrap()));
        assert!(cidr.contains(&"10.255.255.255".parse().unwrap()));
        assert!(!cidr.contains(&"11.0.0.1".parse().unwrap()));
        assert!(!cidr.contains(&"192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn test_cidr_contains_ipv4_slash24() {
        let cidr = CidrRange::parse("192.168.1.0/24").unwrap();

        assert!(cidr.contains(&"192.168.1.1".parse().unwrap()));
        assert!(cidr.contains(&"192.168.1.254".parse().unwrap()));
        assert!(!cidr.contains(&"192.168.2.1".parse().unwrap()));
    }

    #[test]
    fn test_trusted_proxy_config_empty() {
        let config = TrustedProxyConfig::new(&[]);
        assert!(!config.is_enabled());
        // Empty config trusts all
        assert!(config.is_trusted("1.2.3.4"));
        assert!(config.is_trusted("invalid"));
    }

    #[test]
    fn test_trusted_proxy_config_with_ranges() {
        let config =
            TrustedProxyConfig::new(&["10.0.0.0/8".to_string(), "172.16.0.0/12".to_string()]);
        assert!(config.is_enabled());

        // Should trust IPs in configured ranges
        assert!(config.is_trusted("10.0.0.1"));
        assert!(config.is_trusted("172.16.0.1"));
        assert!(config.is_trusted("172.31.255.255"));

        // Should not trust IPs outside ranges
        assert!(!config.is_trusted("192.168.1.1"));
        assert!(!config.is_trusted("8.8.8.8"));
        assert!(!config.is_trusted("invalid"));
    }
}
