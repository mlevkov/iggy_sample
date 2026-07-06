# Session 02 — Code Review, Round 1

**Target:** branch `tech-debt/session-02` vs `main` (9 commits, 27 files) — resilience
executor extraction + paused-clock matrix, half-open probe token limiting,
X-Request-Timeout end-to-end enforcement, metrics exporter smoke test, SHA-pinned
workflows, durable-storage guide re-validation, TD registry updates.

**Provenance:** config: full (Fable 5, high effort, feature-dev + pr-review-toolkit
installed; all 8 agents + verifier pinned to the strongest model; attested by Maxim
via the step-0 gate). Agent fallbacks: none — all eight roster agents ran as their
native types. Verification tally: 18 claims verified, **18 kept / 0 discarded**
(one sub-point of theme 10 trimmed as overstated: `messages_required_to_save`
placement is consistent between the two guides).

Reviewers cited in brackets: [consistency] general-purpose, [architect]
feature-dev:code-architect, [reviewer] feature-dev:code-reviewer, [types]
type-design-analyzer, [silent] silent-failure-hunter, [comments] comment-analyzer,
[tests] pr-test-analyzer, [simplifier] code-simplifier.

---

## HIGH

### H1. Request-scoped timeout leaks into the process-global reconnect session
[architect, types, silent, reviewer, comments — 5/8 agents]
`src/iggy_client/mod.rs:1005-1008` (`with_timeout`), `:444-445` (`reconnect_bounded`
spawns from the scoped clone), `:384` (`attempt_timeout = operation_timeout / 2`).

`with_timeout` expresses the request deadline by mutating `config.operation_timeout`
on a clone, but that field is overloaded: it is also the reconnect session's
per-attempt connect bound. A request scoped to the 100ms header minimum that trips
the reconnect path spawns the **singleton** reconnect session (gated by the
Arc-shared `ConnectionState::start_reconnecting`) with a 50ms connect bound and, at
the default infinite retry setting, that session can loop forever unable to complete
TCP+auth on anything but localhost — while every other caller joins it as a
follower. One short-deadline client during an outage quietly cripples app-level
reconnection process-wide. `health_check` on a scoped clone has the same latent
issue. The `with_timeout` doc claim "only the deadline changes" is falsified.

**Remediation:** separate the per-request deadline from wrapper configuration —
`config: Arc<Config>` plus a distinct `op_deadline: Duration` field read only by
`with_reconnect`/the caller's `reconnect_bounded` wait; `reconnect()` and
`health_check` always read the global `config.operation_timeout`. Also eliminates
the per-request deep `Config` clone (strings incl. credentials) flagged separately
[types LOW, architect MEDIUM].

### H2. Client-shortened timeouts feed the shared circuit breaker — one-client DoS
[silent]
`src/iggy_client/resilience.rs:105-110` with `mod.rs` (Arc-shared breaker).

Every timeout records a breaker failure, and the timeout is now client-controlled
via `X-Request-Timeout` while the breaker is shared across scoped views.
`failure_threshold` (default 5) consecutive requests with a 100ms header against an
operation that legitimately takes longer open the **global** circuit: all clients
get 503 for `open_duration`, repeatable indefinitely. The timeout-as-outage-signal
rationale only holds for the *global* deadline.

**Remediation:** pass deadline provenance into `run_resilient`; record a timeout as
a breaker failure only when the deadline was NOT client-shortened. Covered by new
matrix tests.

### H3. Half-open probe tokens leak on non-connection outcomes — starvation against a healthy server
[silent HIGH, architect LOW]
`src/iggy_client/resilience.rs:101-104`, `:152-156`; `circuit_breaker.rs:262-278`.

An admitted half-open probe whose operation completes with a non-connection error
(BadRequest, NotFound, …) records neither success nor failure — the token is
consumed and never returned, although a completed round-trip is transport-health
evidence. If the first `success_threshold` callers after recovery hit such errors,
all traffic is rejected 503 for a full `open_duration`, and the cycle can repeat
indefinitely. During that window there is no log above debug and rejections share
one unlabeled counter with open-state rejections (see M2).

**Remediation:** release the probe token when the outcome is deliberately
unrecorded (`CircuitBreaker::release_probe()` called from both non-connection
arms); do NOT record success (local errors like validation failures never touched
the server, so treating them as transport success would falsely close the circuit).

### H4. The clamp and the scoped wiring are untested — the headline TD-04 property has no regression net
[tests HIGH, consistency MEDIUM]
`src/iggy_client/mod.rs:1005-1008`; `tests/integration_tests.rs:989-1034`.

All four integration-test assertions pass identically if the header is silently
dropped; nothing anywhere tests `timeout.min(global)` — the security property that
clients can shorten but never extend (300s header max vs 30s global = 10x extension
if the `.min` regresses). Enforcement of the timeout *parameter* is well covered at
the `run_resilient` level; the untested surface is the clamp + wiring.

**Remediation:** extract the clamp into a free function with direct unit tests
(both directions); soften TD-04's "covered by an end-to-end test" wording to
"propagation smoke test". (Falls out naturally from the H1 refactor.)

---

## MEDIUM

### M1. Post-reconnect retry bypasses the breaker gate and token accounting
[architect]
`src/iggy_client/resilience.rs:138-166`; `circuit_breaker.rs:320-323`.

`retry_once` never calls `allow_request`: a half-open probe's retry is a second
server operation on one token (2x the documented probe budget), and when the first
probe's failure has reopened the circuit, the retry still executes while Open — its
success lands in `record_success`'s Open arm, proving the "Shouldn't happen" comment
and `warn!` wrong (also reachable via the legitimate two-probe race [silent, reviewer]).
**Disposition:** documented as intentional (single bounded retry after a successful
reconnect is the feature; re-gating would fail requests whose reconnect just
succeeded). The Open-arm log is downgraded and reworded; both module docs note the
bypass explicitly.

### M2. Observability gaps in the new breaker/timeout paths
[silent, tests]
- Half-open rejections indistinguishable from open-state rejections in metrics
  (`metrics.rs:150-152` single unlabeled counter) and emit no log line.
- Token re-grant logs only `debug!` although it means a probe window produced no
  recorded outcome.
- The timeout-while-connected branch records a breaker failure at `debug!` only —
  the circuit can march to open with zero operator-visible output at
  `RUST_LOG=info`.
- The `CircuitOpen` error message re-reads state after rejection and can report a
  stale value ("Circuit breaker is closed - temporarily unavailable").

**Remediation:** state-labeled rejection metric + rejection/re-grant logging;
timeout branch logs `warn!` when the deadline is the outage-relevant global one;
error message reworded to "current state".

### M3. Client-facing silences in the timeout header path
[silent]
`src/middleware/timeout.rs:106-127`; `src/error.rs`.
Malformed headers are dropped at `debug!` (client believes a deadline is in effect);
a fired client deadline returns the generic 504 indistinguishable from server
slowness. **Disposition:** partially remediated (warn-level log for malformed
headers); response-header echo / error-detail enrichment deferred — recorded as
TD-2026-07-08 with a binding trigger.

### M4. `RequestTimeout` invariants unenforced + dead `RequestTimeoutExt` + 14x extension boilerplate
[types, simplifier, architect, comments, consistency — 5/8 agents]
`src/middleware/timeout.rs:74-95`, `:132-145`; 14 handler sites.
Public fields bypass `from_millis` validation (a zero-duration extension would
poison every op); `original_ms` is never logged; `RequestTimeoutExt` has zero
callers and advertises an unclamped second extraction path; every handler repeats
`timeout.map(|Extension(t)| t)`.
**Remediation:** private fields + `duration()` accessor, drop `original_ms`, delete
the trait + re-export, implement axum 0.8 `OptionalFromRequestParts` so handlers
take `Option<RequestTimeout>` directly.

### M5. Documentation drift and durability overclaims
[consistency, comments]
- `docs/partitioning-guide.md:696-768` still teaches the pre-0.8.0 schema
  (`message_expiry`/`archive_expired` under `[system.segment]`,
  `delete_oldest_segments`) now contradicting the re-validated storage guide.
- `docs/README.md:11,29` still advertises built-in S3/disk archiving.
- `docs/durable-storage-guide.md` states `enforce_fsync = true` ⇒ "data guaranteed
  on disk" while TD-2026-07-06 itself records the 0.8.0 saver flushes with
  `fsync: false`; two different configs both named "Balanced"; benchmark table
  carries no not-re-validated note; TOC omits the Sources section.
- `CLAUDE.md` timeout section omits the up-to-3x worst-case caveat.
- `src/iggy_client/mod.rs:378-383` reconnect comment contradicts
  `reconnect_bounded`'s no-abort design.
- `circuit_breaker.rs:13-32,52-71` module example misses `.await`s and records
  every error as failure; diagram arrows mislabeled.
- `timeout.rs:46-47` "zero or negative values are rejected" — they are ignored.

**Remediation:** all corrected in Round 1 remediation.

### M6. Test-matrix gaps
[tests]
Timeout-branch reconnect failure untested; `retry_once` connection-error breaker
recording unasserted (deleting the `record_failure` passes the suite); no
concurrency test races two `allow_request` callers in HalfOpen; re-grant boundary
untested just-below the window. **Remediation:** all four added; legacy sleep-based
breaker tests converted to paused clock while the file is hot [tests LOW].

---

## LOW

- **L1** `force_open` while already Open refreshes `opened_at`/inflates
  `times_opened`, contradicting the no-refresh policy [architect] — guarded.
- **L2** Rejection bookkeeping triplicated in `allow_request`; HalfOpen transition
  log computes `probes = remaining + 1` after decrementing [simplifier] — helper
  extracted; log moved before decrement.
- **L3** `force_close` leaves stale half-open token fields (benign; every HalfOpen
  entry re-grants) [tests, types] — reset added for hygiene.
- **L4** Metrics smoke test: `reqwest::get` unbounded per-request; integration test
  uses the literal 100ms minimum as a live bound (CI flake risk) [tests, silent] —
  client timeout added; header raised to 1000ms.
- **L5** TD-2026-07-02 note cites `version-update:semver-prerelease`, which is not
  a documented Dependabot `ignore.update-types` value [consistency, verified
  against GitHub docs] — note reworded (pre-existing `dependabot.yml` entry left;
  it is inert and Dependabot does not propose prereleases from stable regardless).
- **L6** `CircuitBreakerConfig` accepts `success_threshold = 0` and the `max(1)`
  patch lives at the grant site rather than the constructor [types] — accepted
  as-is: fields are `pub` so constructor validation is bypassable anyway; the grant
  site is the single consumer needing the floor, and the degenerate case is pinned
  by a regression test.
- **L7** Resilience test-matrix op-counter scaffolding repeats 8x [simplifier] —
  accepted: agent's own verdict is that standalone-readable tests are a feature;
  no clever helper forced.

## Not defects (verified, no action)

- `release.yml` retains the crates.io publish job against `publish = false`
  [reviewer, consistency flagged] — explicit session-01 decision (commit `2b58c23`
  "keep the crates.io publish job; publish=false is the gate"); prior decision, not
  a defect.
- Probe-token arithmetic underflow, `allow_request` read/write-path race handling,
  clamp direction, error-taxonomy preservation, workflow SHA pins (all six
  spot-verified upstream), test-count claims, TD-record accuracy — verified clean
  by multiple agents.

## Ratings (type-design lens)

`run_resilient` signature 9/9/9/9 (encapsulation/invariants/usefulness/enforcement)
— "at-most-once reconnect and no-retry-loop are compile-time facts". Scoped
`with_timeout` views 4/3/8/4 pre-remediation (the H1 refactor is the fix).
`CircuitBreakerState` 8/4/8/6 — enum-with-payload restructuring noted as the ideal
shape; deferred (see TD-2026-07-09) since all mutation is already lock-serialized
and Round 1 adds probe-release + reset hygiene instead.
