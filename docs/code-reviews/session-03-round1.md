# Session 03 — Code Review, Round 1

**Target:** branch `tech-debt/session-03` vs `main` (12 commits at review time, 14
files, +1384/−419 excluding review artifacts) — TD-2026-07-09: circuit-breaker
state enum with payloads, RAII probe permit, synchronous mutex, surface
narrowing, and the v0.4.0 release prep.

**Provenance:** config: full — step-0 gate attested by Maxim this session (8-agent
suite, both rounds, Opus-class). Agent fallbacks: none. Several agents ran their
own mutation experiments against the tree; all reverted. Two agents had no shell
and said so, which mattered once (see the false positive below).

Reviewers cited in brackets: [consistency] general-purpose, [architect]
feature-dev:code-architect, [reviewer] feature-dev:code-reviewer, [types]
type-design-analyzer, [silent] silent-failure-hunter, [comments] comment-analyzer,
[tests] pr-test-analyzer, [simplifier] code-simplifier.

Remediated in `671a27a` and `be74e31`.

---

## T1 — HIGH — The disposition counter was not a partition

*[architect] [types] [silent] [comments] [consistency] — 5 of 8.*

A permit dropped after the breaker left HalfOpen fell through
`release_probe`'s `_ => Effect::None`: no counter, no log. And that is the
**common** recovery path, not a corner — with the default budget of two, one
probe failing reopens the circuit while its sibling is still in flight.

So `consumed + released + stale` was strictly less than the tokens minted, which
breaks the exact denominator argument used to choose a disposition counter over
an abandoned-only one. The metric's own `describe_counter!` text and the
CHANGELOG both asserted totality.

**Remediated:** the catch-all became three explicit arms — a fourth `abandoned`
label for a window that closed under a live probe, and a `debug_assert!` plus
`warn!` for a release into a full window, which cannot happen unless the
accounting is broken.

## T2 — HIGH — The module usage example demonstrated the bug this session fixed

*[comments].*

The rewritten example bound the admission to a live local, called
`record_success()`, and let the permit drop at end of scope — recording an
outcome **and** refunding the token. That is precisely what `ProbePermit`'s own
doc says is now impossible. It is `rust,ignore`, so the compiler cannot catch
it, and it is the first thing a reader copies.

**Remediated:** the example destructures `Admission` and consumes the permit on
both recorded arms, mirroring `run_resilient`.

## T3 — HIGH — A pre-existing test was silently weakened

*[architect] [tests].*

`test_half_open_recovery_within_probe_budget` admitted through temporaries,
which refund their tokens on drop, so it passed with `probe_budget()` hard-coded
to 1 — vacuous against the exact property its name claims. [tests] confirmed by
mutation: capping the budget left it green while two neighbours failed.

This is the one place the session's "no pre-existing test was weakened" claim
did not hold, and it was found by systematic comparison against `main` rather
than by reading.

**Remediated:** named locals, and it now fails under that mutation.

## T4 — HIGH — The `Rejection` → error-message path had no coverage

*[tests].*

Mapping `ProbeBudgetExhausted` to the wrong `CircuitState` left all 191 tests
green. The entire rationale for returning `Rejection` instead of re-reading
`state()` — that the message names the state which actually rejected — was
unpinned, and no test drove `run_resilient` against a budget-exhausted
half-open breaker at all.

**Remediated:** a resilience test asserting both messages, plus pinned label
values for the disposition enum. Bidirectionally mutation-checked.

## T5 — MEDIUM — Prose the final code falsified

*[comments] [consistency] [architect] [simplifier].*

- `admit()`'s re-grant rationale still argued the anti-wedge case for *leaked*
  tokens, which RAII removes — and contradicted the module doc two screens up.
- Three comments were written in the tense of work that had since landed
  ("a **future** RAII probe permit", "once `Drop` **is wired**", "the state enum
  this refactor **is preparing for**").
- One paragraph shipped duplicated verbatim inside `admit()`.
- `Effect::RegrantedProbes` logged the granted budget under the field name
  `outstanding` — a number that can never be wrong-side-low.

## T6 — MEDIUM — A config gap this session made worse

*[architect] [reviewer].*

`CIRCUIT_BREAKER_OPEN_DURATION_SECS=0` disables the breaker outright: Open never
rejects, every admission past the budget re-grants, and each re-grant bumps the
generation so every outstanding permit returns as a stale discard. The stale
`warn!` is new this session, so a pre-existing hole became a per-request warn
flood during exactly the outage the breaker damps. `OPERATION_TIMEOUT_SECS=0`
likewise opens the circuit on a healthy service and never closes it.

**Remediated in `671a27a`** — both rejected by `Config::validate`, with tests.

## T7 — MEDIUM — Records overclaimed

*[consistency].*

TD-09's Resolution said all three leaks are closed "each with its own
mutation-checked test". Leak 1 is closed **by construction** — `Ungated` carries
no permit — and the test written in its slot guards leak 2's mechanism. A
permanent record asserting coverage that does not exist is worse than a gap.

Also corrected: the commit count, the CHANGELOG disposition list, and the
deviation on the deleted `release_probe` test.

## T8 — LOW/MEDIUM — Accumulated

- Commit `7d0513c`'s subject is 75 chars against `.commitlintrc.json`'s 72. No
  workflow runs commitlint, so CI will not catch it. *[consistency] [reviewer]*
- The `stale` `warn!` is near-dead under default config — it needs
  `OPERATION_TIMEOUT_SECS > CIRCUIT_BREAKER_OPEN_DURATION_SECS` to be reachable,
  an undocumented ordering dependency. *[silent]*
- `Rejection::Open` emits no log at any level, only the counter, while
  `ProbeBudgetExhausted` gets a `debug!`. Pre-existing asymmetry. *[silent]*
- `circuit_breaker.rs` grew 783 → 1509 lines, split roughly evenly between
  production, docs and tests. [simplifier] judged `Effect` a genuine value type
  rather than disguised flags, and the `run_resilient`/`retry_once` deferral
  correct — this session made extraction *harder*, not easier, so the residual
  ~15 duplicated lines beat the plumbing a shared helper would need.
- Several doc blocks narrate the diff rather than the steady state ("what used
  to be four literals", "the old flat struct"). *[simplifier]*

## One false positive, recorded

*[reviewer]* reported as CRITICAL that an unresolved `[Display]` intra-doc link
would fail the docs job. It does not: `rustdoc` with `-D warnings` passes, and
the module is private so those links are not rendered. That agent had no shell
and said so. Worth recording because the finding was specific, plausible, and
wrong — the verification step is what separated it from T1-T4.

---

## Verdict

Four HIGH findings, all remediated. Two of them — the disposition gap and the
weakened test — were invisible to the full suite passing, and both were found by
agents that ran their own mutations rather than reading. The permit lifecycle,
generation mechanism, deadlock freedom, `Send`/`Sync` and gauge coverage were all
independently traced and came back clean.

Round 2 follows against the remediated tree.
