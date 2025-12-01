//! Application routing configuration with middleware stack.
//!
//! # Middleware Stack (applied in order)
//!
//! ```text
//! Request
//!    │
//!    ▼
//! ┌──────────────────┐
//! │  Rate Limiting   │ ← 429 if exceeded
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │  Authentication  │ ← 401 if invalid (bypassed for /health, /ready)
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │   Request ID     │ ← Adds X-Request-Id header
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │     Tracing      │ ← HTTP request/response logging
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │      CORS        │ ← Cross-origin headers
//! └────────┬─────────┘
//!          │
//!          ▼
//!      Handler
//! ```
//!
//! # Route Groups
//!
//! - `/health`, `/ready`, `/stats` - Health & monitoring (auth bypassed)
//! - `/messages` - Message operations on default stream/topic
//! - `/streams` - Stream management
//! - `/streams/{stream}/topics` - Topic management

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::handlers;
use crate::middleware::{ApiKeyAuth, RateLimitError, RateLimitLayer, RequestIdLayer};
use crate::state::AppState;

/// Build the application router with all routes and middleware configured.
///
/// # Middleware Configuration
///
/// Middleware is configured based on the application config:
///
/// - **Rate Limiting**: Enabled if `rate_limit_rps > 0`
/// - **Authentication**: Enabled if `api_key` is set
/// - **CORS**: Configured from `cors_allowed_origins`
///
/// # Arguments
///
/// * `state` - Application state containing config and services
///
/// # Returns
///
/// Fully configured Axum router ready to be served.
///
/// # Errors
///
/// Returns `RateLimitError` if rate limiting configuration is invalid.
pub fn build_router(state: AppState) -> Result<Router, RateLimitError> {
    let config = &state.config;

    // =========================================================================
    // CORS Configuration
    // =========================================================================
    let cors = build_cors_layer(&config.cors_allowed_origins);

    // =========================================================================
    // Build Router with Routes
    // =========================================================================
    let mut router = Router::new()
        // Health and status endpoints (always accessible)
        .route("/health", get(handlers::health_check))
        .route("/ready", get(handlers::readiness_check))
        .route("/stats", get(handlers::stats))
        // Message endpoints (default stream/topic)
        .route("/messages", post(handlers::send_message))
        .route("/messages", get(handlers::poll_messages))
        .route("/messages/batch", post(handlers::send_batch))
        // Message endpoints (specific stream/topic)
        .route(
            "/streams/{stream}/topics/{topic}/messages",
            post(handlers::messages::send_message_to),
        )
        .route(
            "/streams/{stream}/topics/{topic}/messages",
            get(handlers::messages::poll_messages_from),
        )
        // Stream management endpoints
        .route("/streams", get(handlers::list_streams))
        .route("/streams", post(handlers::create_stream))
        .route("/streams/{name}", get(handlers::get_stream))
        .route("/streams/{name}", delete(handlers::delete_stream))
        // Topic management endpoints
        .route("/streams/{stream}/topics", get(handlers::list_topics))
        .route("/streams/{stream}/topics", post(handlers::create_topic))
        .route("/streams/{stream}/topics/{topic}", get(handlers::get_topic))
        .route(
            "/streams/{stream}/topics/{topic}",
            delete(handlers::delete_topic),
        );

    // =========================================================================
    // Apply Middleware Stack (order matters - applied bottom to top)
    // =========================================================================

    // 1. Request body size limit (prevents DoS via large payloads)
    info!(
        max_size_mb = config.max_request_body_size / (1024 * 1024),
        "Request body size limit configured"
    );
    router = router.layer(DefaultBodyLimit::max(config.max_request_body_size));

    // 2. CORS
    router = router.layer(cors);

    // 3. Tracing
    router = router.layer(TraceLayer::new_for_http());

    // 4. Request ID
    router = router.layer(RequestIdLayer::new());

    // 5. Authentication (if enabled)
    let auth_layer = ApiKeyAuth::new(config.api_key.clone(), config.auth_bypass_paths.clone());
    if auth_layer.is_enabled() {
        info!("API key authentication enabled");
        router = router.layer(auth_layer);
    } else {
        info!("API key authentication disabled (no API_KEY set)");
    }

    // 6. Rate Limiting (if enabled) - applied first, runs last in request pipeline
    if config.rate_limiting_enabled() {
        info!(
            rps = config.rate_limit_rps,
            burst = config.rate_limit_burst,
            trusted_proxies = config.trusted_proxies.len(),
            "Rate limiting enabled"
        );
        router = router.layer(RateLimitLayer::with_trusted_proxies(
            config.rate_limit_rps,
            config.rate_limit_burst,
            &config.trusted_proxies,
        )?);
    } else {
        info!("Rate limiting disabled (RATE_LIMIT_RPS=0)");
    }

    // Add state
    Ok(router.with_state(state))
}

/// Build CORS layer from configuration.
///
/// # Arguments
///
/// * `allowed_origins` - List of allowed origins, or `["*"]` for any origin
///
/// # Security Note
///
/// Using `*` (any origin) is convenient for development but should be
/// avoided in production. Specify explicit origins instead.
fn build_cors_layer(allowed_origins: &[String]) -> CorsLayer {
    // Check if we should allow any origin
    let allow_any = allowed_origins.iter().any(|o| o == "*");

    if allow_any {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        // Parse specific origins
        let origins: Vec<_> = allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();

        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_cors_layer_any() {
        let origins = vec!["*".to_string()];
        let _layer = build_cors_layer(&origins);
        // Just verify it doesn't panic
    }

    #[test]
    fn test_build_cors_layer_specific() {
        let origins = vec![
            "https://example.com".to_string(),
            "https://app.example.com".to_string(),
        ];
        let _layer = build_cors_layer(&origins);
        // Just verify it doesn't panic
    }
}
