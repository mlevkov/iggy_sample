//! High-level wrapper around the Iggy SDK client.
//!
//! This module provides a production-ready wrapper around the Iggy client with:
//!
//! - **Connection Resilience**: Automatic reconnection with exponential backoff
//! - **Operation Timeouts**: All operations are bounded by configurable timeout
//! - **Health Monitoring**: Background health checks to detect connection issues early
//! - **Error Handling**: All operations return `AppResult` with descriptive errors
//! - **Observability**: Comprehensive tracing instrumentation for all operations
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    IggyClientWrapper                        │
//! │  ┌─────────────────┐  ┌──────────────────────────────────┐  │
//! │  │ Connection      │  │ Operations                       │  │
//! │  │ Manager         │  │ - send_event/send_events_batch   │  │
//! │  │ - connect()     │  │ - poll_messages                  │  │
//! │  │ - reconnect()   │  │ - create/delete stream/topic     │  │
//! │  │ - health_check()│  │ - list streams/topics            │  │
//! │  └─────────────────┘  └──────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Module Structure
//!
//! - `connection` - Connection state tracking for reconnection coordination
//! - `params` - Parameter types like `PollParams`
//! - `helpers` - Utility functions for identifier conversion and jitter
//! - `scopeguard` - RAII guard for cleanup on drop
//!
//! # Connection Resilience (two layers)
//!
//! The SDK itself provides transport-level reconnection for connection-string
//! clients (enabled by default, unlimited retries at ~1s intervals). During an
//! outage most operations therefore block inside the SDK rather than erroring,
//! and surface here as timeouts. On top of that, this wrapper adds:
//! - Per-operation timeouts (so requests never block unboundedly in the SDK)
//! - Circuit breaking driven by classified connection errors AND timeouts
//! - Live health probes (`health_check`) that keep the connection state, and
//!   therefore `/health` and `/ready`, truthful
//! - App-level reconnection with exponential backoff + jitter as a second
//!   line of defense, for error classes that escape the SDK's internal retry
//!   (see `helpers::classify_iggy_error`)
//!
//! # Example
//!
//! ```rust,ignore
//! let client = IggyClientWrapper::new(config).await?;
//! client.initialize_defaults().await?;
//!
//! // Send events - automatic reconnection on failure
//! client.send_event_default(&event, None).await?;
//! ```

mod circuit_breaker;
mod connection;
mod helpers;
mod params;
mod scopeguard;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use iggy::prelude::*;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{debug, error, info, instrument, warn};

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::models::Event;

// Re-exports for public API
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
pub use connection::ConnectionState;
pub use helpers::{rand_jitter, to_identifier};
pub use params::PollParams;

// Internal-only: the error classifier's fallback contract (must be a
// NON-connection variant) is too easy to violate to expose publicly.
use helpers::classify_iggy_error;

// =============================================================================
// Constants
// =============================================================================

/// Jitter percentage for exponential backoff (±20%).
///
/// Adding randomness to retry delays prevents the "thundering herd" problem
/// where many clients reconnect simultaneously after a server restart.
const BACKOFF_JITTER_PERCENT: f64 = 0.2;

/// Minimum delay between reconnection attempts in milliseconds.
///
/// Even with exponential backoff, we never retry faster than this to avoid
/// overwhelming a recovering server.
const MIN_RECONNECT_DELAY_MS: u64 = 100;

/// Maximum backoff exponent. `max_ms` caps the delay long before this in any
/// realistic configuration; the clamp only exists so the shift can never
/// overflow with an unbounded attempt counter.
const MAX_BACKOFF_EXPONENT: u32 = 32;

/// Compute the reconnection delay for `attempt` (1-indexed): exponential
/// backoff from `base_ms`, jittered by ±[`BACKOFF_JITTER_PERCENT`], clamped to
/// `[MIN_RECONNECT_DELAY_MS, max_ms]`.
///
/// `jitter_unit` is a random value in `[0, 1)` (see [`rand_jitter`]). The cap
/// is applied AFTER jitter so the delay never exceeds the configured maximum,
/// and all arithmetic saturates so an unbounded attempt counter (infinite
/// retries are the default) cannot overflow.
fn backoff_delay_ms(attempt: u32, base_ms: u64, max_ms: u64, jitter_unit: f64) -> u64 {
    let exponent = attempt.saturating_sub(1).min(MAX_BACKOFF_EXPONENT);
    let raw = base_ms.saturating_mul(2u64.saturating_pow(exponent));
    let capped = raw.min(max_ms);

    let jitter = (capped as f64 * BACKOFF_JITTER_PERCENT * (jitter_unit * 2.0 - 1.0)) as i64;
    let jittered = (capped as i64).saturating_add(jitter).max(0) as u64;

    jittered.clamp(MIN_RECONNECT_DELAY_MS.min(max_ms), max_ms)
}

// =============================================================================
// IggyClientWrapper
// =============================================================================

/// Production-ready wrapper around the Iggy client.
///
/// Provides automatic reconnection, health monitoring, circuit breaker protection,
/// and consistent error handling for all Iggy operations. Thread-safe and designed
/// for concurrent access.
///
/// # Circuit Breaker
///
/// The client includes a circuit breaker that prevents request pile-up during outages:
/// - **Closed** (normal): All requests pass through
/// - **Open** (failing): Requests fail fast without attempting the operation
/// - **Half-Open** (recovery): Limited requests allowed to test if service recovered
///
/// # Performance Considerations
///
/// The client uses `RwLock<IggyClient>` for thread-safe reconnection support.
/// Most operations only need a read lock (concurrent reads allowed), but
/// reconnection requires a write lock which blocks all other operations.
///
/// ## Design Decision: tokio::sync::RwLock
///
/// We chose `tokio::sync::RwLock` over alternatives because:
/// - **Simplicity**: Standard async-aware lock with clear semantics
/// - **Correctness**: Prevents data races during reconnection
/// - **Sufficient for most use cases**: Contention only occurs during reconnection
///
/// ## Scaling Beyond 10k RPS
///
/// If profiling shows RwLock contention as a bottleneck, consider:
/// - `parking_lot::RwLock` for better performance under contention
/// - `arc_swap::ArcSwap` for lock-free client swapping during reconnect
/// - Connection pool pattern with multiple `IggyClient` instances
/// - Message batching to reduce lock acquisition frequency
///
/// Profile first—premature optimization often adds complexity without benefit.
#[derive(Clone)]
pub struct IggyClientWrapper {
    /// The underlying Iggy client (behind RwLock for reconnection)
    /// See struct-level docs for performance considerations.
    client: Arc<RwLock<IggyClient>>,
    /// Application configuration
    config: Config,
    /// Connection state tracking
    state: Arc<ConnectionState>,
    /// Circuit breaker for fail-fast during outages
    circuit_breaker: Arc<CircuitBreaker>,
}

impl IggyClientWrapper {
    /// Create a new Iggy client wrapper from configuration.
    ///
    /// Establishes the initial connection to the Iggy server, bounded by
    /// `OPERATION_TIMEOUT_SECS`. The SDK's own transport-level reconnection
    /// (enabled by default for connection-string clients) would otherwise
    /// retry indefinitely and hang startup when the server is down.
    ///
    /// # Errors
    ///
    /// Returns `AppError::ConnectionFailed` if:
    /// - The connection string is invalid
    /// - The server is unreachable within the operation timeout
    /// - Authentication fails
    #[instrument(skip(config), fields(connection_string = %config.iggy_connection_string))]
    pub async fn new(config: Config) -> AppResult<Self> {
        info!("Initializing Iggy client");

        let client = IggyClient::from_connection_string(&config.iggy_connection_string)
            .map_err(|e| AppError::ConnectionFailed(e.to_string()))?;

        // Initialize circuit breaker from config
        let circuit_breaker_config = CircuitBreakerConfig::new(
            config.circuit_breaker_failure_threshold,
            config.circuit_breaker_success_threshold,
            config.circuit_breaker_open_duration,
        );

        let wrapper = Self {
            client: Arc::new(RwLock::new(client)),
            config,
            state: Arc::new(ConnectionState::new()),
            circuit_breaker: Arc::new(CircuitBreaker::new(circuit_breaker_config)),
        };

        let timeout = wrapper.config.operation_timeout;
        tokio::time::timeout(timeout, wrapper.connect())
            .await
            .map_err(|_| {
                AppError::ConnectionFailed(format!(
                    "Initial connection timed out after {:?}",
                    timeout
                ))
            })??;

        Ok(wrapper)
    }

    // =========================================================================
    // Connection Management
    // =========================================================================

    /// Connect to the Iggy server.
    ///
    /// This is called automatically during construction and reconnection.
    /// Direct calls are rarely needed.
    #[instrument(skip(self))]
    pub async fn connect(&self) -> AppResult<()> {
        let client = self.client.read().await;

        client
            .connect()
            .await
            .map_err(|e| AppError::ConnectionFailed(e.to_string()))?;

        self.state.set_connected(true);
        info!("Successfully connected to Iggy server");

        Ok(())
    }

    /// Check if the client is currently connected.
    ///
    /// Note: This reflects the last known state. Use `health_check()` for
    /// a live connectivity test.
    pub fn is_connected(&self) -> bool {
        self.state.is_connected()
    }

    /// Perform a live connectivity check against the Iggy server.
    ///
    /// Sends a `ping` bounded by the configured operation timeout and updates
    /// the tracked connection state with the result. This is what keeps
    /// `/health` and `/ready` truthful during an outage: the SDK's internal
    /// transport reconnection swallows most mid-operation failures, so without
    /// an active probe the connected flag would stay latched at its startup
    /// value.
    ///
    /// Called periodically by the background health-check task; safe to call
    /// from handlers as well.
    pub async fn health_check(&self) -> bool {
        let result = {
            let client = self.client.read().await;
            tokio::time::timeout(self.config.operation_timeout, client.ping()).await
        };

        let healthy = matches!(result, Ok(Ok(())));
        self.state.set_connected(healthy);
        crate::metrics::set_connection_status(healthy);
        if !healthy {
            debug!("Live health check failed: server did not answer ping in time");
        }
        healthy
    }

    /// Attempt to reconnect to the Iggy server with exponential backoff.
    ///
    /// This method is called automatically when operations fail due to connection issues.
    /// It implements exponential backoff with jitter to prevent thundering herd problems.
    ///
    /// # Concurrency
    ///
    /// If multiple tasks call reconnect simultaneously, only one will actually perform
    /// the reconnection. Others will wait efficiently using `Notify` (no busy-wait).
    ///
    /// # Returns
    ///
    /// - `Ok(())` if reconnection succeeds
    /// - `Err(AppError::ConnectionFailed)` if max attempts exceeded or reconnection fails
    #[instrument(skip(self))]
    async fn reconnect(&self) -> AppResult<()> {
        // Prevent multiple concurrent reconnection attempts
        if !self.state.start_reconnecting() {
            debug!("Reconnection already in progress, waiting for completion...");

            // Wait efficiently for the other reconnection to complete
            self.state.wait_for_reconnection().await;

            return if self.state.is_connected() {
                debug!("Reconnection by another task succeeded");
                Ok(())
            } else {
                Err(AppError::ConnectionFailed(
                    "Reconnection failed (attempted by another task)".to_string(),
                ))
            };
        }

        // Guard to ensure we always mark reconnection as complete
        let _guard = scopeguard::guard(|| {
            self.state.stop_reconnecting();
        });

        self.state.set_connected(false);
        // Start each reconnection session with a fresh attempt counter so a
        // previously exhausted session cannot poison this one into failing
        // immediately (and so the backoff exponent reflects THIS session).
        self.state.reset_attempts();
        let max_attempts = self.config.max_reconnect_attempts;

        loop {
            let attempt = self.state.increment_attempts();
            crate::metrics::record_reconnect_attempt();

            // Check if we've exceeded max attempts (0 = infinite)
            if max_attempts > 0 && attempt > max_attempts {
                error!(
                    attempts = attempt - 1,
                    max_attempts, "Maximum reconnection attempts exceeded"
                );
                return Err(AppError::ConnectionFailed(format!(
                    "Failed to reconnect after {} attempts",
                    max_attempts
                )));
            }

            let final_delay = backoff_delay_ms(
                attempt,
                self.config.reconnect_base_delay.as_millis() as u64,
                self.config.reconnect_max_delay.as_millis() as u64,
                rand_jitter(),
            );

            warn!(
                attempt,
                delay_ms = final_delay,
                "Attempting to reconnect to Iggy server"
            );

            sleep(Duration::from_millis(final_delay)).await;

            // Create a new client instance for reconnection
            match IggyClient::from_connection_string(&self.config.iggy_connection_string) {
                Ok(new_client) => {
                    // Bound the connect: the SDK's internal reconnection would
                    // otherwise retry inside connect() indefinitely.
                    match tokio::time::timeout(self.config.operation_timeout, new_client.connect())
                        .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            warn!(attempt, error = %e, "Reconnection attempt failed");
                            continue;
                        }
                        Err(_) => {
                            warn!(attempt, "Reconnection attempt timed out");
                            continue;
                        }
                    }

                    // Successfully reconnected - swap the client and shut down
                    // the old one. Without shutdown() the old client's detached
                    // heartbeat task keeps running and can re-establish a
                    // zombie connection to the server (SDK 0.10 behavior).
                    let mut client_guard = self.client.write().await;
                    let old_client = std::mem::replace(&mut *client_guard, new_client);
                    drop(client_guard);
                    if let Err(e) = old_client.shutdown().await {
                        debug!(error = %e, "Old client shutdown returned an error (ignored)");
                    }

                    self.state.set_connected(true);
                    info!(attempt, "Successfully reconnected to Iggy server");
                    return Ok(());
                }
                Err(e) => {
                    warn!(attempt, error = %e, "Failed to create new client");
                    continue;
                }
            }
        }
    }

    /// Reconnect, bounded by the operation timeout.
    ///
    /// Used on the request path so a handler never hangs indefinitely behind
    /// an unbounded reconnection session (the default configuration retries
    /// forever). If the bound elapses, the reconnection attempt is aborted
    /// (its scope guard releases the in-progress flag) and the next failing
    /// request will start a fresh session.
    async fn reconnect_bounded(&self) -> AppResult<()> {
        let timeout = self.config.operation_timeout;
        tokio::time::timeout(timeout, self.reconnect())
            .await
            .map_err(|_| {
                AppError::ConnectionFailed(format!(
                    "Reconnection did not complete within {:?}",
                    timeout
                ))
            })?
    }

    /// Execute an operation with automatic reconnection on connection failure.
    ///
    /// This is the core resilience mechanism. Features:
    /// - **Circuit Breaker**: Fail fast when service is known to be unavailable
    /// - **Timeout**: All operations are bounded by `config.operation_timeout`
    /// - **Retry**: On connection failure, attempts reconnect and retries once
    ///
    /// # Circuit Breaker Integration
    ///
    /// Before attempting the operation, the circuit breaker is checked:
    /// - If **Open**: Returns `CircuitOpen` error immediately (fail fast)
    /// - If **Closed** or **HalfOpen**: Proceeds with the operation
    ///
    /// After the operation, the circuit breaker is updated:
    /// - Success: Records success (may close circuit if in HalfOpen)
    /// - Failure: Records failure (may open circuit if threshold exceeded)
    ///
    /// # Timeout vs Disconnection
    ///
    /// The function differentiates between:
    /// - **Connection errors**: Trigger reconnection and retry
    /// - **Timeouts**: Only trigger reconnection if connection state shows disconnected;
    ///   otherwise return timeout error (slow operation != broken connection)
    ///
    /// This prevents unnecessary reconnection attempts under high latency conditions.
    async fn with_reconnect<F, Fut, T>(&self, operation: F) -> AppResult<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = AppResult<T>>,
    {
        // Check circuit breaker before attempting operation
        if !self.circuit_breaker.allow_request().await {
            let state = self.circuit_breaker.state().await;
            return Err(AppError::CircuitOpen(format!(
                "Circuit breaker is {} - service temporarily unavailable",
                state
            )));
        }

        let timeout_duration = self.config.operation_timeout;

        // First attempt with timeout
        let result = tokio::time::timeout(timeout_duration, operation()).await;

        match result {
            Ok(Ok(value)) => {
                self.circuit_breaker.record_success().await;
                Ok(value)
            }
            Ok(Err(e)) if Self::is_connection_error(&e) => {
                self.circuit_breaker.record_failure().await;
                warn!(error = %e, "Operation failed due to connection error, attempting reconnect");
                self.reconnect_bounded().await?;
                self.retry_once(&operation).await
            }
            Ok(Err(e)) => {
                // Non-connection error - don't record as circuit breaker failure
                Err(e)
            }
            Err(_) => {
                // Timeout on first attempt. The SDK's internal transport
                // reconnection swallows most mid-operation connection failures
                // into blocking retries, so a timeout is often the only outage
                // signal we get - record it as a circuit-breaker failure.
                self.circuit_breaker.record_failure().await;

                // Only reconnect if we have evidence the connection is
                // actually lost (the background health check drives this
                // flag via live pings). A timeout alone could just be a slow
                // operation.
                if !self.state.is_connected() {
                    warn!(
                        timeout = ?timeout_duration,
                        "Operation timed out and connection state is disconnected, attempting reconnect"
                    );
                    self.reconnect_bounded().await?;
                    self.retry_once(&operation).await
                } else {
                    debug!(
                        timeout = ?timeout_duration,
                        "Operation timed out but connection state is healthy, not reconnecting"
                    );
                    Err(AppError::OperationTimeout(format!(
                        "Operation timed out after {:?}",
                        timeout_duration
                    )))
                }
            }
        }
    }

    /// Single post-reconnect retry with timeout and circuit-breaker
    /// bookkeeping. Shared by both reconnect paths of [`Self::with_reconnect`].
    async fn retry_once<F, Fut, T>(&self, operation: &F) -> AppResult<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = AppResult<T>>,
    {
        let timeout_duration = self.config.operation_timeout;
        match tokio::time::timeout(timeout_duration, operation()).await {
            Ok(Ok(value)) => {
                self.circuit_breaker.record_success().await;
                Ok(value)
            }
            Ok(Err(e)) => {
                if Self::is_connection_error(&e) {
                    self.circuit_breaker.record_failure().await;
                }
                Err(e)
            }
            Err(_) => {
                self.circuit_breaker.record_failure().await;
                Err(AppError::OperationTimeout(format!(
                    "Operation timed out after {:?} on retry",
                    timeout_duration
                )))
            }
        }
    }

    /// Check if an error is a connection-related error that warrants reconnection.
    ///
    /// Uses explicit pattern matching on error variants rather than string matching,
    /// which is more reliable and maintainable. Any error indicating connection
    /// issues should trigger a reconnection attempt.
    fn is_connection_error(error: &AppError) -> bool {
        matches!(
            error,
            AppError::ConnectionFailed(_)
                | AppError::Disconnected(_)
                | AppError::ConnectionReset(_)
        )
    }

    // =========================================================================
    // Stream & Topic Initialization
    // =========================================================================

    /// Ensure the specified stream exists, creating it if necessary.
    ///
    /// This is idempotent - calling it multiple times with the same name
    /// will not create duplicate streams.
    #[instrument(skip(self))]
    pub async fn ensure_stream(&self, name: &str) -> AppResult<()> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;
            let stream_id = to_identifier(name, "stream")?;

            match client.get_stream(&stream_id).await {
                Ok(Some(_)) => {
                    debug!(stream = name, "Stream already exists");
                    Ok(())
                }
                Ok(None) => {
                    info!(stream = name, "Creating stream");
                    match client.create_stream(name).await {
                        Ok(_) => Ok(()),
                        // Lost a creation race (e.g. two replicas starting
                        // simultaneously) - the stream exists, which is all
                        // this method guarantees.
                        Err(IggyError::StreamNameAlreadyExists(_)) => {
                            debug!(stream = name, "Stream was created concurrently");
                            Ok(())
                        }
                        Err(e) => Err(classify_iggy_error(e, AppError::StreamError)),
                    }
                }
                // Don't swallow the lookup error: a transient or permission
                // failure here is not "stream missing", and blindly creating
                // would surface a misleading already-exists error instead.
                Err(e) => Err(classify_iggy_error(e, AppError::StreamError)),
            }
        })
        .await
    }

    /// Ensure the specified topic exists within a stream, creating it if necessary.
    ///
    /// This is idempotent - calling it multiple times with the same parameters
    /// will not create duplicate topics.
    #[instrument(skip(self))]
    pub async fn ensure_topic(&self, stream: &str, topic: &str, partitions: u32) -> AppResult<()> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;
            let stream_id = to_identifier(stream, "stream")?;
            let topic_id = to_identifier(topic, "topic")?;

            match client.get_topic(&stream_id, &topic_id).await {
                Ok(Some(_)) => {
                    debug!(stream, topic, "Topic already exists");
                    Ok(())
                }
                Ok(None) => {
                    info!(stream, topic, partitions, "Creating topic");
                    match client
                        .create_topic(
                            &stream_id,
                            topic,
                            partitions,
                            Default::default(),
                            None,
                            IggyExpiry::NeverExpire,
                            MaxTopicSize::Unlimited,
                        )
                        .await
                    {
                        Ok(_) => Ok(()),
                        // Lost a creation race - the topic exists, which is
                        // all this method guarantees.
                        Err(IggyError::TopicNameAlreadyExists(_, _)) => {
                            debug!(stream, topic, "Topic was created concurrently");
                            Ok(())
                        }
                        Err(e) => Err(classify_iggy_error(e, AppError::TopicError)),
                    }
                }
                // Don't swallow the lookup error (see ensure_stream).
                Err(e) => Err(classify_iggy_error(e, AppError::TopicError)),
            }
        })
        .await
    }

    /// Initialize default stream and topic from configuration.
    ///
    /// Call this after creating the wrapper to ensure the default
    /// stream and topic exist before sending messages.
    #[instrument(skip(self))]
    pub async fn initialize_defaults(&self) -> AppResult<()> {
        self.ensure_stream(&self.config.default_stream).await?;
        self.ensure_topic(
            &self.config.default_stream,
            &self.config.default_topic,
            self.config.topic_partitions,
        )
        .await?;
        Ok(())
    }

    // =========================================================================
    // Message Sending
    // =========================================================================

    /// Send an event to the specified stream and topic.
    ///
    /// # Arguments
    ///
    /// * `stream` - Target stream name
    /// * `topic` - Target topic name
    /// * `event` - The event to send
    /// * `partition_key` - Optional key for consistent partition routing
    ///
    /// # Partition Routing
    ///
    /// - If `partition_key` is provided, messages with the same key always go
    ///   to the same partition (useful for ordered processing per entity)
    /// - If `None`, messages are distributed using balanced partitioning
    #[instrument(skip(self, event), fields(event_id = %event.id, event_type = %event.event_type))]
    pub async fn send_event(
        &self,
        stream: &str,
        topic: &str,
        event: &Event,
        partition_key: Option<&str>,
    ) -> AppResult<()> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;

            let payload = serde_json::to_string(event)?;
            let message =
                IggyMessage::from_str(&payload).map_err(|e| AppError::SendError(e.to_string()))?;

            let stream_id = to_identifier(stream, "stream")?;
            let topic_id = to_identifier(topic, "topic")?;

            let partitioning = match partition_key {
                Some(key) => Partitioning::messages_key_str(key)
                    .map_err(|e| AppError::SendError(e.to_string()))?,
                None => Partitioning::balanced(),
            };

            let mut messages = vec![message];
            client
                .send_messages(&stream_id, &topic_id, &partitioning, &mut messages)
                .await
                .map_err(|e| classify_iggy_error(e, AppError::SendError))?;

            debug!(event_id = %event.id, "Event sent successfully");
            Ok(())
        })
        .await
    }

    /// Send an event to the default stream and topic.
    ///
    /// Convenience method that uses the configured default stream and topic.
    pub async fn send_event_default(
        &self,
        event: &Event,
        partition_key: Option<&str>,
    ) -> AppResult<()> {
        self.send_event(
            &self.config.default_stream,
            &self.config.default_topic,
            event,
            partition_key,
        )
        .await
    }

    /// Send multiple events in a single batch to the specified stream and topic.
    ///
    /// This is significantly more efficient than sending messages individually
    /// as it uses a single network round-trip for all messages.
    ///
    /// # Performance
    ///
    /// - Single network call for all messages
    /// - Reduced serialization overhead
    /// - Better throughput for high-volume scenarios
    ///
    /// # Arguments
    ///
    /// * `stream` - Target stream name
    /// * `topic` - Target topic name
    /// * `events` - Slice of events to send (empty slice is a no-op)
    /// * `partition_key` - Optional key for consistent partition routing
    #[instrument(skip(self, events), fields(batch_size = events.len()))]
    pub async fn send_events_batch(
        &self,
        stream: &str,
        topic: &str,
        events: &[Event],
        partition_key: Option<&str>,
    ) -> AppResult<()> {
        if events.is_empty() {
            return Ok(());
        }

        self.with_reconnect(|| async {
            let client = self.client.read().await;

            let stream_id = to_identifier(stream, "stream")?;
            let topic_id = to_identifier(topic, "topic")?;

            let partitioning = match partition_key {
                Some(key) => Partitioning::messages_key_str(key)
                    .map_err(|e| AppError::SendError(e.to_string()))?,
                None => Partitioning::balanced(),
            };

            // Convert all events to messages in one pass
            let mut messages: Vec<IggyMessage> = events
                .iter()
                .map(|event| {
                    let payload = serde_json::to_string(event)?;
                    IggyMessage::from_str(&payload).map_err(|e| AppError::SendError(e.to_string()))
                })
                .collect::<AppResult<Vec<_>>>()?;

            // Send all messages in a single network call
            client
                .send_messages(&stream_id, &topic_id, &partitioning, &mut messages)
                .await
                .map_err(|e| classify_iggy_error(e, AppError::SendError))?;

            debug!(batch_size = events.len(), "Batch sent successfully");
            Ok(())
        })
        .await
    }

    /// Send multiple events in a batch to the default stream and topic.
    pub async fn send_events_batch_default(
        &self,
        events: &[Event],
        partition_key: Option<&str>,
    ) -> AppResult<()> {
        self.send_events_batch(
            &self.config.default_stream,
            &self.config.default_topic,
            events,
            partition_key,
        )
        .await
    }

    // =========================================================================
    // Message Polling
    // =========================================================================

    /// Poll messages from a topic using a parameter struct.
    ///
    /// This is the primary polling method. Parameters are grouped in `PollParams`
    /// for a cleaner API and easier extension.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let params = PollParams::new(1, 1)
    ///     .with_count(50)
    ///     .with_auto_commit(true);
    ///
    /// let messages = client.poll_messages("stream", "topic", params).await?;
    /// ```
    ///
    /// # Consumer Offsets
    ///
    /// Polling uses a standalone consumer (not a consumer group). Each unique
    /// `consumer_id` maintains its own offset. Use the same ID across restarts
    /// to resume from the last committed position.
    #[instrument(skip(self, params), fields(partition_id = params.partition_id, consumer_id = params.consumer_id))]
    pub async fn poll_messages(
        &self,
        stream: &str,
        topic: &str,
        params: PollParams,
    ) -> AppResult<PolledMessages> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;

            let stream_id = to_identifier(stream, "stream")?;
            let topic_id = to_identifier(topic, "topic")?;

            let consumer =
                Consumer::new(Identifier::numeric(params.consumer_id).map_err(|_| {
                    AppError::BadRequest(format!("Invalid consumer ID: {}", params.consumer_id))
                })?);

            let strategy = match params.offset {
                Some(off) => PollingStrategy::offset(off),
                None => PollingStrategy::next(),
            };

            let messages = client
                .poll_messages(
                    &stream_id,
                    &topic_id,
                    Some(params.partition_id),
                    &consumer,
                    &strategy,
                    params.count,
                    params.auto_commit,
                )
                .await
                .map_err(|e| classify_iggy_error(e, AppError::PollError))?;

            debug!(
                count = messages.messages.len(),
                stream, topic, "Messages polled successfully"
            );

            Ok(messages)
        })
        .await
    }

    /// Poll messages from the default stream and topic.
    pub async fn poll_messages_default(&self, params: PollParams) -> AppResult<PolledMessages> {
        self.poll_messages(
            &self.config.default_stream,
            &self.config.default_topic,
            params,
        )
        .await
    }

    // =========================================================================
    // Stream & Topic Management
    // =========================================================================

    /// Get stream information.
    #[instrument(skip(self))]
    pub async fn get_stream(&self, name: &str) -> AppResult<StreamDetails> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;
            let stream_id = to_identifier(name, "stream")?;

            client
                .get_stream(&stream_id)
                .await
                .map_err(|e| classify_iggy_error(e, AppError::StreamError))?
                .ok_or_else(|| AppError::NotFound(format!("Stream '{}' not found", name)))
        })
        .await
    }

    /// Get topic information.
    #[instrument(skip(self))]
    pub async fn get_topic(&self, stream: &str, topic: &str) -> AppResult<TopicDetails> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;
            let stream_id = to_identifier(stream, "stream")?;
            let topic_id = to_identifier(topic, "topic")?;

            client
                .get_topic(&stream_id, &topic_id)
                .await
                .map_err(|e| classify_iggy_error(e, AppError::TopicError))?
                .ok_or_else(|| {
                    AppError::NotFound(format!(
                        "Topic '{}' in stream '{}' not found",
                        topic, stream
                    ))
                })
        })
        .await
    }

    /// List all streams.
    #[instrument(skip(self))]
    pub async fn list_streams(&self) -> AppResult<Vec<Stream>> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;

            client
                .get_streams()
                .await
                .map_err(|e| classify_iggy_error(e, AppError::StreamError))
        })
        .await
    }

    /// List all topics in a stream.
    #[instrument(skip(self))]
    pub async fn list_topics(&self, stream: &str) -> AppResult<Vec<Topic>> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;
            let stream_id = to_identifier(stream, "stream")?;

            client
                .get_topics(&stream_id)
                .await
                .map_err(|e| classify_iggy_error(e, AppError::TopicError))
        })
        .await
    }

    /// Create a new stream.
    #[instrument(skip(self))]
    pub async fn create_stream(&self, name: &str) -> AppResult<()> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;

            client
                .create_stream(name)
                .await
                .map_err(|e| classify_iggy_error(e, AppError::StreamError))?;

            info!(stream = name, "Stream created");
            Ok(())
        })
        .await
    }

    /// Create a new topic.
    #[instrument(skip(self))]
    pub async fn create_topic(&self, stream: &str, topic: &str, partitions: u32) -> AppResult<()> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;
            let stream_id = to_identifier(stream, "stream")?;

            client
                .create_topic(
                    &stream_id,
                    topic,
                    partitions,
                    Default::default(),
                    None,
                    IggyExpiry::NeverExpire,
                    MaxTopicSize::Unlimited,
                )
                .await
                .map_err(|e| classify_iggy_error(e, AppError::TopicError))?;

            info!(stream, topic, partitions, "Topic created");
            Ok(())
        })
        .await
    }

    /// Delete a stream.
    ///
    /// **Warning**: This permanently deletes the stream and all its topics/messages.
    #[instrument(skip(self))]
    pub async fn delete_stream(&self, name: &str) -> AppResult<()> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;
            let stream_id = to_identifier(name, "stream")?;

            client
                .delete_stream(&stream_id)
                .await
                .map_err(|e| classify_iggy_error(e, AppError::StreamError))?;

            warn!(stream = name, "Stream deleted");
            Ok(())
        })
        .await
    }

    /// Delete a topic.
    ///
    /// **Warning**: This permanently deletes the topic and all its messages.
    #[instrument(skip(self))]
    pub async fn delete_topic(&self, stream: &str, topic: &str) -> AppResult<()> {
        self.with_reconnect(|| async {
            let client = self.client.read().await;
            let stream_id = to_identifier(stream, "stream")?;
            let topic_id = to_identifier(topic, "topic")?;

            client
                .delete_topic(&stream_id, &topic_id)
                .await
                .map_err(|e| classify_iggy_error(e, AppError::TopicError))?;

            warn!(stream, topic, "Topic deleted");
            Ok(())
        })
        .await
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    /// Get the default stream name from config.
    pub fn default_stream(&self) -> &str {
        &self.config.default_stream
    }

    /// Get the default topic name from config.
    pub fn default_topic(&self) -> &str {
        &self.config.default_topic
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get the current circuit breaker state.
    pub async fn circuit_breaker_state(&self) -> CircuitState {
        self.circuit_breaker.state().await
    }

    /// Get circuit breaker metrics.
    ///
    /// Returns a tuple of (times_opened, requests_rejected).
    pub fn circuit_breaker_metrics(&self) -> (u32, u64) {
        (
            self.circuit_breaker.times_opened(),
            self.circuit_breaker.requests_rejected(),
        )
    }

    /// Force close the circuit breaker (for manual recovery).
    pub async fn force_close_circuit(&self) {
        self.circuit_breaker.force_close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_first_attempt_is_base_delay() {
        // Zero jitter offset happens at jitter_unit = 0.5
        assert_eq!(backoff_delay_ms(1, 1000, 30_000, 0.5), 1000);
    }

    #[test]
    fn test_backoff_grows_exponentially() {
        assert_eq!(backoff_delay_ms(2, 1000, 30_000, 0.5), 2000);
        assert_eq!(backoff_delay_ms(3, 1000, 30_000, 0.5), 4000);
        assert_eq!(backoff_delay_ms(4, 1000, 30_000, 0.5), 8000);
    }

    #[test]
    fn test_backoff_never_exceeds_max_even_with_max_jitter() {
        // jitter_unit ~1.0 gives the maximum positive jitter; the cap is
        // applied after jitter so the delay must not exceed max_ms.
        for attempt in 1..=100 {
            let delay = backoff_delay_ms(attempt, 1000, 30_000, 0.999_999);
            assert!(
                delay <= 30_000,
                "attempt {} produced delay {} > max",
                attempt,
                delay
            );
        }
    }

    #[test]
    fn test_backoff_respects_minimum_floor() {
        // Tiny base with maximum negative jitter must not go below the floor.
        let delay = backoff_delay_ms(1, 1, 30_000, 0.0);
        assert!(delay >= 100, "delay {} below MIN_RECONNECT_DELAY_MS", delay);
    }

    #[test]
    fn test_backoff_does_not_overflow_at_huge_attempt_counts() {
        // Infinite retries are the default; attempt counts far past the
        // overflow point (~attempt 56 with base 1000) must stay clamped.
        for attempt in [56, 64, 1000, u32::MAX] {
            let delay = backoff_delay_ms(attempt, 1000, 30_000, 0.999_999);
            assert!(
                (100..=30_000).contains(&delay),
                "attempt {attempt} -> {delay}"
            );
        }
    }

    #[test]
    fn test_is_connection_error_connection_failed() {
        let error = AppError::ConnectionFailed("test".to_string());
        assert!(IggyClientWrapper::is_connection_error(&error));
    }

    #[test]
    fn test_is_connection_error_disconnected() {
        let error = AppError::Disconnected("connection lost".to_string());
        assert!(IggyClientWrapper::is_connection_error(&error));
    }

    #[test]
    fn test_is_connection_error_connection_reset() {
        let error = AppError::ConnectionReset("reset by peer".to_string());
        assert!(IggyClientWrapper::is_connection_error(&error));
    }

    #[test]
    fn test_is_connection_error_unrelated_errors() {
        // These errors should NOT trigger reconnection
        let test_cases = vec![
            AppError::BadRequest("invalid input".to_string()),
            AppError::NotFound("resource missing".to_string()),
            AppError::Internal("internal error".to_string()),
            AppError::StreamError("stream issue".to_string()),
            AppError::TopicError("topic issue".to_string()),
            AppError::SendError("send failed".to_string()),
            AppError::PollError("poll failed".to_string()),
            AppError::ConfigError("config issue".to_string()),
            AppError::OperationTimeout("timed out".to_string()),
            AppError::CircuitOpen("circuit open".to_string()),
        ];

        for error in test_cases {
            assert!(
                !IggyClientWrapper::is_connection_error(&error),
                "Error {:?} should not be treated as connection error",
                error
            );
        }
    }
}
