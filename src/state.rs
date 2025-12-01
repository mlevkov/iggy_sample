//! Shared application state for Axum handlers.
//!
//! This module provides thread-safe, clonable state that is shared across
//! all request handlers. It includes:
//!
//! - **Services**: Producer and consumer for message operations
//! - **Client**: Iggy client wrapper for low-level operations
//! - **Configuration**: Runtime configuration access
//! - **Stats Cache**: Background-refreshed statistics for `/stats` endpoint
//!
//! # Thread Safety
//!
//! All state components are wrapped in `Arc` or use interior mutability
//! patterns that are safe for concurrent access from multiple handlers.
//!
//! # Structured Concurrency
//!
//! Background tasks are managed using `tokio_util::task::TaskTracker` and
//! `CancellationToken` for proper lifecycle management. Call `shutdown()`
//! to gracefully stop all background tasks before application exit.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, info, trace, warn};

use crate::config::Config;
use crate::iggy_client::IggyClientWrapper;
use crate::services::{ConsumerService, ProducerService};

/// Cached statistics for efficient `/stats` endpoint.
///
/// Statistics are computed in a background task and cached to avoid
/// expensive Iggy queries on every request.
#[derive(Debug, Clone, Default)]
pub struct CachedStats {
    /// Number of active streams
    pub streams_count: u32,
    /// Number of active topics across all streams
    pub topics_count: u32,
    /// Total messages across all topics
    pub total_messages: u64,
    /// Total data size in bytes
    pub total_size_bytes: u64,
    /// When these stats were last updated
    pub last_updated: Option<Instant>,
}

impl CachedStats {
    /// Check if the cache is stale (hasn't been updated in `ttl`).
    pub fn is_stale(&self, ttl: Duration) -> bool {
        match self.last_updated {
            Some(updated) => updated.elapsed() > ttl,
            None => true,
        }
    }
}

/// Shared application state for Axum handlers.
///
/// This struct is cloned for each request handler. All internal data
/// is wrapped in `Arc` for efficient sharing.
///
/// # Lifecycle
///
/// Background tasks are spawned when the state is created. Call `shutdown()`
/// before dropping to ensure clean task termination:
///
/// ```rust,ignore
/// let state = AppState::new(iggy_client, config);
/// // ... use state ...
/// state.shutdown().await;  // Wait for background tasks to complete
/// ```
#[derive(Clone)]
pub struct AppState {
    /// Iggy client wrapper for low-level operations
    pub iggy_client: IggyClientWrapper,
    /// Producer service for sending messages
    pub producer: ProducerService,
    /// Consumer service for receiving messages
    pub consumer: ConsumerService,
    /// Timestamp when the application started
    pub started_at: Instant,
    /// Application configuration
    pub config: Arc<Config>,
    /// Cached statistics (refreshed in background)
    stats_cache: Arc<RwLock<CachedStats>>,
    /// Tracks spawned background tasks for graceful shutdown
    task_tracker: TaskTracker,
    /// Cancellation token for signaling background tasks to stop
    cancellation_token: CancellationToken,
}

impl AppState {
    /// Create new application state from an Iggy client and configuration.
    ///
    /// # Arguments
    ///
    /// * `iggy_client` - Initialized Iggy client wrapper
    /// * `config` - Application configuration
    ///
    /// # Background Tasks
    ///
    /// This spawns a background task that periodically refreshes the stats cache.
    /// The task runs at the interval specified by `config.stats_cache_ttl`.
    /// Call `shutdown()` to gracefully terminate background tasks.
    pub fn new(iggy_client: IggyClientWrapper, config: Config) -> Self {
        let producer = ProducerService::new(iggy_client.clone());
        let consumer = ConsumerService::new(iggy_client.clone());
        let config = Arc::new(config);
        let stats_cache = Arc::new(RwLock::new(CachedStats::default()));
        let task_tracker = TaskTracker::new();
        let cancellation_token = CancellationToken::new();

        let state = Self {
            iggy_client,
            producer,
            consumer,
            started_at: Instant::now(),
            config,
            stats_cache,
            task_tracker,
            cancellation_token,
        };

        // Spawn background tasks
        state.spawn_stats_refresh_task();
        state.spawn_health_check_task();

        state
    }

    /// Get the current cached statistics.
    ///
    /// Returns the cached stats without blocking. If the cache is empty,
    /// returns default values.
    pub async fn cached_stats(&self) -> CachedStats {
        self.stats_cache.read().await.clone()
    }

    /// Force refresh the stats cache.
    ///
    /// This is called by the background task, but can also be called
    /// manually if needed.
    pub async fn refresh_stats(&self) {
        match self.compute_stats().await {
            Ok(stats) => {
                let mut cache = self.stats_cache.write().await;
                *cache = stats;
                trace!("Stats cache refreshed successfully");
            }
            Err(e) => {
                warn!(error = %e, "Failed to refresh stats cache");
            }
        }
    }

    /// Compute current statistics from Iggy.
    async fn compute_stats(&self) -> Result<CachedStats, crate::error::AppError> {
        compute_stats_from_client(&self.iggy_client).await
    }

    /// Spawn the background stats refresh task.
    ///
    /// The task is tracked by `task_tracker` and respects `cancellation_token`
    /// for graceful shutdown.
    ///
    /// # Implementation Note
    ///
    /// We clone only the fields needed by the task (iggy_client, stats_cache)
    /// rather than the entire AppState to minimize memory overhead.
    fn spawn_stats_refresh_task(&self) {
        let iggy_client = self.iggy_client.clone();
        let stats_cache = self.stats_cache.clone();
        let ttl = self.config.stats_cache_ttl;
        let cancel = self.cancellation_token.clone();

        self.task_tracker.spawn(async move {
            // Initial refresh
            if let Err(e) = refresh_stats_impl(&iggy_client, &stats_cache).await {
                warn!(error = %e, "Initial stats refresh failed");
            }

            // Periodic refresh with cancellation support
            let mut ticker = interval(ttl);
            ticker.tick().await; // Skip the first immediate tick

            loop {
                tokio::select! {
                    biased; // Check cancellation first

                    _ = cancel.cancelled() => {
                        debug!("Stats refresh task received cancellation signal");
                        break;
                    }
                    _ = ticker.tick() => {
                        if let Err(e) = refresh_stats_impl(&iggy_client, &stats_cache).await {
                            warn!(error = %e, "Stats refresh failed");
                        }
                    }
                }
            }

            debug!("Stats refresh task shutting down");
        });
    }

    /// Spawn a background health check task.
    ///
    /// Periodically checks the Iggy connection status and logs warnings
    /// if the connection is lost. This provides early detection of issues
    /// before user requests fail.
    fn spawn_health_check_task(&self) {
        let iggy_client = self.iggy_client.clone();
        let interval_duration = self.config.health_check_interval;
        let cancel = self.cancellation_token.clone();

        self.task_tracker.spawn(async move {
            let mut ticker = interval(interval_duration);
            ticker.tick().await; // Skip first immediate tick

            loop {
                tokio::select! {
                    biased;

                    _ = cancel.cancelled() => {
                        debug!("Health check task received cancellation signal");
                        break;
                    }
                    _ = ticker.tick() => {
                        let connected = iggy_client.is_connected();
                        if !connected {
                            warn!("Health check: Iggy connection is down");
                        } else {
                            trace!("Health check: Iggy connection OK");
                        }
                    }
                }
            }

            debug!("Health check task shutting down");
        });
    }

    /// Gracefully shutdown all background tasks.
    ///
    /// This method:
    /// 1. Signals all tasks to stop via cancellation token
    /// 2. Closes the task tracker (prevents new tasks)
    /// 3. Waits for all tasks to complete
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // In main.rs shutdown handler:
    /// state.shutdown().await;
    /// ```
    pub async fn shutdown(&self) {
        info!("Initiating graceful shutdown of background tasks");

        // Signal all tasks to stop
        self.cancellation_token.cancel();

        // Close the tracker - no new tasks can be spawned
        self.task_tracker.close();

        // Wait for all tasks to complete
        self.task_tracker.wait().await;

        info!("All background tasks have completed");
    }

    /// Get the application uptime in seconds.
    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Refresh stats implementation that works with individual components.
///
/// This is separate from `AppState::refresh_stats` to allow background tasks
/// to hold only the fields they need, rather than cloning the entire state.
async fn refresh_stats_impl(
    iggy_client: &IggyClientWrapper,
    stats_cache: &Arc<RwLock<CachedStats>>,
) -> Result<(), crate::error::AppError> {
    let stats = compute_stats_from_client(iggy_client).await?;

    let mut cache = stats_cache.write().await;
    *cache = stats;
    trace!("Stats cache refreshed successfully");

    Ok(())
}

/// Compute statistics from an Iggy client.
///
/// This is the shared implementation used by both `AppState::compute_stats()`
/// and the background refresh task.
async fn compute_stats_from_client(
    iggy_client: &IggyClientWrapper,
) -> Result<CachedStats, crate::error::AppError> {
    let streams = iggy_client.list_streams().await?;

    let mut topics_count = 0u32;
    let mut total_messages = 0u64;
    let mut total_size_bytes = 0u64;

    for stream in &streams {
        topics_count += stream.topics_count;
        total_messages += stream.messages_count;
        total_size_bytes += stream.size.as_bytes_u64();
    }

    // Use try_into to safely convert stream count, avoiding silent truncation
    let streams_count: u32 = streams.len().try_into().map_err(|_| {
        crate::error::AppError::Internal(format!("Stream count {} exceeds u32::MAX", streams.len()))
    })?;

    Ok(CachedStats {
        streams_count,
        topics_count,
        total_messages,
        total_size_bytes,
        last_updated: Some(Instant::now()),
    })
}
