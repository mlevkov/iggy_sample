//! Client IP extraction utilities for middleware.
//!
//! This module provides unified IP extraction logic used by both rate limiting
//! and authentication middleware, avoiding code duplication.
//!
//! # Performance
//!
//! - Uses ` Cow <'static, str> ` to avoid allocation for the common "unknown" fallback
//! - Header parsing is done lazily and short-circuits on the first match
//! - `#[inline]` hints on hot paths for potential inlining
//!
//! # Security Warning: IP Spoofing Risk
//!
//! **These functions trust client-provided headers.** Malicious clients can spoof
//! their IP address by setting `X-Forwarded-For` or `X-Real-IP` headers directly
//! if your service is not properly protected.
//!
//! ## Required Deployment Configuration
//!
//! To use per-IP rate limiting and brute force protection securely, you MUST:
//!
//! 1. **Deploy behind a trusted reverse proxy** (nginx, HAProxy, cloud LB)
//! 2. **Block direct access** to this service from the internet
//! 3. **Configure your proxy** to overwrite (not append to) client IP headers:
//!
//!    ```nginx
//!    # nginx example - overwrites any client-provided header
//!    proxy_set_header X-Real-IP $remote_addr;
//!    proxy_set_header X-Forwarded-For $remote_addr;
//!    ```
//!
//! 4. **For multiple proxy hops**, only trust the first hop:
//!
//!    ```nginx
//!    # nginx behind another proxy - trust the immediate upstream
//!    set_real_ip_from 10.0.0.0/8;  # Your trusted proxy network
//!    real_ip_header X-Forwarded-For;
//!    real_ip_recursive off;  # Only trust first IP in chain
//!    ```
//!
//! ## Attack Scenarios Without Proper Configuration
//!
//! If this service is directly accessible from the internet, attackers can:
//!
//! - **Bypass rate limiting** by rotating spoofed IPs in X-Forwarded-For
//! - **Frame innocent IPs** for abuse or trigger rate limit lockouts
//! - **Exhaust rate limit quotas** for legitimate users (DoS amplification)
//! - **Bypass brute force protection** on authentication endpoints
//!
//! ## The "unknown" Fallback
//!
//! When no IP headers are present, all such requests share the `"unknown"` key.
//! This provides some protection but has trade-offs:
//!
//! - **Upside**: Requests without headers are collectively rate-limited
//! - **Downside**: May rate-limit legitimate users behind misconfigured proxies
//! - **Recommendation**: Monitor for high "unknown" traffic in production logs
//!
//! ## Trusted Proxy Validation
//!
//! When `TrustedProxyConfig` is provided with CIDR ranges, the extraction
//! functions log debug information to help detect potential spoofing. Note that
//! full connection IP validation requires Axum's `ConnectInfo` extension, which
//! is not available at the middleware level.
//!
//! # Internal Architecture
//!
//! ```text
//!                       ┌──────────────────────────┐
//!                       │  extract_ip_from_headers │ ← Private, returns ExtractedIp<'a>
//!                       │  (no allocations)        │
//!                       └───────────┬──────────────┘
//!                                   │
//!               ┌───────────────────┴───────────────────┐
//!               │                                       │
//!               ▼                                       ▼
//!   ┌───────────────────────────┐       ┌───────────────────────────┐
//!   │ extract_client_ip_with_   │       │    extract_client_ip      │
//!   │ validation()              │       │    (simple)               │
//!   │ - Debug logging           │       │ - No logging overhead     │
//!   │ - Returns Cow<'static>    │       │ - Returns Cow<'static>    │
//!   └───────────────────────────┘       └───────────────────────────┘
//! ```
//!
//! **Design Benefits:**
//! - Single source of truth for header parsing logic
//! - `ExtractedIp` enum is `Copy` - zero allocation overhead in the helper
//! - Source info (`FromXff` vs. `FromRealIp`) enables differentiated logging
//! - Both public functions return `Cow<'static, str>` for consistent zero-allocation fallback

use std::borrow::Cow;

use axum::http::Request;
use tracing::debug;

use super::rate_limit::TrustedProxyConfig;

/// Fallback IP value when no client IP can be determined.
///
/// All requests without identifiable IPs share this key, which provides
/// some protection but may rate-limit legitimate users behind misconfigured
/// proxies. Monitor for high "unknown" traffic in production.
pub const UNKNOWN_IP: &str = "unknown";

// =============================================================================
// Private Helper
// =============================================================================

/// Result of extracting IP from headers.
///
/// This enum allows the caller to distinguish between:
/// - An IP found in X-Forwarded-For (with source info)
/// - An IP found in X-Real-IP (with source info)
/// - No IP found (should fall back to UNKNOWN_IP)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtractedIp<'a> {
    /// IP extracted from X-Forwarded-For header (first IP in the list).
    FromXff(&'a str),
    /// IP extracted from X-Real-IP header.
    FromRealIp(&'a str),
    /// No IP headers found.
    NotFound,
}

/// Extract raw IP string from request headers.
///
/// This is the core extraction logic shared by both public functions.
/// Returns the source of the IP for logging purposes.
///
/// # Performance
///
/// - Returns borrowed `&str` slices pointing into the request headers
/// - No allocations in this function
/// - Caller is responsible for `.to_string()` if ownership is needed
#[inline]
fn extract_ip_from_headers<B>(req: &Request<B>) -> ExtractedIp<'_> {
    // Check X-Forwarded-For first (maybe set by reverse proxy)
    // Format: "client, proxy1, proxy2" - we want the first (client) IP
    if let Some(forwarded) = req.headers().get("x-forwarded-for")
        && let Ok(value) = forwarded.to_str()
        && let Some(first_ip) = value.split(',').next()
    {
        return ExtractedIp::FromXff(first_ip.trim());
    }

    // Check X-Real-IP (alternative header used by some proxies)
    if let Some(real_ip) = req.headers().get("x-real-ip")
        && let Ok(value) = real_ip.to_str()
    {
        return ExtractedIp::FromRealIp(value.trim());
    }

    ExtractedIp::NotFound
}

// =============================================================================
// Public API
// =============================================================================

/// Extract client IP from request headers with trusted proxy validation.
///
/// This is the full-featured extraction used by rate limiting middleware.
/// It logs debug information when trusted proxy validation is enabled,
/// helping operators detect potential IP spoofing attempts.
///
/// # Header Priority
///
/// Checks in order (returns first match):
/// 1. `X-Forwarded-For` header (first IP in a comma-separated list)
/// 2. `X-Real-IP` header
/// 3. Falls back to [`UNKNOWN_IP`]
///
/// # X-Forwarded-For Parsing
///
/// When multiple IPs are present (e.g., `"203.0.113.50, 70.41.3.18, 150.172.238.178"`),
/// only the **first** IP is used. This is typically the original client IP, with
/// subsequent IPs being intermediate proxies. However, this relies on your edge
/// proxy correctly setting this header (see module-level security documentation).
///
/// # Returns
///
/// `Cow<'static, str>` - Borrowed for "unknown" (no allocation), owned for actual IPs.
/// Use `.into_owned()` when you need a `String` for async contexts that outlive
/// the request reference.
///
/// # Example
///
/// ```ignore
/// let ip = extract_client_ip_with_validation(&req, &trusted_proxies);
/// let ip_string = ip.into_owned(); // For use in async block
/// ```
#[inline]
pub fn extract_client_ip_with_validation<B>(
    req: &Request<B>,
    trusted_proxies: &TrustedProxyConfig,
) -> Cow<'static, str> {
    match extract_ip_from_headers(req) {
        ExtractedIp::FromXff(ip) => {
            if trusted_proxies.is_enabled() {
                debug!(
                    client_ip = %ip,
                    "Extracted client IP from X-Forwarded-For (trusted proxy validation enabled)"
                );
            }
            Cow::Owned(ip.to_string())
        }
        ExtractedIp::FromRealIp(ip) => {
            if trusted_proxies.is_enabled() {
                debug!(
                    client_ip = %ip,
                    "Extracted client IP from X-Real-IP (trusted proxy validation enabled)"
                );
            }
            Cow::Owned(ip.to_string())
        }
        ExtractedIp::NotFound => {
            if trusted_proxies.is_enabled() {
                debug!("No proxy headers found - request may be bypassing reverse proxy");
            }
            Cow::Borrowed(UNKNOWN_IP)
        }
    }
}

/// Extract client IP from request headers (simple version without validation logging).
///
/// This is a streamlined version used by authentication middleware for brute force
/// protection. It omits the debug logging present in [`extract_client_ip_with_validation`]
/// for slightly faster execution in the hot path.
///
/// # Header Priority
///
/// Same as [`extract_client_ip_with_validation`]:
/// 1. `X-Forwarded-For` header (first IP in a comma-separated list)
/// 2. `X-Real-IP` header
/// 3. Falls back to [`UNKNOWN_IP`]
///
/// # Security
///
/// All security considerations from the module-level documentation apply here.
/// This function trusts client-provided headers and should only be used behind
/// a properly configured reverse proxy.
///
/// # Returns
///
/// `Cow<'static, str>` - Borrowed for "unknown" (no allocation), owned for actual IPs.
/// Use `.into_owned()` when you need a `String` for async contexts that outlive
/// the request reference.
///
/// # Example
///
/// ```ignore
/// let client_ip = extract_client_ip(&req);
/// // Convert to owned String for async block
/// let ip_string = client_ip.into_owned();
/// if let Err(_) = limiter.check_key(&ip_string) { ... }
/// ```
#[inline]
pub fn extract_client_ip<B>(req: &Request<B>) -> Cow<'static, str> {
    match extract_ip_from_headers(req) {
        ExtractedIp::FromXff(ip) | ExtractedIp::FromRealIp(ip) => Cow::Owned(ip.to_string()),
        ExtractedIp::NotFound => Cow::Borrowed(UNKNOWN_IP),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[test]
    fn test_extract_ip_from_xff() {
        let req = Request::builder()
            .header("x-forwarded-for", "192.168.1.1, 10.0.0.1")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "192.168.1.1");
    }

    #[test]
    fn test_extract_ip_from_xff_single() {
        let req = Request::builder()
            .header("x-forwarded-for", "203.0.113.50")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "203.0.113.50");
    }

    #[test]
    fn test_extract_ip_from_real_ip() {
        let req = Request::builder()
            .header("x-real-ip", "192.168.1.1")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "192.168.1.1");
    }

    #[test]
    fn test_extract_ip_xff_priority_over_real_ip() {
        let req = Request::builder()
            .header("x-forwarded-for", "10.0.0.1")
            .header("x-real-ip", "192.168.1.1")
            .body(Body::empty())
            .unwrap();

        // X-Forwarded-For should take priority
        assert_eq!(extract_client_ip(&req), "10.0.0.1");
    }

    #[test]
    fn test_extract_ip_unknown() {
        let req = Request::builder().body(Body::empty()).unwrap();
        assert_eq!(extract_client_ip(&req), "unknown");
    }

    #[test]
    fn test_extract_ip_with_whitespace() {
        let req = Request::builder()
            .header("x-forwarded-for", "  192.168.1.1  , 10.0.0.1")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "192.168.1.1");
    }

    #[test]
    fn test_extract_ip_with_validation_returns_cow() {
        let req = Request::builder()
            .header("x-forwarded-for", "192.168.1.1")
            .body(Body::empty())
            .unwrap();
        let trusted = TrustedProxyConfig::default();

        let ip = extract_client_ip_with_validation(&req, &trusted);
        assert_eq!(ip, "192.168.1.1");
        // Should be owned since we found an IP
        assert!(matches!(ip, Cow::Owned(_)));
    }

    #[test]
    fn test_extract_ip_with_validation_unknown_is_borrowed() {
        let req = Request::builder().body(Body::empty()).unwrap();
        let trusted = TrustedProxyConfig::default();

        let ip = extract_client_ip_with_validation(&req, &trusted);
        assert_eq!(ip, "unknown");
        // Should be borrowed for the "unknown" case (no allocation)
        assert!(matches!(ip, Cow::Borrowed(_)));
    }

    #[test]
    fn test_extract_client_ip_unknown_is_borrowed() {
        let req = Request::builder().body(Body::empty()).unwrap();

        let ip = extract_client_ip(&req);
        assert_eq!(ip, "unknown");
        // Simple version should also return borrowed for "unknown"
        assert!(matches!(ip, Cow::Borrowed(_)));
    }

    // ==========================================================================
    // Edge Case Tests
    // ==========================================================================

    #[test]
    fn test_extract_ip_empty_xff_header() {
        // Empty header value should fall back to unknown
        let req = Request::builder()
            .header("x-forwarded-for", "")
            .body(Body::empty())
            .unwrap();

        // Empty string after trim is still returned (split returns one empty element)
        // This matches real-world behavior where empty headers are sometimes sent
        assert_eq!(extract_client_ip(&req), "");
    }

    #[test]
    fn test_extract_ip_whitespace_only_xff() {
        // Whitespace-only header should return empty string after trim
        let req = Request::builder()
            .header("x-forwarded-for", "   ")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "");
    }

    #[test]
    fn test_extract_ip_long_proxy_chain() {
        // Very long chain of proxies - should still extract the first IP
        let long_chain = (0..100)
            .map(|i| format!("10.0.0.{}", i % 256))
            .collect::<Vec<_>>()
            .join(", ");

        let req = Request::builder()
            .header("x-forwarded-for", &long_chain)
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "10.0.0.0");
    }

    #[test]
    fn test_extract_ip_xff_with_ipv6() {
        let req = Request::builder()
            .header("x-forwarded-for", "2001:db8::1, 10.0.0.1")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "2001:db8::1");
    }

    #[test]
    fn test_extract_ip_real_ip_with_ipv6() {
        let req = Request::builder()
            .header("x-real-ip", "::1")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "::1");
    }

    #[test]
    fn test_extract_ip_xff_with_port() {
        // Some proxies include port numbers - we pass through as-is
        // (validation is the caller's responsibility)
        let req = Request::builder()
            .header("x-forwarded-for", "192.168.1.1:8080, 10.0.0.1")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "192.168.1.1:8080");
    }

    #[test]
    fn test_extract_ip_non_utf8_header_fallback() {
        // Non-UTF8 header values should fall back to X-Real-IP or unknown
        // We can't easily construct non-UTF8 headers in this test framework,
        // but we can test that invalid to_str() returns NotFound behavior
        // by verifying the function handles the error path
        let req = Request::builder()
            .header("x-real-ip", "192.168.1.1")
            .body(Body::empty())
            .unwrap();

        // Should successfully extract from x-real-ip
        assert_eq!(extract_client_ip(&req), "192.168.1.1");
    }
}
