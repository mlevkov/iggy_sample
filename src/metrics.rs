//! Prometheus metrics for application observability.
//!
//! This module provides Prometheus-compatible metrics for monitoring the application.
//! Metrics are exposed via a dedicated HTTP endpoint (default: `/metrics`).
//!
//! # Available Metrics
//!
//! ## Counters
//! - `iggy_messages_sent_total` - Total messages sent (with labels: stream, topic, status)
//! - `iggy_messages_polled_total` - Total messages polled (with labels: stream, topic)
//! - `iggy_connection_reconnects_total` - Total reconnection attempts
//! - `iggy_circuit_breaker_opens_total` - Times the circuit breaker opened
//! - `iggy_circuit_breaker_rejections_total` - Requests rejected by circuit breaker
//!
//! ## Histograms
//! - `iggy_request_duration_seconds` - Request duration (with labels: endpoint, method, status)
//! - `iggy_send_duration_seconds` - Message send duration
//! - `iggy_poll_duration_seconds` - Message poll duration
//!
//! ## Gauges
//! - `iggy_connection_status` - Current connection status (1 = connected, 0 = disconnected)
//! - `iggy_circuit_breaker_state` - Circuit breaker state (0 = closed, 1 = half-open, 2 = open)
//!
//! # Usage
//!
//! ```rust,ignore
//! use iggy_sample::metrics::{init_metrics, record_message_sent, record_request_duration};
//!
//! // Initialize metrics (call once at startup)
//! init_metrics();
//!
//! // Record metrics in handlers
//! record_message_sent("my-stream", "my-topic", "success");
//! record_request_duration("/messages", "POST", "200", 0.045);
//! ```

use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::SocketAddr;
use tracing::{error, info};

/// Metric names as constants for consistency.
pub mod names {
    pub const MESSAGES_SENT_TOTAL: &str = "iggy_messages_sent_total";
    pub const MESSAGES_POLLED_TOTAL: &str = "iggy_messages_polled_total";
    pub const CONNECTION_RECONNECTS_TOTAL: &str = "iggy_connection_reconnects_total";
    pub const CIRCUIT_BREAKER_OPENS_TOTAL: &str = "iggy_circuit_breaker_opens_total";
    pub const CIRCUIT_BREAKER_REJECTIONS_TOTAL: &str = "iggy_circuit_breaker_rejections_total";
    pub const REQUEST_DURATION_SECONDS: &str = "iggy_request_duration_seconds";
    pub const SEND_DURATION_SECONDS: &str = "iggy_send_duration_seconds";
    pub const POLL_DURATION_SECONDS: &str = "iggy_poll_duration_seconds";
    pub const CONNECTION_STATUS: &str = "iggy_connection_status";
    pub const CIRCUIT_BREAKER_STATE: &str = "iggy_circuit_breaker_state";
}

/// Initialize the Prometheus metrics exporter.
///
/// This sets up metric descriptions and starts the Prometheus HTTP listener
/// on the specified address (default: 0.0.0.0:9090).
///
/// # Arguments
///
/// * `metrics_addr` - Address for the Prometheus metrics endpoint
///
/// # Returns
///
/// `Ok(())` if initialization succeeds, `Err` with message otherwise.
pub fn init_metrics(metrics_addr: SocketAddr) -> Result<(), String> {
    // Set up Prometheus exporter
    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()
        .map_err(|e| format!("Failed to install Prometheus exporter: {e}"))?;

    // Describe all metrics
    describe_counter!(
        names::MESSAGES_SENT_TOTAL,
        "Total number of messages sent to Iggy"
    );
    describe_counter!(
        names::MESSAGES_POLLED_TOTAL,
        "Total number of messages polled from Iggy"
    );
    describe_counter!(
        names::CONNECTION_RECONNECTS_TOTAL,
        "Total number of connection reconnection attempts"
    );
    describe_counter!(
        names::CIRCUIT_BREAKER_OPENS_TOTAL,
        "Total number of times the circuit breaker opened"
    );
    describe_counter!(
        names::CIRCUIT_BREAKER_REJECTIONS_TOTAL,
        "Total number of requests rejected by circuit breaker"
    );

    describe_histogram!(
        names::REQUEST_DURATION_SECONDS,
        "HTTP request duration in seconds"
    );
    describe_histogram!(
        names::SEND_DURATION_SECONDS,
        "Message send operation duration in seconds"
    );
    describe_histogram!(
        names::POLL_DURATION_SECONDS,
        "Message poll operation duration in seconds"
    );

    describe_gauge!(
        names::CONNECTION_STATUS,
        "Iggy connection status (1 = connected, 0 = disconnected)"
    );
    describe_gauge!(
        names::CIRCUIT_BREAKER_STATE,
        "Circuit breaker state (0 = closed, 1 = half-open, 2 = open)"
    );

    info!(addr = %metrics_addr, "Prometheus metrics endpoint started");
    Ok(())
}

/// Try to initialize metrics, logging any errors but not failing.
///
/// This is useful for cases where metrics are optional.
pub fn try_init_metrics(metrics_addr: SocketAddr) {
    if let Err(e) = init_metrics(metrics_addr) {
        error!(error = %e, "Failed to initialize metrics, continuing without metrics");
    }
}

// =============================================================================
// Counter Recording Functions
// =============================================================================

/// Record a message sent event.
pub fn record_message_sent(stream: &str, topic: &str, status: &str) {
    counter!(names::MESSAGES_SENT_TOTAL, "stream" => stream.to_string(), "topic" => topic.to_string(), "status" => status.to_string())
        .increment(1);
}

/// Record messages sent in batch.
pub fn record_messages_sent_batch(stream: &str, topic: &str, status: &str, count: u64) {
    counter!(names::MESSAGES_SENT_TOTAL, "stream" => stream.to_string(), "topic" => topic.to_string(), "status" => status.to_string())
        .increment(count);
}

/// Record messages polled.
pub fn record_messages_polled(stream: &str, topic: &str, count: u64) {
    counter!(names::MESSAGES_POLLED_TOTAL, "stream" => stream.to_string(), "topic" => topic.to_string())
        .increment(count);
}

/// Record a reconnection attempt.
pub fn record_reconnect_attempt() {
    counter!(names::CONNECTION_RECONNECTS_TOTAL).increment(1);
}

/// Record circuit breaker opening.
pub fn record_circuit_breaker_open() {
    counter!(names::CIRCUIT_BREAKER_OPENS_TOTAL).increment(1);
}

/// Record circuit breaker rejection.
pub fn record_circuit_breaker_rejection() {
    counter!(names::CIRCUIT_BREAKER_REJECTIONS_TOTAL).increment(1);
}

// =============================================================================
// Histogram Recording Functions
// =============================================================================

/// Record HTTP request duration.
pub fn record_request_duration(endpoint: &str, method: &str, status: &str, duration_secs: f64) {
    histogram!(names::REQUEST_DURATION_SECONDS, "endpoint" => endpoint.to_string(), "method" => method.to_string(), "status" => status.to_string())
        .record(duration_secs);
}

/// Record message send duration.
pub fn record_send_duration(stream: &str, topic: &str, duration_secs: f64) {
    histogram!(names::SEND_DURATION_SECONDS, "stream" => stream.to_string(), "topic" => topic.to_string())
        .record(duration_secs);
}

/// Record message poll duration.
pub fn record_poll_duration(stream: &str, topic: &str, duration_secs: f64) {
    histogram!(names::POLL_DURATION_SECONDS, "stream" => stream.to_string(), "topic" => topic.to_string())
        .record(duration_secs);
}

// =============================================================================
// Gauge Recording Functions
// =============================================================================

/// Update connection status gauge.
pub fn set_connection_status(connected: bool) {
    gauge!(names::CONNECTION_STATUS).set(if connected { 1.0 } else { 0.0 });
}

/// Update circuit breaker state gauge.
///
/// States: 0 = closed, 1 = half-open, 2 = open
pub fn set_circuit_breaker_state(state: u8) {
    gauge!(names::CIRCUIT_BREAKER_STATE).set(f64::from(state));
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests verify the functions don't panic.
    // Full metrics testing requires integration tests with a Prometheus scraper.

    #[test]
    fn test_record_message_sent() {
        // Should not panic even without metrics initialized
        record_message_sent("test-stream", "test-topic", "success");
    }

    #[test]
    fn test_record_messages_polled() {
        record_messages_polled("test-stream", "test-topic", 10);
    }

    #[test]
    fn test_record_request_duration() {
        record_request_duration("/messages", "POST", "200", 0.1);
    }

    #[test]
    fn test_set_connection_status() {
        set_connection_status(true);
        set_connection_status(false);
    }

    #[test]
    fn test_set_circuit_breaker_state() {
        set_circuit_breaker_state(0); // closed
        set_circuit_breaker_state(1); // half-open
        set_circuit_breaker_state(2); // open
    }
}
