# Code Review — Session 01, Round 2 (post-remediation)

**Branch:** `chore/iggy-0.10-deps-refresh`
**Scope:** verification of the Round 1 remediation (commits `6ff23ec..61b40ca`)
plus regression hunting in the fixes themselves.
**Dates:** 2026-07-03 → 2026-07-04 (interrupted by a session usage limit and
resumed).

## Provenance

Config: **full**, same basis as Round 1 (Fable 5 parent, both review plugins,
all agents pinned to `fable`). **Suite degradation, recorded:** 6 of 8 lenses
delivered Round 2 reports (consistency, architect→partial, logic-security,
types, silent-failure, comments→none, tests, simplifier). The **architect**
and **comment-analyzer** agents were killed by the session usage limit
mid-review; resume requests were sent after the reset but no reports had
arrived when this artifact was written — their scope was substantially
covered by overlap (silent-failure independently audited the resilience
commits including SDK-source verification; consistency audited all prose and
CHANGELOG claims). Any late reports will be triaged and appended.
Verification: Round 2 findings were **verified inline by the reporting
agents themselves** (each report cites file:line it re-read, several
re-ran the suites and cargo-deny live); no separate verifier pass was run
for Round 2.

## Round 1 fixes — verification results

Every Round 1 remediation was independently confirmed as landed and
effective by at least one Round 2 lens, most by several: RUSTSEC patches
(live cargo-audit), iggy 0.10/server 0.8.0 pin, deny.toml migration + CI
gate, error classification at all 15 network call sites, live health
probes, trusted-proxy enforcement through the real serve path, failure-only
auth metering, metrics exporter startup, count=0 → 400, `Identifier::named`,
`PollMessagesRequest` removal, test-count/dependency-table corrections.
**No Round 1 fix was found regressed or ineffective.**

## Round 2 findings and their disposition

### Fixed in-branch (commits 56d5fd0, c23bef2, 237abaa)

| Finding | Source | Fix |
|---------|--------|-----|
| Port migration incomplete (2 config tables, architecture.md docker/K8s examples, 4 rustdoc curls) — falsified a CHANGELOG claim | consistency (MED) | completed everywhere; CHANGELOG claim now true |
| CHANGELOG duplicate Unreleased headings + stale Security block contradicting deny.toml | consistency (MED) | sections merged, stale block removed |
| First-entry XFF parsing defeated by APPENDING proxies (the common default) — the exact attack the round-1 fix targeted | logic-sec (MED) | rightmost-untrusted resolution + IpAddr parsing, peer fallback on garbage; 6 new tests |
| Forwarded values used as limiter keys without parsing (unlimited key minting) | logic-sec (MED) | honored values now parse as IpAddr or fall back to peer |
| Trusted-proxy config parsed twice; routes.rs "shared" comment false | types (MED) | `RateLimitLayer::with_trusted_proxies` takes the shared `Arc` |
| Zombie-client leak on failed/aborted reconnect attempts (same class round 1 fixed for success path) | silent-failure (MED) | shutdown() on failure arms; per-attempt bound halved so cleanup arms are reachable |
| Three histograms described but never emitted; `status` label only ever "success" (outages invisible in metrics) | silent-failure (MED) | send/poll durations + failure statuses wired; unwired request-duration histogram deleted |
| Metrics recorder installed after client init (startup window dropped); gauges never seeded; metrics_addr()/try_init_metrics orphaned+divergent | silent-failure (LOW), consistency | init first, gauges seeded, single canonical accessor honoring HOST |
| force_open/force_close diverge Prometheus gauge from internal counters | simplifier (LOW, behavioral) | `open_now()` helper unifies all three Open transitions |
| Old-client shutdown error at debug; recovery transition only at trace | silent-failure (LOW) | warn / info respectively |
| Missing-ConnectInfo warning per-request per-middleware (latent log flood) | silent-failure, logic-sec (LOW) | warn once per process |
| Dead surface: `is_trusted(&str)`, `extract_client_ip`/`UNKNOWN_IP` re-exports, stale auth.rs comment, ip.rs diagram | 3 lenses converged (LOW) | removed/rewired; `extract_client_ip` is now the documented fallback the validated variant delegates to |
| CI deny job checked only advisories+licenses while [bans]/[sources] were configured-but-unenforced | tests (LOW) | `cargo deny check` (all sections) |
| Dockerfile: HEALTHCHECK invokes curl not present in image (pre-existing); metrics port unexposed | consistency (LOW-MED) | curl installed; EXPOSE 8000 9090 |
| Semantic integration tests flake-prone (single fixed sleep + exact asserts) | tests (MED flakiness) | poll-until-deadline visibility waits |
| Missing high-value tests: invalid-CIDR fail-fast, wire-level spoof rotation, health_check live path, defaults idempotence, count=0 routes, IPv6/zero-prefix CIDR containment, backoff floor/zero-max edges, per-IP failure-bucket isolation, budget bounds derived from constants | tests (HIGH..LOW) | all added (+7 unit, +3 integration; secure fixture now runs with enforcement ON) |
| Fixture Ok(())-exit misattributed as panic; secure fixture lacked fast-fail | silent-failure (INFO), tests | both fixtures fail fast with accurate wording |
| App metrics endpoint wired but undocumented | consistency (LOW-MED) | documented in README/CLAUDE.md observability sections |

### Accepted / deferred (recorded, not fixed)

- **TD-2026-07-01 trigger fired in-session** [tests A2, HIGH risk-posture]:
  the with_reconnect matrix was due by the TD's own trigger; accepted
  explicitly as session debt, trigger re-armed (see the TD's history note).
- Key-length timing oracle in `constant_time_eq` (pre-existing, LOW) —
  optional digest-comparison hardening noted in the Round 2 report.
- `RateLimitError` name now broader than its scope (LOW) — cosmetic rename
  candidate (`MiddlewareConfigError`) for a future pass.
- `PollParams::with_count(0)` remains representable at the library level
  (INFO) — boundary validation covers all production constructors (verified);
  NonZeroU32 stays in the backlog.
- Poll count silently clamps while batch send 400s over limit (INFO,
  documented asymmetry) — no change.
- `.env.example` remains a partial listing (softened claim instead).
- Branch-protection requirement on "CI Success" not verifiable in-repo —
  operator action item.

## Gate status at close

`cargo fmt --check` ✓ · `clippy --all-targets -D warnings` ✓ ·
158 lib + 29 integration + 18 model tests ✓ · `cargo audit` 0 vulns ✓ ·
`cargo deny check` all four sections ✓.
