//! API key authentication middleware.
//!
//! # Security Features
//!
//! - **Constant-time comparison**: Prevents timing attacks on API key validation
//! - **Multiple input methods**: Header (`X-API-Key`) or query parameter (`api_key`)
//! - **Selective protection**: Health endpoints bypassed for monitoring
//!
//! # Usage
//!
//! Set the `API_KEY` environment variable to enable authentication:
//!
//! ```bash
//! API_KEY=your-secret-key cargo run
//! ```
//!
//! Clients must then provide the key via:
//!
//! ```bash
//! # Header method (preferred)
//! curl -H "X-API-Key: your-secret-key" http://localhost:3000/messages
//!
//! # Query parameter method
//! curl "http://localhost:3000/messages?api_key=your-secret-key"
//! ```
//!
//! # Bypassed Endpoints
//!
//! The following paths are accessible without authentication:
//! - `/health` - Health check
//! - `/ready` - Readiness probe
//!
//! This allows Kubernetes/load balancer health checks to function.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use subtle::ConstantTimeEq;
use tower::{Layer, Service};
use tracing::{debug, error, warn};

use super::ip::extract_client_ip;

/// Header name for API key.
pub const API_KEY_HEADER: &str = "x-api-key";

/// Query parameter name for API key.
pub const API_KEY_QUERY: &str = "api_key";

/// Default paths that bypass authentication.
///
/// These endpoints are accessible without an API key to support:
/// - Kubernetes health/readiness probes
/// - Load balancer health checks
/// - Monitoring systems
///
/// # Path Matching Behavior
///
/// Bypass paths use **exact string matching** against `request.uri().path()`.
/// This means:
/// - `/health` is bypassed, but `/health/` (trailing slash) is NOT
/// - `/ready` is bypassed, but `/ready?foo=bar` IS bypassed (query params are stripped)
/// - `/HEALTH` (uppercase) is NOT bypassed (case-sensitive)
///
/// This strictness is intentional for security: it prevents accidental bypasses
/// via path manipulation. Configure bypass paths exactly as your health checks
/// will request them.
///
/// Note: The `/stats` endpoint is NOT bypassed and requires authentication,
/// even though it might seem like a health-related endpoint. This is intentional
/// as stats can reveal information about system usage.
const DEFAULT_BYPASS_PATHS: [&str; 2] = ["/health", "/ready"];

/// Default maximum auth failures per IP per minute before blocking.
/// After this many failures, further requests from the IP are blocked temporarily.
const DEFAULT_AUTH_FAILURE_LIMIT: NonZeroU32 = NonZeroU32::new(10).unwrap();

/// Default burst capacity for auth failure rate limiting.
const DEFAULT_AUTH_FAILURE_BURST: NonZeroU32 = NonZeroU32::new(5).unwrap();

/// Type alias for auth failure rate limiter (per-IP).
type AuthFailureLimiter = RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>;

/// API key authentication layer.
///
/// When the expected key is `None`, all requests are allowed (auth disabled).
/// Bypass paths can be configured via the `AUTH_BYPASS_PATHS` environment variable.
///
/// # Brute Force Protection
///
/// Includes per-IP rate limiting for authentication failures. After too many
/// failed attempts, further requests from that IP are temporarily blocked
/// even before validating the API key.
#[derive(Clone)]
pub struct ApiKeyAuth {
    /// Expected API key (None = auth disabled)
    expected_key: Option<Arc<String>>,
    /// Paths that bypass authentication
    bypass_paths: Arc<Vec<String>>,
    /// Rate limiter for tracking auth failures per IP
    failure_limiter: Option<Arc<AuthFailureLimiter>>,
}

impl ApiKeyAuth {
    /// Create a new API key auth layer.
    ///
    /// # Arguments
    ///
    /// * `api_key` - Expected API key, or `None` to disable authentication
    /// * `bypass_paths` - Paths that bypass authentication (e.g., health endpoints)
    pub fn new(api_key: Option<String>, bypass_paths: Vec<String>) -> Self {
        let failure_limiter = if api_key.is_some() {
            // Only create rate limiter when auth is enabled
            let quota = Quota::per_minute(DEFAULT_AUTH_FAILURE_LIMIT)
                .allow_burst(DEFAULT_AUTH_FAILURE_BURST);
            Some(Arc::new(RateLimiter::keyed(quota)))
        } else {
            None
        };

        Self {
            expected_key: api_key.map(Arc::new),
            bypass_paths: Arc::new(bypass_paths),
            failure_limiter,
        }
    }

    /// Create with default bypass paths ("/health", "/ready").
    pub fn with_defaults(api_key: Option<String>) -> Self {
        Self::new(
            api_key,
            DEFAULT_BYPASS_PATHS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        )
    }

    /// Check if authentication is enabled.
    pub fn is_enabled(&self) -> bool {
        self.expected_key.is_some()
    }
}

impl<S> Layer<S> for ApiKeyAuth {
    type Service = ApiKeyAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ApiKeyAuthService {
            inner,
            expected_key: self.expected_key.clone(),
            bypass_paths: self.bypass_paths.clone(),
            failure_limiter: self.failure_limiter.clone(),
        }
    }
}

/// API key authentication service wrapper.
#[derive(Clone)]
pub struct ApiKeyAuthService<S> {
    inner: S,
    expected_key: Option<Arc<String>>,
    bypass_paths: Arc<Vec<String>>,
    failure_limiter: Option<Arc<AuthFailureLimiter>>,
}

impl<S> Service<Request<Body>> for ApiKeyAuthService<S>
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
        let expected_key = self.expected_key.clone();
        let bypass_paths = self.bypass_paths.clone();
        let failure_limiter = self.failure_limiter.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // If no API key is configured, allow all requests
            let expected = match expected_key {
                Some(key) => key,
                None => return inner.call(req).await,
            };

            // Check if path should bypass authentication
            let path = req.uri().path();
            if bypass_paths.iter().any(|p| p == path) {
                debug!(path, "Bypassing auth for health endpoint");
                return inner.call(req).await;
            }

            // Extract client IP for failure tracking (convert to owned String for rate limiter)
            let client_ip = extract_client_ip(&req).into_owned();

            // Check if this IP is blocked due to too many failures
            if let Some(ref limiter) = failure_limiter {
                // We use check_key in reverse: if it fails, the IP has too many failures
                // The rate limiter tracks "tokens consumed", so we check available capacity
                if let Err(not_until) = limiter.check_key(&client_ip) {
                    let wait_time =
                        not_until.wait_time_from(governor::clock::DefaultClock::default().now());
                    let retry_after = wait_time.as_secs().max(1);

                    error!(
                        client_ip = %client_ip,
                        retry_after_secs = retry_after,
                        "IP blocked due to excessive auth failures"
                    );

                    return Ok(rate_limited_response(retry_after));
                }
            }

            // Extract API key from request
            let provided_key = extract_api_key(&req);

            match provided_key {
                Some(extracted) if constant_time_eq(&extracted.key, &expected) => {
                    // Valid API key - proceed
                    debug!(
                        from_query = extracted.from_query,
                        "API key authentication successful"
                    );
                    inner.call(req).await
                }
                Some(_) => {
                    // Invalid API key - record failure
                    if let Some(ref limiter) = failure_limiter {
                        // Consume a token for this failure
                        let _ = limiter.check_key(&client_ip);
                    }
                    warn!(
                        path = %req.uri().path(),
                        client_ip = %client_ip,
                        "Invalid API key provided"
                    );
                    Ok(unauthorized_response("Invalid API key"))
                }
                None => {
                    // No API key provided - record failure
                    if let Some(ref limiter) = failure_limiter {
                        // Consume a token for this failure
                        let _ = limiter.check_key(&client_ip);
                    }
                    warn!(
                        path = %req.uri().path(),
                        client_ip = %client_ip,
                        "Missing API key"
                    );
                    Ok(unauthorized_response("API key required"))
                }
            }
        })
    }
}

/// Result of extracting an API key with metadata about the source.
struct ExtractedApiKey {
    key: String,
    from_query: bool,
}

/// Extract API key from request (header or query parameter).
///
/// Checks in order:
/// 1. `X-API-Key` header (preferred, secure)
/// 2. `api_key` query parameter (deprecated, logs warning)
///
/// # Security Warning
///
/// Query parameter authentication is deprecated because:
/// - API keys appear in server logs
/// - API keys appear in browser history
/// - API keys may be cached by proxies
///
/// Use the `X-API-Key` header instead.
fn extract_api_key<B>(req: &Request<B>) -> Option<ExtractedApiKey> {
    // Check header first (preferred method)
    if let Some(header_value) = req.headers().get(API_KEY_HEADER)
        && let Ok(value) = header_value.to_str()
    {
        return Some(ExtractedApiKey {
            key: value.to_string(),
            from_query: false,
        });
    }

    // Check query parameter (deprecated - log warning)
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=')
                && key == API_KEY_QUERY
            {
                // Log deprecation warning - API keys in URLs are a security risk
                warn!(
                    path = %req.uri().path(),
                    "DEPRECATED: API key provided via query parameter. \
                     Use X-API-Key header instead. Query parameters expose \
                     credentials in logs and browser history."
                );
                return Some(ExtractedApiKey {
                    key: value.to_string(),
                    from_query: true,
                });
            }
        }
    }

    None
}

/// Perform constant-time comparison of two strings.
///
/// This prevents timing attacks where an attacker could determine
/// the correct API key by measuring response times.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();

    // Use subtle crate for constant-time comparison
    // This returns 1 if equal, 0 if not
    a_bytes.ct_eq(b_bytes).into()
}

/// Build an unauthorized (401) response.
fn unauthorized_response(message: &str) -> Response<Body> {
    (
        StatusCode::UNAUTHORIZED,
        [
            ("WWW-Authenticate", "API-Key"),
            ("Content-Type", "application/json"),
        ],
        format!(r#"{{"error":"unauthorized","message":"{}"}}"#, message),
    )
        .into_response()
}

/// Build a rate limited (429) response for auth failures.
fn rate_limited_response(retry_after: u64) -> Response<Body> {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            ("Retry-After", retry_after.to_string()),
            ("Content-Type", "application/json".to_string()),
        ],
        r#"{"error":"too_many_requests","message":"Too many failed authentication attempts. Please wait before retrying."}"#.to_string(),
    )
        .into_response()
}

// Note: extract_client_ip is imported from super::ip module

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_auth_enabled() {
        let auth = ApiKeyAuth::with_defaults(Some("secret".to_string()));
        assert!(auth.is_enabled());
    }

    #[test]
    fn test_api_key_auth_disabled() {
        let auth = ApiKeyAuth::with_defaults(None);
        assert!(!auth.is_enabled());
    }

    #[test]
    fn test_extract_api_key_from_header() {
        let req = Request::builder()
            .header("x-api-key", "my-secret-key")
            .body(Body::empty())
            .unwrap();

        let extracted = extract_api_key(&req).expect("Should extract API key");
        assert_eq!(extracted.key, "my-secret-key");
        assert!(!extracted.from_query, "Should be from header, not query");
    }

    #[test]
    fn test_extract_api_key_from_query() {
        let req = Request::builder()
            .uri("/path?api_key=query-secret&other=value")
            .body(Body::empty())
            .unwrap();

        let extracted = extract_api_key(&req).expect("Should extract API key");
        assert_eq!(extracted.key, "query-secret");
        assert!(
            extracted.from_query,
            "Should be marked as from query (deprecated)"
        );
    }

    #[test]
    fn test_extract_api_key_header_priority() {
        let req = Request::builder()
            .uri("/path?api_key=query-secret")
            .header("x-api-key", "header-secret")
            .body(Body::empty())
            .unwrap();

        // Header should take priority
        let extracted = extract_api_key(&req).expect("Should extract API key");
        assert_eq!(extracted.key, "header-secret");
        assert!(
            !extracted.from_query,
            "Header should take priority over query"
        );
    }

    #[test]
    fn test_extract_api_key_none() {
        let req = Request::builder().body(Body::empty()).unwrap();
        assert!(extract_api_key(&req).is_none());
    }

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq("secret123", "secret123"));
    }

    #[test]
    fn test_constant_time_eq_not_equal() {
        assert!(!constant_time_eq("secret123", "secret456"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq("short", "much-longer-string"));
    }
}
