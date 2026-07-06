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
//! Handlers extract the parsed value (via the `OptionalFromRequestParts`
//! impl below) and scope their Iggy access to it:
//! ```rust,ignore
//! async fn handler(
//!     State(state): State<AppState>,
//!     timeout: Option<RequestTimeout>,
//! ) -> impl IntoResponse {
//!     // Iggy operations bounded by the request deadline (clamped to the
//!     // configured global OPERATION_TIMEOUT_SECS — clients may shorten,
//!     // never extend).
//!     let client = state.iggy_scoped(timeout);
//!     client.list_streams().await
//! }
//! ```
//!
//! # Enforcement path
//!
//! All Iggy-touching handlers pass the extracted timeout through
//! `AppState::{producer_scoped, consumer_scoped, iggy_scoped}` into
//! `IggyClientWrapper::with_timeout`, which bounds every operation attempt
//! (and the caller's wait on any reconnect) by the request deadline.
//! Requests without the header use the global `OPERATION_TIMEOUT_SECS`.
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
//! - The effective deadline is additionally clamped to the server's global
//!   operation timeout — the header can never EXTEND server-side work
//! - Invalid, zero, or out-of-range values are ignored: the request
//!   proceeds under the global timeout (negative values never parse as
//!   `u64` and are likewise ignored)

use std::time::Duration;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use tracing::{debug, warn};

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
/// Stored in request extensions by the middleware; handlers receive it via
/// the `Option<RequestTimeout>` extractor. The field is private so
/// [`RequestTimeout::from_millis`] is the only constructor — a value outside
/// `[MIN_REQUEST_TIMEOUT_MS, MAX_REQUEST_TIMEOUT_MS]` is unrepresentable.
#[derive(Debug, Clone, Copy)]
pub struct RequestTimeout {
    /// The timeout duration specified by the client (range-validated).
    duration: Duration,
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
        })
    }

    /// The client-requested timeout duration.
    pub fn duration(&self) -> Duration {
        self.duration
    }
}

/// Lets handlers declare `timeout: Option<RequestTimeout>` directly instead
/// of unwrapping `Option<Extension<RequestTimeout>>` at every call site.
/// Absence means the client sent no (valid) `X-Request-Timeout` — or the
/// route is not behind the timeout middleware, which degrades safely to
/// the global deadline.
impl<S> axum::extract::OptionalFromRequestParts<S> for RequestTimeout
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts.extensions.get::<RequestTimeout>().copied())
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
                // Same silence class as a malformed value: the caller
                // believes a deadline is in effect that is not.
                warn!(
                    timeout_ms = ms,
                    min = MIN_REQUEST_TIMEOUT_MS,
                    max = MAX_REQUEST_TIMEOUT_MS,
                    "Client timeout outside allowed range, ignoring"
                );
            }
        } else {
            // warn: a malformed header is a client bug - the caller believes
            // a deadline is in effect that is not (TD-2026-07-08 tracks
            // echoing the effective deadline back to the client).
            warn!(
                value = value_str,
                "Invalid X-Request-Timeout header value, ignoring"
            );
        }
    }

    next.run(request).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_request_timeout_from_millis_valid() {
        let timeout = RequestTimeout::from_millis(5000).unwrap();
        assert_eq!(timeout.duration(), Duration::from_millis(5000));
    }

    #[test]
    fn test_request_timeout_from_millis_minimum() {
        let timeout = RequestTimeout::from_millis(MIN_REQUEST_TIMEOUT_MS).unwrap();
        assert_eq!(
            timeout.duration(),
            Duration::from_millis(MIN_REQUEST_TIMEOUT_MS)
        );
    }

    #[test]
    fn test_request_timeout_from_millis_maximum() {
        let timeout = RequestTimeout::from_millis(MAX_REQUEST_TIMEOUT_MS).unwrap();
        assert_eq!(
            timeout.duration(),
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

    #[tokio::test]
    async fn test_optional_extractor_reads_extension_presence() {
        use axum::extract::OptionalFromRequestParts;

        let (mut parts, ()) = axum::http::Request::new(()).into_parts();

        let absent =
            <RequestTimeout as OptionalFromRequestParts<()>>::from_request_parts(&mut parts, &())
                .await
                .unwrap();
        assert!(absent.is_none(), "no extension => None (global timeout)");

        let inserted = RequestTimeout::from_millis(5000).unwrap();
        parts.extensions.insert(inserted);
        let present =
            <RequestTimeout as OptionalFromRequestParts<()>>::from_request_parts(&mut parts, &())
                .await
                .unwrap();
        assert_eq!(
            present.map(|t| t.duration()),
            Some(Duration::from_millis(5000))
        );
    }
}
