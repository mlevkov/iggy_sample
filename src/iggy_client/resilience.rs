//! Resilience executor: the timeout + circuit-breaker + reconnect-and-retry
//! composition applied to every Iggy operation.
//!
//! Extracted from `IggyClientWrapper::with_reconnect` so the composition can
//! be exercised against fake operations and a fake reconnect step (no live
//! server), with `tokio::time::pause()` driving the timeout branches — see
//! the test matrix at the bottom of this file (TD-2026-07-01).
//!
//! # Composition semantics
//!
//! 1. **Circuit breaker gate**: if the breaker rejects the request, fail
//!    fast with `CircuitOpen` without running the operation.
//! 2. **First attempt**, bounded by `timeout`.
//! 3. **Classified connection error** → record breaker failure, run the
//!    `reconnect` step, then retry the operation exactly once.
//! 4. **Non-connection error** → returned as-is; the breaker records
//!    neither success nor failure (bad requests must not open the circuit),
//!    but any half-open probe token the request consumed is RELEASED so an
//!    unrecorded outcome cannot starve recovery (see
//!    `CircuitBreaker::release_probe`).
//! 5. **Timeout** → recorded as a breaker failure only when
//!    `timeout_is_outage_signal` is true, i.e. the deadline is the global
//!    operation timeout (the SDK's internal transport reconnection swallows
//!    most outages into blocking retries, so such a timeout is often the
//!    only outage signal). A client-shortened `X-Request-Timeout` deadline
//!    expiring says nothing about an outage and must not feed the shared
//!    breaker — otherwise one client could open the circuit for everyone.
//!    Reconnect + single retry happen only if `is_connected` reports the
//!    connection as lost — a timeout alone could just be a slow operation.
//!
//! # Retry and the breaker gate
//!
//! The single post-reconnect retry deliberately does NOT re-pass the
//! breaker gate: a request whose reconnect just succeeded gets its one
//! retry even if the first attempt's recorded failure opened the circuit.
//! In half-open this means one probe token can cover up to two server
//! operations (attempt + retry); the retry's outcome is still recorded, so
//! the breaker converges on real evidence. A retry success that lands after
//! another probe's failure reopened the circuit is discarded (see
//! `CircuitBreaker::record_success`).
//!
//! # Worst-case latency
//!
//! A request on the reconnect path can take up to 3x `timeout` (first
//! attempt + bounded reconnect + retry) — bounded, but well above the
//! typical single-operation expectation.
//!
//! # Breaker false positives — and who feeds the breaker
//!
//! GLOBAL-deadline timeouts count as breaker failures, so N consecutive
//! merely-SLOW operations (including the background stats refresher's) can
//! open the circuit without a real outage. Failures must be consecutive —
//! any success resets the count — which bounds the risk. (Client-scoped
//! deadlines are exempt per semantics #5, which SHRINKS this
//! false-positive surface.)
//!
//! The flip side of that exemption: during a pure-hang outage where ALL
//! request traffic carries short client deadlines, the only guaranteed
//! breaker feeder is the background stats refresher, which runs on the
//! root (global-deadline) wrapper — health-check pings keep `is_connected`
//! truthful but never touch the breaker. Convergence to Open is therefore
//! minutes-scale (≈ failure_threshold × (global timeout + refresh
//! interval)) instead of seconds. Scoped CONNECTION errors still record
//! immediately. This is the accepted trade against the one-client-DoS the
//! exemption prevents.
//!
//! Note also that a single request may release its probe token twice
//! (scoped first-attempt timeout, then an unrecorded retry outcome); the
//! budget cap bounds the effect to transiently admitting one extra
//! concurrent probe — the same order of slack as the documented
//! retry-gate bypass.

use std::time::Duration;

use tracing::{debug, warn};

use super::circuit_breaker::CircuitBreaker;
use crate::error::{AppError, AppResult};

/// Check if an error is a connection-related error that warrants reconnection.
///
/// Uses explicit pattern matching on error variants rather than string
/// matching, which is more reliable and maintainable.
pub(super) fn is_connection_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::ConnectionFailed(_) | AppError::Disconnected(_) | AppError::ConnectionReset(_)
    )
}

/// Execute `operation` under the full resilience composition.
///
/// Generic over its collaborators so the composition is testable without a
/// live server:
///
/// - `breaker` — gates the request and records the outcome
/// - `timeout` — per-attempt bound (both the first attempt and the retry)
/// - `timeout_is_outage_signal` — whether an expired `timeout` may be
///   recorded as a breaker failure (false for client-shortened deadlines)
/// - `is_connected` — consulted only on the timeout branch, to distinguish
///   "slow operation" from "lost connection"
/// - `reconnect` — the bounded reconnect step; invoked at most once
/// - `operation` — the Iggy call; invoked once, plus at most one retry
pub(super) async fn run_resilient<T, F, Fut, C, R, RFut>(
    breaker: &CircuitBreaker,
    timeout: Duration,
    timeout_is_outage_signal: bool,
    is_connected: C,
    reconnect: R,
    operation: F,
) -> AppResult<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = AppResult<T>>,
    C: FnOnce() -> bool,
    R: FnOnce() -> RFut,
    RFut: Future<Output = AppResult<()>>,
{
    // Check circuit breaker before attempting operation
    if !breaker.allow_request().await {
        // The state is re-read after the rejection, so under a concurrent
        // transition it reports the CURRENT state, not necessarily the one
        // that rejected the request.
        let state = breaker.state().await;
        return Err(AppError::CircuitOpen(format!(
            "Circuit breaker rejected the request (current state: {}) - service temporarily unavailable",
            state
        )));
    }

    // First attempt with timeout
    match tokio::time::timeout(timeout, operation()).await {
        Ok(Ok(value)) => {
            breaker.record_success().await;
            Ok(value)
        }
        Ok(Err(e)) if is_connection_error(&e) => {
            breaker.record_failure().await;
            warn!(error = %e, "Operation failed due to connection error, attempting reconnect");
            reconnect().await?;
            retry_once(breaker, timeout, timeout_is_outage_signal, &operation).await
        }
        Ok(Err(e)) => {
            // Non-connection error - record neither success nor failure,
            // but hand back any half-open probe token this request consumed
            // so an unrecorded outcome cannot starve recovery.
            breaker.release_probe().await;
            Err(e)
        }
        Err(_) => {
            // Timeout on first attempt. The SDK's internal transport
            // reconnection swallows most mid-operation connection failures
            // into blocking retries, so a GLOBAL-deadline timeout is often
            // the only outage signal we get - record it as a breaker
            // failure. A client-shortened deadline expiring is not outage
            // evidence; hand back any consumed probe token instead.
            if timeout_is_outage_signal {
                breaker.record_failure().await;
                warn!(
                    timeout = ?timeout,
                    "Operation timed out at the global deadline (recorded as circuit-breaker failure)"
                );
            } else {
                breaker.release_probe().await;
                debug!(
                    timeout = ?timeout,
                    "Operation timed out at a client-scoped deadline (not a breaker failure)"
                );
            }

            // Only reconnect if we have evidence the connection is actually
            // lost (the background health check drives this flag via live
            // pings). A timeout alone could just be a slow operation.
            if !is_connected() {
                warn!(
                    timeout = ?timeout,
                    "Operation timed out and connection state is disconnected, attempting reconnect"
                );
                reconnect().await?;
                retry_once(breaker, timeout, timeout_is_outage_signal, &operation).await
            } else {
                debug!(
                    timeout = ?timeout,
                    "Operation timed out but connection state is healthy, not reconnecting"
                );
                Err(AppError::OperationTimeout(format!(
                    "Operation timed out after {:?}",
                    timeout
                )))
            }
        }
    }
}

/// Single post-reconnect retry with timeout and circuit-breaker bookkeeping.
/// Shared by both reconnect paths of [`run_resilient`]. Deliberately does
/// not re-pass the breaker gate (see module docs, "Retry and the breaker
/// gate").
async fn retry_once<T, F, Fut>(
    breaker: &CircuitBreaker,
    timeout: Duration,
    timeout_is_outage_signal: bool,
    operation: &F,
) -> AppResult<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = AppResult<T>>,
{
    match tokio::time::timeout(timeout, operation()).await {
        Ok(Ok(value)) => {
            breaker.record_success().await;
            Ok(value)
        }
        Ok(Err(e)) => {
            if is_connection_error(&e) {
                breaker.record_failure().await;
                warn!(error = %e, "Retry failed with a connection error (recorded as breaker failure)");
            } else {
                // Unrecorded outcome: hand back any consumed probe token.
                breaker.release_probe().await;
            }
            Err(e)
        }
        Err(_) => {
            if timeout_is_outage_signal {
                breaker.record_failure().await;
                warn!(
                    timeout = ?timeout,
                    "Retry timed out at the global deadline (recorded as breaker failure)"
                );
            } else {
                breaker.release_probe().await;
            }
            Err(AppError::OperationTimeout(format!(
                "Operation timed out after {:?} on retry",
                timeout
            )))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::super::circuit_breaker::{CircuitBreakerConfig, CircuitState};
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);

    fn breaker_with(failure_threshold: u32) -> CircuitBreaker {
        CircuitBreaker::new(CircuitBreakerConfig::new(
            failure_threshold,
            1,
            Duration::from_secs(30),
        ))
    }

    /// Fake reconnect step that counts invocations and returns `result`.
    fn fake_reconnect(
        counter: &Arc<AtomicU32>,
        result: AppResult<()>,
    ) -> impl FnOnce() -> std::future::Ready<AppResult<()>> {
        let counter = Arc::clone(counter);
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
            std::future::ready(result)
        }
    }

    // =========================================================================
    // TD-2026-07-01 composition matrix - one test per branch of the executor
    // =========================================================================

    #[tokio::test(start_paused = true)]
    async fn breaker_open_fails_fast_without_running_operation() {
        let breaker = breaker_with(5);
        breaker.force_open().await;
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true,
            fake_reconnect(&reconnects, Ok(())),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(42)
                }
            },
        )
        .await;

        assert!(matches!(result, Err(AppError::CircuitOpen(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "operation must not run");
        assert_eq!(reconnects.load(Ordering::SeqCst), 0);
        assert_eq!(breaker.requests_rejected(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn success_passes_through_and_records_breaker_success() {
        // Enter HalfOpen (success_threshold = 1) so record_success is
        // observable: the single success must close the circuit. Zero
        // open_duration makes the Open->HalfOpen transition immediate.
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::new(1, 1, Duration::ZERO));
        breaker.force_open().await;
        let reconnects = Arc::new(AtomicU32::new(0));

        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true,
            fake_reconnect(&reconnects, Ok(())),
            || async { Ok(42) },
        )
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(breaker.state().await, CircuitState::Closed);
        assert_eq!(reconnects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_while_connected_returns_timeout_without_reconnecting() {
        let breaker = breaker_with(1);
        let reconnects = Arc::new(AtomicU32::new(0));

        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true, // connected: slow operation, not an outage
            fake_reconnect(&reconnects, Ok(())),
            || async { std::future::pending().await },
        )
        .await;

        assert!(matches!(result, Err(AppError::OperationTimeout(_))));
        assert_eq!(reconnects.load(Ordering::SeqCst), 0, "must not reconnect");
        // The timeout still counts as a breaker failure (threshold 1 -> Open).
        assert_eq!(breaker.state().await, CircuitState::Open);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_while_disconnected_reconnects_and_retry_succeeds() {
        let breaker = breaker_with(5);
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || false, // health checks say the connection is lost
            fake_reconnect(&reconnects, Ok(())),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        std::future::pending().await
                    } else {
                        Ok(7)
                    }
                }
            },
        )
        .await;

        assert_eq!(result.unwrap(), 7);
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "exactly one retry");
    }

    #[tokio::test(start_paused = true)]
    async fn connection_error_reconnects_and_retry_succeeds() {
        let breaker = breaker_with(5);
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true,
            fake_reconnect(&reconnects, Ok(())),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        Err(AppError::ConnectionFailed("first attempt".into()))
                    } else {
                        Ok(7)
                    }
                }
            },
        )
        .await;

        assert_eq!(result.unwrap(), 7);
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Failure then success: the success resets the consecutive count.
        assert_eq!(breaker.state().await, CircuitState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn connection_error_on_retry_does_not_loop() {
        // Threshold 2: the first-attempt and retry connection errors are the
        // two consecutive recorded failures - the retry's record_failure is
        // load-bearing (the circuit must end Open).
        let breaker = breaker_with(2);
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true,
            fake_reconnect(&reconnects, Ok(())),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(AppError::ConnectionFailed("still down".into()))
                }
            },
        )
        .await;

        assert!(matches!(result, Err(AppError::ConnectionFailed(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "single retry, no loop");
        assert_eq!(reconnects.load(Ordering::SeqCst), 1, "single reconnect");
        assert_eq!(breaker.state().await, CircuitState::Open);
    }

    #[tokio::test(start_paused = true)]
    async fn non_connection_error_leaves_breaker_untouched() {
        // failure_threshold = 1: a single recorded failure would open the
        // circuit, so Closed-after proves record_failure was never called.
        let breaker = breaker_with(1);
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true,
            fake_reconnect(&reconnects, Ok(())),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(AppError::BadRequest("invalid input".into()))
                }
            },
        )
        .await;

        assert!(matches!(result, Err(AppError::BadRequest(_))));
        assert_eq!(breaker.state().await, CircuitState::Closed);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry");
        assert_eq!(reconnects.load(Ordering::SeqCst), 0, "no reconnect");
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_failure_propagates_without_retry() {
        let breaker = breaker_with(5);
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true,
            fake_reconnect(
                &reconnects,
                Err(AppError::ConnectionFailed("reconnect exhausted".into())),
            ),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(AppError::ConnectionFailed("first attempt".into()))
                }
            },
        )
        .await;

        assert!(
            matches!(&result, Err(AppError::ConnectionFailed(msg)) if msg.contains("reconnect exhausted"))
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "no retry after failed reconnect"
        );
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_on_retry_records_breaker_failure() {
        // Threshold 2: first-attempt connection error + retry timeout are the
        // two consecutive failures that open the circuit.
        let breaker = breaker_with(2);
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true,
            fake_reconnect(&reconnects, Ok(())),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        Err(AppError::ConnectionFailed("first attempt".into()))
                    } else {
                        std::future::pending().await
                    }
                }
            },
        )
        .await;

        assert!(
            matches!(&result, Err(AppError::OperationTimeout(msg)) if msg.contains("on retry"))
        );
        assert_eq!(breaker.state().await, CircuitState::Open);
    }

    #[tokio::test(start_paused = true)]
    async fn non_connection_error_on_retry_skips_breaker_failure() {
        // Threshold 2: only the first-attempt connection error is recorded;
        // the retry's BadRequest must not be, so the circuit stays Closed.
        let breaker = breaker_with(2);
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true,
            fake_reconnect(&reconnects, Ok(())),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        Err(AppError::ConnectionFailed("first attempt".into()))
                    } else {
                        Err(AppError::BadRequest("rejected by server".into()))
                    }
                }
            },
        )
        .await;

        assert!(matches!(result, Err(AppError::BadRequest(_))));
        assert_eq!(breaker.state().await, CircuitState::Closed);
    }

    #[tokio::test(start_paused = true)]
    async fn scoped_timeout_is_not_a_breaker_failure() {
        // H2 (session-02 round 1): a client-shortened deadline expiring says
        // nothing about an outage; with outage-signal false the breaker must
        // stay Closed even at failure_threshold 1.
        let breaker = breaker_with(1);
        let reconnects = Arc::new(AtomicU32::new(0));

        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            false, // client-scoped deadline
            || true,
            fake_reconnect(&reconnects, Ok(())),
            || async { std::future::pending().await },
        )
        .await;

        assert!(matches!(result, Err(AppError::OperationTimeout(_))));
        assert_eq!(breaker.state().await, CircuitState::Closed);
        assert_eq!(reconnects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_branch_reconnect_failure_propagates() {
        // The timeout-while-disconnected branch has its own `reconnect().await?`;
        // its failure must propagate without a retry.
        let breaker = breaker_with(5);
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || false, // disconnected
            fake_reconnect(
                &reconnects,
                Err(AppError::ConnectionFailed("reconnect exhausted".into())),
            ),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::future::pending().await
                }
            },
        )
        .await;

        assert!(
            matches!(&result, Err(AppError::ConnectionFailed(msg)) if msg.contains("reconnect exhausted"))
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "no retry after failed reconnect"
        );
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn half_open_probe_token_released_on_non_connection_error() {
        // H3 (session-02 round 1): a probe completing with a non-connection
        // error records neither counter but must hand its token back, or
        // recovery starves until the re-grant window.
        let breaker = breaker_with(1); // success_threshold 1 => single token
        breaker.record_failure().await;
        assert_eq!(breaker.state().await, CircuitState::Open);
        tokio::time::advance(Duration::from_secs(30)).await;

        let reconnects = Arc::new(AtomicU32::new(0));
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            true,
            || true,
            fake_reconnect(&reconnects, Ok(())),
            || async { Err(AppError::NotFound("no such topic".into())) },
        )
        .await;

        assert!(matches!(result, Err(AppError::NotFound(_))));
        assert_eq!(breaker.state().await, CircuitState::HalfOpen);
        // Without release_probe the single token would be gone and this
        // would be rejected until the re-grant window.
        assert!(
            breaker.allow_request().await,
            "released token must admit the next probe"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn scoped_timeout_while_disconnected_still_reconnects_without_breaker_failures() {
        // The H2 exemption suppresses breaker RECORDING for scoped
        // deadlines, not RECOVERY: a scoped request finding the connection
        // down must still trigger the reconnect, and neither its
        // first-attempt timeout nor its retry timeout may feed the breaker.
        let breaker = breaker_with(1); // any recorded failure would open it
        let calls = Arc::new(AtomicU32::new(0));
        let reconnects = Arc::new(AtomicU32::new(0));

        let op_calls = Arc::clone(&calls);
        let result: AppResult<u32> = run_resilient(
            &breaker,
            TIMEOUT,
            false,    // client-scoped deadline
            || false, // health checks say the connection is lost
            fake_reconnect(&reconnects, Ok(())),
            move || {
                let calls = Arc::clone(&op_calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::future::pending().await
                }
            },
        )
        .await;

        assert!(
            matches!(&result, Err(AppError::OperationTimeout(msg)) if msg.contains("on retry"))
        );
        assert_eq!(reconnects.load(Ordering::SeqCst), 1, "recovery still fires");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "single retry");
        assert_eq!(
            breaker.state().await,
            CircuitState::Closed,
            "scoped timeouts on both attempts must not feed the breaker"
        );
    }

    // =========================================================================
    // Error classifier
    // =========================================================================

    #[test]
    fn connection_variants_are_connection_errors() {
        for error in [
            AppError::ConnectionFailed("test".to_string()),
            AppError::Disconnected("connection lost".to_string()),
            AppError::ConnectionReset("reset by peer".to_string()),
        ] {
            assert!(is_connection_error(&error), "{error:?}");
        }
    }

    #[test]
    fn non_connection_variants_are_not_connection_errors() {
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
                !is_connection_error(&error),
                "Error {:?} should not be treated as connection error",
                error
            );
        }
    }
}
