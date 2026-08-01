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
//! each admitted request receives a `ProbePermit` owning one, and requests
//! beyond the budget are rejected so a recovering server never receives a
//! thundering herd of probes. See [`CircuitBreaker::admit`].
//!
//! A permit returns its token unless the outcome was recorded, so a probe that
//! ends without feeding the breaker — an unrecorded error, or a request future
//! dropped mid-flight — no longer strands its token. Windows still re-grant
//! after `open_duration`, which now covers only genuinely long-running probes
//! rather than leaked ones; a token from an expired window is discarded on
//! release rather than credited to the live one.
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
//! // The gate is synchronous; only the guarded operation itself awaits.
//! // A half-open admission carries a permit owning one probe token; a Closed
//! // one consumes no token and so carries none.
//! let permit = match cb.admit() {
//!     Ok(Admission::Ungated) => None,
//!     Ok(Admission::Probe(permit)) => Some(permit),
//!     Err(rejection) => return Err(AppError::CircuitOpen(rejection.to_string())),
//! };
//!
//! // Execute the operation. Only CONNECTION-CLASS outcomes feed the breaker
//! // (see `resilience::run_resilient` for the real composition). Recording an
//! // outcome must CONSUME the permit: letting it drop as well would return a
//! // token the request already accounted for.
//! match operation().await {
//!     Ok(result) => {
//!         cb.record_success();
//!         permit.map(ProbePermit::consume);
//!         Ok(result)
//!     }
//!     Err(e) if is_connection_error(&e) => {
//!         cb.record_failure();
//!         permit.map(ProbePermit::consume);
//!         Err(e)
//!     }
//!     // Neither counter moves, so the token goes back: just drop the permit.
//!     Err(e) => Err(e),
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

/// Everything the state mutex protects.
///
/// `probe_generation` lives here rather than as an atomic on
/// [`CircuitBreaker`] deliberately: every access already happens under this
/// guard, and an atomic field would advertise lock-free access that is not
/// actually safe to use that way. Keeping it beside `state` also makes
/// "bumped on every grant" checkable in one place.
struct Guarded {
    state: State,
    /// Monotonic id of the current half-open probe window.
    ///
    /// Bumped on EVERY grant — both entering HalfOpen and re-granting within
    /// it — so a token minted in one window is distinguishable from the window
    /// it is dropped into. It cannot live inside `State::HalfOpen`: passing
    /// through Open would leave the next entry with nothing to read, and any
    /// restart lets a straggler match a recycled id.
    ///
    /// Never reset. `force_close`/`force_open` deliberately leave it alone for
    /// the same reason.
    probe_generation: u64,
}

/// Open a fresh half-open probe window and return its id.
///
/// The single place a window is granted, so the generation cannot advance
/// without a grant, nor a grant happen without advancing it — the pairing is
/// structural rather than a convention repeated at three call sites.
///
/// Takes the two guarded fields separately rather than `&mut Guarded` because
/// callers hold them through a split borrow: the state must stay mutably
/// borrowed by the match arm that decided to grant.
///
/// `consecutive_successes` is the caller's decision, and is exactly what
/// separates entering from re-granting — see the [`State`] docs.
fn grant_window(
    state: &mut State,
    probe_generation: &mut u64,
    probes_remaining: u32,
    consecutive_successes: u32,
) -> u64 {
    *probe_generation += 1;
    *state = State::HalfOpen {
        probes_remaining,
        granted_at: Instant::now(),
        consecutive_successes,
    };
    *probe_generation
}

/// How a half-open probe token ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Every minted token ends in exactly one of these, so the four together
/// partition the admissions that carried a permit. That totality is the point:
/// `consumed` is only usable as a denominator if nothing ends uncounted.
enum Disposition {
    /// The outcome was recorded, so the token was not handed back.
    Consumed,
    /// The outcome was never recorded; the token returned to its own window.
    Released,
    /// Dropped into a different window than it was minted in, and discarded.
    Stale,
    /// The breaker left HalfOpen while the probe was in flight, so the window
    /// died with the variant and there was nothing to return the token to.
    /// The common shape during recovery: a sibling probe closes or reopens the
    /// circuit while this one is still running.
    Abandoned,
}

impl Disposition {
    fn label(self) -> &'static str {
        match self {
            Disposition::Consumed => "consumed",
            Disposition::Released => "released",
            Disposition::Stale => "stale",
            Disposition::Abandoned => "abandoned",
        }
    }
}

/// Why the gate turned a request away.
///
/// Captured under the same guard that made the decision, so — unlike re-reading
/// `state()` afterwards — it always names the state that actually rejected,
/// even under a concurrent transition.
///
/// Deliberately has no `Closed` variant: Closed never rejects, so an
/// `Err(Closed)` would be representable and meaningless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejection {
    /// The circuit is open and its open window has not yet elapsed.
    Open,
    /// Half-open, but this window's probe budget is already spent.
    ProbeBudgetExhausted,
}

impl Rejection {
    /// Value for the `state` label on the rejection counter.
    ///
    /// Deliberately not [`CircuitState`]'s `Display`, which renders
    /// `"half-open"` as user-facing prose. The metric vocabulary is
    /// `"half_open"`, and routing the label through `Display` would silently
    /// rename an exported label value and break existing queries.
    fn metric_label(self) -> &'static str {
        match self {
            Rejection::Open => "open",
            Rejection::ProbeBudgetExhausted => "half_open",
        }
    }
}

impl From<Rejection> for CircuitState {
    fn from(rejection: Rejection) -> Self {
        match rejection {
            Rejection::Open => CircuitState::Open,
            Rejection::ProbeBudgetExhausted => CircuitState::HalfOpen,
        }
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        CircuitState::from(*self).fmt(f)
    }
}

/// Ownership of one half-open probe token.
///
/// Minted only where a token is actually taken, so the permit's *existence* is
/// the proof that this request holds one. That is what makes a phantom release
/// — handing back a token the request never took — unrepresentable rather than
/// merely guarded against.
///
/// Dropping a permit hands its token back; [`ProbePermit::consume`] gives it up
/// without releasing, for outcomes that WERE recorded. That is the whole
/// mechanism: a request cannot both record an outcome and return its token,
/// and a request future dropped mid-probe — a client disconnect during an
/// outage — returns the token it was holding instead of leaking it until the
/// re-grant window.
///
/// The lifetime borrows the breaker, so a permit cannot outlive it or be
/// smuggled into a detached task. Deliberately not `Clone` — a cloned permit
/// would release twice.
///
/// One caveat worth stating rather than implying: binding a permit to a named
/// local is a review rule, not a compiler-enforced one. `#[must_use]` on
/// [`Admission`] catches a discarded admission, but `admit().is_ok()` and
/// `let _ = admit()` both silence it while dropping the token immediately —
/// `clippy::let_underscore_must_use` would catch the latter and is a pedantic
/// lint this crate does not enable. Tests whose assertion depends on a token
/// staying held say so at the binding.
pub(crate) struct ProbePermit<'a> {
    breaker: &'a CircuitBreaker,
    /// The probe window this token was taken from.
    generation: u64,
}

impl<'a> ProbePermit<'a> {
    fn new(breaker: &'a CircuitBreaker, generation: u64) -> Self {
        Self {
            breaker,
            generation,
        }
    }

    /// The outcome was recorded, so the token must NOT be handed back.
    ///
    /// Consumes by value, so use-after-consume is a compile error rather than a
    /// silent no-op. The disarm is a move with no lock involved, which is what
    /// makes it safe to call from anywhere, including a path where a state
    /// guard is still live — which `Drop` itself is not.
    pub(crate) fn consume(self) {
        self.breaker.record_disposition(Disposition::Consumed);
        // Suppress the release without running Drop. ManuallyDrop rather than
        // mem::forget only for clarity - the permit owns a shared reference
        // and a u64, so neither leaks anything.
        let _ = std::mem::ManuallyDrop::new(self);
    }
}

impl Drop for ProbePermit<'_> {
    fn drop(&mut self) {
        self.breaker.release_probe(self.generation);
    }
}

/// Outcome of the circuit-breaker gate for one request.
#[must_use = "an admission decides whether the operation may run"]
pub(crate) enum Admission<'a> {
    /// Closed: the request passes and consumes no probe token, so there is
    /// nothing it could later hand back.
    Ungated,
    /// HalfOpen: the request holds one of the window's probe tokens.
    Probe(ProbePermit<'a>),
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
    /// A request was turned away.
    Rejected(Rejection),
    /// A probe window elapsed with probes still unaccounted for, and was
    /// re-granted so recovery cannot wedge. `granted` is the size of the NEW
    /// window - not a count of what is still in flight, which the breaker does
    /// not track.
    RegrantedProbes { granted: u32 },
    /// A token was dropped into a different window than it was minted in.
    StaleProbeDiscarded { minted: u64, current: u64 },
    /// A token outlived its window entirely - the breaker is no longer HalfOpen.
    ProbeWindowGone,
    /// A token came back to a window that is already whole. Unreachable unless
    /// the accounting is wrong.
    ProbeOverRelease,
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
/// awaits, and a blocking guard is what lets [`ProbePermit::drop`] return its
/// token, which it could not do if releasing required an await. No guard may be held across an
/// `.await` — `clippy::await_holding_lock` enforces that, and CI denies warnings.
pub struct CircuitBreaker {
    /// Configuration parameters.
    config: CircuitBreakerConfig,
    /// Internal state protected by a synchronous mutex.
    state: Mutex<Guarded>,
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
            state: Mutex::new(Guarded {
                state: State::initial(),
                probe_generation: 0,
            }),
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
    fn lock(&self) -> MutexGuard<'_, Guarded> {
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
            Effect::Rejected(rejection) => {
                if rejection == Rejection::ProbeBudgetExhausted {
                    debug!("Circuit breaker rejected request: half-open probe budget exhausted");
                }
                self.requests_rejected.fetch_add(1, Ordering::Relaxed);
                crate::metrics::record_circuit_breaker_rejection(rejection.metric_label());
            }
            Effect::RegrantedProbes { granted } => {
                // Reachable for a legitimate reason even with RAII release:
                // probes still IN FLIGHT past open_duration, which is the
                // normal half-open case during an outage. Not an invariant
                // violation, so not a warning - the leak alarm is the stale
                // discard below.
                info!(
                    granted,
                    "Circuit breaker re-granted half-open probe tokens (previous window did not complete)"
                );
            }
            Effect::ReleasedProbe => {
                // Routine: every non-connection error in half-open lands here.
                self.record_disposition(Disposition::Released);
                debug!("Circuit breaker released a half-open probe token (outcome not recorded)");
            }
            Effect::ProbeWindowGone => {
                // Routine during recovery, so debug rather than warn - but it
                // is counted, because an uncounted ending would break the
                // partition the disposition metric depends on.
                self.record_disposition(Disposition::Abandoned);
                debug!("Circuit breaker probe ended after its window closed");
            }
            Effect::ProbeOverRelease => {
                self.record_disposition(Disposition::Abandoned);
                debug_assert!(false, "probe token released into a full window");
                warn!(
                    "Circuit breaker saw a probe token released into a full window; \
                     token accounting is inconsistent"
                );
            }
            Effect::StaleProbeDiscarded { minted, current } => {
                // The token outlived its window. Bounded and self-correcting,
                // but it means a probe ran longer than open_duration, so it is
                // worth an operator's attention.
                self.record_disposition(Disposition::Stale);
                warn!(
                    minted_generation = minted,
                    current_generation = current,
                    "Circuit breaker discarded a probe token from an expired window"
                );
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

    /// Ask the breaker to admit one request.
    ///
    /// `Ok` carries an [`Admission`] describing what the caller now holds:
    /// nothing (Closed) or a [`ProbePermit`] (HalfOpen). `Err` carries the
    /// [`Rejection`] reason, captured under the guard that made the decision.
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
    /// Tokens re-grant after `open_duration` elapses in HalfOpen. A probe
    /// whose outcome is never recorded now returns its token when its permit
    /// drops, so the re-grant no longer covers leaked tokens — what remains is
    /// a probe still IN FLIGHT past the window, which the breaker cannot
    /// distinguish from a lost one and must not wait on forever.
    pub(crate) fn admit(&self) -> Result<Admission<'_>, Rejection> {
        /// Decided under the guard; the permit is minted only afterwards.
        ///
        /// Keeping construction outside the critical section is structural
        /// deadlock safety: once `Drop` releases a token it must take the same
        /// non-reentrant mutex, so no permit may exist while a guard is live.
        /// Deferring the mint makes that impossible rather than merely
        /// discouraged, and a `?` added mid-function later cannot reintroduce it.
        enum Decision {
            Ungated,
            /// Admitted holding a token from this probe window.
            Probe(u64),
            Rejected(Rejection),
        }

        // One exclusive acquisition covers every case, and every arm yields
        // (decision, effect) rather than returning early, so no path can skip
        // the emit below.
        let (decision, effect) = {
            let mut guard = self.lock();
            let budget = self.probe_budget();
            // Split the borrow so a grant can bump the window id while the
            // match still holds `state` mutably.
            let Guarded {
                state,
                probe_generation,
            } = &mut *guard;

            match state {
                State::Closed { .. } => (Decision::Ungated, Effect::None),
                State::Open { opened_at } => {
                    if opened_at.elapsed() >= self.config.open_duration {
                        // A NEW recovery attempt: the success count starts at
                        // zero (contrast the re-grant arm below). The
                        // transitioning caller takes the first of the granted
                        // tokens as it passes, so the window opens at
                        // budget - 1; probe_budget() floors at 1, so this
                        // cannot underflow.
                        let generation = grant_window(state, probe_generation, budget - 1, 0);
                        self.set_gauge(CircuitState::HalfOpen);
                        (
                            Decision::Probe(generation),
                            Effect::EnteredHalfOpen { probes: budget },
                        )
                    } else {
                        (
                            Decision::Rejected(Rejection::Open),
                            Effect::Rejected(Rejection::Open),
                        )
                    }
                }
                State::HalfOpen {
                    probes_remaining,
                    granted_at,
                    consecutive_successes,
                } => {
                    if *probes_remaining > 0 {
                        *probes_remaining -= 1;
                        (Decision::Probe(*probe_generation), Effect::None)
                    } else if granted_at.elapsed() >= self.config.open_duration {
                        // Re-grant after open_duration so outstanding probes
                        // cannot wedge the breaker in HalfOpen. This is the
                        // SAME recovery attempt continuing, so the success
                        // count carries over.
                        let successes = *consecutive_successes;
                        let generation =
                            grant_window(state, probe_generation, budget - 1, successes);
                        (
                            Decision::Probe(generation),
                            Effect::RegrantedProbes { granted: budget },
                        )
                    } else {
                        (
                            Decision::Rejected(Rejection::ProbeBudgetExhausted),
                            Effect::Rejected(Rejection::ProbeBudgetExhausted),
                        )
                    }
                }
            }
        };

        self.emit(effect);
        match decision {
            Decision::Ungated => Ok(Admission::Ungated),
            Decision::Probe(generation) => Ok(Admission::Probe(ProbePermit::new(self, generation))),
            Decision::Rejected(rejection) => Err(rejection),
        }
    }

    /// Half-open probe budget: `success_threshold`, floored at one so a
    /// zero threshold cannot deadlock the breaker. Single definition shared
    /// by the grant and release paths so the cap cannot drift.
    fn probe_budget(&self) -> u32 {
        self.config.success_threshold.max(1)
    }

    /// Test-only: enter HalfOpen with a full probe budget, spending nothing.
    ///
    /// The production path reaches HalfOpen only through [`Self::admit`],
    /// which consumes the transitioning caller's token on the way in. A race
    /// over the budget therefore cannot be staged through it — the setup call
    /// would take the very token under contention. Granting the window directly
    /// leaves the budget as the only contended resource.
    #[cfg(test)]
    fn force_half_open(&self) {
        let budget = self.probe_budget();
        let mut guard = self.lock();
        let Guarded {
            state,
            probe_generation,
        } = &mut *guard;
        grant_window(state, probe_generation, budget, 0);
    }

    /// Hand back a half-open probe token whose outcome was deliberately not
    /// recorded (non-connection errors touch neither breaker counter).
    ///
    /// Without this, probes completing with e.g. `NotFound` — a full
    /// round-trip proving transport health — would permanently consume
    /// tokens and starve recovery until the re-grant window. No-op outside
    /// HalfOpen; capped at the granted budget.
    fn release_probe(&self, generation: u64) {
        let effect = {
            let mut guard = self.lock();
            let budget = self.probe_budget();
            let current = guard.probe_generation;

            match &mut guard.state {
                // A token minted in an earlier window must not credit this
                // one: the re-grant already replaced the budget it belonged
                // to, so crediting it here would inflate the live window.
                State::HalfOpen { .. } if current != generation => Effect::StaleProbeDiscarded {
                    minted: generation,
                    current,
                },
                State::HalfOpen {
                    probes_remaining, ..
                } if *probes_remaining < budget => {
                    *probes_remaining += 1;
                    Effect::ReleasedProbe
                }
                // Same window, already at full budget. Since remaining plus
                // outstanding always equals the budget, no permit can exist to
                // reach this arm - getting here means the token accounting is
                // broken, not that a release was merely redundant.
                State::HalfOpen { .. } => Effect::ProbeOverRelease,
                // The breaker left HalfOpen while this probe was in flight: a
                // sibling probe closed or reopened the circuit. The window died
                // with the variant, so there is nothing to return the token to.
                State::Closed { .. } | State::Open { .. } => Effect::ProbeWindowGone,
            }
        };
        self.emit(effect);
    }

    /// Count how a probe token ended. Touches no state and takes no lock, so
    /// it is safe from [`ProbePermit::consume`], which may run anywhere.
    fn record_disposition(&self, disposition: Disposition) {
        crate::metrics::record_circuit_breaker_probe_disposition(disposition.label());
    }

    /// Record a successful operation.
    ///
    /// In HalfOpen state, consecutive successes can close the circuit.
    pub fn record_success(&self) {
        let effect = {
            let mut guard = self.lock();

            match &mut guard.state {
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
                        guard.state = State::Closed {
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

            match &mut guard.state {
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
                        self.open_now(&mut guard.state);
                    }
                    Effect::Failure { failures, opened }
                }
                State::HalfOpen { .. } => {
                    // Any failure in half-open state reopens the circuit; the
                    // accumulated success count dies with the variant.
                    self.open_now(&mut guard.state);
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
    ///
    /// Test-only. Production code used to call this immediately after a
    /// rejection to name the state in the error message; [`Rejection`] now
    /// carries that, captured under the guard that rejected, so the last
    /// non-test caller is gone along with its second lock acquisition.
    #[cfg(test)]
    pub fn state(&self) -> CircuitState {
        CircuitState::from(&self.lock().state)
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
            guard.state = State::Closed {
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
            let changed = !matches!(guard.state, State::Open { .. });
            if changed {
                self.open_now(&mut guard.state);
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
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::default();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.admit().is_ok());
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
        assert!(cb.admit().is_err());
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
        assert!(cb.admit().is_ok());
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
        assert!(cb.admit().is_ok());
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
        assert!(cb.admit().is_ok());
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
        assert!(cb.admit().is_ok());
    }

    #[tokio::test]
    async fn test_force_open() {
        let cb = CircuitBreaker::default();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.force_open();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.admit().is_err());
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
        //
        // The permits are bound to named locals deliberately. Once Drop
        // releases the token, letting these fall as temporaries would hand
        // both tokens straight back and invert the rejection asserted below.
        let _probe_1 = cb.admit().expect("first probe admitted");
        let _probe_2 = cb.admit().expect("second probe admitted");
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        let rejected_before = cb.requests_rejected();
        assert!(cb.admit().is_err());
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
        // HalfOpen with zero tokens. Held in a named local: a temporary would
        // return the token once Drop releases, and the whole point of this
        // test is a window that stays exhausted.
        let _leaked_probe = cb.admit().expect("transitioning caller admitted");
        assert!(cb.admit().is_err());

        // Just below the window boundary the budget must stay exhausted -
        // an unconditional re-grant would defeat the probe cap entirely.
        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(cb.admit().is_err());

        // The re-grant window keeps the breaker from wedging permanently.
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(cb.admit().is_ok());
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
        // Permits are held in named locals throughout: dropping one returns
        // its token, and this test is entirely about a budget that stays spent.
        let _probe_1 = cb.admit().expect("first probe admitted");
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Spend the second token, then exhaust the budget. Without this the
        // re-grant branch is never reached and the rest passes vacuously.
        let _probe_2 = cb.admit().expect("second probe admitted");
        assert!(
            cb.admit().is_err(),
            "budget must be exhausted for the re-grant branch to be exercised"
        );

        // Window expiry re-grants; the success recorded above must survive.
        tokio::time::advance(Duration::from_secs(30)).await;
        let _probe_3 = cb.admit().expect("re-granted window admits");

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
        assert!(cb.admit().is_ok());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Next half-open entry starts with a fresh token budget.
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(cb.admit().is_ok());
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

        // Permits held in named locals: as temporaries they would refund
        // their tokens before the next admit, and the test would pass with the
        // budget capped at one - which is exactly the property it claims to
        // check.
        let _probe_1 = cb.admit().expect("first probe admitted");
        cb.record_success();
        let _probe_2 = cb.admit().expect("second probe admitted");
        cb.record_success();

        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.admit().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_zero_success_threshold_still_grants_a_probe() {
        // Degenerate config: success_threshold = 0 must not deadlock the
        // breaker with a zero-token grant.
        let config = CircuitBreakerConfig::new(1, 0, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        assert!(cb.admit().is_ok());
    }

    #[test]
    fn test_half_open_concurrent_probes_admit_exactly_the_budget() {
        // Two OS threads race admit() inside HalfOpen over a one-token
        // budget: exactly one may pass.
        //
        // This deliberately no longer uses tokio::join!. A synchronous
        // admit() is not a future, and the obvious sequential rewrite
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
            // The admissions are carried OUT of the threads and held until the
            // assertion. Testing `is_ok()` inside a thread would drop the
            // permit there, hand the token straight back, and let both racers
            // pass - which is the bug this test exists to catch.
            let (a, b) = std::thread::scope(|s| {
                let first = s.spawn(|| {
                    gate.wait();
                    cb.admit()
                });
                let second = s.spawn(|| {
                    gate.wait();
                    cb.admit()
                });
                (first.join().unwrap(), second.join().unwrap())
            });

            assert!(
                a.is_ok() ^ b.is_ok(),
                "iteration {i}: exactly one of two racing probes may pass, got ({}, {})",
                a.is_ok(),
                b.is_ok()
            );
            assert_eq!(cb.state(), CircuitState::HalfOpen);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_dropping_a_permit_returns_its_token() {
        // Replaces the old direct test of release_probe, which is now private
        // and reachable only through Drop. Same property, driven through the
        // public path: an unrecorded outcome hands its token back.
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        let probe = cb.admit().expect("only token admitted");
        assert!(
            cb.admit().is_err(),
            "budget exhausted while the probe is held"
        );

        drop(probe);
        let _readmitted = cb.admit().expect("released token re-admits");
        assert!(
            cb.admit().is_err(),
            "release must not push the budget above its cap"
        );
    }

    // =========================================================================
    // TD-2026-07-09: the three probe-accounting leaks, one test each
    // =========================================================================

    #[tokio::test(start_paused = true)]
    async fn test_straggler_from_an_expired_window_does_not_inflate_the_new_one() {
        // Leak 2. A re-grant replaces the budget while an earlier window's
        // token is still outstanding; when that straggler finally returns it
        // must be discarded, not credited to the live window. Ownership alone
        // does not fix this - it needs the window id.
        let cb = CircuitBreaker::new(CircuitBreakerConfig::new(1, 1, Duration::from_secs(30)));
        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        let stale = cb.admit().expect("window 1 token");
        assert!(cb.admit().is_err(), "window 1 holds a single token");

        // Window 2, granted while window 1's token is still out.
        tokio::time::advance(Duration::from_secs(30)).await;
        let _current = cb.admit().expect("window 2 re-granted");
        assert!(cb.admit().is_err(), "window 2 also holds a single token");

        drop(stale);
        assert!(
            cb.admit().is_err(),
            "a token from an expired window must not inflate the live one"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_permit_dropped_after_leaving_half_open_does_not_seed_the_next_window() {
        // Leak 1, respecified. The original phantom release - a request
        // admitted while Closed handing back a token it never took - is
        // unrepresentable now that Ungated carries no permit, so there is
        // nothing left to assert about it. What remains testable is the same
        // hazard one step later: a permit that outlives its window entirely.
        let cb = CircuitBreaker::new(CircuitBreakerConfig::new(1, 1, Duration::from_secs(30)));
        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        let stale = cb.admit().expect("window 1 token");

        // Fail the probe: back to Open, then into a brand new window.
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        tokio::time::advance(Duration::from_secs(30)).await;
        let _fresh = cb.admit().expect("window 2 token");
        assert!(cb.admit().is_err(), "window 2 holds a single token");

        drop(stale);
        assert!(
            cb.admit().is_err(),
            "a permit that outlived its window must not seed the next one"
        );
    }

    #[tokio::test]
    async fn test_a_probe_cancelled_mid_flight_returns_its_token() {
        // Leak 3, and the only one Drop is strictly required for: a client
        // disconnecting during an outage drops the request future while its
        // probe is in flight. An explicit release call cannot cover this -
        // there is no code path left to run it on.
        let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::new(
            1,
            1,
            Duration::from_secs(30),
        )));
        cb.force_half_open();

        let task_cb = Arc::clone(&cb);
        let in_flight = tokio::spawn(async move {
            let _probe = task_cb.admit().expect("token admitted");
            // Hold the permit across a suspension point, as a real operation
            // awaiting the server would.
            std::future::pending::<()>().await;
        });

        tokio::task::yield_now().await;
        assert!(
            cb.admit().is_err(),
            "the in-flight probe holds the only token"
        );

        in_flight.abort();
        let _ = in_flight.await;

        assert!(
            cb.admit().is_ok(),
            "a probe cancelled mid-flight must return its token"
        );
    }

    #[test]
    fn test_dropping_a_permit_after_recording_does_not_deadlock() {
        // Drop takes the same non-reentrant mutex that record_* takes, so the
        // ordering is load-bearing. It holds structurally rather than by
        // convention: record_success releases its guard before returning, and
        // admit mints the permit only after releasing its own, so no permit
        // can exist while a guard is live.
        //
        // A regression here HANGS rather than fails - a deadlocked thread
        // cannot assert its own deadlock - so this is an executable statement
        // of the ordering rather than a detector.
        let cb = CircuitBreaker::new(CircuitBreakerConfig::new(1, 1, Duration::from_secs(30)));
        cb.force_half_open();

        let probe = cb.admit().expect("token admitted");
        cb.record_success();
        drop(probe);

        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_metric_projections_are_distinct_from_display() {
        assert_eq!(CircuitState::Closed.gauge(), 0);
        assert_eq!(CircuitState::HalfOpen.gauge(), 1);
        assert_eq!(CircuitState::Open.gauge(), 2);

        assert_eq!(Rejection::Open.metric_label(), "open");
        assert_eq!(Rejection::ProbeBudgetExhausted.metric_label(), "half_open");

        // Exported Prometheus label values; renaming one silently breaks
        // existing queries, so they are pinned rather than trusted.
        assert_eq!(Disposition::Consumed.label(), "consumed");
        assert_eq!(Disposition::Released.label(), "released");
        assert_eq!(Disposition::Stale.label(), "stale");
        assert_eq!(Disposition::Abandoned.label(), "abandoned");

        // The distinction is the point: Display is user-facing prose and
        // renders a hyphen, while the exported Prometheus label uses an
        // underscore. Routing the label through Display would silently rename
        // it and break existing queries.
        assert_eq!(CircuitState::HalfOpen.to_string(), "half-open");
        assert_ne!(
            CircuitState::HalfOpen.to_string(),
            Rejection::ProbeBudgetExhausted.metric_label()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_consuming_a_permit_does_not_return_its_token() {
        // The other half of the mechanism: a RECORDED outcome must not also
        // hand the token back, or a request would both count and refund.
        let config = CircuitBreakerConfig::new(1, 1, Duration::from_secs(30));
        let cb = CircuitBreaker::new(config);

        cb.record_failure();
        tokio::time::advance(Duration::from_secs(30)).await;

        let Admission::Probe(probe) = cb.admit().expect("only token admitted") else {
            panic!("half-open admission must carry a permit");
        };
        probe.consume();

        assert!(cb.admit().is_err(), "a consumed token must stay consumed");
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
