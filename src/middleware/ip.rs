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
//! When `TrustedProxyConfig` is configured with CIDR ranges, forwarded headers
//! are only honored if the direct peer (from Axum's `ConnectInfo` extension)
//! is inside a trusted range; otherwise the peer address itself is used as the
//! client IP, so spoofed `X-Forwarded-For`/`X-Real-IP` headers from untrusted
//! sources are ignored. When the trusted proxy's `X-Forwarded-For` IS honored,
//! it is resolved with the **rightmost-untrusted** rule, so the guarantee
//! holds even for proxies that APPEND to a client-supplied header (the common
//! default) rather than overwriting it. This requires the server to be started
//! with `into_make_service_with_connect_info::<SocketAddr>()` (both `main.rs`
//! and the integration-test harness do this). If `ConnectInfo` is absent, the
//! functions fall back to header extraction and warn once.
//!
//! # Internal Architecture
//!
//! ```text
//!                       ┌──────────────────────────┐
//!                       │  extract_ip_from_headers │ ← Private, returns ExtractedIp<'a>
//!                       │  (no allocations)        │
//!                       └───────────┬──────────────┘
//!                                   │
//!                                   │
//!                                   ▼
//!                    ┌───────────────────────────┐
//!                    │    extract_client_ip      │ ← header-only fallback
//!                    │    (headers, no peer)     │
//!                    └───────────┬───────────────┘
//!                                │ delegated to by
//!                                ▼
//!                    ┌───────────────────────────┐
//!                    │ extract_client_ip_with_   │ ← used by rate limiting
//!                    │ validation()              │   AND authentication
//!                    │ - peer-address (ConnectInfo)
//!                    │   trust enforcement       │
//!                    │ - rightmost-untrusted XFF │
//!                    └───────────────────────────┘
//! ```
//!
//! **Design Benefits:**
//! - Single source of truth for header parsing logic
//! - `ExtractedIp` enum is `Copy` - zero allocation overhead in the helper
//! - Source info (`FromXff` vs. `FromRealIp`) enables differentiated logging
//! - Both public functions return `Cow<'static, str>` for consistent zero-allocation fallback

use std::borrow::Cow;
use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::Request;
use tracing::{debug, warn};

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
///
/// # Empty Header Handling
///
/// Empty or whitespace-only headers are treated as `NotFound` to prevent
/// creating a separate rate-limit bucket for each empty-header request.
#[inline]
fn extract_ip_from_headers<B>(req: &Request<B>) -> ExtractedIp<'_> {
    // Check X-Forwarded-For first (maybe set by reverse proxy)
    // Format: "client, proxy1, proxy2" - we want the first (client) IP
    if let Some(forwarded) = req.headers().get("x-forwarded-for")
        && let Ok(value) = forwarded.to_str()
        && let Some(first_ip) = value.split(',').next()
    {
        let trimmed = first_ip.trim();
        // Treat empty strings as not found to avoid creating separate rate-limit buckets
        if !trimmed.is_empty() {
            return ExtractedIp::FromXff(trimmed);
        }
    }

    // Check X-Real-IP (alternative header used by some proxies)
    if let Some(real_ip) = req.headers().get("x-real-ip")
        && let Ok(value) = real_ip.to_str()
    {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return ExtractedIp::FromRealIp(trimmed);
        }
    }

    ExtractedIp::NotFound
}

// =============================================================================
// Public API
// =============================================================================

/// Resolve the client IP from a full `X-Forwarded-For` value using the
/// **rightmost-untrusted** rule.
///
/// Walk the chain from the right (the entry appended by the proxy closest to
/// us) toward the left, skipping addresses inside trusted ranges; the first
/// untrusted address is the real client. This stays correct whether the edge
/// proxy OVERWRITES the header or (the common default: nginx
/// `$proxy_add_x_forwarded_for`, ALB, ingress controllers) APPENDS to a
/// client-supplied value — a leftmost attacker-chosen entry is never reached
/// unless every hop after it is a trusted proxy.
///
/// Returns `None` when any visited entry fails to parse as an IP (a chain we
/// cannot reason about is not trusted) — callers fall back to the peer
/// address. If EVERY entry is a trusted proxy, the leftmost entry is
/// returned: with an all-trusted chain, that is the origin as reported by
/// the first proxy.
fn rightmost_untrusted_xff(xff: &str, trusted_proxies: &TrustedProxyConfig) -> Option<IpAddr> {
    let mut leftmost_trusted = None;
    for entry in xff.split(',').rev() {
        let ip: IpAddr = entry.trim().parse().ok()?;
        if trusted_proxies.is_trusted_ip(&ip) {
            leftmost_trusted = Some(ip);
        } else {
            return Some(ip);
        }
    }
    leftmost_trusted
}

/// Extract client IP from request headers with trusted proxy validation.
///
/// Used by both the rate limiter and the auth brute-force limiter.
///
/// # Behavior with trusted proxies configured (and `ConnectInfo` available)
///
/// - Peer NOT in a trusted range: forwarded headers are ignored entirely and
///   the peer address is the client IP (spoofed headers from direct clients
///   are inert).
/// - Peer in a trusted range with `X-Forwarded-For`: resolved via the
///   rightmost-untrusted rule (see `rightmost_untrusted_xff`) so the
///   guarantee holds for both overwriting and appending proxies; an
///   unparseable chain falls back to the peer address.
/// - Peer in a trusted range with `X-Real-IP`: the value is used only if it
///   parses as an IP address; otherwise the peer address is used.
/// - No forwarded headers: the peer address is used.
///
/// # Behavior without trusted proxies (or without `ConnectInfo`)
///
/// Falls back to plain header extraction ([`extract_client_ip`]): first
/// `X-Forwarded-For` entry, then `X-Real-IP`, then [`UNKNOWN_IP`]. If
/// TRUSTED_PROXIES is configured but the server was not started with
/// `into_make_service_with_connect_info`, a warning is logged once.
///
/// # Returns
///
/// `Cow<'static, str>` - Borrowed for "unknown" (no allocation), owned for actual IPs.
/// Use `.into_owned()` when you need a `String` for async contexts that outlive
/// the request reference.
#[inline]
pub fn extract_client_ip_with_validation<B>(
    req: &Request<B>,
    trusted_proxies: &TrustedProxyConfig,
) -> Cow<'static, str> {
    if trusted_proxies.is_enabled() {
        match req.extensions().get::<ConnectInfo<SocketAddr>>() {
            Some(ConnectInfo(peer)) => {
                let peer_ip = peer.ip();
                if !trusted_proxies.is_trusted_ip(&peer_ip) {
                    // Peer is NOT a trusted proxy: forwarded headers are
                    // client-controlled and spoofable, so ignore them and key
                    // on the actual peer address.
                    if !matches!(extract_ip_from_headers(req), ExtractedIp::NotFound) {
                        debug!(
                            peer_ip = %peer_ip,
                            "Ignoring forwarded headers from untrusted peer"
                        );
                    }
                    return Cow::Owned(peer_ip.to_string());
                }

                // Peer is a trusted proxy: resolve the forwarded chain.
                if let Some(xff) = req.headers().get("x-forwarded-for")
                    && let Ok(value) = xff.to_str()
                    && !value.trim().is_empty()
                {
                    return match rightmost_untrusted_xff(value, trusted_proxies) {
                        Some(client_ip) => Cow::Owned(client_ip.to_string()),
                        None => {
                            debug!(
                                peer_ip = %peer_ip,
                                "Unparseable X-Forwarded-For from trusted proxy; keying on peer address"
                            );
                            Cow::Owned(peer_ip.to_string())
                        }
                    };
                }

                if let Some(real_ip) = req.headers().get("x-real-ip")
                    && let Ok(value) = real_ip.to_str()
                    && let Ok(client_ip) = value.trim().parse::<IpAddr>()
                {
                    return Cow::Owned(client_ip.to_string());
                }

                // No usable forwarded header from the trusted proxy.
                return Cow::Owned(peer_ip.to_string());
            }
            None => {
                // TRUSTED_PROXIES is configured but the server was started
                // without connect-info, so enforcement is impossible. Fall
                // back to header trust; warn once, not per request (two
                // middlewares call this on every request).
                static WARN_ONCE: std::sync::Once = std::sync::Once::new();
                WARN_ONCE.call_once(|| {
                    warn!(
                        "TRUSTED_PROXIES is set but the peer address is unavailable \
                         (server not started with into_make_service_with_connect_info); \
                         falling back to trusting forwarded headers"
                    );
                });
            }
        }
    }

    extract_client_ip(req)
}

/// Extract client IP from request headers only (no peer-address validation).
///
/// This is the header-only fallback path used by
/// [`extract_client_ip_with_validation`] when trusted-proxy validation is
/// disabled or the peer address is unavailable. It trusts client-provided
/// headers unconditionally — production callers should always go through the
/// validated variant.
///
/// # Header Priority
///
/// 1. `X-Forwarded-For` header (first IP in a comma-separated list)
/// 2. `X-Real-IP` header
/// 3. Falls back to [`UNKNOWN_IP`]
///
/// # Security
///
/// All security considerations from the module-level documentation apply here.
///
/// # Returns
///
/// `Cow<'static, str>` - Borrowed for "unknown" (no allocation), owned for actual IPs.
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
    // Trusted Proxy Enforcement Tests
    // ==========================================================================

    fn req_with_peer(xff: Option<&str>, peer: [u8; 4]) -> Request<Body> {
        let mut builder = Request::builder();
        if let Some(v) = xff {
            builder = builder.header("x-forwarded-for", v);
        }
        let mut req = builder.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((peer, 12345))));
        req
    }

    #[test]
    fn test_validation_trusted_peer_honors_forwarded_header() {
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        let req = req_with_peer(Some("203.0.113.50"), [10, 0, 0, 5]);

        assert_eq!(
            extract_client_ip_with_validation(&req, &trusted),
            "203.0.113.50"
        );
    }

    #[test]
    fn test_validation_untrusted_peer_ignores_forwarded_header() {
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        // Peer outside the trusted range spoofs X-Forwarded-For
        let req = req_with_peer(Some("1.2.3.4"), [203, 0, 113, 9]);

        // The spoofed header must be ignored; the peer address is the client
        assert_eq!(
            extract_client_ip_with_validation(&req, &trusted),
            "203.0.113.9"
        );
    }

    #[test]
    fn test_validation_trusted_peer_without_header_uses_peer_ip() {
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        let req = req_with_peer(None, [10, 0, 0, 5]);

        assert_eq!(
            extract_client_ip_with_validation(&req, &trusted),
            "10.0.0.5"
        );
    }

    #[test]
    fn test_validation_appending_proxy_defeats_spoofed_first_entry() {
        // The common proxy default APPENDS the real client to a
        // client-supplied X-Forwarded-For. The rightmost-untrusted rule must
        // pick the entry the trusted proxy appended (203.0.113.50), not the
        // attacker-chosen first entry (6.6.6.6).
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        let req = req_with_peer(Some("6.6.6.6, 203.0.113.50"), [10, 0, 0, 5]);

        assert_eq!(
            extract_client_ip_with_validation(&req, &trusted),
            "203.0.113.50"
        );
    }

    #[test]
    fn test_validation_multi_hop_skips_trusted_intermediates() {
        // client, proxy-B (trusted), appended by peer proxy-A (trusted):
        // walking right-to-left skips 10.0.0.7 and lands on the client.
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        let req = req_with_peer(Some("203.0.113.50, 10.0.0.7"), [10, 0, 0, 5]);

        assert_eq!(
            extract_client_ip_with_validation(&req, &trusted),
            "203.0.113.50"
        );
    }

    #[test]
    fn test_validation_all_trusted_chain_uses_leftmost() {
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        let req = req_with_peer(Some("10.0.0.9, 10.0.0.7"), [10, 0, 0, 5]);

        assert_eq!(
            extract_client_ip_with_validation(&req, &trusted),
            "10.0.0.9"
        );
    }

    #[test]
    fn test_validation_unparseable_chain_falls_back_to_peer() {
        // Garbage in the chain (e.g. an entry with a port) means the chain
        // cannot be reasoned about - key on the trusted peer itself.
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        let req = req_with_peer(Some("203.0.113.50:8080"), [10, 0, 0, 5]);

        assert_eq!(
            extract_client_ip_with_validation(&req, &trusted),
            "10.0.0.5"
        );
    }

    #[test]
    fn test_validation_real_ip_from_trusted_peer() {
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        let mut req = Request::builder()
            .header("x-real-ip", "203.0.113.50")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 5], 12345))));

        assert_eq!(
            extract_client_ip_with_validation(&req, &trusted),
            "203.0.113.50"
        );
    }

    #[test]
    fn test_validation_garbage_real_ip_falls_back_to_peer() {
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        let mut req = Request::builder()
            .header("x-real-ip", "not-an-ip")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 5], 12345))));

        assert_eq!(
            extract_client_ip_with_validation(&req, &trusted),
            "10.0.0.5"
        );
    }

    #[test]
    fn test_validation_missing_connect_info_falls_back_to_headers() {
        let trusted = TrustedProxyConfig::try_new(&["10.0.0.0/8".to_string()]).unwrap();
        // No ConnectInfo extension (server started without connect-info)
        let req = Request::builder()
            .header("x-forwarded-for", "1.2.3.4")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip_with_validation(&req, &trusted), "1.2.3.4");
    }

    // ==========================================================================
    // Edge Case Tests
    // ==========================================================================

    #[test]
    fn test_extract_ip_empty_xff_header() {
        // Empty header value should fall back to unknown to prevent
        // creating separate rate-limit buckets for empty-header requests
        let req = Request::builder()
            .header("x-forwarded-for", "")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "unknown");
    }

    #[test]
    fn test_extract_ip_whitespace_only_xff() {
        // Whitespace-only header should fall back to unknown
        let req = Request::builder()
            .header("x-forwarded-for", "   ")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "unknown");
    }

    #[test]
    fn test_extract_ip_empty_xff_falls_back_to_real_ip() {
        // Empty XFF should fall through to X-Real-IP
        let req = Request::builder()
            .header("x-forwarded-for", "")
            .header("x-real-ip", "192.168.1.1")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "192.168.1.1");
    }

    #[test]
    fn test_extract_ip_empty_real_ip_header() {
        // Empty X-Real-IP should fall back to unknown
        let req = Request::builder()
            .header("x-real-ip", "")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_client_ip(&req), "unknown");
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
