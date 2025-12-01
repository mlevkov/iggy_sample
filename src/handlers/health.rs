//! Health, readiness, and statistics endpoints.
//!
//! # Endpoints
//!
//! - `GET /health` - Health check with Iggy connection status
//! - `GET /ready` - Kubernetes-compatible readiness probe
//! - `GET /stats` - Service statistics (uses background cache)
//!
//! # Health vs Readiness
//!
//! - **Health** (`/health`): Returns 200 even if degraded, includes details
//! - **Readiness** (`/ready`): Returns 503 if not ready to serve traffic
//!
//! # Statistics Caching
//!
//! The `/stats` endpoint uses a background-refreshed cache to avoid
//! expensive Iggy queries on every request. Cache TTL is configurable
//! via `STATS_CACHE_TTL_SECS`.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use tracing::instrument;

use crate::error::AppResult;
use crate::models::{HealthResponse, StatsResponse};
use crate::state::AppState;

/// Health check endpoint.
///
/// Returns service health status including Iggy connection state.
/// Always returns 200 OK with status details in the body.
///
/// # Response Body
///
/// ```json
/// {
///   "status": "healthy",
///   "iggy_connected": true,
///   "version": "0.1.0",
///   "timestamp": "2024-01-15T10:30:00Z"
/// }
/// ```
#[instrument(skip(state))]
pub async fn health_check(State(state): State<AppState>) -> Json<HealthResponse> {
    let iggy_connected = state.iggy_client.is_connected();

    Json(HealthResponse {
        status: if iggy_connected {
            "healthy"
        } else {
            "degraded"
        }
        .to_string(),
        iggy_connected,
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: Utc::now(),
    })
}

/// Readiness check endpoint for Kubernetes probes.
///
/// Returns 200 OK if the service is ready to accept traffic,
/// 503 Service Unavailable otherwise.
///
/// # Usage
///
/// Configure in Kubernetes:
/// ```yaml
/// readinessProbe:
///   httpGet:
///     path: /ready
///     port: 3000
///   initialDelaySeconds: 5
///   periodSeconds: 10
/// ```
#[instrument(skip(state))]
pub async fn readiness_check(State(state): State<AppState>) -> Result<StatusCode, StatusCode> {
    if state.iggy_client.is_connected() {
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

/// Statistics endpoint with cached data.
///
/// Returns service statistics from a background-refreshed cache.
/// This avoids expensive Iggy queries on every request.
///
/// # Response Body
///
/// ```json
/// {
///   "streams_count": 2,
///   "topics_count": 5,
///   "total_messages": 12345,
///   "total_size_bytes": 1048576,
///   "uptime_seconds": 3600
/// }
/// ```
///
/// # Caching
///
/// Statistics are refreshed in the background at the interval configured
/// by `STATS_CACHE_TTL_SECS` (default: 5 seconds).
#[instrument(skip(state))]
pub async fn stats(State(state): State<AppState>) -> AppResult<Json<StatsResponse>> {
    let cached = state.cached_stats().await;
    let ttl = state.config.stats_cache_ttl;

    // Calculate cache age and staleness
    let cache_age_seconds = cached
        .last_updated
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(u64::MAX); // Never updated = infinitely old

    let cache_stale = cached.is_stale(ttl);

    Ok(Json(StatsResponse {
        streams_count: cached.streams_count,
        topics_count: cached.topics_count,
        total_messages: cached.total_messages,
        total_size_bytes: cached.total_size_bytes,
        uptime_seconds: state.uptime_seconds(),
        cache_age_seconds,
        cache_stale,
    }))
}
