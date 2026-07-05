# Code Review — Session 01, Round 1

**Branch:** `chore/iggy-0.10-deps-refresh` (5 commits vs `main`)
**Scope:** full repository, with emphasis on the change set — lockfile security
refresh (issues #13–#22), direct dependency bumps, iggy SDK 0.8.0 → 0.10.0,
`apache/iggy:0.8.0` image pins, deny.toml schema migration, docs sync.
**Date:** 2026-07-03

## Provenance

Config: **full** (evidence-based). The step-0 `AskUserQuestion` gate received no
response within the timeout (user away from keyboard); the run proceeded on
direct evidence: parent session model is Claude Fable 5 (Mythos-class, above the
Opus-class bar), and both `feature-dev` and `pr-review-toolkit` plugins are
installed (all eight native agent types spawned — **no fallbacks**). Reasoning
effort could not be verified and is noted as the one unattested dimension.
Model pin deviation, recorded deliberately: the skill text says pin
`model: opus`; every agent (and the verifier) was pinned to `fable` instead,
because the skill predates the Fable tier and pinning opus would have
*downgraded* the suite below the parent session — the strongest-model intent
was honored over the letter.
Authoritative-spec note: this repo carries no `docs/quality-assurance.md`; the
skill's inline spec was followed. No ticket scaffolding exists; this artifact
lives at `docs/code-reviews/` keyed by session number.
Verification (step 4.5): one `general-purpose` verifier re-read every cited
`file:line` for all consolidated claims — **findings: 34 kept / 0 drifted /
0 discarded**.

Agents: rev-consistency (general-purpose), rev-architect
(feature-dev:code-architect), rev-logic-sec (feature-dev:code-reviewer),
rev-types (pr-review-toolkit:type-design-analyzer), rev-silentfail
(pr-review-toolkit:silent-failure-hunter), rev-comments
(pr-review-toolkit:comment-analyzer), rev-tests
(pr-review-toolkit:pr-test-analyzer), rev-simplify
(pr-review-toolkit:code-simplifier).

## Verdict on the change set itself

The five commits are clean: every agent that checked confirmed the 10 RUSTSEC
advisories are genuinely patched (verified against the advisory DB and live
`cargo audit`), the rand 0.10 fix is behaviorally identical (verified against
vendored rand sources), iggy 0.10 is API- and wire-compatible for this app's
surface (verified by source diff of 0.8 vs 0.10 and a live integration run
against the pinned server), and the Client-trait-vs-producer/consumer rationale
holds [architect: "could not break it"]. **Every finding below except the
directly-remediable doc/CI items is pre-existing on `main`** — surfaced because
the review scope was the whole repo and the SDK bump changes the context these
paths run in.

---

## Theme A — The resilience layer is dead code, and SDK 0.10 makes it deader
[architect #1–#4 + startup corollary; silentfail C1, H1, M2, M3; types F1; tests #3]
**Severity: HIGH (pre-existing; partially activated by SDK 0.10 semantics)**

Four agents independently traced the same root cause:

1. `is_connection_error()` (src/iggy_client/mod.rs:446-453) can never match:
   `AppError::Disconnected`/`ConnectionReset` have **zero producers** in
   production code; every SDK error is stringified into
   SendError/PollError/StreamError/TopicError. The reconnect-and-retry branch,
   the circuit breaker's `record_failure`, and all `CIRCUIT_BREAKER_*` config
   are therefore unreachable/inert.
2. `set_connected(false)` exists only inside the unreachable `reconnect()`, so
   the connected flag latches true: `/health`, `/ready`, and the background
   health task report healthy through a total Iggy outage (no live ping
   anywhere).
3. SDK 0.10 widens the failure classes its **default-on internal reconnection**
   intercepts (adds NotConnected/CannotEstablishConnection/TcpError vs 0.8);
   operations during an outage block inside the SDK's unlimited 1s retry loop
   and surface to the wrapper only as `OperationTimeout` — which the wrapper
   deliberately does not count as a breaker failure. Request pile-up is exactly
   the failure the breaker exists to prevent. Corollary: `IggyClientWrapper::new`'s
   "returns an error immediately" doc is false on 0.10 — startup hangs retrying.
4. Latent once (1) is fixed: `reconnect()` swaps in a new `IggyClient` without
   `shutdown()` on the old one, leaking a detached heartbeat task that can
   resurrect a zombie server connection (verified in vendored SDK source).

**Remediation (this branch):** structured IggyError→AppError classification at
the mapping sites (0.10's error discriminants make this clean); make the
health task actually `ping()` and drive `ConnectionState`; `shutdown()` the
old client before swap; record breaker failures on timeout paths.
*[Round 2 correction: the originally planned "disable the SDK's internal
reconnection" proved impossible — `enabled: true` is hardcoded in the SDK's
connection-string parsing (see TD-2026-07-02); the shipped design keeps the
SDK as the transport-level reconnection layer with the wrapper observing
truthfully (timeouts as breaker signal, live pings, classified errors).]* **Deferred with TD records:** DiagnosticEvents-driven state,
with_reconnect paused-clock test matrix.

## Theme B — CI never enforces the deny.toml this branch migrated
[tests #1 (CRITICAL); logic-sec #4; consistency #5; architect #11]
**Severity: HIGH (consensus after severity reconciliation)**

`ci.yml` runs only `cargo deny check licenses`, with `continue-on-error: true`,
and the job is excluded from `ci-success`; `cargo deny check advisories` is
never run; `rustsec/audit-check` does not read deny.toml. The advisories
section — including the paste ignore — gates nothing.
**Remediation (this branch):** gated `cargo deny check advisories licenses`
step, drop `continue-on-error`, add to `ci-success` needs.

## Theme C — deny.toml comment is wrong on three counts; dead license allowances
[comments H1; logic-sec #2, #3; consistency #3, #4]
**Severity: MEDIUM**

The new comment claims ">=0.14 schema"; actual floor for `unmaintained = "all"`
is cargo-deny **0.18.2**; yanked crates default to **warn**, not error;
unmaintained advisories are **errors**, not warnings. `MPL-2.0` and
`Unicode-DFS-2016` are now `license-not-encountered` dead allowances.
**Remediation (this branch):** rewrite the comment accurately; prune both dead
allowances.

## Theme D — Stale rows and counts the docs-sync commit missed
[simplify #5; logic-sec #1; consistency #1, #6; comments M4, M5, M6, L3]
**Severity: MEDIUM (4-agent overlap on the README rows)**

README.md:661/663 still say uuid 1.18 / testcontainers 0.24; README.md:44 says
"93 unit tests" (actual: 130). CHANGELOG says astral-tokio-tar was "removed"
(it was upgraded to patched 0.6.3) and attributes RUSTSEC-2026-0044..0048 to
aws-lc-rs/aws-lc-sys (all five are against aws-lc-sys).
**Remediation (this branch):** fix all four.

## Theme E — extended-tests.yml still floats `apache/iggy:latest`
[tests #6; consistency #2; comments M7]
**Severity: MEDIUM**

Three server-version declarations existed; this branch pinned two. The weekly
stress workflow still tests whatever `latest` points at.
**Remediation (this branch):** pin to 0.8.0.

## Theme F — Prose that contradicts the code
[comments M1, M2, M3, L1, L2, L4, L5; consistency #10]
**Severity: MEDIUM**

`poll_with_params()` doesn't exist (CLAUDE.md:657, architecture.md:244; actual:
`poll_messages`); CLAUDE.md's middleware order swaps Timeout/Request-ID;
routes.rs:155's "applied first, runs last" is inverted; "edge server" comment
survives in tests; four references to the removed `src/iggy_client.rs` file
layout; "# Consumer Groups" heading on a non-group consumer; a doc reference to
a nonexistent `health_check()` method; durable-storage guide stamped v0.6.0.
**Remediation (this branch):** fix all.

## Theme G — Observability stack is wired to nothing
[silentfail H2; consistency #7, #8, #9]
**Severity: HIGH (H2) / MEDIUM (docs)**

`init_metrics`/`try_init_metrics` and every `record_*` helper have **zero call
sites** — the Prometheus exporter never starts, `METRICS_PORT` is inert, and
prometheus.yml scrapes `app:8000/metrics`, a route that doesn't exist, on a
port that isn't the metrics listener. Compose neither sets nor exposes
METRICS_PORT. Separately: README contradicts itself on the app port (8000 vs
curl examples on 3000 = the Iggy server's port), and the documented
integration-test command (`-- --ignored`) runs zero tests.
**Remediation (this branch):** initialize the exporter at startup (fail fast on
bind error), wire the natural record_* call sites, fix prometheus.yml/compose,
fix the README port story and test instructions.

## Theme H — Security middleware fail-open holes
[silentfail H3, H4, H5, L1; types F2; logic-sec clean-verdict caveats]
**Severity: HIGH**

(1) `TrustedProxyConfig::is_trusted` has zero call sites; XFF/X-Real-IP are
trusted unconditionally, and an invalid-CIDR TRUSTED_PROXIES silently degrades
to trust-all — the exact spoofing scenario CLAUDE.md's security section warns
about. (2) The auth brute-force limiter consumes a token on every request
*before* validation: valid-key clients are capped at ~10 req/min/IP, and all
direct clients share one "unknown" bucket — enabling API_KEY on a
directly-exposed service throttles the whole service with a misattributed 429.
(3) Invalid CORS origins are silently dropped (fail-closed, but unlogged).
**Remediation (this branch):** parse TRUSTED_PROXIES at startup and fail fast
on invalid entries; enforce header trust against the parsed ranges; restructure
auth to validate first and count only failures; warn on dropped CORS origins.
*[Round 2 correction: peer-address (ConnectInfo) as the trust anchor was
NOT deferred — it was implemented in-branch (commit 5998dfe); no TD record
exists or is needed for it.]*

## Theme I — Defects inside the (currently dead) reconnect machinery
[architect #5–#9; simplify #1, #2; silentfail M3; tests #2, #8]
**Severity: MEDIUM (latent behind Theme A; real once A is fixed)**

Unchecked backoff multiplication overflows u64 at attempt ~56; the attempt
counter never resets per reconnect session (permanent-failure mode after
exhaustion); jitter is applied after the max-delay cap (delays can exceed the
documented max by 20%); reconnect/follower waits are unbounded on the request
path; the post-reconnect retry block is duplicated verbatim (~20 lines × 2);
the scopeguard's generic value channel is dead weight; half-open allows
unlimited concurrent probes and Open-state failures extend the open window.
**Remediation (this branch):** all of the above except the probe-limiting
design change (TD record).

## Theme J — ensure_stream/ensure_topic swallow errors and race
[silentfail M1]
**Severity: MEDIUM**

`Ok(None) | Err(_)` swallows the get error; two replicas racing on first boot
both create, and the loser's "already exists" error crash-loops the process via
main.rs exit.
**Remediation (this branch):** split the arms; treat already-exists as success.

## Theme K — API-boundary and type nits
[types F3, F4, F7, F8; comments/verify #24-26; silentfail M5, L2; simplify #3, #4]
**Severity: MEDIUM (F3, F4) / LOW (rest)**

`count=0` polls surface as 500 instead of 400; all-digit stream names ("42")
silently become numeric-ID lookups (wrong-resource risk on DELETE);
`PollMessagesRequest` is dead API surface documenting a capability nothing
implements; a dead error mapping on the infallible `Identifier::numeric`; the
test fixture masks server-task panics as generic timeouts; serve-error path
skips `state.shutdown()`; duplicated response-building in producer/consumer
services; `rand_jitter` is now a trivial alias (kept for its documented role).
**Remediation (this branch):** F3 (validate → 400), F4 (`Identifier::named`),
delete dead type + dead mapping, fixture panic surfacing, shutdown-on-error
path, service dedup. **Backlog:** the rest of the LOW nits.

## Theme L — Test coverage gaps the bump exposes
[tests #2, #3, #4, #5, #7, #9]
**Severity: MEDIUM**

Nothing pins PollingStrategy::next/auto-commit semantics, partition-key
routing, with_reconnect composition, or SDK-error classification; metrics
exporter has zero runtime coverage.
**Remediation (this branch):** classifier unit tests + extracted-backoff bounds
tests + auto-commit/partition-key integration tests where they fit. **Deferred
with TD records:** with_reconnect paused-clock matrix, metrics smoke test.

---

## Triage summary

| Theme | Severity | Disposition |
|-------|----------|-------------|
| A resilience dead code | HIGH | Fix core in-branch + 2 TD records |
| B CI deny gate | HIGH | Fix in-branch |
| G observability unwired | HIGH | Fix in-branch |
| H security fail-open | HIGH | Fix in-branch + 1 TD record |
| C deny.toml prose | MEDIUM | Fix in-branch |
| D stale rows/counts | MEDIUM | Fix in-branch |
| E extended-tests pin | MEDIUM | Fix in-branch |
| F prose vs code | MEDIUM | Fix in-branch |
| I reconnect internals | MEDIUM | Fix in-branch + 1 TD record |
| J ensure_* race | MEDIUM | Fix in-branch |
| K API nits | MEDIUM/LOW | Fix most in-branch |
| L test gaps | MEDIUM | Partial in-branch + TD records |

Round 2 re-review follows remediation.
