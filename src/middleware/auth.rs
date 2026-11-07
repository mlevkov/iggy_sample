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
//! curl -H "X-API-Key: your-secret-key" http://localhost:8000/messages
//!
//! # Query parameter method
//! curl "http://localhost:8000/messages?api_key=your-secret-key"
//! ```
//!
//! # Bypassed Endpoints
//!
//! The following paths are accessible without authentication:
//! - `/health` - Health check
//! - `/ready` - Readiness probe
//!
//! This allows Kubernetes/load balancer health checks to function.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use subtle::ConstantTimeEq;
use tower::{Layer, Service};
use tracing::{debug, error, warn};

use super::ip::{client_ip_is_verified, extract_client_ip_with_validation};
use super::rate_limit::TrustedProxyConfig;

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
/// Once an IP exhausts its failure budget, EVERY request from it is refused
/// until the budget refills, valid key or not.
const DEFAULT_AUTH_FAILURE_LIMIT: NonZeroU32 = NonZeroU32::new(10).unwrap();

/// Default burst capacity for auth failure rate limiting.
const DEFAULT_AUTH_FAILURE_BURST: NonZeroU32 = NonZeroU32::new(5).unwrap();

/// Number of tracked blocks at which expired entries are swept on insert.
///
/// A block lasts about one refill interval (60 s / `DEFAULT_AUTH_FAILURE_LIMIT`),
/// so sweeping when the map reaches this size keeps it near the number of IPs
/// blocked within one interval instead of every IP ever blocked.
const BLOCK_SWEEP_THRESHOLD: usize = 4096;

/// Type alias for auth failure rate limiter (per-IP).
type AuthFailureLimiter = RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>;

/// Per-IP brute-force guard: a failure budget plus the deadlines of active blocks.
///
/// Only failures are charged to the budget, so valid-key clients are never
/// throttled by it. What happens once an IP exhausts it depends on whether the
/// IP is verified (see [`client_ip_is_verified`]):
///
/// - **Verified** (`TRUSTED_PROXIES` set, peer address available): the IP is
///   blocked, and the service consults [`AuthFailureGuard::remaining_block`]
///   BEFORE it compares the key. Every request from a blocked IP gets the same
///   `429`, so the block tells an attacker nothing about whether a guess was
///   correct. Comparing first let a blocked IP keep guessing at the general
///   rate limit and tell a hit (`200`) from a miss (`429`).
/// - **Unverified**: further failures get `429`, but a valid key still
///   passes. The IP comes from headers any client can set, so an attacker
///   escapes a block by rotating them; a block that refused valid keys would
///   buy no protection and would let anyone lock out the shared `"unknown"`
///   bucket or a spoofed victim IP.
struct AuthFailureGuard {
    /// GCRA failure budget per IP (`DEFAULT_AUTH_FAILURE_LIMIT`/min, burst
    /// `DEFAULT_AUTH_FAILURE_BURST`).
    limiter: AuthFailureLimiter,
    /// Block deadline per IP, set when a failure finds the budget exhausted.
    blocked_until: Mutex<HashMap<String, Instant>>,
}

impl AuthFailureGuard {
    fn new() -> Self {
        let quota =
            Quota::per_minute(DEFAULT_AUTH_FAILURE_LIMIT).allow_burst(DEFAULT_AUTH_FAILURE_BURST);
        Self {
            limiter: RateLimiter::keyed(quota),
            blocked_until: Mutex::new(HashMap::new()),
        }
    }

    /// Lock the block map, recovering it if a panic poisoned the lock.
    ///
    /// Every critical section is a single `HashMap` call, so a poisoned map
    /// still holds consistent entries and recovering it is safe.
    fn lock_blocks(&self) -> MutexGuard<'_, HashMap<String, Instant>> {
        self.blocked_until.lock().unwrap_or_else(|poisoned| {
            error!("Auth block map lock poisoned; recovering inner state");
            poisoned.into_inner()
        })
    }

    /// Time left on `client_ip`'s block at `now`, or `None` when it is not
    /// blocked. An expired entry is removed.
    fn remaining_block(&self, client_ip: &str, now: Instant) -> Option<Duration> {
        let mut blocks = self.lock_blocks();
        match blocks.get(client_ip) {
            Some(&deadline) if deadline > now => Some(deadline - now),
            Some(_) => {
                blocks.remove(client_ip);
                None
            }
            None => None,
        }
    }

    /// Charge one authentication failure to `client_ip`.
    ///
    /// Returns how long until the budget admits another failure when this one
    /// found it exhausted, or `None` when the failure fit in the budget.
    fn charge_failure(&self, client_ip: &str) -> Option<Duration> {
        let not_until = self.limiter.check_key(&client_ip.to_owned()).err()?;
        Some(not_until.wait_time_from(self.limiter.clock().now()))
    }

    /// Block `client_ip` for `duration` from `now`.
    ///
    /// Expired entries are swept first once the map reaches
    /// `BLOCK_SWEEP_THRESHOLD`.
    fn block(&self, client_ip: &str, now: Instant, duration: Duration) {
        let mut blocks = self.lock_blocks();
        if blocks.len() >= BLOCK_SWEEP_THRESHOLD {
            blocks.retain(|_, deadline| *deadline > now);
        }
        blocks.insert(client_ip.to_owned(), now + duration);
    }

    #[cfg(test)]
    fn tracked_blocks(&self) -> usize {
        self.lock_blocks().len()
    }
}

/// API key authentication layer.
///
/// When the expected key is `None`, all requests are allowed (auth disabled).
/// Bypass paths can be configured via the `AUTH_BYPASS_PATHS` environment variable.
///
/// # Brute Force Protection
///
/// Authentication FAILURES are charged to a per-IP budget; valid-key requests
/// never consume from it. Once a verified IP (`TRUSTED_PROXIES` set, peer
/// address available) exhausts its budget it is blocked: every request from
/// it, valid key or not, receives `429 Too Many Requests` until the budget
/// refills. The block is checked before the key is compared, so a blocked IP
/// cannot use the response to tell a correct guess from a wrong one. A client
/// sharing that IP with an attacker is locked out with it.
///
/// Without `TRUSTED_PROXIES` the client IP comes from client-supplied headers,
/// so the protection is advisory: failures past the budget get `429`, valid
/// keys still pass, and rotating the headers escapes it. Startup warns about
/// this configuration.
#[derive(Clone)]
pub struct ApiKeyAuth {
    /// Expected API key (None = auth disabled)
    expected_key: Option<Arc<String>>,
    /// Paths that bypass authentication
    bypass_paths: Arc<Vec<String>>,
    /// Per-IP failure budget and active blocks
    failure_guard: Option<Arc<AuthFailureGuard>>,
    /// Trusted proxy configuration for spoofing-resistant IP extraction
    trusted_proxies: Arc<TrustedProxyConfig>,
}

impl ApiKeyAuth {
    /// Create a new API key auth layer.
    ///
    /// # Arguments
    ///
    /// * `api_key` - Expected API key, or `None` to disable authentication
    /// * `bypass_paths` - Paths that bypass authentication (e.g., health endpoints)
    pub fn new(api_key: Option<String>, bypass_paths: Vec<String>) -> Self {
        Self::with_trusted_proxies(
            api_key,
            bypass_paths,
            Arc::new(TrustedProxyConfig::default()),
        )
    }

    /// Create a new API key auth layer with trusted-proxy validation for the
    /// per-IP brute-force limiter.
    ///
    /// With trusted proxies configured, forwarded headers are only honored
    /// when the direct peer is inside a trusted range, and honored
    /// `X-Forwarded-For` chains are resolved rightmost-untrusted (see
    /// `middleware::ip`), so attackers cannot rotate spoofed values to
    /// escape failure tracking - whether the proxy overwrites or appends.
    pub fn with_trusted_proxies(
        api_key: Option<String>,
        bypass_paths: Vec<String>,
        trusted_proxies: Arc<TrustedProxyConfig>,
    ) -> Self {
        // Only track failures when auth is enabled
        let failure_guard = api_key.as_ref().map(|_| Arc::new(AuthFailureGuard::new()));

        Self {
            expected_key: api_key.map(Arc::new),
            bypass_paths: Arc::new(bypass_paths),
            failure_guard,
            trusted_proxies,
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
            failure_guard: self.failure_guard.clone(),
            trusted_proxies: self.trusted_proxies.clone(),
        }
    }
}

/// API key authentication service wrapper.
#[derive(Clone)]
pub struct ApiKeyAuthService<S> {
    inner: S,
    expected_key: Option<Arc<String>>,
    bypass_paths: Arc<Vec<String>>,
    failure_guard: Option<Arc<AuthFailureGuard>>,
    trusted_proxies: Arc<TrustedProxyConfig>,
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
        let failure_guard = self.failure_guard.clone();
        let trusted_proxies = self.trusted_proxies.clone();
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

            let ip_verified = client_ip_is_verified(&req, &trusted_proxies);
            let client_ip = extract_client_ip_with_validation(&req, &trusted_proxies).into_owned();

            // Refuse a blocked IP BEFORE comparing its key. Comparing first
            // would answer every guess (200 for a hit, 429 for a miss), so the
            // block would not slow a brute-force attack at all. Unverified
            // IPs are never blocked (see AuthFailureGuard).
            if ip_verified
                && let Some(ref guard) = failure_guard
                && let Some(remaining) = guard.remaining_block(&client_ip, Instant::now())
            {
                debug!(client_ip = %client_ip, "Refusing request from blocked IP");
                return Ok(rate_limited_response(retry_after_secs(remaining)));
            }

            // Only failures are charged to the budget: charging every request
            // would throttle legitimate clients down to ~10 req/min/IP, and
            // all header-less direct clients share the "unknown" bucket.
            let provided_key = extract_api_key(&req);

            match provided_key {
                Some(extracted) if constant_time_eq(&extracted.key, &expected) => {
                    // Valid API key - proceed without touching the budget
                    debug!(
                        from_query = extracted.from_query,
                        "API key authentication successful"
                    );
                    inner.call(req).await
                }
                provided => {
                    // Auth failure: charge it to this IP. A failure that finds
                    // the budget exhausted gets 429, and blocks a verified IP.
                    if let Some(ref guard) = failure_guard
                        && let Some(wait) = guard.charge_failure(&client_ip)
                    {
                        if ip_verified {
                            guard.block(&client_ip, Instant::now(), wait);
                        }
                        let retry_after = retry_after_secs(wait);

                        error!(
                            client_ip = %client_ip,
                            ip_verified,
                            retry_after_secs = retry_after,
                            "IP blocked due to excessive auth failures"
                        );

                        return Ok(rate_limited_response(retry_after));
                    }

                    if provided.is_some() {
                        warn!(
                            path = %req.uri().path(),
                            client_ip = %client_ip,
                            "Invalid API key provided"
                        );
                        Ok(unauthorized_response("Invalid API key"))
                    } else {
                        warn!(
                            path = %req.uri().path(),
                            client_ip = %client_ip,
                            "Missing API key"
                        );
                        Ok(unauthorized_response("API key required"))
                    }
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

/// `Retry-After` seconds for a block of `remaining`: rounded up, at least 1.
fn retry_after_secs(remaining: Duration) -> u64 {
    let whole_secs = remaining.as_secs();
    let rounded_up = if remaining.subsec_nanos() > 0 {
        whole_secs.saturating_add(1)
    } else {
        whole_secs
    };
    rounded_up.max(1)
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::extract::ConnectInfo;
    use std::net::SocketAddr;

    /// Minimal inner service returning 200 OK, for driving the auth layer.
    #[derive(Clone)]
    struct OkService;

    impl Service<Request<Body>> for OkService {
        type Response = Response<Body>;
        type Error = std::convert::Infallible;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: Request<Body>) -> Self::Future {
            std::future::ready(Ok(StatusCode::OK.into_response()))
        }
    }

    fn request_with_key(key: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri("/stats");
        if let Some(k) = key {
            builder = builder.header(API_KEY_HEADER, k);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn test_valid_key_requests_never_consume_failure_budget() {
        let auth = ApiKeyAuth::with_defaults(Some("secret".to_string()));
        let mut svc = auth.layer(OkService);

        // Far more valid requests than the failure budget. A regression to
        // counting every request would 429 partway through. Derived from the
        // constants so raising the budget cannot silently weaken the test.
        let beyond_budget =
            2 * (DEFAULT_AUTH_FAILURE_LIMIT.get() + DEFAULT_AUTH_FAILURE_BURST.get()) + 10;
        for i in 0..beyond_budget {
            let resp = svc.call(request_with_key(Some("secret"))).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "valid-key request {} was throttled by the failure limiter",
                i
            );
        }
    }

    /// Auth layer with `TRUSTED_PROXIES` set, so peer-addressed requests are
    /// verified.
    fn verified_auth() -> ApiKeyAuth {
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        ApiKeyAuth::with_trusted_proxies(Some("secret".to_string()), vec![], Arc::new(trusted))
    }

    /// Request from the direct peer `peer`. Outside the trusted range, so a
    /// verified layer keys it on the peer address.
    fn request_from_peer(key: Option<&str>, peer: [u8; 4]) -> Request<Body> {
        let mut req = request_with_key(key);
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((peer, 40_000))));
        req
    }

    /// Send failing requests built by `make_request` until one is refused
    /// with 429; return how many were answered 401 first. Panics if the
    /// budget is never exhausted.
    async fn exhaust_failure_budget<S, F>(svc: &mut S, make_request: F) -> u32
    where
        S: Service<Request<Body>, Response = Response<Body>, Error = std::convert::Infallible>,
        F: Fn(Option<&str>) -> Request<Body>,
    {
        let beyond_budget =
            2 * (DEFAULT_AUTH_FAILURE_LIMIT.get() + DEFAULT_AUTH_FAILURE_BURST.get()) + 10;
        let mut unauthorized = 0;
        for _ in 0..beyond_budget {
            let resp = svc.call(make_request(Some("wrong"))).await.unwrap();
            match resp.status() {
                StatusCode::UNAUTHORIZED => unauthorized += 1,
                StatusCode::TOO_MANY_REQUESTS => return unauthorized,
                other => panic!("unexpected status {other}"),
            }
        }
        panic!("failure budget was never exhausted");
    }

    async fn response_parts(resp: Response<Body>) -> (StatusCode, Option<String>, Vec<u8>) {
        let status = resp.status();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .map(|v| v.to_str().unwrap().to_string());
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, retry_after, body)
    }

    #[tokio::test]
    async fn test_auth_failures_throttled_after_budget_exhausted() {
        let mut svc = verified_auth().layer(OkService);

        // Failures are answered 401 until the burst is spent, then 429.
        let unauthorized =
            exhaust_failure_budget(&mut svc, |key| request_from_peer(key, [203, 0, 113, 1])).await;
        assert_eq!(unauthorized, DEFAULT_AUTH_FAILURE_BURST.get());
    }

    #[tokio::test]
    async fn test_blocked_verified_ip_is_refused_even_with_valid_key() {
        let mut svc = verified_auth().layer(OkService);
        exhaust_failure_budget(&mut svc, |key| request_from_peer(key, [203, 0, 113, 1])).await;

        // While blocked, the correct key gets the same answer as a wrong one.
        // A regression to comparing the key before the block check returns
        // 200 here, which turns the block into a hit/miss oracle.
        let with_valid = svc
            .call(request_from_peer(Some("secret"), [203, 0, 113, 1]))
            .await
            .unwrap();
        let with_wrong = svc
            .call(request_from_peer(Some("wrong"), [203, 0, 113, 1]))
            .await
            .unwrap();
        let (valid_status, valid_retry, valid_body) = response_parts(with_valid).await;
        let (wrong_status, wrong_retry, wrong_body) = response_parts(with_wrong).await;

        assert_eq!(valid_status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(valid_status, wrong_status);
        assert_eq!(valid_body, wrong_body);
        assert!(valid_retry.is_some() && wrong_retry.is_some());
    }

    #[tokio::test]
    async fn test_block_does_not_affect_valid_key_from_other_verified_ip() {
        let mut svc = verified_auth().layer(OkService);
        exhaust_failure_budget(&mut svc, |key| request_from_peer(key, [203, 0, 113, 1])).await;

        let resp = svc
            .call(request_from_peer(Some("secret"), [203, 0, 113, 2]))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_unverified_ip_throttles_failures_but_never_blocks_valid_key() {
        // No TRUSTED_PROXIES: every header-less client shares the "unknown"
        // bucket. One client's failures must not lock the others out. A
        // regression that blocks unverified IPs returns 429 for the valid key.
        let mut svc = ApiKeyAuth::with_defaults(Some("secret".to_string())).layer(OkService);
        exhaust_failure_budget(&mut svc, request_with_key).await;
        let guard = svc.failure_guard.as_ref().unwrap();
        assert_eq!(
            guard.tracked_blocks(),
            0,
            "unverified IPs must not enter the block map (spoofed values would fill it)"
        );

        let valid = svc.call(request_with_key(Some("secret"))).await.unwrap();
        assert_eq!(valid.status(), StatusCode::OK);

        let wrong = svc.call(request_with_key(Some("wrong"))).await.unwrap();
        assert_eq!(wrong.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_trusted_proxies_without_peer_address_is_unverified() {
        // TRUSTED_PROXIES set but no ConnectInfo: the IP falls back to
        // headers, so it is unverified and must not block a valid key.
        let mut svc = verified_auth().layer(OkService);
        let from_header = |key: Option<&str>| {
            let mut req = request_with_key(key);
            req.headers_mut()
                .insert("x-forwarded-for", "203.0.113.1".parse().unwrap());
            req
        };
        exhaust_failure_budget(&mut svc, from_header).await;

        let resp = svc.call(from_header(Some("secret"))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn test_charge_failure_reports_exhaustion_only_past_the_burst() {
        let guard = AuthFailureGuard::new();

        for i in 0..DEFAULT_AUTH_FAILURE_BURST.get() {
            assert!(
                guard.charge_failure("198.51.100.7").is_none(),
                "failure {i} within the burst must fit the budget"
            );
        }
        let wait = guard
            .charge_failure("198.51.100.7")
            .expect("the failure past the burst must find the budget exhausted");
        assert!(wait > Duration::ZERO);
        assert_eq!(guard.tracked_blocks(), 0, "charging alone must not block");
    }

    #[test]
    fn test_block_sets_remaining_and_expires_with_entry_removed() {
        let guard = AuthFailureGuard::new();
        let now = Instant::now();
        let duration = Duration::from_secs(6);
        guard.block("198.51.100.7", now, duration);

        assert_eq!(guard.remaining_block("198.51.100.7", now), Some(duration));
        assert_eq!(guard.tracked_blocks(), 1);

        assert!(
            guard
                .remaining_block("198.51.100.7", now + duration)
                .is_none()
        );
        assert_eq!(guard.tracked_blocks(), 0, "expired block was not removed");
    }

    #[test]
    fn test_block_sweeps_expired_entries_at_threshold() {
        let guard = AuthFailureGuard::new();
        let past = Instant::now();
        {
            let mut blocks = guard.lock_blocks();
            for i in 0..BLOCK_SWEEP_THRESHOLD {
                blocks.insert(format!("stale-{i}"), past);
            }
        }

        guard.block(
            "198.51.100.7",
            past + Duration::from_secs(1),
            Duration::from_secs(6),
        );
        assert_eq!(guard.tracked_blocks(), 1, "expired blocks were not swept");
    }

    #[test]
    fn test_retry_after_secs_rounds_up_and_never_returns_zero() {
        assert_eq!(retry_after_secs(Duration::ZERO), 1);
        assert_eq!(retry_after_secs(Duration::from_millis(1)), 1);
        assert_eq!(retry_after_secs(Duration::from_secs(6)), 6);
        assert_eq!(retry_after_secs(Duration::from_millis(5_001)), 6);
    }

    #[tokio::test]
    async fn test_failure_buckets_are_per_ip() {
        let auth = ApiKeyAuth::with_defaults(Some("secret".to_string()));
        let mut svc = auth.layer(OkService);
        let beyond_budget =
            2 * (DEFAULT_AUTH_FAILURE_LIMIT.get() + DEFAULT_AUTH_FAILURE_BURST.get()) + 10;

        // Exhaust IP A's failure budget (default config trusts headers).
        for _ in 0..beyond_budget {
            let mut req = request_with_key(Some("wrong"));
            req.headers_mut()
                .insert("x-forwarded-for", "203.0.113.1".parse().unwrap());
            let _ = svc.call(req).await.unwrap();
        }

        // A failing request from IP B has a fresh budget: 401, not 429.
        // A regression collapsing all clients into one global bucket would
        // return 429 here.
        let mut req = request_with_key(Some("wrong"));
        req.headers_mut()
            .insert("x-forwarded-for", "203.0.113.2".parse().unwrap());
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

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
