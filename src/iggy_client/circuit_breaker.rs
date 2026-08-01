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
//! │       │ │                                    │ timeout expires    │
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
//! One consumed token can cover up to two server operations: the
//! post-reconnect retry in `resilience::run_resilient` deliberately does
//! not re-pass this gate (see that module's "Retry and the breaker gate").
//!
//! # Usage
//!
//! ```rust,ignore
//! let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
//!
//! // Check if request should be allowed. The gate is synchronous; only the
//! // guarded operation itself awaits.
//! if !cb.allow_request() {
//!     return Err(AppError::CircuitOpen);
//! }
//!
//! // Execute the operation. Only CONNECTION-CLASS outcomes feed the
//! // breaker (see `resilience::run_resilient` for the real composition):
//! match operation().await {
//!     Ok(result) => {
//!         cb.record_success();
//!         Ok(result)
//!     }
//!     Err(e) if is_connection_error(&e) => {
//!         cb.record_failure();
//!         Err(e)
//!     }
//!     // Other errors record neither; release any half-open probe token.
//!     Err(e) => {
//!         cb.release_probe();
//!         Err(e)
//!     }
//! }
//! ```

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

// tokio's Instant (a thin wrapper over std's) so breaker timing follows the
// pausable test clock; identical behavior in production.
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

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

impl CircuitState {
    /// Value for the `iggy_circuit_breaker_state` Prometheus gauge.
    ///
    /// The single mapping for what used to be four hand-written `0`/`1`/`2`
    /// literals; exhaustive with no `_` arm, so a new state cannot be added
    /// without deciding what operators should see.
    ///
    /// Deliberately not [`Display`], whose rendering is user-facing prose
    /// (`"half-open"`, hyphenated) and differs from the metric vocabulary.
    fn gauge(self) -> u8 {
        match self {
            CircuitState::Closed => 0,
            CircuitState::HalfOpen => 1,
            CircuitState::Open => 2,
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

/// Internal state, with each variant owning exactly the data that is
/// meaningful while the breaker is in it.
///
/// The flat struct this replaces kept every field in every state, so
/// `opened_at` was `Some` only in Open, the two half-open fields only in
/// HalfOpen, and `consecutive_failures` only in Closed — invariants maintained
/// by convention at each mutation site, and re-established by hand on every
/// transition. Payload variants make a stale field unrepresentable instead.
///
/// # Entering versus re-granting a probe window
///
/// Both paths refill `probes_remaining` and stamp `granted_at`, and they differ
/// on exactly one field:
///
/// | field | enter (Open -> HalfOpen) | re-grant (within HalfOpen) |
/// |---|---|---|
/// | `probes_remaining` | budget | budget |
/// | `granted_at` | now | now |
/// | `consecutive_successes` | **0** | **preserved** |
///
/// Entering is a new recovery attempt, so probes recorded against the previous
/// one must not count toward closing. A re-grant is the *same* attempt
/// continuing — its window simply expired with probes unaccounted for — so a
/// success already recorded still counts. Zeroing it there would silently
/// discard a probe success and force a full fresh run after every window
/// expiry. Pinned by `test_half_open_regrant_preserves_consecutive_successes`.
enum State {
    /// Normal operation. Consecutive failures accumulate toward the threshold.
    Closed { consecutive_failures: u32 },
    /// Failing fast until `open_duration` elapses from `opened_at`.
    Open { opened_at: Instant },
    /// Token-limited recovery probing.
    HalfOpen {
        probes_remaining: u32,
        granted_at: Instant,
        consecutive_successes: u32,
    },
}

impl State {
    /// The initial state: closed, with no failures recorded.
    fn initial() -> Self {
        State::Closed {
            consecutive_failures: 0,
        }
    }
}

/// Projection to the public, data-less state.
///
/// Deliberately exhaustive with no `_` arm, so adding an internal variant is a
/// compile error here rather than a silent mis-projection.
impl From<&State> for CircuitState {
    fn from(state: &State) -> Self {
        match state {
            State::Closed { .. } => CircuitState::Closed,
            State::Open { .. } => CircuitState::Open,
            State::HalfOpen { .. } => CircuitState::HalfOpen,
        }
    }
}

/// What a completed state transition should report, emitted once the state
/// guard has been released.
///
/// Logging reaches a global `tracing` subscriber and the metrics recorder takes
/// a registry shard lock and allocates a label key; neither belongs inside the
/// mutex that gates every Iggy operation. Everything carried here is a log line
/// or a **monotonic counter**, so reordering two concurrent transitions'
/// emissions is harmless.
///
/// The Prometheus state gauge is deliberately NOT here — see
/// [`CircuitBreaker::set_gauge`].
enum Effect {
    /// Nothing to report.
    None,
    /// Open -> HalfOpen. `probes` is the budget as granted, captured before the
    /// transitioning caller takes its own token.
    EnteredHalfOpen { probes: u32 },
    /// A request was turned away. The label separates "circuit is open" from
    /// "half-open probe budget exhausted" — materially different situations.
    Rejected {
        label: &'static str,
        budget_exhausted: bool,
    },
    /// A probe window elapsed with no recorded outcome and was re-granted.
    RegrantedProbes,
    /// A probe token was handed back because its outcome was never recorded.
    ReleasedProbe,
    /// A success landed in HalfOpen; `closed` if it reached the threshold.
    HalfOpenSuccess { successes: u32, closed: bool },
    /// A success arrived after another probe's failure had reopened the circuit.
    SuccessWhileOpen,
    /// A failure landed in Closed; `opened` if it reached the threshold.
    Failure { failures: u32, opened: bool },
    /// A failure in HalfOpen reopened the circuit.
    ReopenedFromHalfOpen,
}

/// Thread-safe circuit breaker implementation.
///
/// Prevents cascading failures by failing fast when a service is unavailable.
///
/// State is guarded by a synchronous [`std::sync::Mutex`] rather than an async
/// lock: every critical section is a short run of field updates that never
/// awaits, and a blocking guard is what lets a future RAII probe permit release
/// its token from `Drop`, which cannot await. No guard may be held across an
/// `.await` — `clippy::await_holding_lock` enforces that, and CI denies warnings.
pub struct CircuitBreaker {
    /// Configuration parameters.
    config: CircuitBreakerConfig,
    /// Internal state protected by a synchronous mutex.
    state: Mutex<State>,
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
            state: Mutex::new(State::initial()),
            times_opened: AtomicU32::new(0),
            requests_rejected: AtomicU64::new(0),
        }
    }

    /// Acquire the state lock, recovering the inner value if it was poisoned.
    ///
    /// Poisoning requires a panic while the guard is held. The release profile
    /// sets `panic = "abort"` (see `Cargo.toml`), so such a panic kills the
    /// process and this branch is unreachable in production; it is reachable
    /// only under `cargo test`, where silently continuing on half-updated state
    /// would surface as a baffling failure in some later test instead of at its
    /// origin — hence the `debug_assert!`.
    ///
    /// The `panicking()` guard is load-bearing: poisoning implies an in-flight
    /// unwind, and asserting during unwind double-panics straight to abort,
    /// destroying the very test report the assertion exists to sharpen.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| {
            if !std::thread::panicking() {
                debug_assert!(false, "circuit breaker state lock poisoned");
            }
            error!("Circuit breaker state lock poisoned; recovering inner state");
            poisoned.into_inner()
        })
    }

    /// Write the Prometheus state gauge. Called **while the guard is held**.
    ///
    /// This is the one emission that cannot be hoisted out of the critical
    /// section. The gauge is a last-writer-wins register, so if two racing
    /// transitions each computed a value and emitted it after releasing the
    /// guard, the older value could land second and the gauge would disagree
    /// with the breaker until the next transition — which, during a stalled
    /// recovery, may never come. Holding the guard makes the write order match
    /// the transition order by construction.
    ///
    /// The value itself comes from [`CircuitState::gauge`], the single
    /// exhaustive mapping that replaced four hand-written literals.
    fn set_gauge(&self, state: CircuitState) {
        crate::metrics::set_circuit_breaker_state(state.gauge());
    }

    /// Monotonic bookkeeping for an Open transition, emitted after the guard is
    /// released. Both are counters, so ordering against other transitions is
    /// immaterial.
    fn count_open(&self) {
        self.times_opened.fetch_add(1, Ordering::Relaxed);
        crate::metrics::record_circuit_breaker_open();
    }

    /// Emit the logs and monotonic counters for a completed transition.
    ///
    /// Always called with the state guard already dropped.
    fn emit(&self, effect: Effect) {
        match effect {
            Effect::None => {}
            Effect::EnteredHalfOpen { probes } => {
                info!(
                    probes,
                    "Circuit breaker transitioning from Open to HalfOpen"
                );
            }
            Effect::Rejected {
                label,
                budget_exhausted,
            } => {
                if budget_exhausted {
                    debug!("Circuit breaker rejected request: half-open probe budget exhausted");
                }
                self.requests_rejected.fetch_add(1, Ordering::Relaxed);
                crate::metrics::record_circuit_breaker_rejection(label);
            }
            Effect::RegrantedProbes => {
                // info: a full probe window elapsed without a recorded outcome
                // - recovery is stalling, not progressing.
                info!("Circuit breaker re-granted half-open probe tokens");
            }
            Effect::ReleasedProbe => {
                debug!("Circuit breaker released a half-open probe token (outcome not recorded)");
            }
            Effect::HalfOpenSuccess { successes, closed } => {
                debug!(
                    consecutive_successes = successes,
                    threshold = self.config.success_threshold,
                    "Circuit breaker recorded success in HalfOpen state"
                );
                if closed {
                    info!("Circuit breaker closed after successful recovery");
                }
            }
            Effect::SuccessWhileOpen => {
                debug!(
                    "Success recorded while Open (in-flight probe finished after reopen); discarded"
                );
            }
            Effect::Failure { failures, opened } => {
                debug!(
                    consecutive_failures = failures,
                    threshold = self.config.failure_threshold,
                    "Circuit breaker recorded failure"
                );
                if opened {
                    self.count_open();
                    warn!(
                        failures,
                        open_duration = ?self.config.open_duration,
                        "Circuit breaker opened due to consecutive failures"
                    );
                }
            }
            Effect::ReopenedFromHalfOpen => {
                self.count_open();
                warn!("Circuit breaker reopened after failure in HalfOpen state");
            }
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
    pub fn allow_request(&self) -> bool {
        // One exclusive acquisition covers every case. The former read-lock
        // fast path existed to avoid an async write lock on the common Closed
        // path; a synchronous mutex makes it unnecessary, and dropping it also
        // removes the read->write upgrade race that forced the state to be
        // re-matched after the second acquisition.
        // Single-tail: every arm yields (allowed, effect) rather than returning
        // early, so no path can skip the emit below.
        let (allowed, effect) = {
            let mut guard = self.lock();
            let budget = self.probe_budget();

            match &mut *guard {
                State::Closed { .. } => (true, Effect::None),
                State::Open { opened_at } => {
                    if opened_at.elapsed() >= self.config.open_duration {
                        // A NEW recovery attempt: the success count starts at
                        // zero (contrast the re-grant arm below). The
                        // transitioning caller takes the first of the granted
                        // tokens as it passes, so the window opens at
                        // budget - 1; probe_budget() floors at 1, so this
                        // cannot underflow.
                        *guard = State::HalfOpen {
                            probes_remaining: budget - 1,
                            granted_at: Instant::now(),
                            consecutive_successes: 0,
                        };
                        self.set_gauge(CircuitState::HalfOpen);
                        (true, Effect::EnteredHalfOpen { probes: budget })
                    } else {
                        (
                            false,
                            Effect::Rejected {
                                label: "open",
                                budget_exhausted: false,
                            },
                        )
                    }
                }
                State::HalfOpen {
                    probes_remaining,
                    granted_at,
                    ..
                } => {
                    if *probes_remaining > 0 {
                        *probes_remaining -= 1;
                        (true, Effect::None)
                    } else if granted_at.elapsed() >= self.config.open_duration {
                        // Re-grant after open_duration so leaked probes cannot
                        // wedge the breaker in HalfOpen (see doc above). This
                        // is the SAME recovery attempt continuing, so
                        // consecutive_successes is deliberately untouched.
                        *probes_remaining = budget - 1;
                        *granted_at = Instant::now();
                        (true, Effect::RegrantedProbes)
                    } else {
                        (
                            false,
                            Effect::Rejected {
                                label: "half_open",
                                budget_exhausted: true,
                            },
                        )
                    }
                }
            }
        };

        self.emit(effect);
        allowed
    }

    /// Half-open probe budget: `success_threshold`, floored at one so a
    /// zero threshold cannot deadlock the breaker. Single definition shared
    /// by the grant and release paths so the cap cannot drift.
    fn probe_budget(&self) -> u32 {
        self.config.success_threshold.max(1)
    }

    /// Test-only: enter HalfOpen with a full probe budget, spending nothing.
    ///
    /// The production path reaches HalfOpen only through [`Self::allow_request`],
    /// which consumes the transitioning caller's token on the way in. A race
    /// over the budget therefore cannot be staged through it — the setup call
    /// would take the very token under contention. Granting the window directly
    /// leaves the budget as the only contended resource.
    #[cfg(test)]
    fn force_half_open(&self) {
        let mut guard = self.lock();
        *guard = State::HalfOpen {
            probes_remaining: self.probe_budget(),
            granted_at: Instant::now(),
            consecutive_successes: 0,
        };
    }

    /// Hand back a half-open probe token whose outcome was deliberately not
    /// recorded (non-connection errors touch neither breaker counter).
    ///
    /// Without this, probes completing with e.g. `NotFound` — a full
    /// round-trip proving transport health — would permanently consume
    /// tokens and starve recovery until the re-grant window. No-op outside
    /// HalfOpen; capped at the granted budget.
    pub(super) fn release_probe(&self) {
        let effect = {
            let mut guard = self.lock();
            let budget = self.probe_budget();
            if let State::HalfOpen {
                probes_remaining, ..
            } = &mut *guard
                && *probes_remaining < budget
            {
                *probes_remaining += 1;
                Effect::ReleasedProbe
            } else {
                Effect::None
            }
        };
        self.emit(effect);
    }

    /// Record a successful operation.
    ///
    /// In HalfOpen state, consecutive successes can close the circuit.
    pub fn record_success(&self) {
        let effect = {
            let mut guard = self.lock();

            match &mut *guard {
                State::Closed {
                    consecutive_failures,
                } => {
                    // Reset failure counter on success
                    *consecutive_failures = 0;
                    Effect::None
                }
                State::HalfOpen {
                    consecutive_successes,
                    ..
                } => {
                    *consecutive_successes += 1;
                    let successes = *consecutive_successes;
                    let closed = successes >= self.config.success_threshold;
                    if closed {
                        *guard = State::Closed {
                            consecutive_failures: 0,
                        };
                        self.set_gauge(CircuitState::Closed);
                    }
                    Effect::HalfOpenSuccess { successes, closed }
                }
                State::Open { .. } => {
                    // Reachable through legitimate interleavings: a half-open
                    // probe (or its post-reconnect retry, which bypasses the
                    // gate) can complete successfully after another probe's
                    // failure reopened the circuit. The success is deliberately
                    // discarded - recovery restarts from the next half-open
                    // window's probes.
                    Effect::SuccessWhileOpen
                }
            }
        };
        self.emit(effect);
    }

    /// Record a failed operation.
    ///
    /// In Closed state, consecutive failures can open the circuit.
    /// In HalfOpen state, any failure reopens the circuit.
    pub fn record_failure(&self) {
        let effect = {
            let mut guard = self.lock();

            match &mut *guard {
                State::Closed {
                    consecutive_failures,
                } => {
                    *consecutive_failures += 1;
                    // Captured before open_now: the state enum this refactor is
                    // preparing for carries no failure count in its Open
                    // variant, so reading it after the transition would not
                    // survive that change.
                    let failures = *consecutive_failures;
                    let opened = failures >= self.config.failure_threshold;
                    if opened {
                        self.open_now(&mut guard);
                    }
                    Effect::Failure { failures, opened }
                }
                State::HalfOpen { .. } => {
                    // Any failure in half-open state reopens the circuit; the
                    // accumulated success count dies with the variant.
                    self.open_now(&mut guard);
                    Effect::ReopenedFromHalfOpen
                }
                State::Open { .. } => {
                    // Already open. Deliberately do NOT refresh opened_at:
                    // straggler failures from in-flight requests would otherwise
                    // extend the open window indefinitely and delay recovery.
                    Effect::None
                }
            }
        };
        self.emit(effect);
    }

    /// Get the current circuit state.
    pub fn state(&self) -> CircuitState {
        CircuitState::from(&*self.lock())
    }

    /// Get the number of times the circuit has been opened.
    ///
    /// Test-only. The counters are reported to operators through the
    /// Prometheus gauge and counters emitted on each transition, not through
    /// this getter; it exists so tests can assert on transitions directly.
    #[cfg(test)]
    pub fn times_opened(&self) -> u32 {
        self.times_opened.load(Ordering::Relaxed)
    }

    /// Get the number of requests rejected due to open circuit.
    ///
    /// Test-only, for the same reason as [`Self::times_opened`].
    #[cfg(test)]
    pub fn requests_rejected(&self) -> u64 {
        self.requests_rejected.load(Ordering::Relaxed)
    }

    /// Force the circuit to close.
    ///
    /// Test-only: there is no manual-recovery path into the breaker, so the
    /// only callers are tests staging a known state.
    #[cfg(test)]
    pub fn force_close(&self) {
        {
            let mut guard = self.lock();
            // One assignment replaces six field resets. The old flat struct
            // needed explicit hygiene so stale half-open values could not
            // outlive the reset; the enum drops them with the variant.
            *guard = State::Closed {
                consecutive_failures: 0,
            };
            self.set_gauge(CircuitState::Closed);
        }
        info!("Circuit breaker forcibly closed");
    }

    /// Force the circuit to open.
    ///
    /// Test-only, as [`Self::force_close`]. A no-op when already Open,
    /// preserving the no-refresh policy for `opened_at` (see `record_failure`)
    /// and keeping `times_opened` honest.
    #[cfg(test)]
    pub fn force_open(&self) {
        let opened = {
            let mut guard = self.lock();
            let changed = !matches!(*guard, State::Open { .. });
            if changed {
                self.open_now(&mut guard);
            }
            changed
        };
        if opened {
            self.count_open();
            warn!("Circuit breaker forcibly opened");
        }
    }

    /// Transition to Open under the state guard.
    ///
    /// Shared by the threshold, half-open-failure and forced-open paths so the
    /// gauge cannot drift from the state: [`Self::set_gauge`] runs here, still
    /// holding the guard, which is what keeps gauge writes ordered with
    /// transitions.
    ///
    /// The `times_opened` atomic and the `circuit_breaker_open` counter are
    /// deliberately NOT bumped here — they are monotonic, so they move to
    /// [`Self::count_open`] and are emitted after the guard is released.
    /// Callers must pair this with the matching [`Effect`].
    fn open_now(&self, state: &mut State) {
        *state = State::Open {
            opened_at: Instant::now(),
        };
        self.set_gauge(CircuitState::Open);
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
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
    }

    #[tokio::test]
    async fn test_circuit_opens_after_threshold_failures() {
        let config = CircuitBreakerConfig::new(3, 2, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        // Record failures below threshold
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        // One more failure should open the circuit
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(cb.times_opened(), 1);
    }

    #[tokio::test]
    async fn test_circuit_rejects_when_open() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Requests should be rejected
        assert!(!cb.allow_request());
        assert_eq!(cb.requests_rejected(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_circuit_transitions_to_half_open() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_millis(10));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Advance the paused clock past the open window
        tokio::time::advance(Duration::from_millis(20)).await;

        // Should allow request and transition to half-open
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn test_circuit_closes_after_success_in_half_open() {
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_millis(10));
        let cb = CircuitBreaker::new(config);

        // Open the circuit
        cb.record_failure();
        tokio::time::advance(Duration::from_millis(20)).await;

        // Transition to half-open
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Record successes
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn test_circuit_reopens_on_failure_in_half_open() {
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_millis(10));
        let cb = CircuitBreaker::new(config);

        // Open the circuit
        cb.record_failure();
        tokio::time::advance(Duration::from_millis(20)).await;

        // Transition to half-open
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Failure should reopen
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(cb.times_opened(), 2);
    }

    #[tokio::test]
    async fn test_success_resets_failure_counter() {
        let config = CircuitBreakerConfig::new(3, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        cb.record_failure();
        // Success should reset the counter
        cb.record_success();

        // Now we need 3 more failures to open
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[tokio::test]
    async fn test_force_close() {
        let cb = CircuitBreaker::default();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        cb.force_close();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
    }

    #[tokio::test]
    async fn test_force_open() {
        let cb = CircuitBreaker::default();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.force_open();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request());
    }

    // =========================================================================
    // Half-open probe limiting (TD-2026-07-03)
    // =========================================================================

    #[tokio::test(start_paused = true)]
    async fn test_half_open_limits_probes_to_success_threshold() {
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        tokio::time::advance(Duration::from_secs(30)).await;

        // success_threshold = 2 probe tokens: two callers pass, the third
        // is rejected instead of piling onto the recovering server.
        assert!(cb.allow_request());
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        let rejected_before = cb.requests_rejected();
        assert!(!cb.allow_request());
        assert_eq!(cb.requests_rejected(), rejected_before + 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_probe_tokens_regrant_after_open_duration() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        // Single token consumed by the transitioning caller; its outcome is
        // never recorded (the leaked-probe case) so the breaker sits in
        // HalfOpen with zero tokens.
        assert!(cb.allow_request());
        assert!(!cb.allow_request());

        // Just below the window boundary the budget must stay exhausted -
        // an unconditional re-grant would defeat the probe cap entirely.
        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(!cb.allow_request());

        // The re-grant window keeps the breaker from wedging permanently.
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_regrant_preserves_consecutive_successes() {
        // TD-2026-07-09. Entering HalfOpen resets `consecutive_successes`;
        // re-granting an expired window inside HalfOpen must PRESERVE it. The
        // two paths sat one statement apart in the flat struct and are now two
        // arms of the state enum, and in both shapes the tempting port writes
        // a zero at both sites - silently discarding a recorded probe success
        // and forcing a full fresh run after every window expiry. Written
        // before the enum landed so it pins the behavior across that change;
        // see the `State` docs for the full disposition table.
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        // Entry consumes one of the two tokens; record a success against it.
        assert!(cb.allow_request());
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Spend the second token, then exhaust the budget. Without this the
        // re-grant branch is never reached and the rest passes vacuously.
        assert!(cb.allow_request());
        assert!(
            !cb.allow_request(),
            "budget must be exhausted for the re-grant branch to be exercised"
        );

        // Window expiry re-grants; the success recorded above must survive.
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(cb.allow_request());

        // This second success reaches success_threshold only if the first one
        // survived the re-grant - a resetting re-grant leaves it HalfOpen.
        cb.record_success();
        assert_eq!(
            cb.state(),
            CircuitState::Closed,
            "re-grant must preserve consecutive_successes"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_reentry_grants_fresh_probe_tokens() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        // Consume the only token, then fail the probe: back to Open.
        assert!(cb.allow_request());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Next half-open entry starts with a fresh token budget.
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_recovery_within_probe_budget() {
        // The token budget equals success_threshold, so a healthy server
        // can be probed back to Closed without any rejection in between.
        let config = CircuitBreakerConfig::new(1, 2, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        assert!(cb.allow_request());
        cb.record_success();
        assert!(cb.allow_request());
        cb.record_success();

        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_request());
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_zero_success_threshold_still_grants_a_probe() {
        // Degenerate config: success_threshold = 0 must not deadlock the
        // breaker with a zero-token grant.
        let config = CircuitBreakerConfig::new(1, 0, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        assert!(cb.allow_request());
    }

    #[test]
    fn test_half_open_concurrent_probes_admit_exactly_the_budget() {
        // Two OS threads race allow_request inside HalfOpen over a one-token
        // budget: exactly one may pass.
        //
        // This deliberately no longer uses tokio::join!. A synchronous
        // allow_request is not a future, and the obvious sequential rewrite
        // would still pass with the cap removed entirely - it takes two
        // genuinely concurrent callers to test a cap. A Barrier releases both
        // threads at once and the loop amplifies a narrow window; a fresh
        // breaker per iteration keeps them independent. open_duration is far
        // longer than an iteration, so the re-grant window cannot fire
        // mid-race and hand the loser a second token.
        for i in 0..200 {
            let cb = CircuitBreaker::new(CircuitBreakerConfig::new(1, 1, Duration::from_secs(30)));
            cb.force_half_open();

            let gate = std::sync::Barrier::new(2);
            let (a, b) = std::thread::scope(|s| {
                let first = s.spawn(|| {
                    gate.wait();
                    cb.allow_request()
                });
                let second = s.spawn(|| {
                    gate.wait();
                    cb.allow_request()
                });
                (first.join().unwrap(), second.join().unwrap())
            });

            assert!(
                a ^ b,
                "iteration {i}: exactly one of two racing probes may pass, got ({a}, {b})"
            );
            assert_eq!(cb.state(), CircuitState::HalfOpen);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_release_probe_returns_token_capped_at_budget() {
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        // No-op while Closed.
        cb.release_probe();
        assert!(cb.allow_request());

        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        // Consume the only token, release it, and it must admit again.
        assert!(cb.allow_request());
        assert!(!cb.allow_request());
        cb.release_probe();
        assert!(cb.allow_request());

        // Releases never exceed the granted budget (single token here).
        cb.release_probe();
        cb.release_probe();
        assert!(cb.allow_request());
        assert!(
            !cb.allow_request(),
            "budget cap must hold after over-release"
        );
    }

    #[tokio::test]
    async fn test_force_open_when_already_open_does_not_double_count() {
        let cb = CircuitBreaker::default();

        cb.force_open();
        cb.force_open();

        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(
            cb.times_opened(),
            1,
            "repeat force_open must not inflate the counter"
        );
    }
}
