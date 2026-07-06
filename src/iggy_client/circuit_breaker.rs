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
//! │       │    success_threshold         │ (token-limited│            │
//! │       │    consecutive successes     │    probes)    │            │
//! │       │                              └───────┬───────┘            │
//! │       │                                      │                    │
//! │       │                                      │ failure            │
//! │       │                                      ▼                    │
//! │       │                              ┌─────────┐                  │
//! │       └───────────────────────────── │  Open   │ ◄────────────────┘
//! │         (after open_duration +       └─────────┘                  │
//! │          successful probes)                                       │
//! └────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Configuration
//!
//! - `failure_threshold`: Number of consecutive failures before opening
//! - `success_threshold`: Number of consecutive successes in half-open to close
//! - `open_duration`: How long to stay open before trying half-open
//!
//! # Half-Open Probe Limiting
//!
//! Entering half-open grants `success_threshold` probe tokens (minimum one);
//! each allowed request consumes one, and requests beyond the budget are
//! rejected so a recovering server never receives a thundering herd of
//! probes. Tokens re-grant after `open_duration` elapses in half-open,
//! guaranteeing the breaker cannot wedge if a probe's outcome is never
//! recorded. See [`CircuitBreaker::allow_request`].
//!
//! # Usage
//!
//! ```rust,ignore
//! let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
//!
//! // Check if request should be allowed
//! if !cb.allow_request().await {
//!     return Err(AppError::CircuitOpen);
//! }
//!
//! // Execute the operation. Only CONNECTION-CLASS outcomes feed the
//! // breaker (see `resilience::run_resilient` for the real composition):
//! match operation().await {
//!     Ok(result) => {
//!         cb.record_success().await;
//!         Ok(result)
//!     }
//!     Err(e) if is_connection_error(&e) => {
//!         cb.record_failure().await;
//!         Err(e)
//!     }
//!     // Other errors record neither; release any half-open probe token.
//!     Err(e) => {
//!         cb.release_probe().await;
//!         Err(e)
//!     }
//! }
//! ```

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::RwLock;
// tokio's Instant (a thin wrapper over std's) so breaker timing follows the
// pausable test clock; identical behavior in production.
use tokio::time::Instant;
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
    /// Probe tokens remaining in the current half-open window.
    half_open_probes_remaining: u32,
    /// When the current half-open probe window was granted (for re-grant).
    half_open_granted_at: Option<Instant>,
}

impl CircuitBreakerState {
    fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            opened_at: None,
            consecutive_failures: 0,
            consecutive_successes: 0,
            half_open_probes_remaining: 0,
            half_open_granted_at: None,
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
    /// - **HalfOpen**: Allows a token-limited number of probes (see below)
    ///
    /// # Half-open probe limiting
    ///
    /// Entering HalfOpen grants `success_threshold` probe tokens (at least
    /// one) — exactly the number of successes needed to close the circuit.
    /// Each allowed request consumes a token; with no tokens left, requests
    /// are rejected, which caps the probe load on a recovering server
    /// instead of letting every concurrent caller through at once.
    ///
    /// Tokens re-grant after `open_duration` elapses in HalfOpen. This is
    /// the anti-wedge guarantee: a probe whose outcome is never recorded
    /// (e.g. the operation failed with a non-connection error, which by
    /// design touches neither breaker counter) would otherwise leave the
    /// breaker half-open with zero tokens forever.
    pub async fn allow_request(&self) -> bool {
        // First, check with a read lock for the common cases that don't
        // mutate state (Closed passes, still-Open rejects).
        {
            let state = self.state.read().await;
            match state.state {
                CircuitState::Closed => return true,
                // Consuming a probe token requires the write lock below.
                CircuitState::HalfOpen => {}
                CircuitState::Open => {
                    // Check if timeout has expired
                    if let Some(opened_at) = state.opened_at
                        && opened_at.elapsed() < self.config.open_duration
                    {
                        return self.reject_request("open");
                    }
                    // Timeout expired - need to transition to half-open
                }
            }
        }

        // Write lock: Open -> HalfOpen transition, or HalfOpen token use.
        let mut state = self.state.write().await;

        match state.state {
            // Another task closed the circuit while we waited for the lock.
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(opened_at) = state.opened_at
                    && opened_at.elapsed() >= self.config.open_duration
                {
                    state.state = CircuitState::HalfOpen;
                    state.consecutive_successes = 0;
                    self.grant_probe_tokens(&mut state);
                    info!(
                        probes = state.half_open_probes_remaining,
                        "Circuit breaker transitioning from Open to HalfOpen"
                    );
                    // The transitioning caller takes the first probe token.
                    state.half_open_probes_remaining -= 1;
                    crate::metrics::set_circuit_breaker_state(1);
                    return true;
                }
                self.reject_request("open")
            }
            CircuitState::HalfOpen => {
                if state.half_open_probes_remaining == 0 {
                    // Re-grant after open_duration so leaked probes cannot
                    // wedge the breaker in HalfOpen (see doc above).
                    let window_expired = state
                        .half_open_granted_at
                        .is_none_or(|granted| granted.elapsed() >= self.config.open_duration);
                    if !window_expired {
                        debug!(
                            "Circuit breaker rejected request: half-open probe budget exhausted"
                        );
                        return self.reject_request("half_open");
                    }
                    // info: a full probe window elapsed without a recorded
                    // outcome - recovery is stalling, not progressing.
                    info!("Circuit breaker re-granted half-open probe tokens");
                    self.grant_probe_tokens(&mut state);
                }
                state.half_open_probes_remaining -= 1;
                true
            }
        }
    }

    /// Grant a fresh window of half-open probe tokens.
    ///
    /// `success_threshold` tokens (at least one, so a zero threshold cannot
    /// deadlock the breaker) — exactly enough probes to close the circuit
    /// if all of them succeed.
    fn grant_probe_tokens(&self, state: &mut CircuitBreakerState) {
        state.half_open_probes_remaining = self.config.success_threshold.max(1);
        state.half_open_granted_at = Some(Instant::now());
    }

    /// Record a rejection (counter + state-labeled metric) and return `false`.
    ///
    /// Single site for the bookkeeping so no rejection path can forget the
    /// metrics half; the label lets operators distinguish "circuit is open"
    /// from "half-open probe budget exhausted" — materially different
    /// situations.
    fn reject_request(&self, state_label: &'static str) -> bool {
        self.requests_rejected.fetch_add(1, Ordering::Relaxed);
        crate::metrics::record_circuit_breaker_rejection(state_label);
        false
    }

    /// Hand back a half-open probe token whose outcome was deliberately not
    /// recorded (non-connection errors touch neither breaker counter).
    ///
    /// Without this, probes completing with e.g. `NotFound` — a full
    /// round-trip proving transport health — would permanently consume
    /// tokens and starve recovery until the re-grant window. No-op outside
    /// HalfOpen; capped at the granted budget.
    pub(super) async fn release_probe(&self) {
        let mut state = self.state.write().await;
        if state.state == CircuitState::HalfOpen {
            let cap = self.config.success_threshold.max(1);
            if state.half_open_probes_remaining < cap {
                state.half_open_probes_remaining += 1;
                debug!("Circuit breaker released a half-open probe token (outcome not recorded)");
            }
        }
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
                    crate::metrics::set_circuit_breaker_state(0);
                    info!("Circuit breaker closed after successful recovery");
                }
            }
            CircuitState::Open => {
                // Reachable through legitimate interleavings: a half-open
                // probe (or its post-reconnect retry, which bypasses the
                // gate) can complete successfully after another probe's
                // failure reopened the circuit. The success is deliberately
                // discarded - recovery restarts from the next half-open
                // window's probes.
                debug!(
                    "Success recorded while Open (in-flight probe finished after reopen); discarded"
                );
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
                    self.open_now(&mut state);
                    warn!(
                        failures = state.consecutive_failures,
                        open_duration = ?self.config.open_duration,
                        "Circuit breaker opened due to consecutive failures"
                    );
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open state reopens the circuit
                state.consecutive_successes = 0;
                self.open_now(&mut state);
                warn!("Circuit breaker reopened after failure in HalfOpen state");
            }
            CircuitState::Open => {
                // Already open. Deliberately do NOT refresh opened_at:
                // straggler failures from in-flight requests would otherwise
                // extend the open window indefinitely and delay recovery.
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
        // Hygiene: half-open probe fields are re-granted on every HalfOpen
        // entry, but stale values should not outlive a manual reset.
        state.half_open_probes_remaining = 0;
        state.half_open_granted_at = None;
        crate::metrics::set_circuit_breaker_state(0);
        info!("Circuit breaker forcibly closed");
    }

    /// Force the circuit to open (for testing or manual intervention).
    ///
    /// A no-op when already Open, preserving the no-refresh policy for
    /// `opened_at` (see `record_failure`) and keeping `times_opened` honest.
    pub async fn force_open(&self) {
        let mut state = self.state.write().await;
        if state.state != CircuitState::Open {
            self.open_now(&mut state);
        }
        warn!("Circuit breaker forcibly opened");
    }

    /// Transition to Open, keeping internal counters and Prometheus metrics
    /// in lockstep. Shared by the threshold, half-open-failure, and forced
    /// open transitions so the gauge cannot drift from the atomics.
    fn open_now(&self, state: &mut CircuitBreakerState) {
        state.state = CircuitState::Open;
        state.opened_at = Some(Instant::now());
        self.times_opened.fetch_add(1, Ordering::Relaxed);
        crate::metrics::record_circuit_breaker_open();
        crate::metrics::set_circuit_breaker_state(2);
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

    #[tokio::test(start_paused = true)]
    async fn test_circuit_transitions_to_half_open() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_millis(10));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Advance the paused clock past the open window
        tokio::time::advance(Duration::from_millis(20)).await;

        // Should allow request and transition to half-open
        assert!(cb.allow_request().await);
        assert_eq!(cb.state().await, CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn test_circuit_closes_after_success_in_half_open() {
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_millis(10));
        let cb = CircuitBreaker::new(config);

        // Open the circuit
        cb.record_failure().await;
        tokio::time::advance(Duration::from_millis(20)).await;

        // Transition to half-open
        assert!(cb.allow_request().await);
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        // Record successes
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn test_circuit_reopens_on_failure_in_half_open() {
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_millis(10));
        let cb = CircuitBreaker::new(config);

        // Open the circuit
        cb.record_failure().await;
        tokio::time::advance(Duration::from_millis(20)).await;

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

    // =========================================================================
    // Half-open probe limiting (TD-2026-07-03)
    // =========================================================================

    #[tokio::test(start_paused = true)]
    async fn test_half_open_limits_probes_to_success_threshold() {
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
        tokio::time::advance(Duration::from_secs(30)).await;

        // success_threshold = 2 probe tokens: two callers pass, the third
        // is rejected instead of piling onto the recovering server.
        assert!(cb.allow_request().await);
        assert!(cb.allow_request().await);
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        let rejected_before = cb.requests_rejected();
        assert!(!cb.allow_request().await);
        assert_eq!(cb.requests_rejected(), rejected_before + 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_probe_tokens_regrant_after_open_duration() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        tokio::time::advance(Duration::from_secs(30)).await;

        // Single token consumed by the transitioning caller; its outcome is
        // never recorded (the leaked-probe case) so the breaker sits in
        // HalfOpen with zero tokens.
        assert!(cb.allow_request().await);
        assert!(!cb.allow_request().await);

        // Just below the window boundary the budget must stay exhausted -
        // an unconditional re-grant would defeat the probe cap entirely.
        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(!cb.allow_request().await);

        // The re-grant window keeps the breaker from wedging permanently.
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(cb.allow_request().await);
        assert_eq!(cb.state().await, CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_reentry_grants_fresh_probe_tokens() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        tokio::time::advance(Duration::from_secs(30)).await;

        // Consume the only token, then fail the probe: back to Open.
        assert!(cb.allow_request().await);
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Next half-open entry starts with a fresh token budget.
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(cb.allow_request().await);
        assert_eq!(cb.state().await, CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_recovery_within_probe_budget() {
        // The token budget equals success_threshold, so a healthy server
        // can be probed back to Closed without any rejection in between.
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        tokio::time::advance(Duration::from_secs(30)).await;

        assert!(cb.allow_request().await);
        cb.record_success().await;
        assert!(cb.allow_request().await);
        cb.record_success().await;

        assert_eq!(cb.state().await, CircuitState::Closed);
        assert!(cb.allow_request().await);
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_zero_success_threshold_still_grants_a_probe() {
        // Degenerate config: success_threshold = 0 must not deadlock the
        // breaker with a zero-token grant.
        let config = CircuitBreakerConfig::new(1, 0, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        tokio::time::advance(Duration::from_secs(30)).await;

        assert!(cb.allow_request().await);
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_concurrent_probes_admit_exactly_the_budget() {
        // Two callers race allow_request at the Open->HalfOpen boundary with
        // a single-token budget: exactly one may pass. On the deterministic
        // paused single-thread runtime, join! interleaves both futures
        // through the same write-lock protocol the production path uses.
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        tokio::time::advance(Duration::from_secs(30)).await;

        let (a, b) = tokio::join!(cb.allow_request(), cb.allow_request());
        assert!(
            a ^ b,
            "exactly one of two racing probes may pass, got ({a}, {b})"
        );
        assert_eq!(cb.state().await, CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn test_release_probe_returns_token_capped_at_budget() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        // No-op while Closed.
        cb.release_probe().await;
        assert!(cb.allow_request().await);

        cb.record_failure().await;
        tokio::time::advance(Duration::from_secs(30)).await;

        // Consume the only token, release it, and it must admit again.
        assert!(cb.allow_request().await);
        assert!(!cb.allow_request().await);
        cb.release_probe().await;
        assert!(cb.allow_request().await);

        // Releases never exceed the granted budget (single token here).
        cb.release_probe().await;
        cb.release_probe().await;
        assert!(cb.allow_request().await);
        assert!(
            !cb.allow_request().await,
            "budget cap must hold after over-release"
        );
    }

    #[tokio::test]
    async fn test_force_open_when_already_open_does_not_double_count() {
        let cb = CircuitBreaker::default();

        cb.force_open().await;
        cb.force_open().await;

        assert_eq!(cb.state().await, CircuitState::Open);
        assert_eq!(
            cb.times_opened(),
            1,
            "repeat force_open must not inflate the counter"
        );
    }
}
