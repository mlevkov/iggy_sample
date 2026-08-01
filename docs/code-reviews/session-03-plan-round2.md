# Session 03 — Plan Review, Round 2

**Target:** the Round-1-remediated session-03 plan, reviewed against the tree at
`tech-debt/session-03` @ `e12a513`. Scope = plan. Round 2's purpose is to catch
regressions introduced by Round 1's remediation — and it did.

**Provenance:** config: full — step-0 gate attested by Maxim this session (8-agent
suite, both rounds, Opus-class). Agent fallbacks: none. Verification: this round's
agents each re-read their own citations; the comment lens independently re-verified
**47 citations (45 exact, 2 drifted, both LOW)** and re-derived all four disputed
counts from scratch — all four confirmed. Round 1's one drift correction
(`mod.rs:192-194` → `:194-196`) was absorbed correctly in both places.

Reviewers cited in brackets as in Round 1.

**Headline:** Round 1's structural findings are genuinely closed — T1, T2, T4, T6,
T7, T8, T11, T14, T15, T16, T17 all verified resolved. But the remediation created
four new defects of its own, two of them the same anti-patterns the session exists to
remove. **Do not begin implementation.**

---

## R2-A — CRITICAL — TD-08's third silence was dropped in the rewrite, and M5 would pin it as a contract

*[consistency] [architect] [reviewer] [silent] [tests] — 5 of 8.*

Round 1 T3 named three holes. The remediation closed items 1 and 2 and **silently
dropped item 3**, while M3 still flips TD-08 to resolved. Grepping the remediated plan
for `malform|reject|ignor`, the only hit is a comment-rewrite line in M3.

With the stock 30 s global, four distinct client intents produce one byte-identical
response:

| Client sends | Parse result | `X-Effective-Timeout` |
|---|---|---|
| *(nothing)* | `None` | `30000` |
| `"5s"` (malformed, `timeout.rs:153-156`) | `None` | `30000` |
| `50` (below MIN, `timeout.rs:142-147`) | `None` | `30000` |
| `300000` (valid, clamped at `mod.rs:1047`) | `Some` → 30 s | `30000` |

That is `TD-2026-07-08.md:14-16` verbatim, including its own parenthetical
*"malformed headers are warn-logged server-side as of session 02, but the client still
sees nothing."*

**It is now worse than unfixed.** M5's `extract_request_timeout` table test enumerates
`< MIN`, `0`, `"-1"`, non-numeric, whitespace-padded, non-ASCII — and every one of
those rows would assert the *same* observable output as the absent case. Round 1 warned
about relabelling silences as contracts; a test that pins them converts an
acknowledged gap into intended behavior.

**A second consequence [consistency]:** M1's `details` fix fires only on the 504 path,
so on a **200** response a clamped header is still indistinguishable from no header.
TD-08 item 2 asks a client to confirm its header was *honored*; a clamped header was
not honored, and a single-number echo cannot say so.

**Root cause is type-level, one variant from fixed:** `client_requested: Option<Duration>`
makes `None` mean both "absent" and "rejected".

**Remediation options, strongest first:** (1) `400 Bad Request` on a present-but-unusable
header — v0.4.0 is already breaking, and `AppError::BadRequest` echoes its message
(`error.rs:157`); (2) a three-state source projection plus a second header
(`X-Timeout-Source: client|clamped|default|rejected-range|rejected-malformed`),
added to `expose_headers`; (3) minimum acceptable — **do not resolve TD-08**; rename
commit 6, and add a `## Residual (session 03)` section with a binding trigger.

**Also newly found [tests]:** `timeout.rs:129-131` — the `warn!` at `:153` sits *inside*
the `to_str()` success branch, so a non-UTF-8 header value is dropped with **no log at
all**. A fourth silence class, quieter than the two already named.

---

## R2-B — CRITICAL — `Deadline.outage_signal` as a stored bool re-creates the anti-pattern TD-09 exists to delete

*[architect] [types] [simplifier].*

`outage_signal` is *defined* as `per_attempt >= global`, but `global` is not a field.
So given a `Deadline` in isolation, nothing — not the type, not a `debug_assert`, not a
test — can check the field against its own definition. Consistency is maintained by
convention at every construction site. Round 1's own T18 condemned exactly this shape
for the Prometheus gauge ("a third encoding … the convention-maintained pattern TD-09
exists to kill"), and the remediation introduced it in the commit that is supposed to be
the type-design win. Round 1 asked for three **projections**; the plan delivered three
**fields**.

Both drift directions are one assignment away and both are security-relevant:

- `outage_signal: true` with `per_attempt: 5s` (global 30 s) → a client's short deadline
  feeds the shared breaker. N slow short-deadline clients open the circuit for everyone
  — **T2's DoS in mirror image, introduced by T2's fix.**
- `outage_signal: false` with `per_attempt == global` → T2's exemption hole verbatim.

Not reachable in today's call graph, but **unenforced** — and M2 adds the second builder
that makes agreement a convention. It *is* reachable today in the test module:
`mod.rs:1085-1092` builds `IggyClientWrapper` by struct literal and would gain the new
field, and M5 designates those tests as the specification.

**Remediation:** store `global`, delete the bool.

```rust
pub(crate) struct Deadline {
    global: Duration,                   // non-zero by constructor
    client_requested: Option<Duration>, // unclamped
    per_attempt: Duration,              // enforced
}
impl Deadline {
    pub(crate) fn is_outage_signal(&self) -> bool { self.per_attempt >= self.global }
    pub(crate) fn was_clamped(&self) -> bool {
        self.client_requested.is_some_and(|r| r > self.per_attempt)
    }
    #[must_use] pub(crate) fn narrow_to(self, d: Duration) -> Self { /* only mutator */ }
}
```

Smart constructors alone are **not** sufficient — they make the first derivation correct;
`with_timeout`'s clamp is where it rots. `was_clamped()` is also exactly the fact R2-A
needs, and a validating constructor discharges the `OPERATION_TIMEOUT_SECS=0` item the
plan parked in Out-of-scope.

---

## R2-C — CRITICAL — M1 and M2 specify mutually exclusive construction sites, the root path was missed, and one branch deletes a pinned security property

*[consistency] [architect] [reviewer] [types] [silent] [simplifier] — 6 of 8.*

The plan says all three of: built **once** in `with_timeout`; carried "next to
`op_deadline`"; and `IggyClientWrapper` "gains `client_requested`". Those are three
different layouts, and M2 adds a fourth builder (the middleware). The choice is
load-bearing: M2's headline claim — *"the header cannot diverge from enforcement because
it **is** what was enforced"* — holds only under one reading.

**Two construction sites the plan missed entirely:**
- **The root wrapper** is a struct literal at `mod.rs:226-232` inside `new()`, reached
  from `main.rs:70`. It never calls `with_timeout`.
- **`unconnected_wrapper()`** at `mod.rs:1085-1092` — the fixture for
  `test_with_timeout_wiring_clamps_and_is_shrink_only`. `session-02-round2.md:30-32`
  records that deleting the `.min` clamp passed all 180 tests *until this fixture
  existed*. It breaks on the field change and is in no test count.

**The shrink-only property is silently at stake.** `with_timeout` clamps against
`self.op_deadline` (`mod.rs:1047`), deliberately — documented at `:1044-1046`, pinned at
`:1109-1112`, and a recorded `TD-2026-07-04` security property. If the wrapper consumes
the middleware's value verbatim, that property and its test die. If it re-clamps, the
already-stamped header can exceed what was enforced — the one failure M2 exists to
prevent — and M5's "header parity observes enforcement, not a recomputation" names a
test the architecture forbids.

**Two further blockers on this path:** `clamp_deadline` is **private** (`mod.rs:194`), so
M2's "do not mint a twin" is unimplementable as written; and threading a new type through
`*_scoped` ripples to **13 handler signatures** plus the `OptionalFromRequestParts` impl
(`timeout.rs:109-121`) — uncounted.

**Remediation — one construction rule, written down.** Two private constructors
(`Deadline::global`, `Deadline::for_request`) plus one narrower (`narrow_to`), routed
through by `new()`, `with_timeout`, the fixture, and the middleware. Then pick a single
authority for the header: either (a) *enforcement is authoritative* — the middleware
seeds a slot extension, `*_scoped` writes the post-narrow value into it, the middleware
stamps from the slot on the response path (this also **closes** T5 for free, because
`/health`, `/ready`, `/stats` never fill the slot and therefore omit the header rather
than advertising a fiction); or (b) *middleware is authoritative* with
`debug_assert!` that the second clamp is a no-op. **[simplifier] offers a cheaper (c):**
keep `Option<RequestTimeout>` everywhere (zero handler churn), make `clamp_deadline`
`pub(crate)`, and satisfy T10 with a test that reads back the wrapper's deadline
accessor after `iggy_scoped` — ~16 fewer changed sites.

---

## R2-D — HIGH — M4c's metrics hoist introduces a gauge race that leaves Prometheus permanently wrong

*[reviewer] [silent] — both traced the interleaving independently.*

Today every gauge write happens inside the exclusive guard (`:264` →1, `:363` →0,
`:446` →0, `:469-470` →2). The lock serializes them, so gauge writes are totally
ordered consistently with state transitions and last-writer-wins is always correct.
Under "compute an `Effect`, drop the guard, then emit":

```
T1: lock → Closed→Open,    Effect(gauge=2) → unlock → [preempted inside metrics registry]
T2: lock → Open→HalfOpen,  Effect(gauge=1) → unlock → emits 1
T1: resumes                                          → emits 2
```

Final state HalfOpen, gauge 2. **Nothing corrects it** — the gauge is written only on
transitions, and a HalfOpen breaker with an exhausted budget may take no further
transition during a stalled recovery. The mirror case is worse: state Open, gauge 0 →
the dashboard reads "healthy" while every request gets a 503 `circuit_open`, silently.

This is not exotic: `allow_request`'s Open→HalfOpen (`:246-265`) races
`record_failure`'s HalfOpen→Open (`:409`) on the request path during exactly the outage
the gauge exists to display, and `gauge!().set()` takes a registry shard lock — a real
preemption window.

**And the hazard T9 was removing is unreachable in release.** `panic = "abort"`
(`Cargo.toml:96`, `:100`) means a panic under the lock kills the process, so no surviving
process holds a stale gauge. **The remediation trades a release-impossible failure for a
release-normal one.**

**The distinction the plan is missing: counters commute, gauges do not.**
`record_circuit_breaker_open`, `record_circuit_breaker_rejection`, and the two atomics
are monotonic — hoisting them is free. `set_circuit_breaker_state` is a
last-writer-wins register and is the only order-sensitive emission in the module.

**Remediation (preferred):** hoist tracing and the counters; **keep the single
`gauge!().set()` inside the guard.** T9's goal is 95% met and ordering is preserved by
construction. Q-C does not depend on the gauge moving. If it must come out, stamp each
`Effect` with a monotonic sequence taken under the lock and publish through a
`fetch_max` CAS guard.

**Two docs assert the invariant being traded away and are in no doc list:**
`circuit_breaker.rs:462-464` (`open_now` — "keeping internal counters and Prometheus
metrics in lockstep … so the gauge cannot drift from the atomics") and `:310-315`
(`reject_request` — "single site … so no rejection path can forget the metrics half").

---

## R2-E — HIGH — `generation` inside `HalfOpen` cannot be monotone, and "payload-preserving regrant" points the wrong way

*[consistency] [types].*

Two independent defects in the T6 fix:

1. **`Open { opened_at }` carries no generation.** On `HalfOpen(gen=N) → Open →
   HalfOpen(?)`, `enter_half_open` has no previous generation to read. Every available
   implementation reintroduces the leak: start at 0 → every window is generation 0;
   derive from `Instant` → already ruled out by T6 (paused clock, two grants without
   `advance` compare equal); read from the previous state → correct only on the regrant
   path, resets to 0 through `Open`.
2. **"Payload-preserving `regrant`" is backwards on the one field that must NOT be
   preserved.** The regrant path fires precisely when the budget is exhausted *and* the
   window expired — i.e. when outstanding probes are presumed lost. Preserving
   `generation` there lets a straggler from the old window credit the fresh budget:
   T6's leak in its purest form, surviving the fix.

**Remediation:** move the counter to `CircuitBreaker` as `probe_generation: AtomicU64`
— the struct already holds `times_opened: AtomicU32` and `requests_rejected: AtomicU64`
at `:185-187`, exact precedent — and write the disposition table into the plan:

| field | `enter_half_open` | `regrant` |
|---|---|---|
| `probes_remaining` | `probe_budget()` | `probe_budget()` |
| `granted_at` | `now()` | `now()` |
| `consecutive_successes` | `0` | **preserve** (T16) |
| `generation` | **bump** | **bump** |

M0.5's pin covers only the third row; add a fourth-row test.

---

## R2-F — HIGH — M4d inverts 4 breaker tests, hollows 5 more, and is the only M4 commit with no stated review discipline

*[tests].*

Under `allow_request() -> Option<Admission>`, `assert!(cb.allow_request().is_some())`
binds the `Admission` to a **temporary** that drops at the end of the statement — `Drop`
runs and the token goes straight back. Every test that spends tokens across statements
changes meaning:

| Test | Line | Outcome |
|---|---|---|
| `test_half_open_limits_probes_to_success_threshold` | `:636-642` | budget never exhausts → `assert!(!…)` **fails** |
| `test_half_open_probe_tokens_regrant_after_open_duration` | `:656-657` | same inversion; re-grant premise collapses |
| `test_release_probe_returns_token_capped_at_budget` | `:742-766` | `release_probe` goes private → **won't compile** |
| `test_half_open_concurrent_probes_admit_exactly_the_budget` | `:733` | broken by M4c *and* M4d |
| **M0.5's own re-grant pin** | new | **fails the same way — the T16 pin dies at M4d** |

Silently hollowed (green, testing nothing): `:533`, `:547`, `:568`, `:679`, `:699-705`.
`test_half_open_recovery_within_probe_budget` is worst — its stated property is
"probed back to Closed *without any rejection*", which passes vacuously once the budget
is never pressured.

M4b gets `byte-identical`; M4c gets `mechanical-only`; **M4d gets nothing** — and M4d is
where semantics change *and* the oracle is rewritten, on the same lines. That is T11's
original complaint, relocated one level down.

**Remediation:** (1) every `allow_request()` result binds to a named local — a review
checklist line, not a convention; (2) **split M4d in two** — types-and-bindings first
(`Drop` a no-op stub, all assertions still pass under today's semantics), then the
3-line behavioral delta; (3) re-assert the T16 pin after M4d with permits held.

---

## R2-G — HIGH — The reconnect reclassification is stated unconditionally, would misreport a real outage, and has zero coverage

*[silent] [tests].*

**Unconditional is wrong.** `reconnect_bounded` bounds the wait with `self.op_deadline`
(`mod.rs:469`). On the **root** wrapper that *is* `config.operation_timeout` — the
background stats refresher (`state.rs:210-220`) and every header-less request run there.
Today they get 503 `connection_failed`, which is **correct**: a 30 s reconnect wait that
expired is strong outage evidence. M1's wording would tell them "Operation timed out.
Please try again." during a genuine broker outage — **TD-08's complaint inverted by
TD-08's fix.** Condition the branch on `client_requested.is_some()`.

**Zero coverage, and two tests give a false green.**
`reconnect_failure_propagates_without_retry` (`:480`) and
`timeout_branch_reconnect_failure_propagates` (`:606`) both inject
`fake_reconnect(… ConnectionFailed("reconnect exhausted"))` — a closure, never
`reconnect_bounded`. They stay green through M1 and *read* as covering the path. Nothing
in the suite exercises `mod.rs:468-483` in either classification.

**Remediation:** extract `bound_reconnect_wait(deadline, session)` — the same move
TD-2026-07-01 made for `run_resilient` — so a test can pass `std::future::pending()`
deterministically instead of racing real I/O; assert the variant *and* the 504 for both
provenances.

*Verified safe [silent]:* propagation and classification do **not** silently change.
`is_connection_error` is applied only to the operation's error (`:137`, `:215`), never
the reconnect step's; both `reconnect().await?` sites propagate straight out.

**One prose casualty [comments]:** `mod.rs:495-498` reads as an iff — *"A timeout counts
as a circuit-breaker failure only when this view runs at the global deadline."* After M1
there exists a global-deadline `OperationTimeout` that records no breaker failure
(it escapes via `?` before any breaker call), which that sentence excludes. Unlisted.

---

## R2-H — HIGH — `pub(super)` on the new types is a CI build break

*[consistency] [architect] [types] [tests] — 4 of 8.*

`AppState::{producer,consumer,iggy}_scoped` are `pub` on a re-exported type
(`state.rs:148-169`, `lib.rs:78`), and `allow_request` is `pub` on a re-exported
`CircuitBreaker` (`mod.rs:80`). Threading a crate-visible `Deadline`/`Admission`/
`ProbePermit` through them trips rustc's `private_interfaces`, which
`ci.yml:58` (`clippy --all-targets -- -D warnings`) and `RUSTDOCFLAGS: -D warnings`
turn into failures. `TimeoutContext` inside the **public** `AppError::OperationTimeout`
must be `pub` outright.

Note also [types]: `pub(super)` declared in `mod.rs` resolves to the crate root
(effectively `pub(crate)`), but declared in a child file it stops at `iggy_client` and
M2 cannot compile. Put `Deadline` in its own file at `pub(crate)` with private fields —
which also removes `mod.rs`'s test module's raw field access.

**Related, [tests]:** dropping `,ignore` from the breaker usage example is infeasible as
written — doc tests compile as external crates, and the example calls `release_probe`
(`pub(super)`) and `is_connection_error` (`pub(super)`). Rewrite to the public surface
only.

---

## R2-I — MEDIUM — Q-F answered unanimously: split M1

Every lens that addressed it said split. Measured surface: `error.rs` (new type,
variant reshape, 15-arm match widening ≈ 90 lines), `mod.rs` (field, root literal,
`reconnect_bounded` **503→504 user-visible**, `with_reconnect`, `with_timeout`, four doc
blocks, fixture, tests), `resilience.rs` (module docs, both signatures, four flag reads,
two constructions, all 14 composition tests, the `"on retry"` asserts, and the
classifier fixture at `:740` that constructs `OperationTimeout(String)`). That is
**~20 tests across three files, not 16** — and it bundles a user-visible status-code
change inside a commit typed `refactor`.

**Recommended split:** `refactor(error)!` (error.rs only) → `refactor(iggy-client)`
(Deadline threading, behavior-identical) → `fix(iggy-client)!` (the reconnect
reclassification, ~15 lines, its own test, its own CHANGELOG line).

---

## R2-J — MEDIUM — Adopt `Rejected(CircuitState)`; it is a smaller diff, not a larger one

*[architect] [types] [simplifier].*

`Option<Admission>` preserves both defects at `resilience.rs:120-129`: two lock
acquisitions on the fail-fast path (worse once M4c makes it a `Mutex`), and the
self-documented inaccuracy at `:121-123` — the reported state may not be the one that
rejected. `reject_request` already computes the correct label under the lock at `:238`,
`:267`, `:280`, and discards it.

`Result<Admission<'_>, CircuitState>` (or a third variant) **deletes** `resilience.rs:124`
and the `state_label` parameter, makes the client-visible message structurally accurate,
and lets the metric label plus the Prometheus gauge become `CircuitState` projections —
landing T18's dropped `gauge()` item at zero extra cost. Test churn is identical
(`.is_some()` → `.is_ok()`). Rename `allow_request` → `admit`, since a non-boolean named
"allow" misleads.

---

## R2-K — MEDIUM — The plan is mis-sequenced; M4a is in the worst position, and the TD-09→TD-08 seam is clean

*[simplifier], with [architect] concurring on M4a.*

**Size is not the problem.** Session 02 shipped **35 files, +2503/−622 = 3125 lines**
(`268a9e8`), and `pr.yml:50-56` only *warns* above 500 — it says so in a comment.
(`CLAUDE.md`'s "error >1000" claim is stale; M3 should fix it.)

**Ordering is the problem.** M4d deletes all four `release_probe()` sites
(`resilience.rs:147`, `:164`, `:220`, `:232`) and collapses the non-connection-error arm
— the same arms M1 edits. So M4a-at-commit-3 authors a helper that is then re-edited by
M1, M4c, **and** M4d, whose deletion erases the primary duplication the helper existed
to absorb. Its stated proof ("the 16-test matrix passing untouched") is spent on the
least valuable version, and is invalid at any later position because M1/M4c/M4d all
legitimately change those tests.

**Move M4a after M4d — then ask whether it is needed at all.** Post-M4d the residual
duplication is three one-line calls plus one `if outage { … }` block.

**And the strategic option: split the session on the TD-09→TD-08 seam, in that order.**
TD-09 first shrinks the `resilience.rs` surface M1 must edit; TD-09's binding trigger is
the harder one and gets discharged outright; and 03a stands alone coherently (pr.yml →
pinning test → enum → sync → RAII → TD-09 resolution). Cost to state explicitly: both
halves are breaking, so it is v0.4.0 + v0.5.0, or v0.4.0 waits for both.

---

## R2-L — MEDIUM — Two lenses disagree on scope; recorded rather than resolved

**[simplifier] says cut, [types] says keep:**

- **The 4-tuple `error.rs` match.** [simplifier] S1: 14 mechanical edits across
  unrelated arms to guard against *under*-exposure, which is the safe direction; and
  `error.rs:142-155` already shows the cheaper early-return pattern. [types] M3/F3:
  the 4-tuple is what makes exposure a compile-time decision — but concedes
  [reviewer] F3's point that the early-return arm **escapes the discipline anyway**,
  so the guarantee is partial unless the message slot widens to `Cow<str>`.
- **`TimeoutContext` as a separate type.** [simplifier] S4: it duplicates `Deadline`'s
  two core fields and names no invariant, so it is encapsulation without a purpose in a
  `publish = false` crate. [types] F9: it reaches `RequestTimeout`-grade encapsulation
  **only if** the constructor takes `Deadline`, in which case it is infallible and
  correct.

**Convergence:** both accept `OperationTimeout(TimeoutContext)` where
`TimeoutContext { deadline: Deadline, attempt: Attempt }` — one type, no duplicated
fields, and the invariant lives in `Deadline`. That resolves the disagreement; it needs
Maxim's call only if the 4-tuple is kept.

---

## R2-M — MEDIUM — Accumulated smaller regressions and corrections

- **M5 has no commit slot.** Its ~14 tests are distributed across commits 4-9 with no
  mapping, so nobody can tell at review time whether a commit's tests were written for
  it or backfilled. At minimum the 504/boundary/negative tests must precede commit 6's
  TD-08 resolution. *[consistency] [simplifier]*
- **The M0.5 `error.rs` pin is hollow for the variant it names** and the plan describes
  its value backwards. The load-bearing half is the **13 control arms** that must stay
  byte-identical across the widening. Isolate the changing constructor behind one
  test-local helper so M1 edits a helper body, never an assertion. *[consistency] [tests]*
- **The concurrency-test decision cannot be deferred** — `tokio::join!` on sync methods
  does not compile, so it blocks M4c. [tests] verified both premises of a working design
  (`tokio::time::Instant::now()` outside a runtime falls back to the real clock;
  `std::thread::scope` works in a plain `#[test]`) and recommends a `Barrier` + 200-iteration
  real-thread racer with `open_duration = ZERO`. It also rules out `spawn_blocking` +
  `start_paused`, whose auto-advance fires while blocking tasks run.
- **`debug!` on the Drop path is invisible in production.** CLAUDE.md's own tiering puts
  production at `RUST_LOG=info`, and this *replaces* an `info!` (`:284`). Correct levels:
  no log when consumed; **`warn!`** when a probe is abandoned unrecorded (volume bounded
  by the probe budget); **`warn!`** with both generations on a stale-generation discard.
  The counter has no home — `src/metrics.rs` is in no commit's file list, and an
  undescribed metric ships with no HELP text. *[silent]*
- **The poison branch is production-dead and the counter is false comfort.** A counter
  that structurally cannot increment reads as "poisoning never happened". Use
  `debug_assert!(false, …)` — which fails at the true origin in tests, the one profile
  where the branch runs — and add
  `#[cfg(all(not(debug_assertions), panic = "unwind"))] compile_error!(…)` so flipping
  the profile is a build error rather than silent decay. *[silent]*
- **Breaker metric assertions need a new dev-dependency and a sequencing constraint.**
  `metrics.rs:215-218` asserts nothing today because no recorder is installed, and
  `metrics_smoke_test.rs` exists precisely because the recorder is process-global.
  `metrics-util` with `debugging` gives `with_local_recorder`, which is **thread-local
  and closure-scoped** — wrong for async methods, right for sync ones. So these tests
  must land after M4c, and that is a genuine extra argument for Q-D the plan has not
  claimed. *[tests]*
- **`tower = "0.5"` declares no features**; `ServiceExt` resolves only via axum's
  feature unification. Pin `features = ["util"]` while adding the crate's first
  `ServiceExt` consumer. *[tests]*
- **401/429 negative tests cannot be unit tests.** `build_router` takes `AppState` by
  value and `IggyClientWrapper::new` connects (`mod.rs:234-242`), so they belong in
  `tests/integration_tests.rs` under `SecureTestFixture` (`:1340-1394`) — the standard
  fixture sets `api_key: None, rate_limit_rps: 0` and cannot reach either code. *[tests]*
- **Four new prose falsifications** the remediation created, all unlisted:
  `mod.rs:495-498` (R2-G), `circuit_breaker.rs:462-464` and `:310-315` (R2-D),
  `timeout.rs:123-126` (the doc on the function M2 rewrites most). Three are
  guarantee-bearing sentences a "mechanical-only" M4c pass is instructed not to touch.
  Also unlisted: `CONTRIBUTING.md:28-55` (the only human-facing statement of the commit
  convention M0 changes, and it shows no `!`), `routes.rs:199-208` (the CORS doc
  immediately above M2's edit), `mod.rs:1023-1040` + `:177-184`. *[comments]*
- **Corrections:** `circuit_breaker_metrics` (`mod.rs:1059`) is **already sync** — only
  two accessors change, not three. M4b's proof count is **18**, not 17 (M0.5 adds one).
  Commit 2's subject is **77 chars**, over `.commitlintrc.json`'s 72. `architecture.md:293/:297/:301`
  carry test counts off by up to 90 and `:19-22` omits the Timeout layer entirely — worse
  than the `README.md:46` drift the plan does sweep. TD-09 gets no `## Resolution` section,
  and `TD-2026-07-09.md:42`'s "shape-only with no behavioral delta" will sit directly under
  a resolution that disproves it. M4d names only the Closed arm at `:230`; there are two
  (`:230` and `:250`). *[comments] [reviewer] [tests]*
- **Three Round-1 items resolved in code but with no test:** T5 (`/health` advertises an
  unenforced deadline — documented, untested), T7 (the deadlock regression test was part
  of the remediation and is absent; note a deadlock hangs rather than fails, so it needs
  a watchdog), and R2-A's rejected-header case. *[tests]*
- **T18's `CircuitState::gauge()` projection was dropped** in remediation without a note.
  Four bare `0`/`1`/`2` literals remain at `:264`, `:363`, `:446`, `:470`. R2-D and R2-J
  both make it *more* valuable. *[silent] [tests] [simplifier]*

---

## Verdict

Round 1's remediation genuinely closed 11 of 18 themes, and every one of the 20
orientation facts now holds against the code. But Round 2 surfaced **four
CRITICAL/HIGH regressions introduced by the remediation itself** (R2-A, R2-B, R2-C,
R2-D), two of which recreate the exact anti-patterns this session exists to remove, plus
three further HIGH findings (R2-E, R2-F, R2-G) and a CI build break (R2-H).

Per `docs/quality-assurance.md § Double-review protocol`, CRITICAL-class regressions in
Round 2 indicate **Round 3**. Before spending it, the scope question R2-K raises should
be settled: the plan is mis-sequenced rather than merely mis-specified, and splitting on
the TD-09→TD-08 seam would remove most of the interaction that produced R2-B, R2-C, and
R2-K in the first place. A Round 3 against a re-scoped plan is a materially different —
and much cheaper — review than a Round 3 against this one.
