# Session 02 — Code Review, Round 2

**Target:** the Round 1 remediation (`997c2d0..HEAD`: resilience fixes, RequestTimeout
hardening, doc corrections, TD-08/09) plus re-verification of every Round 1 theme
against `main...HEAD`.

**Provenance:** config: full (Fable 5, high effort, feature-dev + pr-review-toolkit;
all 8 agents pinned to the strongest model; step-0 gate attested by Maxim in Round 1,
same session). Agent fallbacks: none. Verification: Round 2 findings were verified by
the reporting agents against the live tree (multiple agents re-read every cited line;
the test analyzer **mutation-tested** the new regression nets); no separate
verification pass was run because every kept finding carries in-report evidence
quotes and the two cross-agent MEDIUMs were confirmed by independent agents.

**Headline: 0 HIGH, 3 MEDIUM, ~10 LOW. All eight agents independently verified every
Round 1 theme (H1–H4, M1–M6, L1–L5) genuinely closed** — including an exhaustive
reader audit of `op_deadline` vs `config.operation_timeout` (architect), a
five-arm outcome table proving no scoped timeout can feed the breaker (reviewer),
and mutation kills on each new H2/H3/M6 test (tests).

---

## MEDIUM (all remediated in `a27b9df` unless noted)

### R2-M1. Stale CLAUDE.md unit-test count [consistency, comments]
The remediation added lib tests after the 54c318c sync (172 → 180 at review time).
**Fixed:** synced to the final 183 after the Round 2 additions.

### R2-M2. The clamp *wiring* was invisible to the suite — mutation-proven [tests]
Deleting the `.min` clamp from `with_timeout` passed all 180 tests; only the free
function was pinned. **Fixed:** a `#[cfg(test)]` unconnected constructor (parse-only,
no server) enables `test_with_timeout_wiring_clamps_and_is_shrink_only`, which pins
shorten/extend/re-scope behavior and the outage-signal derivation at the wiring
level.

### R2-M3. Probe token leaks when the request future is cancelled [silent]
Hyper drops handler futures on client disconnect; a probe dropped between
`allow_request` and its outcome releases nothing. Bounded and self-healing: the
`open_duration` re-grant window recovers it and now logs at `info!`.
**Disposition:** deferred into TD-2026-07-09 (the RAII `ProbePermit` guard belongs
with the enum-with-payloads restructuring), which now records this plus the
phantom-release and stale-window loosenesses explicitly.

## LOW (remediated in `a27b9df`)

- `with_timeout` re-scoping could widen a scoped view back toward the global
  (latent; no chaining call sites) [simplifier, types, architect — 3/8 agents] →
  clamps against the current view's deadline; shrink-only pinned by the wiring test.
- `retry_once` recorded breaker failures silently — the M2 asymmetry half-fixed
  [simplifier] → warn on both retry recording arms.
- Out-of-range header values logged `debug!` while malformed logged `warn!` — same
  silence class [types, comments] → both warn; TD-08 wording updated.
- Probe budget expression duplicated between grant and release [types] →
  `probe_budget()` single definition.
- `force_open` warned even on the no-op path [architect, comments] → logs only when
  it acts.
- Resilience "Breaker false positives" doc contradicted the H2 exemption; nobody
  documented that the stats refresher is the SOLE guaranteed breaker feeder under
  all-scoped pure-hang outages (minutes-scale convergence — accepted trade)
  [comments, silent, architect] → module-doc section rewritten; double-release slack
  noted there too.
- `attempt_timeout` comment over-generalized to scoped callers; `op_deadline` /
  `reconnect_bounded` doc precision; extractor absence doc; stale diagram label
  [comments, silent] → all corrected.
- Scoped+disconnected path and `retry_once`'s scoped arms untested [tests F2/F3] →
  new matrix case `scoped_timeout_while_disconnected_still_reconnects_without_breaker_failures`.
- `OptionalFromRequestParts` extractor unpinned [tests F4] → direct unit test
  (present/absent).
- TD-09 sketch omitted `consecutive_successes`; test-count claim imprecise [types,
  consistency] → completed.
- Round 1 artifact accuracy (9 commits not 10; "claims" not "themes"; phantom "H5"
  ref; M1 "both module docs" made true by adding the bypass note to the
  circuit_breaker module doc; durable guide's inline fsync comment contradicted its
  own caveat) [comments, consistency] → all corrected.

## Accepted without action (on record)

- **Commit subjects vs commitlint**: five subjects exceed the configured 72-char
  `header-max-length` [reviewer]. CI enforces only a `type(scope): subject` regex,
  so the gate passes; subjects were shortened by history rewrite before the PR
  (branch unpushed). The config/CI divergence itself is noted here — the pr.yml
  check does not run commitlint against `.commitlintrc.json`.
- **Warn burst at outage onset** (~RPS x global-timeout one-time burst as in-flight
  requests time out) [silent] — bounded, one-time, and each warn corresponds to a
  real 504.
- **Probe cap is soft against deliberate half-open abuse** (scoped/NotFound traffic
  gets tokens released) [reviewer] — the deliberate H3 anti-starvation trade; rate
  limiting bounds it.
- **`timeout_is_outage_signal: bool`** kept over a provenance enum [types verdict:
  "acceptable, at the threshold"] — derived once at the single production call site;
  tripwire recorded: convert if `run_resilient` grows another parameter or flag.
- **Test-run anomaly** [silent]: four half-open tests failed once in a sibling
  agent's first run — explained: the test-analyzer agent was mutation-testing the
  shared tree concurrently (its report documents mutate-run-revert cycles); 35
  subsequent runs and the orchestrator's post-review run are green at HEAD.

## Round 1 themes — closure verification

| Theme | Verdict | Verified by |
|-------|---------|-------------|
| H1 op_deadline/Arc<Config> split | closed, exhaustive reader audit | architect, reviewer, types, consistency |
| H2 scoped-timeout breaker exemption | closed, five-arm outcome table | reviewer, architect, silent, tests (mutation) |
| H3 probe release | closed, all arms + ordering (release precedes reconnect `?`) | silent, architect, tests (mutation) |
| H4 clamp tests | closed at fn level; wiring gap found and closed this round | tests |
| M1–M6, L1–L5 | closed as dispositioned | all eight |

Re-ratings (type-design lens): scoped views 4/3/8/4 → **9/8/9/8**;
`RequestTimeout` → **9/9/9/9**; `run_resilient` holds **9/9/9/9**.
