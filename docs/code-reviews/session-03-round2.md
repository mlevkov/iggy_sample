# Session 03 — Code Review, Round 2

**Target:** `tech-debt/session-03` after Round 1's remediation (`671a27a`,
`be74e31`). Round 2's purpose is regressions introduced by that remediation.

**Provenance:** config: full — same step-0 attestation. Six agents (the lenses
with live findings to re-check); no fallbacks. Agents again ran their own
mutations; several transiently observed each other's, which is why two reported
the tree dirty — it was clean at HEAD before and after.

Remediated in `58db0a2`.

---

## R2-1 — HIGH — The remediation reintroduced the exact hazard it cited elsewhere

*[consistency] [architect] [silent] [comments] [reviewer] — 5 of 6.*

Round 1's fix added `debug_assert!(false)` to the new over-release arm in
`emit()`. That arm is reachable from `ProbePermit::drop` → `release_probe` →
`emit`. Under `cargo test` — which unwinds, with debug assertions on — a panic
anywhere with a live permit drops that permit mid-unwind and lands here.
Asserting during an unwind double-panics straight to abort, destroying the test
report **including the failure that started it**.

This is verbatim the hazard the `lock()` helper is gated against seventy lines
above, with the reasoning spelled out in its doc comment. Round 1 reproduced the
hazard while citing the rule.

[consistency] proved it rather than arguing it: with the generation check
removed, a leak test fails, unwinds, drops a permit, and the run ends in SIGABRT
rather than a test failure.

**Remediated:** guarded on `!std::thread::panicking()`, and the `warn!` moved
ahead of the assert so the diagnostic survives the build that catches the bug.

## R2-2 — HIGH — An invariant violation was given the same label as routine recovery

*[architect] [silent] [comments] [tests].*

Round 1 recorded the over-release arm as `Disposition::Abandoned` — the same
label as a window closing under a live probe, which is the common, high-volume
recovery case. In release the `debug_assert!` compiles out, so a broken
invariant's only trace was one `warn!` line inside a counter that is already
climbing. Nothing alertable.

**Remediated:** `inconsistent` is its own label. `rate(...{disposition="inconsistent"}) > 0`
is now a real alert, and the partition still holds.

## R2-3 — HIGH — The exported metric description still listed three labels

*[silent] [reviewer] [architect] [consistency].*

`describe_counter!`'s HELP text ships to `/metrics` and renders in Grafana. Round 1
added `abandoned` to the enum, the CHANGELOG and the TD record — but not to the
string operators actually read, nor to the module inventory. The label domain
shown to operators did not match what the code emits, for the very metric whose
value depends on totality.

**Remediated:** both updated, and all five labels pinned by test.

## R2-4 — MEDIUM — The stated unreachability invariant was false

*[architect] [comments].*

The new comment justified the over-release arm with "remaining plus outstanding
always equals the budget". That is false after any `consume()` — a consumed
token never comes back, so the sum is `budget − consumed`. Under the stated
equality the arm would be reachable whenever anything had been consumed.

The conclusion survives on a correct argument: `remaining + outstanding +
consumed == budget`, so `remaining == budget` implies `outstanding == 0`, and no
permit exists to reach the arm. A maintainer checking the stated invariant would
have found it violated on the commonest path.

## R2-5 — MEDIUM — Records drifted again

*[consistency] [comments] [tests].*

Round 1 corrected TD-09's counts and immediately introduced new ones that were
also wrong, in both directions: "two tests guard the generation comparison…and
nothing else" (three), "making `Drop` inert fails it and two others" (four),
"eight commits" (nine touch the breaker). The counts were written before the
commits they describe.

Worse, the deviation paragraph contradicted itself in consecutive sentences —
the replacement tests drive "the same property", then that property is
"unreachable through that path". Both cannot hold.

And the **Binding trigger** still named `CircuitBreakerState` and
`allow_request`, both deleted by this record's own resolution — the same two-site
drift TD-2026-07-03 exists to prevent.

**Remediated:** numbers measured rather than asserted; the deviation says
plainly that the budget cap now has no test; the trigger carries a discharge
note.

## R2-6 — MEDIUM — Leftovers

- The test-module banner still read "the three probe-accounting leaks, one test
  each" after the record was corrected to say leak 1 has none. *[comments]*
- `record_disposition`'s doc claimed it "takes no lock" while the `Effect` doc in
  the same file says the metrics recorder takes a registry lock — and the false
  claim was the stated licence for calling it near a guard. Now scoped to "no
  *breaker* lock". *[comments] [architect]*
- The `Disposition` doc paragraph was inserted *after* `#[derive(...)]`, so
  rustdoc collapsed summary and body. *[consistency] [comments]*
- A resilience test built `CircuitBreakerConfig::new(1, 1, Duration::ZERO)` — a
  configuration `Config::validate` now refuses. Rewritten with a real window and
  an advance, so no test rests on a config that cannot exist. *[architect]*

## Open, not remediated — recorded deliberately

- **`Effect::ProbeWindowGone` has no test.** Reverting it to `Effect::None` — the
  catch-all Round 1 removed — leaves all 194 green. The arm *is* executed (by the
  deadlock ordering test), but nothing asserts on it, because
  `record_disposition` only calls `metrics::counter!` and no recorder is
  installed under `cargo test`. Closing this needs a `#[cfg(test)]` tally on the
  breaker. **Trigger:** add it with the next change to disposition accounting.
  *[tests]*
- **`test_half_open_reentry_grants_fresh_probe_tokens` is vacuous for its name** —
  with `success_threshold = 1` the granted budget is unobservable. Pre-existing
  on `main`, not a session weakening. **Trigger:** fix when that test is next
  touched. *[tests]*
- **Commit `7d0513c` is 75 chars** against the declared 72. Fixing it means a
  rebase of merged-in history on the branch; left for Maxim's call at merge time.
  *[reviewer] [consistency]*

---

## Verdict

One HIGH regression from Round 1's remediation, found by five of six agents and
proven rather than argued, plus two HIGH observability defects and a set of
records that drifted a second time. All remediated in `58db0a2`.

No CRITICAL-class regression and no design question reopened, so **Round 3 is not
indicated**. Three items are recorded above as open with triggers rather than
silently dropped.

The permit lifecycle, disposition partition, arm ordering and unreachability
argument were each re-derived independently this round and hold. 194 lib tests,
18 model, 1 doc; fmt, clippy `--all-targets --all-features -D warnings` and
rustdoc `-D warnings` clean.
