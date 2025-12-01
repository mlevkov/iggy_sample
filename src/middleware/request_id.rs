//! Request ID middleware for distributed tracing.
//!
//! # Features
//!
//! - Generates UUIDv4 request IDs for incoming requests without one
//! - Propagates existing `X-Request-Id` headers
//! - Adds `X-Request-Id` to all responses
//! - Integrates with tracing spans for observability
//!
//! # Usage
//!
//! The middleware automatically:
//! 1. Checks for an existing `X-Request-Id` header
//! 2. Generates a new UUID if none exists
//! 3. Adds the ID to the response headers
//! 4. Includes the ID in tracing spans
//!
//! # Client Usage
//!
//! Clients can provide their own request ID:
//!
//! ```bash
//! curl -H "X-Request-Id: my-correlation-id" http://localhost:3000/messages
//! ```
//!
//! The same ID will be returned in the response for correlation.

use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::header::HeaderValue;
use axum::http::{Request, Response};
use tower::{Layer, Service};
use tracing::{Span, debug};
use uuid::Uuid;

/// Header name for request ID.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Fallback header value when request ID parsing fails.
/// Using `from_static` avoids runtime parsing and is infallible.
static UNKNOWN_REQUEST_ID: HeaderValue = HeaderValue::from_static("unknown");

/// Request ID layer for Tower middleware stack.
#[derive(Clone, Default)]
pub struct RequestIdLayer;

impl RequestIdLayer {
    /// Create a new request ID layer.
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for RequestIdLayer {
    type Service = RequestIdService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestIdService { inner }
    }
}

/// Request ID service wrapper.
#[derive(Clone)]
pub struct RequestIdService<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for RequestIdService<S>
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

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        // Extract or generate request ID
        let request_id = extract_or_generate_request_id(&req);

        // Add request ID to request headers (so handlers can access it)
        req.headers_mut().insert(
            REQUEST_ID_HEADER,
            request_id
                .parse()
                .unwrap_or_else(|_| UNKNOWN_REQUEST_ID.clone()),
        );

        // Record in current span
        Span::current().record("request_id", &request_id);
        debug!(request_id = %request_id, "Processing request");

        let mut inner = self.inner.clone();

        Box::pin(async move {
            let mut response = inner.call(req).await?;

            // Add request ID to response headers
            response.headers_mut().insert(
                REQUEST_ID_HEADER,
                request_id
                    .parse()
                    .unwrap_or_else(|_| UNKNOWN_REQUEST_ID.clone()),
            );

            Ok(response)
        })
    }
}

/// Extract request ID from headers or generate a new one.
fn extract_or_generate_request_id<B>(req: &Request<B>) -> String {
    // Check for existing request ID
    if let Some(header_value) = req.headers().get(REQUEST_ID_HEADER)
        && let Ok(value) = header_value.to_str()
        && !value.is_empty()
    {
        return value.to_string();
    }

    // Generate new UUID
    Uuid::new_v4().to_string()
}

/// Extension trait to extract request ID from requests.
pub trait RequestIdExt {
    /// Get the request ID from the request headers.
    fn request_id(&self) -> Option<String>;
}

impl<B> RequestIdExt for Request<B> {
    fn request_id(&self) -> Option<String> {
        self.headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_existing_request_id() {
        let req = Request::builder()
            .header("x-request-id", "existing-id-123")
            .body(Body::empty())
            .unwrap();

        assert_eq!(extract_or_generate_request_id(&req), "existing-id-123");
    }

    #[test]
    fn test_generate_new_request_id() {
        let req = Request::builder().body(Body::empty()).unwrap();

        let id = extract_or_generate_request_id(&req);

        // Should be a valid UUID
        assert!(Uuid::parse_str(&id).is_ok());
    }

    #[test]
    fn test_request_id_ext_trait() {
        let req = Request::builder()
            .header("x-request-id", "test-id")
            .body(Body::empty())
            .unwrap();

        assert_eq!(req.request_id(), Some("test-id".to_string()));
    }

    #[test]
    fn test_request_id_ext_trait_none() {
        let req = Request::builder().body(Body::empty()).unwrap();

        assert_eq!(req.request_id(), None);
    }
}
