//! Request timeout propagation middleware.
//!
//! This module provides middleware for propagating client-specified request timeouts,
//! allowing clients to specify how long they're willing to wait for a response.
//!
//! # Usage
//!
//! Clients can specify a timeout via the `X-Request-Timeout` header:
//! ```text
//! X-Request-Timeout: 5000  # 5 seconds in milliseconds
//! ```
//!
//! Handlers can then extract this timeout:
//! ```rust,ignore
//! async fn handler(
//!     timeout: Option<Extension<RequestTimeout>>,
//!     // ...
//! ) -> impl IntoResponse {
//!     let effective_timeout = timeout
//!         .map(|t| t.0.duration)
//!         .unwrap_or(default_timeout);
//!     // Use effective_timeout for operations
//! }
//! ```
//!
//! # Benefits
//!
//! - Prevents wasted work when clients have already given up
//! - Allows clients to specify shorter timeouts for time-sensitive operations
//! - Enables deadline propagation in distributed systems
//!
//! # Security Considerations
//!
//! - Minimum and maximum timeout bounds are enforced to prevent abuse
//! - Invalid values are ignored (fall back to server default)
//! - Zero or negative values are rejected

use std::time::Duration;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use tracing::debug;

/// Minimum allowed request timeout (100ms).
///
/// Prevents clients from requesting unreasonably short timeouts that
/// would cause operations to always fail.
pub const MIN_REQUEST_TIMEOUT_MS: u64 = 100;

/// Maximum allowed request timeout (5 minutes).
///
/// Prevents clients from requesting excessively long timeouts that
/// could tie up resources. Adjust based on your longest expected operation.
pub const MAX_REQUEST_TIMEOUT_MS: u64 = 300_000;

/// Header name for client-specified request timeout.
pub const REQUEST_TIMEOUT_HEADER: &str = "x-request-timeout";

/// Extracted request timeout from client header.
///
/// This is stored in request extensions and can be extracted by handlers.
#[derive(Debug, Clone, Copy)]
pub struct RequestTimeout {
    /// The timeout duration specified by the client.
    pub duration: Duration,
    /// The original value from the header (for logging).
    pub original_ms: u64,
}

impl RequestTimeout {
    /// Create a new RequestTimeout from milliseconds.
    ///
    /// Returns `None` if the value is outside the allowed range.
    pub fn from_millis(ms: u64) -> Option<Self> {
        if !(MIN_REQUEST_TIMEOUT_MS..=MAX_REQUEST_TIMEOUT_MS).contains(&ms) {
            return None;
        }
        Some(Self {
            duration: Duration::from_millis(ms),
            original_ms: ms,
        })
    }
}

/// Middleware that extracts and validates the `X-Request-Timeout` header.
///
/// If a valid timeout is present, it's stored in request extensions
/// for handlers to use.
pub async fn extract_request_timeout(mut request: Request, next: Next) -> Response {
    // Try to extract and parse the timeout header
    if let Some(timeout_value) = request.headers().get(REQUEST_TIMEOUT_HEADER)
        && let Ok(value_str) = timeout_value.to_str()
    {
        if let Ok(ms) = value_str.trim().parse::<u64>() {
            if let Some(timeout) = RequestTimeout::from_millis(ms) {
                debug!(
                    timeout_ms = ms,
                    "Client specified request timeout via header"
                );
                request.extensions_mut().insert(timeout);
            } else {
                debug!(
                    timeout_ms = ms,
                    min = MIN_REQUEST_TIMEOUT_MS,
                    max = MAX_REQUEST_TIMEOUT_MS,
                    "Client timeout outside allowed range, ignoring"
                );
            }
        } else {
            debug!(
                value = value_str,
                "Invalid X-Request-Timeout header value, ignoring"
            );
        }
    }

    next.run(request).await
}

/// Extension trait for extracting request timeout from request extensions.
pub trait RequestTimeoutExt {
    /// Get the client-specified timeout, or fall back to the provided default.
    fn effective_timeout(&self, default: Duration) -> Duration;
}

impl<B> RequestTimeoutExt for axum::http::Request<B> {
    fn effective_timeout(&self, default: Duration) -> Duration {
        self.extensions()
            .get::<RequestTimeout>()
            .map(|t| t.duration)
            .unwrap_or(default)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_request_timeout_from_millis_valid() {
        let timeout = RequestTimeout::from_millis(5000).unwrap();
        assert_eq!(timeout.duration, Duration::from_millis(5000));
        assert_eq!(timeout.original_ms, 5000);
    }

    #[test]
    fn test_request_timeout_from_millis_minimum() {
        let timeout = RequestTimeout::from_millis(MIN_REQUEST_TIMEOUT_MS).unwrap();
        assert_eq!(
            timeout.duration,
            Duration::from_millis(MIN_REQUEST_TIMEOUT_MS)
        );
    }

    #[test]
    fn test_request_timeout_from_millis_maximum() {
        let timeout = RequestTimeout::from_millis(MAX_REQUEST_TIMEOUT_MS).unwrap();
        assert_eq!(
            timeout.duration,
            Duration::from_millis(MAX_REQUEST_TIMEOUT_MS)
        );
    }

    #[test]
    fn test_request_timeout_from_millis_too_low() {
        let timeout = RequestTimeout::from_millis(MIN_REQUEST_TIMEOUT_MS - 1);
        assert!(timeout.is_none());
    }

    #[test]
    fn test_request_timeout_from_millis_too_high() {
        let timeout = RequestTimeout::from_millis(MAX_REQUEST_TIMEOUT_MS + 1);
        assert!(timeout.is_none());
    }

    #[test]
    fn test_request_timeout_from_millis_zero() {
        let timeout = RequestTimeout::from_millis(0);
        assert!(timeout.is_none());
    }
}
