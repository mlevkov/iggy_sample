//! HTTP middleware for security, rate limiting, and observability.
//!
//! This module provides production-ready middleware components:
//!
//! - **Rate Limiting**: Token bucket algorithm with configurable RPS and burst
//! - **API Key Authentication**: Constant-time comparison for security
//! - **Request ID**: Automatic generation and propagation for distributed tracing
//! - **Request Timeout**: Client-specified timeout propagation
//! - **Trusted Proxy Validation**: CIDR-based proxy source validation
//!
//! # Architecture
//!
//! ```text
//! Request → Rate Limiter → Auth → Timeout → Request ID → Handler → Response
//!              ↓              ↓       ↓           ↓
//!          429 Too Many   401 Unauth  ext    X-Request-Id header
//! ```
//!
//! # Security Considerations
//!
//! - API key comparison uses constant-time equality to prevent timing attacks
//! - Rate limiting prevents abuse and DoS attacks
//! - Trusted proxy configuration mitigates IP spoofing attacks
//! - Request IDs enable audit trails and debugging
//! - Request timeout bounds prevent abuse via extreme values

pub mod auth;
pub mod ip;
pub mod rate_limit;
pub mod request_id;
pub mod timeout;

pub use auth::ApiKeyAuth;
pub use ip::{UNKNOWN_IP, extract_client_ip, extract_client_ip_with_validation};
pub use rate_limit::{RateLimitError, RateLimitLayer, TrustedProxyConfig};
pub use request_id::RequestIdLayer;
pub use timeout::{
    MAX_REQUEST_TIMEOUT_MS, MIN_REQUEST_TIMEOUT_MS, REQUEST_TIMEOUT_HEADER, RequestTimeout,
    RequestTimeoutExt, extract_request_timeout,
};
