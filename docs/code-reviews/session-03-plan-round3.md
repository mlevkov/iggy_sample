# Session 03 — Plan Review, Round 3

**Target:** the plan re-scoped to **TD-2026-07-09 only** (TD-2026-07-08 deferred to
session 04), reviewed against the tree at `tech-debt/session-03` @ `e12a513`.
Round 3 was triggered by Round 2's CRITICAL-class regressions, and run against a
materially smaller plan after Maxim's re-scope decision.

**Provenance:** config: full — step-0 attestation carried from this session's gate
(8-agent suite, Opus-class). Agent fallbacks: none. Agents were asked for terse,
findings-only output this round; each verified its own citations, and the
consistency and comment lenses independently re-verified **all 40+ plan citations**
against the tree — no regressions from Rounds 1-2, no new drift.

**Character of this round:** qualitatively different from Round 2. **Zero new
architectural regressions.** Every finding is implementation-order, test-writability,
or a mechanical detail. The design has converged; the plan has not yet.

---

## R3-1 — CRITICAL — M1's second pin cannot pass at commit 1 and duplicates an M7 test

*[consistency] [reviewer] [tests] — three independent traces, same conclusion.*

The pin reads "mint a permit, force a re-grant, drop the permit, assert the budget did
not grow." Two independent reasons it cannot be written at commit 1:

1. **`ProbePermit` does not exist until commit 5.** The only commit-1 mechanism is
   `allow_request()` + `release_probe()`.
2. **Expressed that way it asserts the *fixed* behavior.** `release_probe` is
   window-blind (`:329-338`): after a re-grant, `remaining(0) < cap(1)` → `remaining = 1`.
   The budget **does** grow today. It is a red test parked for five commits.

It is also the *same test* as M7's "straggler-after-re-grant".

**Remediation:** delete it from M1; M7 is its only correct home. If a commit-1 signal is
wanted, it must assert today's leak with a `// KNOWN LEAK — inverts at M6` marker, and
M6's checklist must name the inversion.

## R3-2 — HIGH — M1's first pin is vacuous as written

*[consistency] [tests].*

With `success_threshold = 2`, the Open→HalfOpen transition leaves `remaining = 1`
(`:257`, `:263`), so the next `allow_request` fails the `remaining == 0` test at `:270`
and **skips the re-grant block entirely** — it just spends the second token.
`consecutive_successes` is then preserved trivially because no re-grant occurred. The
test passes today and after M3 for the wrong reason, over exactly the row M3's
disposition table changes.

**Remediation:** exhaust first — admit → `record_success` → admit → **assert the third
admit is rejected** → advance past `open_duration` → admit (this is the re-grant) →
`record_success` → assert `Closed`. Without the rejection assertion the pin cannot fail.

## R3-3 — HIGH — The redesigned concurrency test fails as specified, and no clock value fixes it

*[tests].*

`open_duration = Duration::ZERO` **deletes the cap it is meant to test.**
`Instant::elapsed()` is never negative, so the re-grant guard
`granted_at.elapsed() >= ZERO` (`:273-275`) is always true: thread A takes the only
token, thread B finds `remaining == 0`, re-grants unconditionally, and is admitted.
`a.is_ok() ^ b.is_ok()` fails every iteration.

Worse, **no real-clock value fixes it** — `open_duration` gates *both* the Open→HalfOpen
transition and the re-grant window, so any duration short enough to reach HalfOpen
without `advance()` is short enough to re-grant.

**Remediation:** add `#[cfg(test)] fn force_half_open(&self)` granting a window under a
long `open_duration`, then race two threads *inside* HalfOpen over the token — the
actually-contended resource. Construct a fresh breaker per iteration (unstated in the
plan).

## R3-4 — HIGH — The commit order is wrong again: M2 and M3 author code that M4 deletes

*[architect] [simplifier], with different corrections.*

M4 removes the read-lock pre-check block `:227-243`, which contains the `:230` Closed arm
**and** the `:238` reject site. So under the current order M2 must plumb an `Effect`
across *two* guard scopes with a third "fall through to the write lock" outcome (a
`ControlFlow`, unstated), and M3 must port the same match twice — both thrown away at M4.
This is precisely the "each pays the other's churn" failure used to justify dropping
TD-08.

The two lenses agree the order is wrong and disagree on the fix:
- **[architect]:** M0, M1, **M4**, M2, M3, M5, M6, M7 — M4 is not purely mechanical today
  anyway (poisoning helper, `compile_error!` guard, redesigned concurrency test), so the
  "keep M4 mechanical" argument is already spent. Post-M4 there is one guard per method,
  the "helper must never read state" invariant is trivially true, and M7's gauge tests —
  the only verification M2's hoist ever gets — land one commit later instead of five.
- **[simplifier]:** M1 → **M3** → M2 → M4 — M2's only hard constraint is *before M4*
  (self-deadlock under a non-reentrant `Mutex`), not before M3; merging M2 into M3 would
  bury R2-D's gauge decision inside a state reshape.

Both orders beat the current one. [architect]'s is stronger on the `:227-243` point,
which is the deletion actually causing the churn.

## R3-5 — HIGH — The plan contradicts itself on "both Closed arms"

*[architect].*

M4/Q-D says the pre-check block `:227-243` disappears; M5 says `Ungated` covers "**both**
`:230` and `:250`". `:230` lives *inside* `:227-243`. Post-M4 there is exactly **one**
Closed arm. A reviewer auditing "did we cover both?" will hunt for something that no
longer exists.

*[types] supplies the correct post-M4 mapping:* 7 return sites pre-M4, **5** after —
`Ungated` ×1 (`:250`), `Probe` ×2 (`:265`, `:288`), `Err` ×2 (`:267` Open, `:280` budget
exhausted). The plan only *implies* `:265`, which is the likeliest miss because its
decrement at `:263` is textually separated from the grant.

## R3-6 — HIGH — M5's named permit locals will fail `-D warnings`

*[simplifier], verified by building a scratch crate.*

`unused_variables` fires on `Drop`-typed bindings too. With `Drop` inert and no
`consume()` in M5, every `permit` local is write-only → clippy `-D warnings` errors.

**Remediation:** land `consume()` in M5 at all ten outcome sites; M6 then adds only
`impl Drop`. Preserves the M5/M6 split and makes M5 compile.

*[architect] adds a related point:* with `Drop` inert, a correctly-bound site and a
mis-bound temporary are **indistinguishable**, so M5 cannot prove its own binding
discipline. Enforce it mechanically — `#[must_use]` on `Admission`, plus a grep-able ban
on `admit()` inside `assert!`/`matches!`.

## R3-7 — HIGH — The `compile_error!` guard breaks `cargo bench` and `cargo test --release`

*[reviewer] [simplifier].*

Cargo forces `-C panic=unwind` for **all test and bench units regardless of profile**, so
`cfg(all(not(debug_assertions), panic = "unwind"))` fires under
`cargo bench --all-features` — which `extended-tests.yml:58` runs weekly — and under
`cargo test --release`. `cfg(panic = ...)` is stable (1.60) and does evaluate correctly;
the guard simply catches more than intended.

**Remediation:** add `not(test)` to the `cfg`, or delete the guard — a comment at
`Cargo.toml:100` carries the same information at zero risk. [simplifier] prefers deletion.

## R3-8 — HIGH — Projecting the metric label through `CircuitState` silently renames it

*[architect] [types] [comments] [simplifier] — 4 of 8.*

`circuit_breaker.rs:109` renders `HalfOpen` as `"half-open"` (hyphen); the shipped
Prometheus label at `:280` is `"half_open"` (underscore). M5's "the label becomes a
`CircuitState` projection" would rename the exported label value, falsify
`metrics.rs:13` and `:150-153`, and M7's own new label test would codify the wrong value
as intended. `Display` must stay hyphenated — it feeds the user-visible `CircuitOpen`
message at `resilience.rs:126`.

**Remediation:** a `fn metric_label(&self) -> &'static str` explicitly distinct from
`Display`; state in M5 that label values are unchanged.

## R3-9 — HIGH — M8's `CLAUDE.md` task is a phantom, deleted by this session's own PR #30

*[consistency] [comments].*

"Fix `CLAUDE.md`'s stale 'error >1000' PR-size claim" — that text no longer exists.
Commit `9d645aa` (this session's CLAUDE.md trim) removed the entire CI/CD section;
`grep -E "1000|>500|PR size"` returns nothing across all 233 lines, in the worktree and
at `HEAD`. Round 2 asserted it from a pre-trim reading.

**Remediation:** delete the M8 clause — an implementer "fixing" it may re-add a CI section
the trim deliberately removed. (The orientation-table row is still correct:
`pr.yml:50-56` warns only, and `:55` says so in a comment.)

## R3-10 — MEDIUM-HIGH — No release-prep commit, and TD-08's new findings have no durable home

*[consistency].*

- The sequence ends at commit 8 + artifacts, then "PR; v0.4.0" — but `Cargo.toml:3` is
  still `0.3.0`, `CHANGELOG.md` has a live `## [Unreleased]` Security entry, and the
  repo's precedent (`274bbc3 chore(release): prepare v0.3.0`) touches `CHANGELOG.md`,
  `Cargo.toml`, `Cargo.lock`, `README.md`. Add a `chore(release): prepare v0.4.0` slot.
- **The two new TD-08 findings are recorded only in untracked artifacts.** The plan points
  session 04 at `docs/code-reviews/session-03-plan-round{1,2}.md`, which are committed
  only at the tail slots, and the plan itself lives in scratchpad and is never committed.
  `docs/tech-debt/TD-2026-07-08.md` — the record reachable from the registry index — is
  untouched and still lists only two silences. **M8 must append both findings there.**

## R3-11 — MEDIUM — Accumulated corrections

- **`HalfOpen.generation` is a redundant copy** of `probe_generation`: both constructors
  bump the atomic and write the variant under the same lock, so while the state *is*
  HalfOpen the field is provably equal to the atomic. The tell is the plan's own
  disposition table — `generation` is the one row whose two cells are identical. Delete
  the field; compare the permit's `u64` against the atomic. *[types]*
- **`Err(CircuitState)` has no correct `Closed` arm** — `Err(Closed)` is representable and
  meaningless, and `metric_label()` would need a bogus arm. Prefer
  `enum Rejection { Open, ProbeBudgetExhausted }` with an exhaustive `label()` plus
  `From<Rejection> for CircuitState` for the message. *[types]*
- **`allow_request` has three early `return`s** (`:230`, `:238`, `:265`); hoisting the
  emit to a post-guard tail silently drops any Effect whose arm still returns early.
  `:238` is the trap — it both bumps `requests_rejected` and returns. M2 must state that
  the method becomes single-tail, and add a rejection-count assertion for the pre-check
  path, which no test distinguishes from `:267` today. *[silent]*
- **M2's `:399-403` capture rationale is wrong but the action is right for a better
  reason:** `open_now` never touches `consecutive_failures` — the real point is that M3's
  `Open { opened_at }` **deletes the field**, so uncaptured, the path of least resistance
  in M3 is to drop `failures =` from the log, losing the only report of the threshold
  count at the moment the circuit opens. Make capture an **M3 prerequisite**. Same shape
  at `:258-261` vs the `-= 1` at `:263`. *[silent]*
- **The re-grant `info!` rewording is factually wrong.** Under RAII the re-grant path
  stays reachable for a legitimate reason — probes still **in flight** past
  `open_duration`, which is the normal HalfOpen case during an outage and exactly what
  `generation` exists to handle. Labelling it an invariant violation misdiagnoses a hang
  and cries wolf on every real outage. Keep it as `info!` reporting outstanding probes;
  the RAII-leak alarm is the *stale-generation discard*, a distinct event. *[silent]*
- **The abandoned-only counter reproduces the structural-zero defect** that killed the
  poison counter: `probes_abandoned_total == 0` cannot distinguish "healthy" from "the
  Drop release path is dead code". Use a disposition denominator —
  `probe_dispositions_total{disposition=consumed|released|stale}`. *[silent]*
- **`debug_assert!(false)` in a `Drop`-reachable `lock()` defeats its own rationale.**
  Poisoning requires a panic under the guard; in dev that panic unwinds, unwinding drops a
  live permit, `Drop` calls `lock()`, the assert panics *during unwind* → **abort**,
  losing the original panic's origin and the whole libtest report. Gate on
  `!std::thread::panicking()`. *[reviewer] [silent]*
- **`warn!` is wrong for the routine abandon path** — drop-without-consume with a matching
  generation *is* today's `release_probe` path (currently `debug!`) and fires on every
  non-connection error in half-open. `debug!` + counter there; reserve `warn!` for the
  stale-generation discard. Also: `CIRCUIT_BREAKER_OPEN_DURATION_SECS` is unvalidated
  (`config.rs:207-209`), so `open_duration = 0` makes the re-grant path fire on every
  request — "bounded by `probe_budget`" is false when the window is zero. *[silent] [simplifier]*
- **`counter!` is invoked nowhere outside `metrics.rs`** — every emitter is a `record_*`
  wrapper — so a `record_circuit_breaker_probe_*` function in the `:144-156` block is
  **required**, not optional, and the module-doc counter inventory at `:9-13` goes stale.
  Five bare gauge literals exist, not four: `main.rs:63` also passes a bare `0`. *[silent] [types] [simplifier]*
- **Forbid `Clone` on `ProbePermit`/`Admission`** — `Copy` is already impossible
  (mutually exclusive with `Drop`), but a `Clone` derive double-releases and is one line
  away from reopening the leak M6 closes. Also state that `probe_generation` is **never**
  reset by `force_close`/`force_open`, or M3's deletion of the hygiene-reset instinct
  returns and an ancient permit matches a recycled generation. *[types]*
- **`test_release_probe_returns_token_capped_at_budget` (`:742-766`) is still unaddressed.**
  Correcting Round 2: it does *not* fail on visibility — `mod tests` is a child module and
  can call a fully private fn. It fails on **arity** once `release_probe` takes a
  generation, and post-M6 its over-release branch is unreachable through the public path.
  Keep it, renamed to signal "defensive-only branch". *[reviewer] [types] [tests]*
- **`resilience.rs:670`** is `assert!(breaker.allow_request().await, …)` — a temporary,
  the exact pattern M5 bans — and sits outside the plan's audit set ("the 18 breaker
  tests"). Its comment at `:667-668` also dies at M6. *[types] [comments]*
- **Doc sites attributed to the wrong commit**, each shipping one milestone stale:
  `:201-223` and `:310-315` die at **M5** (rename + `Result` + the deleted `state_label`),
  `:293-297` dies at **M3** (split into two constructors), and `:49`'s intra-doc link
  `[CircuitBreaker::allow_request]` dies at M5. Also unlisted: `TD-2026-07-03.md:24`
  (whose Resolution prose names `allow_request()` and the anti-wedge mechanism),
  `pr.yml:86-87` (the contributor-facing failure message, which still shows no `!`), and
  `metrics.rs:8-13`. `mod.rs:28` is an orphan — it stays true. *[comments]*
- **The `,ignore` drop is unimplementable.** `mod.rs:59` declares `mod circuit_breaker`
  **private** and only three items are re-exported; after M5, `admit`/`Admission`/
  `ProbePermit` are `pub(super)`, so a doctest — which compiles as an external crate —
  cannot demonstrate admission at all. Keep `,ignore`. *[reviewer] [types]*
- **Counts:** M1 adds tests, so M2's "17 byte-identical" and M3's "18" should both read
  **19**. Commit #5's subject is **88** chars (not 89) and the shortened form is **72**
  (not 71) — exactly at the limit, zero margin. `README.md:46` and `CLAUDE.md`'s "183
  tests" are newly falsified by this session and are unmentioned now that TD-08's doc
  milestone is gone. *[consistency] [reviewer] [comments] [tests]*
- **`metrics-util` may be unnecessary** — the new risk M3 introduces is the
  `CircuitState → 0/1/2` mapping, covered by a four-assert pure unit test on `gauge()`
  with zero dependencies. Dropping it also deletes the artificial "must land after M4"
  constraint. Check `deny.toml` before adding it either way. *[simplifier] [tests]*
- **The deadlock watchdog guards an unreachable hang** — `record_success()` releases its
  guard on return, so dropping the permit afterwards cannot self-deadlock. Either use a
  plain `#[test]`, or redesign to drop a permit *while* a guard is live — the shape that
  can actually deadlock. *[simplifier]*
- **Make the deadlock invariant structural:** in `admit`, compute `Option<u64>` under the
  guard, **drop the guard, then construct the permit**. Then no permit can exist while a
  guard is live, and a future `?` added mid-function cannot deadlock. *[types] [architect]*
- **The real constraint forcing M4 before M6 is `Send`, not lifetimes** — no
  `std::sync::MutexGuard` may cross an `.await`, or every handler future stops being
  `Send`. Say so. *[reviewer]*

## R3-12 — The one open design question: this session need not be breaking at all

*[simplifier], with supporting verification from [consistency] and [types].*

`CircuitBreaker`, `CircuitBreakerConfig`, `CircuitState` and the three
`IggyClientWrapper` accessors have **zero users outside `src/iggy_client/`** — verified
across `src`, `tests`, and `fuzz`. If the breaker methods narrow to `pub(super)` and the
three dead accessors are deleted (folded into M3), then **M4, M5 and M6 all become
non-breaking**: ~20 lines disappear, M4's Q-E vanishes entirely (converting zero-caller
dead code to sync is cost for no benefit), and v0.4.0 gets one honest changelog line
instead of three.

The counter-consideration: narrowing `mod.rs:80`'s `pub use` is *itself* a semver break,
so a version bump is still owed — but one deliberate narrowing beats three incidental
signature breaks, and it also dissolves R3-6's visibility pressure and lets
`Admission`/`ProbePermit` be plain `pub(crate)`.

This is a scope decision, not a defect. It is the last thing standing between this plan
and implementation.

---

## Verdict

**No new architectural regressions.** Rounds 1 and 2 each invalidated a design premise;
Round 3 found none — every finding is ordering, test-writability, or a mechanical detail,
and the two lenses that disagreed (R3-4) disagreed only about *which* better order to
adopt. That is convergence.

**Round 4 is not indicated.** R3-1 is CRITICAL by severity but is a test-writability error
in a newly-added item, not a regression in the design; it and every other finding here is
remediable by editing the plan text, with no design question reopened except the
optional R3-12.

Remediate Round 3, settle R3-12, and implementation may begin.
