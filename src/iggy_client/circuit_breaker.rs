//! Circuit breaker pattern for connection resilience.
//!
//! The circuit breaker prevents request pile-up during outages by failing fast
//! when the system is known to be unavailable. This reduces load on the failing
//! service and improves recovery time.
//!
//! # States
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────┐
//! │                        Circuit Breaker                             │
//! │                                                                    │
//! │  ┌─────────┐    failures ≥ threshold    ┌─────────┐               │
//! │  │  Closed │ ────────────────────────► │  Open   │               │
//! │  │ (Normal)│                            │ (Fail   │               │
//! │  └────┬────┘                            │  Fast)  │               │
//! │       │ ▲                               └────┬────┘               │
//! │       │ │                                    │                    │
//! │       │ │ success                            │ timeout expires    │
//! │       │ │                                    ▼                    │
//! │       │ │                            ┌───────────────┐            │
//! │       │ └─────────────────────────── │   HalfOpen    │            │
//! │       │              success         │ (Probe with   │            │
//! │       │                              │  one request) │            │
//! │       │                              └───────┬───────┘            │
//! │       │                                      │                    │
//! │       │                                      │ failure            │
//! │       │                                      ▼                    │
//! │       │                              ┌─────────┐                  │
//! │       └───────────────────────────── │  Open   │ ◄────────────────┘
//! │                    reset             └─────────┘                  │
//! └────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Configuration
//!
//! - `failure_threshold`: Number of consecutive failures before opening
//! - `success_threshold`: Number of consecutive successes in half-open to close
//! - `open_duration`: How long to stay open before trying half-open
//!
//! # Usage
//!
//! ```rust,ignore
//! let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
//!
//! // Check if request should be allowed
//! if !cb.allow_request() {
//!     return Err(AppError::CircuitOpen);
//! }
//!
//! // Execute the operation
//! match operation().await {
//!     Ok(result) => {
//!         cb.record_success();
//!         Ok(result)
//!     }
//!     Err(e) => {
//!         cb.record_failure();
//!         Err(e)
//!     }
//! }
//! ```

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - all requests pass through.
    Closed,
    /// Failing fast - all requests are rejected immediately.
    Open,
    /// Testing recovery - allowing limited requests through.
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitState::Closed => write!(f, "closed"),
            CircuitState::Open => write!(f, "open"),
            CircuitState::HalfOpen => write!(f, "half-open"),
        }
    }
}

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Number of consecutive successes in half-open state to close the circuit.
    pub success_threshold: u32,
    /// How long to stay in open state before transitioning to half-open.
    pub open_duration: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 2,
            open_duration: Duration::from_secs(30),
        }
    }
}

impl CircuitBreakerConfig {
    /// Create a new circuit breaker configuration.
    pub fn new(failure_threshold: u32, success_threshold: u32, open_duration: Duration) -> Self {
        Self {
            failure_threshold,
            success_threshold,
            open_duration,
        }
    }
}

/// Internal state for the circuit breaker.
struct CircuitBreakerState {
    /// Current circuit state.
    state: CircuitState,
    /// When the circuit was opened (for timeout calculation).
    opened_at: Option<Instant>,
    /// Number of consecutive failures (in closed state).
    consecutive_failures: u32,
    /// Number of consecutive successes (in half-open state).
    consecutive_successes: u32,
}

impl CircuitBreakerState {
    fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            opened_at: None,
            consecutive_failures: 0,
            consecutive_successes: 0,
        }
    }
}

/// Thread-safe circuit breaker implementation.
///
/// Prevents cascading failures by failing fast when a service is unavailable.
/// Uses RwLock internally for thread-safe state management.
pub struct CircuitBreaker {
    /// Configuration parameters.
    config: CircuitBreakerConfig,
    /// Internal state protected by RwLock.
    state: RwLock<CircuitBreakerState>,
    /// Total number of times the circuit has been opened (for metrics).
    times_opened: AtomicU32,
    /// Total number of requests rejected due to open circuit (for metrics).
    requests_rejected: AtomicU64,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: RwLock::new(CircuitBreakerState::new()),
            times_opened: AtomicU32::new(0),
            requests_rejected: AtomicU64::new(0),
        }
    }

    /// Check if a request should be allowed through the circuit breaker.
    ///
    /// Returns `true` if the request can proceed, `false` if it should be rejected.
    ///
    /// # State Transitions
    ///
    /// - **Closed**: Always allows requests
    /// - **Open**: Rejects requests; transitions to HalfOpen after timeout
    /// - **HalfOpen**: Allows requests (for probing)
    pub async fn allow_request(&self) -> bool {
        // First, check with a read lock for the common case
        {
            let state = self.state.read().await;
            match state.state {
                CircuitState::Closed => return true,
                CircuitState::HalfOpen => return true,
                CircuitState::Open => {
                    // Check if timeout has expired
                    if let Some(opened_at) = state.opened_at
                        && opened_at.elapsed() < self.config.open_duration
                    {
                        self.requests_rejected.fetch_add(1, Ordering::Relaxed);
                        return false;
                    }
                    // Timeout expired - need to transition to half-open
                }
            }
        }

        // Need write lock to transition from Open to HalfOpen
        let mut state = self.state.write().await;

        // Re-check state in case another task already transitioned
        if state.state == CircuitState::Open {
            if let Some(opened_at) = state.opened_at
                && opened_at.elapsed() >= self.config.open_duration
            {
                state.state = CircuitState::HalfOpen;
                state.consecutive_successes = 0;
                info!("Circuit breaker transitioning from Open to HalfOpen");
                return true;
            }
            self.requests_rejected.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        true
    }

    /// Record a successful operation.
    ///
    /// In HalfOpen state, consecutive successes can close the circuit.
    pub async fn record_success(&self) {
        let mut state = self.state.write().await;

        match state.state {
            CircuitState::Closed => {
                // Reset failure counter on success
                state.consecutive_failures = 0;
            }
            CircuitState::HalfOpen => {
                state.consecutive_successes += 1;
                debug!(
                    consecutive_successes = state.consecutive_successes,
                    threshold = self.config.success_threshold,
                    "Circuit breaker recorded success in HalfOpen state"
                );

                if state.consecutive_successes >= self.config.success_threshold {
                    state.state = CircuitState::Closed;
                    state.opened_at = None;
                    state.consecutive_failures = 0;
                    info!("Circuit breaker closed after successful recovery");
                }
            }
            CircuitState::Open => {
                // Shouldn't happen - requests are rejected in Open state
                warn!("Unexpected success recorded in Open state");
            }
        }
    }

    /// Record a failed operation.
    ///
    /// In Closed state, consecutive failures can open the circuit.
    /// In HalfOpen state, any failure reopens the circuit.
    pub async fn record_failure(&self) {
        let mut state = self.state.write().await;

        match state.state {
            CircuitState::Closed => {
                state.consecutive_failures += 1;
                debug!(
                    consecutive_failures = state.consecutive_failures,
                    threshold = self.config.failure_threshold,
                    "Circuit breaker recorded failure"
                );

                if state.consecutive_failures >= self.config.failure_threshold {
                    state.state = CircuitState::Open;
                    state.opened_at = Some(Instant::now());
                    self.times_opened.fetch_add(1, Ordering::Relaxed);
                    warn!(
                        failures = state.consecutive_failures,
                        open_duration = ?self.config.open_duration,
                        "Circuit breaker opened due to consecutive failures"
                    );
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open state reopens the circuit
                state.state = CircuitState::Open;
                state.opened_at = Some(Instant::now());
                state.consecutive_successes = 0;
                self.times_opened.fetch_add(1, Ordering::Relaxed);
                warn!("Circuit breaker reopened after failure in HalfOpen state");
            }
            CircuitState::Open => {
                // Already open, refresh the timer
                state.opened_at = Some(Instant::now());
            }
        }
    }

    /// Get the current circuit state.
    pub async fn state(&self) -> CircuitState {
        self.state.read().await.state
    }

    /// Get the number of times the circuit has been opened.
    pub fn times_opened(&self) -> u32 {
        self.times_opened.load(Ordering::Relaxed)
    }

    /// Get the number of requests rejected due to open circuit.
    pub fn requests_rejected(&self) -> u64 {
        self.requests_rejected.load(Ordering::Relaxed)
    }

    /// Force the circuit to close (for testing or manual recovery).
    pub async fn force_close(&self) {
        let mut state = self.state.write().await;
        state.state = CircuitState::Closed;
        state.opened_at = None;
        state.consecutive_failures = 0;
        state.consecutive_successes = 0;
        info!("Circuit breaker forcibly closed");
    }

    /// Force the circuit to open (for testing or manual intervention).
    pub async fn force_open(&self) {
        let mut state = self.state.write().await;
        state.state = CircuitState::Open;
        state.opened_at = Some(Instant::now());
        self.times_opened.fetch_add(1, Ordering::Relaxed);
        warn!("Circuit breaker forcibly opened");
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::default();
        assert_eq!(cb.state().await, CircuitState::Closed);
        assert!(cb.allow_request().await);
    }

    #[tokio::test]
    async fn test_circuit_opens_after_threshold_failures() {
        let config = CircuitBreakerConfig::new(3, 2, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        // Record failures below threshold
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        // One more failure should open the circuit
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
        assert_eq!(cb.times_opened(), 1);
    }

    #[tokio::test]
    async fn test_circuit_rejects_when_open() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Requests should be rejected
        assert!(!cb.allow_request().await);
        assert_eq!(cb.requests_rejected(), 1);
    }

    #[tokio::test]
    async fn test_circuit_transitions_to_half_open() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_millis(10));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Wait for timeout
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Should allow request and transition to half-open
        assert!(cb.allow_request().await);
        assert_eq!(cb.state().await, CircuitState::HalfOpen);
    }

    #[tokio::test]
    async fn test_circuit_closes_after_success_in_half_open() {
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_millis(10));
        let cb = CircuitBreaker::new(config);

        // Open the circuit
        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Transition to half-open
        assert!(cb.allow_request().await);
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        // Record successes
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_circuit_reopens_on_failure_in_half_open() {
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_millis(10));
        let cb = CircuitBreaker::new(config);

        // Open the circuit
        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Transition to half-open
        assert!(cb.allow_request().await);
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        // Failure should reopen
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
        assert_eq!(cb.times_opened(), 2);
    }

    #[tokio::test]
    async fn test_success_resets_failure_counter() {
        let config = CircuitBreakerConfig::new(3, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        cb.record_failure().await;
        // Success should reset the counter
        cb.record_success().await;

        // Now we need 3 more failures to open
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn test_force_close() {
        let cb = CircuitBreaker::default();
        cb.record_failure().await;
        cb.record_failure().await;
        cb.record_failure().await;
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        cb.force_close().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
        assert!(cb.allow_request().await);
    }

    #[tokio::test]
    async fn test_force_open() {
        let cb = CircuitBreaker::default();
        assert_eq!(cb.state().await, CircuitState::Closed);

        cb.force_open().await;
        assert_eq!(cb.state().await, CircuitState::Open);
        assert!(!cb.allow_request().await);
    }
}
