# Session 03 — Plan Review, Round 1

**Target:** the session-03 plan (TD-2026-07-08 client-visible `X-Request-Timeout`
feedback + TD-2026-07-09 circuit-breaker enum-with-payloads and RAII `ProbePermit`),
reviewed against the tree at `tech-debt/session-03` @ `e12a513`. Scope = plan; no
implementation exists yet. The plan itself is a working artifact and is not committed
(this repo has never committed a plan file).

**Provenance:** config: full — step-0 gate attested by Maxim this session (8-agent
suite, both rounds, all agents plus the verifier pinned to Opus-class). Agent
fallbacks: none; all eight roster agents ran as their native types. Verification
tally: 20 claims re-read against cited `file:line`, **19 kept / 0 discarded**, 1
`drifted` (corrected inline, theme T10) and one embedded count correction (T15).

Reviewers cited in brackets: [consistency] general-purpose, [architect]
feature-dev:code-architect, [reviewer] feature-dev:code-reviewer, [types]
pr-review-toolkit:type-design-analyzer, [silent] pr-review-toolkit:silent-failure-hunter,
[comments] pr-review-toolkit:comment-analyzer, [tests] pr-review-toolkit:pr-test-analyzer,
[simplifier] pr-review-toolkit:code-simplifier.

---

## T1 — CRITICAL — The plan's central premise is false: `timeout_is_outage_signal` is not provenance

*[consistency] [architect] [reviewer] [types] [silent] [tests] [simplifier] — 7 of 8 agents, independently.*

Plan lines 54-57 assert that "provenance is already fully determined by the existing
`timeout_is_outage_signal` + `timeout` pair, so no new parameter is required." It is not.

`src/iggy_client/mod.rs:504`:
```rust
let timeout_is_outage_signal = self.op_deadline >= self.config.operation_timeout;
```

That is a **duration comparison**, not "did the client send a header". `with_timeout`
clamps at `mod.rs:1047` via `clamp_deadline` (`mod.rs:194-196`, `requested.min(global)`),
so every requested deadline ≥ the global collapses onto the global.

Concrete failure, with stock config (`OPERATION_TIMEOUT_SECS` default 30, `config.rs:196`;
`MAX_REQUEST_TIMEOUT_MS = 300_000`, `middleware/timeout.rs:68`):

| Client sends | Enforced | `timeout_is_outage_signal` | Plan's `client_deadline_ms` | Correct answer |
|---|---|---|---|---|
| *(nothing)* | 30 s | true | `None` | `None` ✓ |
| `5000` | 5 s | false | `Some(5000)` | `Some(5000)` ✓ |
| `30000` | 30 s | **true** | **`None`** ✗ | `Some(30000)` |
| `300000` | 30 s (clamped) | **true** | **`None`** ✗ | requested 300000, enforced 30000 |

The whole 30 s–300 s acceptance band — which includes the largest value a client can
legally send — renders a 504 byte-identical to a request that sent no header at all.
That is TD-08's problem statement item 2 (`TD-2026-07-08.md:14-16`) surviving the fix
intended to close it, and the clamped case is precisely where a diagnostic is most
load-bearing: the client asked for five minutes, silently got thirty seconds.

A second defect rides along: the value available at both construction sites
(`resilience.rs:186-189`, `:234-237`) is the **enforced** duration. TD-08's trigger
requires details that "name the client-requested deadline" — which is not the number
in scope.

**Remediation:** provenance must be **carried**, not inferred. Q-A stops being a
preference and becomes a requirement, and the type must hold both numbers.

---

## T2 — CRITICAL — The obvious fix for T1 would silently reverse an anti-DoS property

*[architect] [reviewer] [types] [simplifier].*

If `Deadline { Global, Client }` is introduced and the discriminant drives the breaker
predicate at `resilience.rs:157`, then every request carrying a header ≥ global stops
feeding the breaker — today it does feed it. `resilience.rs:56-65` already documents
that when client-deadline traffic dominates, the only guaranteed breaker feeder is the
background stats refresher and convergence to Open drops to minutes-scale. This would
widen that hole to *all* header-bearing traffic, so one client (hostile, or just an SDK
that always sets a large header) could blind outage detection for everyone.

A deadline expiring at exactly the global carries identical evidentiary value to the
global expiring — so the value-based predicate is the correct one and must stay.

**Remediation:** the type carries three independent projections; the breaker predicate
stays duration-derived. Never one discriminant serving both questions.

---

## T3 — HIGH — TD-08 is not fully discharged, yet M3 marks it resolved

*[consistency] [architect] [silent] [types].*

Three distinct holes; the plan closes roughly one and a half and then flips the record:

1. **The clamped/equal band stays silent** — T1.
2. **The reconnect-wait path reports the client's own expiry as a broker outage.**
   `mod.rs:478-481`, inside `reconnect_bounded`, bounds the wait with `self.op_deadline`
   (`:469` — the possibly client-scoped value) and on expiry returns
   `AppError::ConnectionFailed("Reconnection did not complete within {:?} …")`, which
   renders as **503 `connection_failed`** ("Message broker is temporarily unavailable"),
   not a 504. M1 only touches `OperationTimeout`. So a client whose own 100 ms deadline
   expired is affirmatively told the broker is down — TD-08 item 1 in a strictly worse
   form than the generic 504 it complains about.
3. **A rejected header is indistinguishable from an absent one.** Out-of-range
   (`timeout.rs:142-147`) and malformed (`:153-156`) both warn-log and drop the value;
   under M2 both yield `X-Effective-Timeout: <global>` — exactly what a header-less
   request gets. A client that typos `X-Request-Timeout: 5s` gets a 200 and never learns
   its integration is broken. TD-08 item 2 asks for confirmation the header was
   *honored*; the echo confirms only the enforced number.

Marking a TD resolved while its named silences are live is the deferral-without-a-plan
failure mode.

---

## T4 — HIGH — "on every response" is unachievable where the layer sits: 401 and 429 cannot carry the header

*[architect] [reviewer] [silent] [comments] [tests] — 5 of 8.*

`src/routes.rs` applies the timeout middleware at `:154`, auth at `:172`, rate limiting
at `:186`. Layers apply bottom-to-top, which the file states itself at `:136` ("order
matters - applied bottom to top") and `:177-178` ("applied last, so it runs FIRST …
outermost layer"). Both outer layers short-circuit without invoking the inner service —
`rate_limit.rs:403-414` returns the 429 directly, `auth.rs:273`/`:282` the 401 — so
`extract_request_timeout` never runs and cannot stamp anything.

The result is inverted from what a client needs: **present** on 404 (axum's `Router::layer`
wraps the fallback), **absent** on exactly the two rejection paths a client most wants to
diagnose. This is the same pre-existing reason 401s lack `X-Request-Id`.

Shipping an attested decision whose stated scope the wiring silently contradicts is the
class of client-facing lie TD-08 exists to remove.

---

## T5 — HIGH — The header would advertise a deadline on responses where nothing was enforced

*[architect] [reviewer] [silent] [simplifier].*

`health_check` (`handlers/health.rs:46`), `readiness_check` (`:79`) and `stats` (`:109`)
take no `Option<RequestTimeout>` and never call `*_scoped`; they read a cached flag
(`:47`, `:80`) or the background stats cache (`:110`). Under "every response",
`GET /health` with `X-Request-Timeout: 100` returns `X-Effective-Timeout: 100` while
nothing whatsoever was bounded by 100 ms.

Note this is **not** the divergence Q-B is guarding against, and a shared clamp function
cannot detect it: the two computations agree perfectly: the mismatch is between the
middleware's input and whether any handler built a scoped client at all. The plan's
proposed test matrix (absent / in-range / clamped / out-of-range) contains no case that
would catch it.

Related, from [architect] and [silent]: the header names a **per-attempt** bound.
`resilience.rs:42-46` documents the reconnect path costing up to 3× the deadline, and
`handlers/topics.rs:38`+`:40` issue *two* scoped operations per request — so a response
can legitimately take ~6× the number in the header.

---

## T6 — HIGH — `ProbePermit` as sketched closes 2 of TD-09's 3 leaks, not 3

*[consistency] [architect] [reviewer] [types] [simplifier] — 5 of 8.*

Plan lines 121-124 claim all three named accounting leaks become "unrepresentable".
Checked against `TD-2026-07-09.md:29-35`:

- **Phantom release** (admitted while Closed, releases a token it never consumed) — only
  fixed if `allow_request` stops returning `bool`. `circuit_breaker.rs:230` returns
  `true` on the Closed path with no token behind it; a uniform permit whose `Drop` calls
  release would mint a token from nothing. Fix: `fn allow_request(&self) -> Option<Admission>`
  where `Admission::{Ungated, Probe(ProbePermit)}` — `Ungated` has no release path, so
  the Closed case cannot phantom-release.
- **Dropped future** (client disconnect mid-probe) — genuinely fixed, and only fixable
  by `Drop`. This is the honest justification for RAII.
- **Straggler re-grant** — **not fixed.** `release_probe` (`:329-338`) checks only
  `state == HalfOpen` and the budget cap; it has no notion of *which* window a token
  belonged to. A permit minted in window N and dropped in window N+1 still credits N+1.

`granted_at: Instant` cannot serve as the window identity: every relevant test runs
`start_paused = true` and only moves the clock via `tokio::time::advance`, so two grants
without an intervening advance compare equal.

**Remediation:** add a monotonic `u64` generation to the `HalfOpen` payload, stamp it
into the permit, and release only on a generation match.

*Credit where due [simplifier]:* the permit also deletes the documented double-release at
`resilience.rs:67-71` — a stronger argument than the three leaks, and one the plan omits.

---

## T7 — HIGH — RAII release over a non-reentrant `std::sync::Mutex` is a self-deadlock hazard

*[architect] [reviewer] [types] [simplifier].*

`ProbePermit::drop` must take the same mutex that `record_success` (`:343-344`) and
`record_failure` (`:385-386`) take as their first statement. If a consuming API takes the
lock and *then* drops the permit — the most natural way to write it — it self-deadlocks.
Under `tokio::sync::RwLock` that hangs one task; under `std::sync::Mutex` it parks an OS
**worker thread**, and repeats exhaust the runtime on the path that gates every Iggy
operation. `panic = "abort"` (`Cargo.toml:100`) means there is no unwind to bail out.

Rust's drop order (locals before params) accidentally rescues the most obvious shape,
which makes this worse — it will work until someone restructures.

**Remediation:** the disarm must be a **lock-free** flag flip performed *before* any
guard is acquired (`breaker: Option<&'a CircuitBreaker>`, `consume()` sets it to `None`),
with a documented invariant "never drop a `ProbePermit` while a state guard is live",
plus a regression test. Borrow rather than `Arc` so a permit cannot be smuggled into a
detached `tokio::spawn`.

---

## T8 — HIGH — Both breaking-change commits will fail the "Conventional Commits" CI job

*[reviewer]; verified `present`.*

`.github/workflows/pr.yml:77`:
```
PATTERN="^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\(.+\))?: .+"
```
There is no `!?` before the colon, and the check at `:84` exits 1 on no-match.
`refactor(error)!: …` cannot match. `.commitlintrc.json` *does* permit `!` — but no
workflow anywhere runs commitlint (confirmed repo-wide; no husky, no pre-commit), so the
stricter regex is the only enforcer and it wins. Plan commits 1 and 4 both fail.

**Remediation:** land a `ci:` commit relaxing the pattern to `…(\(.+\))?!?: .+` first, or
drop `!` from subjects and declare breaks in a `BREAKING CHANGE:` footer (the check reads
only `%s`).

---

## T9 — MEDIUM-HIGH — Q-C and Q-D are both answered "yes", but the plan's stated reasons are factually wrong

*[architect] [reviewer] [silent].*

The plan justifies both with "every critical section is a handful of field assignments
with no panic path and no `await`". Verified false: the guarded regions call `tracing`
macros and the global metrics recorder throughout — `:264`, `:363`, `:446`, `:469-470`,
and `:318` inside `reject_request`, which is called at `:238` **while the read guard
taken at `:228` is still live**. `record_circuit_breaker_rejection` builds a labeled key
per call: hashing, a registry shard lock inside `metrics-exporter-prometheus`, and an
allocation.

Concrete consequence a panic there would leave: `open_now` (`:465-471`) sets internal
state to Open, then panics in `record_circuit_breaker_open()` before
`set_circuit_breaker_state(2)` — breaker Open, Prometheus gauge permanently reading 0
(closed). With `into_inner` and no logging, nobody is ever told.

**The conclusions survive, on better grounds:**
- **Q-C — yes.** The strong argument is `Cargo.toml:100` `panic = "abort"`: poisoning is
  structurally unreachable in release and `unwrap_or_else(PoisonError::into_inner)` only
  ever executes under `cargo test`. `clippy::unwrap_used` does not fire on
  `unwrap_or_else`, so `Cargo.toml:84-89` is satisfied with no `#[allow]`. In a `Drop`
  impl `into_inner` also avoids the panic-during-unwind → abort that `.unwrap()` risks.
- **Q-D — yes.** [architect] corrects the framing: tokio's `RwLock::read()` is a fair
  semaphore permit acquisition (atomic RMW, possible task park behind a queued writer),
  not a free shared read, and the HalfOpen path currently takes **two** locks (`:228`
  then `:246`) where a `Mutex` takes one. The change is a *win*, not a cost.

**Remediation (required for both answers to hold):** hoist logging and metrics out of the
critical sections — compute a small `Effect` value, drop the guard, then emit. That makes
"no panic path" true by construction, shrinks exclusive hold time, and removes the only
realistic poisoning source. If `into_inner` is retained it must `error!` + increment a
counter, not swallow.

---

## T10 — MEDIUM — Q-B's mitigation is tautological, targets the wrong divergence, and duplicates an existing function

*[architect] [types] [silent] [tests] [simplifier] [comments] — 6 of 8.*

Three separate problems with "extract a single `effective_deadline(client, global)`":

1. **It already exists.** `clamp_deadline` at `mod.rs:194-196` is that function.
   *(Verifier correction: the plan-review citation `192-194` was drifted; `192-193` are
   the doc comment.)*
2. **The test is vacuous.** If both sides call the same helper, "header == enforced"
   holds by construction and can never fail.
3. **It clamps against the wrong operand.** `with_timeout` clamps against
   `self.op_deadline` (`:1047`), deliberately — the comment at `:1044-1046` and the test
   at `:1111-1112` pin that re-scoping is shrink-only. A middleware clamping against
   `config.operation_timeout` agrees only while the handler scopes the *root* wrapper.
   Nothing enforces that.

**Remediation:** make the **value** the shared artifact, not a function. Build the
`Deadline` once in the middleware, put it in request extensions, stamp the header from
it, and pass the same value into the wrapper. The header cannot lie because it *is* what
was enforced. At least one assertion must *observe* the enforced deadline rather than
recompute it.

---

## T11 — MEDIUM — Commit 4 bundles three-to-four independent changes and rewrites its own oracle

*[consistency] [architect] [simplifier].*

`refactor(circuit-breaker)!: per-state payloads and RAII probe permit` contains: the
sync-lock conversion, the enum reshape, `allow_request`'s return-type change across 28
assert sites, and new `Drop` semantics — while mechanically rewriting the 17 breaker and
16 resilience tests that are the *only* specification of this behavior. A single diff
that changes semantics and rewrites every assertion cannot be reviewed for behavior
preservation, and `pr.yml` warns above 500 changed lines. `refactor` is also the wrong
conventional type for a behavioral delta, contradicting `TD-2026-07-09.md:42`'s own
"shape-only with no behavioral delta" framing.

**Remediation — split into three:**
- **4a** `refactor(circuit-breaker): flat struct → state enum with payloads` — `RwLock`
  retained, **all 17 tests byte-identical**. That invariance *is* the proof.
- **4b** `refactor(circuit-breaker)!: sync Mutex, breaker methods become sync` — purely
  mechanical `.await` removal.
- **4c** `feat(circuit-breaker)!: RAII ProbePermit replaces explicit release_probe` — the
  only commit carrying a behavioral delta.

---

## T12 — MEDIUM — M4 has zero documentation deliverables, and the breaker's usage example rots invisibly

*[comments] (lead) [architect] [types] [silent] [tests].*

M3 lists four doc items; **M4 lists none** — yet M4 falsifies the two largest prose blocks
in the crate (~150 lines).

- `circuit_breaker.rs:57-82` is a `rust,ignore` fenced example calling
  `cb.allow_request().await` (`:61`), `cb.record_success().await` (`:69`),
  `cb.record_failure().await` (`:73`), `cb.release_probe().await` (`:78`). Under M4 all
  four are wrong and the final arm teaches an API RAII deletes. **`ignore` means
  `cargo test --doc` will never catch it** — permanent silent rot. Drop `,ignore` after
  rewriting so the compiler pins it thereafter.
- `resilience.rs:1-71` — semantics #5 names `timeout_is_outage_signal` by identifier
  (`:21-29`); `:96-103` documents the six parameters by name; `:66-71` documents the
  double-release that M4 makes unrepresentable.
- Per-method docs at `circuit_breaker.rs:175-183` ("Uses RwLock internally"), `:201-223`
  (the re-grant "anti-wedge guarantee" the permit supersedes), `:293-297`, `:303-308`,
  `:322-328`, `:340-342`, `:381-384`, `:420-423`, `:435-448`, `:450-453`.
- `TD-2026-07-03.md`'s resolution recorded a **two-site invariant** — the breaker module
  doc *and* the `IggyClientWrapper` struct doc (`mod.rs:28`, `:139-146`) must move
  together. The plan names neither.

M3 additions [comments] [reviewer]: `error.rs:163` (whose `// Never expose internal
details to clients` becomes the policy M1 breaks), `README.md:46`, `README.md:611`,
`routes.rs:25` + `:152-153`, `timeout.rs:28-34` + `:42-49` + `:140-141`, `CLAUDE.md:142`.

---

## T13 — MEDIUM — Test-coverage gaps at exactly the places the plan changes

*[tests] (lead) [comments].*

- **`src/error.rs` has no test module at all** (verified: zero `cfg(test)` matches in 207
  lines; no `AppError` reference in `tests/`). M1 rewrites its rendering — status,
  message, and the `details` gating — with nothing checking any of it.
- **Nothing in the suite produces a 504.** `tests/integration_tests.rs:988-1035` exercises
  four header paths and asserts status only (`:1001`, `:1013`, `:1024`, `:1034`) — no
  header reads, no timeout path. TD-08's headline case is untested.
- **The concurrency test cannot survive the sync switch.** `circuit_breaker.rs:722-739`
  uses `tokio::join!(cb.allow_request(), cb.allow_request())`, and its own comment
  (`:723-726`) names the write-lock yield point as the mechanism. Sync methods aren't
  futures — `join!` won't compile, and the obvious rewrite (two sequential calls) passes
  even with the cap removed. `start_paused` also forbids `multi_thread`, and `std::thread`
  racers escape the frozen clock.
- **Two of TD-09's three leaks have no planned test** — phantom-release-after-transition
  and straggler-after-re-grant. The plan's three tests map to the dropped-future leak plus
  one new property.
- **"Permit released on drop" tests the wrong thing** — a scope-exit drop proves `Drop` is
  wired; the leak TD-09 names is *cancellation*. Make it a mid-flight future drop.
  Relatedly, `reconnect().await?` at `resilience.rs:140`/`:179` exits with a live token and
  no release today; RAII changes that — an observable delta, and both existing tests of
  that path use Closed-state breakers so neither would notice.
- **Churn is understated in the direction that matters.** Of 123 breaker `.await` sites,
  **105 are inside `#[cfg(test)]`** — ~85% of the churn is a mechanical edit pass across
  the suites that are this feature's only specification. That is exactly where "make it
  compile" quietly weakens an assertion.
- **Every breaker Prometheus transition is rewritten and none is asserted** — the
  `"open"` vs `"half_open"` rejection label, documented at `:313-315` as distinguishing
  "materially different situations", has no coverage at all.

---

## T14 — MEDIUM — Q-E is moot: both accessors have zero callers

*[consistency] [architect] [reviewer] [types] [tests] [simplifier] — 6 of 8, unanimous on the answer.*

`circuit_breaker_state()` (`mod.rs:1052`), `force_close_circuit()` (`:1067`) and
`circuit_breaker_metrics()` (`:1059`) have **no callers anywhere** in `src/`, `tests/`, or
`fuzz/`. `src/state.rs` contains zero breaker references, so the plan's "keep `AppState`'s
surface stable" rationale (line 162) is unsupported — `AppState` never touches them. The
crate is `publish = false` (`Cargo.toml:10`) and `cargo semver-checks` is
`continue-on-error` (`pr.yml:154`).

**Answer: make them sync** (or delete them). An `async fn` that awaits nothing advertises
a suspension point that does not exist, and `clippy::unused_async` is not enabled so CI
will never flag it. Do not spend a "second breaking change" budget defending a caller set
of size zero.

---

## T15 — MEDIUM — Count drift in the plan, and two stale counts in the repo

*[consistency] [comments] [tests]; resolved by the verifier.*

- Resilience tests: plan says 15; actual **16** (14 composition `:277`-`:712` + 2
  classifier `:718`, `:729`).
- Breaker tests: plan says 17; **correct** (the naive grep returns 7 because 10 carry
  `(start_paused = true)`).
- Call sites: plan says 124; actual **123**, split **105 test / 18 non-test** — and 4 of
  those 18 are the doc-comment lines of T12's example, so **14 genuine production call
  sites**. (Reviewer figures of 124 and 131 are both wrong.)
- `README.md:611` documents the timeout error as `operation_timeout` / **503**; the code
  renders `"timeout"` / **504** (`error.rs:128-132`). Both columns wrong, and M1 edits
  that row's body.
- `README.md:46` states "183 unit tests, 30 integration tests, 18 model tests" — a live
  claim this session invalidates. `CHANGELOG.md:40` carries the same numbers inside the
  released `## [0.3.0]` section and must **not** be touched.

---

## T16 — MEDIUM — The enum refactor will silently drop a recorded probe success

*[simplifier]; verified `present`. Single-source but high-value.*

`grant_probe_tokens` (`:298-301`) writes only `half_open_probes_remaining` and
`half_open_granted_at`. Its two callers differ on the third field:

- HalfOpen **entry** (`:255-257`) sets `consecutive_successes = 0` as a separate statement
  before calling it.
- **Re-grant** (`:284-285`) does not — it must *preserve* the accumulated count.

Under `State::HalfOpen { probes_remaining, granted_at, consecutive_successes }` you can no
longer mutate two fields in isolation. The obvious port —
`*state = State::HalfOpen { …, consecutive_successes: 0 }` at both sites — **drops a
recorded success at the re-grant**, and no existing test catches it:
`test_half_open_probe_tokens_regrant_after_open_duration` (`:646-668`) never calls
`record_success`.

**Remediation:** two constructors — `enter_half_open(budget)` (successes = 0) and a
payload-preserving `regrant(...)`. Add the missing test (success → advance past window →
re-grant → second success closes) **before** the refactor, so it pins current behavior.

---

## T17 — MEDIUM — The plan edits 100% of the `run_resilient`/`retry_once` duplication without collapsing it

*[simplifier].*

The two functions perform identical breaker bookkeeping on the same three outcome classes
(`:133-136`/`:210-213` success, `:138`/`:216` connection failure, `:147`/`:220` release,
`:157-169`/`:225-233` timeout); the only real difference is escalate-vs-give-up. M1
rewrites both `OperationTimeout` constructions and Q-A rewrites both signatures and all
four flag reads — so the plan already touches essentially the whole duplicated surface.
Consolidating is close to free now and will not be free later.

**Remediation:** extract bookkeeping only, no control flow, as its own commit *before* M1.
The 16-test matrix passing untouched is the proof the extraction was behavior-preserving.

---

## T18 — LOW/MEDIUM — Accumulated smaller findings

- **Browser clients cannot read the new header.** Neither CORS branch sets
  `expose_headers` (`routes.rs:214-217`, `:239-242`); the token appears nowhere in the
  repo. The header's entire purpose is client visibility. Same pre-existing gap affects
  `X-Request-Id`. *[consistency] [reviewer]*
- **`error.rs:163` is the single enforcement point for "never expose internals".** Widen
  the match at `:76-158` to a 4-tuple so every arm must state its exposure decision, and a
  new variant cannot compile without deciding. *[types]*
- **`OperationTimeout { message: String, … }` has zero encapsulation** — enum struct-variant
  fields are as public as the enum (`lib.rs:75`), and the message duplicates the deadline
  already in the payload. Compare `RequestTimeout` (`timeout.rs:79-102`): private field,
  one constructor, invalid states unrepresentable. Prefer `OperationTimeout(TimeoutContext)`
  with private fields, and make "on retry" a field rather than a substring asserted at
  `resilience.rs:545`, `:703`. *[types] [simplifier]*
- **The Prometheus gauge is a third encoding of breaker state**, set with bare `0`/`1`/`2`
  at four sites — the same convention-maintained pattern TD-09 exists to kill. Give
  `CircuitState` a `gauge()` projection. *[types]*
- **`probe_budget()`'s `.max(1)` is the sole guard against a zero budget**;
  `CIRCUIT_BREAKER_SUCCESS_THRESHOLD=0` is reachable (`config.rs:203-206`, `parse_env` has
  no range check, `validate()` at `:244-281` checks neither breaker threshold). In release,
  overflow-checks-off turns the underflow into `u32::MAX` — the probe cap silently
  disappears. `failure_threshold` has no `.max(1)` equivalent at all. *[reviewer] [types]*
- **`OPERATION_TIMEOUT_SECS=0` is unvalidated** — every operation times out instantly and,
  being global-provenance, records a breaker failure. A validating `Deadline` constructor
  is the natural place to make that unrepresentable. *[types]*
- **Pass `Duration`, not `AppState`, to the timeout middleware.** `AppState::new` requires
  a live wrapper and spawns background tasks (`state.rs:132-133`), which would push the
  parity test into the testcontainers suite; `config.operation_timeout` keeps it a unit
  test next to `timeout.rs:163-233`. *[architect] [tests]*
- **`release_probe` should become private** once permits exist — leaving it `pub(super)`
  preserves a way to release a token nobody holds, the exact hole T6 closes. *[types]*
- **`ProbePermit::drop` deletes the operator's only stalled-recovery signal.** The
  re-grant `info!` at `:284` is today the sole indication that probes are being lost; once
  the permit hands tokens back it stops firing and the underlying condition goes
  invisible. Add a `debug!` + counter on the Drop path; keep the re-grant `info!` as a
  wedge alarm. *[silent]*
- **TD records need a `## Resolution (session NN)` section**, per the convention in
  `TD-2026-07-03.md` and `TD-2026-07-04.md` — a bare status flip leaves TD-08's
  "Mitigations in place (session 02)" reading as current state. *[comments]*
- **"Regenerate the registry index" implies tooling that does not exist** — no justfile,
  no Makefile, no scripts dir. `docs/tech-debt/README.md` is a hand-maintained table
  (TD-08 and TD-09 are rows `:15` and `:16`). *[consistency] [comments]*
- **Review artifacts are missing from the commit sequence.** Session 02 shipped
  `docs(code-reviews): session-02 round-{1,2} review artifact` as commits; the plan's five
  contain no equivalent. *[consistency]*
- **`panic = "abort"` makes half of M4's safety story profile-dependent.** `Drop` does not
  run on panic in release, so `ProbePermit` has no panic-safety in production — the paths
  that matter are *cancellation* (`tokio::time::timeout` dropping the operation future at
  `resilience.rs:132`, `:209`, client disconnect, `CancellationToken` shutdown). Document
  that, and test the cancellation path specifically. *[silent]*

---

## Verdict

**Do not begin implementation.** T1 invalidates M1's design premise, T2 shows the obvious
repair carries an anti-DoS regression, T3 means the plan would close TD-08 while three of
its silences remain live, and T4/T5 mean the attested "every response" scope is partly
unimplementable and partly untruthful. T6 and T7 mean M4 as written under-delivers its
headline claim and carries a runtime-wedge hazard.

Remediation of Round 1 proceeds in batched edits to the plan, followed by Round 2 against
the remediated plan — where regressions introduced by these edits are the expected find.

**Answers to the five open questions, as resolved by this round:**

| Q | Answer |
|---|---|
| **Q-A** | **Yes — required, not optional.** But not `{ Global(Duration), Client(Duration) }`: the type must carry `requested` and `enforced` separately, be **constructed** where provenance is known, and expose three independent projections so the breaker predicate stays duration-derived (T1, T2). |
| **Q-B** | **No — insufficient.** Share the *value*, not a function; `clamp_deadline` already exists; and the real gap is handlers that enforce nothing at all (T5, T10). |
| **Q-C** | **Yes**, but on the `panic = "abort"` argument, and only after metrics/tracing move out of the critical sections (T9). |
| **Q-D** | **Yes** — and it is a performance *win*, not a cost; the plan's justification and its claimed arm-deletion are both wrong (T9). |
| **Q-E** | **Moot — make them sync.** Zero callers repo-wide (T14). |
