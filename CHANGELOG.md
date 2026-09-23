# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.1] - 2026-09-23

Security patch for four RustSec advisories: one reachable on the public API
listener (h2; the listener has been accepting cleartext HTTP/2,
TD-2026-09-01), one exercised only on a TLS or QUIC Iggy connection
(rustls), and two not reachable at all. The root cause of the drift is
repaired too:
Dependabot had never run, and its version updates could not have reached
these transitive crates anyway. Reviewed with a four-agent double review
(tier-graduated cadence); artifacts under
`docs/code-reviews/rustsec-2026-09-round{1,2}.md`. TD-2026-07-02's trigger
fired during this release (iggy 0.11.0), and it is deliberately not in it.
TD-2026-09-02 records the check that would have caught the broken
Dependabot config: validating the file in CI.

### Added

- Tripwires for TD-2026-09-01. CI checks that `hyper-util`'s `http2`
  feature is still in the production dependency graph, the one thing that
  makes the API listener serve h2c (test builds get it from dev-dependencies
  too, so no test can see production lose it); tests pin that the API
  listener serves an h2c prior-knowledge request and the metrics listener
  refuses one. A dependency bump that flips either surface fails CI instead
  of passing unnoticed
- CI fails when the Dockerfile's Rust image falls below `rust-version`, the
  gap that left the image unbuildable from 2026-07-04 (see Fixed)

### Changed

- Replaced three yanked lockfile entries with their successors:
  `chacha20` 0.10.2 (via `rand`), `spin` 0.9.9 (via the iggy SDK) and
  `num-bigint` 0.4.8 (built only for tests, via `testcontainers`)
- Dependabot's cargo updates cover transitive dependencies
  (`allow: dependency-type: all`), so the weekly grouped PR doubles as a
  lockfile refresh. Version updates otherwise touch only what `Cargo.toml`
  names, and the GitHub Advisory Database carried none of this release's
  four advisories, so no Dependabot mode could have raised them
- `deny.toml` fails on yanked crates (`yanked = "deny"`) and on unsound
  advisories in any crate (`unsound = "all"`). cargo-deny's defaults only
  warned on the three yanked crates replaced above and never reported the
  transitive event-listener unsoundness at all. Run against the pre-fix
  lockfile, the stricter policy turns the unsoundness and all three yanked
  crates into errors. The yanked check reads cargo's local index cache, so
  the Dependency Policy job no longer restores one (an entry fetched before
  a yank would hide it), and an unreadable entry now fails the check
  (`-D index-failure`) instead of warning
- Every dependency-resolving cargo call in CI, the extended tests, the
  release build and the Dockerfile passes `--locked`. In CI's gating jobs
  and the release build, a `Cargo.lock` that does not match `Cargo.toml`
  now fails the job instead of being silently re-resolved on the runner
  (while `cargo audit` would still scan the committed file). The
  informational pr.yml and extended-tests.yml steps mask failures by
  design, so there it only stops the re-resolve
- The Security Audit job pins `rustsec/audit-check` to its Node 24 commit on
  `main`, which takes it out of that job's Node 20 deprecation annotation
  (`actions/checkout@v4` still triggers it until Dependabot bumps checkout).
  Upstream has cut no release since v2.0.0; the bundled action code is
  byte-identical

### Fixed

- Dependabot never ran: no run or PR appeared for either ecosystem after
  `.github/dependabot.yml` landed on 2025-12-01. The file fails schema
  validation on an ignore rule using the nonexistent
  `version-update:semver-prerelease` and on the `reviewers` option GitHub
  retired in 2025. Two settings that would have misfired once it ran are
  fixed too: the cargo
  commit prefix `deps(cargo)`, a type PR Checks rejects, is now
  `chore(deps)`; and custom labels that do not exist in the repository,
  which Dependabot silently drops, gave way to its auto-created defaults
- `SECURITY.md` listed only 0.1.x as supported; it now names 0.4.x and
  describes what each audit gate actually covers
- The Docker image could not be built since 2026-07-04: its builder stage
  used `rust:1.91.1` after `rust-version` rose to 1.93.0, and cargo refuses
  to build below the MSRV, so the compose quick start's `app` service
  failed at build time. The builder now uses `rust:1.98.1`, the current
  stable (the release binaries build on the floating stable channel), and
  builds with `--locked`
- A push to `main` during the Monday scheduled CI run would have cancelled
  it, since both shared one concurrency group, and with it that week's
  audit issue filing (only the scheduled run files issues); none of the 28
  scheduled runs so far was actually cancelled. CI's concurrency group now
  includes the event, so scheduled and push runs cannot cancel each other

### Security

- Bumped transitive `h2` 0.4.15 -> 0.4.19 (lockfile-only) to patch
  RUSTSEC-2026-0258: empty DATA frames were queued without limit, so a peer
  could grow memory on a stream that was not being drained, or overflow a
  length and panic, which this crate's `panic = "abort"` release profile
  turns into a process exit. Reachable here
  even though axum's `http2` feature is off: `metrics-exporter-prometheus`
  enables `hyper-util/server-auto`, which compiles HTTP/2 into the builder
  `axum::serve` uses, so the API listener accepts cleartext HTTP/2 (h2c)
  from any client that can reach it. Whether to keep that surface is
  tracked as TD-2026-09-01.
- Bumped transitive `rustls` 0.23.41 -> 0.23.45 (lockfile-only, with the
  `aws-lc-rs`, `aws-lc-sys` and `rustls-webpki` bumps it requires) to patch
  RUSTSEC-2026-0285: TLS 1.3 handshake messages were accepted at the wrong
  encryption level when packed into the same record as a key change. The
  transcript stays authenticated, so a peer cannot alter a handshake with
  it. Client-side only here: the service terminates no TLS, and rustls runs
  only on the Iggy connection when `IGGY_CONNECTION_STRING` selects TLS: a
  TLS-enabled transport, or `iggy+quic://`, which always uses it.
- Bumped transitive `event-listener` 5.4.1 -> 5.4.2 (lockfile-only) to patch
  RUSTSEC-2026-0221, an unsoundness: `StackSlot` was `Send + Sync`
  unconditionally, so a `!Send` tag set with `Event::with_tag` could cross
  threads through a `listener!` slot. Pulled in only under the iggy SDK:
  directly by `async-broadcast`, and by `iggy_common`'s `moka`, both
  directly and through `async-lock`. Not reachable: no crate in
  the graph calls `Event::with_tag`, so every event carries the default,
  `Send`, unit tag.
- Removed `rkyv` 0.7.46 from `Cargo.lock` (RUSTSEC-2026-0235: out-of-bounds
  reads through shared-pointer validation; only 0.8.17+ is patched) by
  raising `rust_decimal` to 1.43. It was never compiled: `rust_decimal`
  1.42 names it through the weak `rkyv?/std` feature, which pins an optional
  dependency in the lockfile without activating it. That is why
  `cargo deny` (resolved graph) stayed quiet while `cargo audit` (lockfile
  scan) flagged it. `rust_decimal` 1.43 drops the rkyv 0.7 bridge, taking
  `rkyv` and 13 crates that were in the lockfile only because of it.

## [0.4.0] - 2026-08-01

Session-03 tech-debt sweep: TD-2026-07-09 resolved. Plan review ran three
rounds before any code was written (25 agents; artifacts under
`docs/code-reviews/session-03-plan-round{1,2,3}.md`), which is why
TD-2026-07-08 moved to session 04 — round 2 established the two records were
mis-sequenced rather than merely mis-specified.

### Added

- Ownership of half-open circuit-breaker probe tokens. `admit()` returns a
  `ProbePermit` that returns its token on drop and is consumed when the
  outcome is recorded, closing the three accounting leaks TD-2026-07-09
  named: a request admitted while closed can no longer release a token it
  never took, a token from an expired probe window is discarded instead of
  credited to the live one, and a request future dropped mid-probe — a client
  disconnecting during an outage — returns its token instead of stranding it
  until the re-grant window
- `iggy_circuit_breaker_probe_dispositions_total{disposition}` — how probe
  tokens end, as `consumed` / `released` / `stale` / `abandoned` /
  `inconsistent`. The labels partition every admitted token, which is what
  makes `consumed` usable as a denominator; an abandoned-only counter could not
  distinguish a healthy system from a dead release path. `inconsistent` is
  separate on purpose — it means the token accounting is wrong, and it must not
  hide inside the routine `abandoned` volume

### Changed

- Circuit-breaker state is an enum whose variants own their own data, so a
  field belonging to another state is unrepresentable. Deletes two `Option`s,
  a window guard, a six-field hygiene reset and three defensive resets that
  were previously maintained by convention at each mutation site
- The breaker's state is guarded by `std::sync::Mutex` and its methods are
  synchronous. Not a preference: `Drop` cannot await, so a blocking guard is
  what makes the probe permit's release possible at all. Also removes the
  read-lock fast path and the read-to-write upgrade race it required
- Tracing and monotonic counters moved out of the breaker's critical section.
  The Prometheus state gauge deliberately stays inside it: it is
  last-writer-wins, and emitting it after releasing the guard would let racing
  transitions leave it permanently disagreeing with the breaker
- **Breaking**: `CircuitBreaker`, `CircuitBreakerConfig` and `CircuitState`
  are crate-internal, and `IggyClientWrapper`'s `circuit_breaker_state`,
  `circuit_breaker_metrics` and `force_close_circuit` accessors are removed.
  All had zero callers; narrowing the surface is what keeps the rest of this
  release non-breaking
- CI accepts the Conventional Commits `!` breaking-change marker, which its
  regex previously rejected outright

### Fixed

- `Config::validate` rejects a zero `CIRCUIT_BREAKER_OPEN_DURATION_SECS`, which
  disabled the breaker entirely — Open never rejected, and every admission past
  the budget re-granted — and a zero `OPERATION_TIMEOUT_SECS`, which opened the
  circuit on a healthy service and never closed it
- The 503 body for a rejected request now names the state that actually
  rejected. It previously re-read the breaker after the fact and could report a
  state a concurrent transition had already moved past

### Security

- Bumped transitive `crossbeam-epoch` 0.9.18 -> 0.9.20 (lockfile-only) to
  patch RUSTSEC-2026-0204: invalid pointer dereference in the
  `fmt::Pointer`/`Display` impls for `Atomic`/`Shared` on null pointers.
  Pulled in via the iggy SDK and `metrics-exporter-prometheus`; not used
  directly by this crate.

## [0.3.0] - 2026-07-05

Session-02 tech-debt sweep (PR #26): six registry records resolved, one
parked, two filed. Full eight-agent double review (Round 1 + Round 2);
artifacts under `docs/code-reviews/`.

### Added

- Token-limited half-open circuit-breaker probing: entering half-open
  grants `success_threshold` probe tokens per `open_duration` window
  (thundering-herd protection), with an anti-wedge re-grant and probe-token
  release for outcomes that deliberately record neither success nor
  failure; rejections are now labeled by breaker state in Prometheus
- End-to-end enforcement of client request deadlines: the parsed
  `X-Request-Timeout` extension (previously stored but unused) now bounds
  every Iggy operation through request-scoped client views, clamped so a
  client may shorten but never extend the global operation timeout;
  client-visible feedback (echoing the effective deadline) is tracked in
  TD-2026-07-08 before the header joins the external API reference
- Resilience composition test matrix: the timeout/breaker/reconnect-retry
  logic is extracted to `iggy_client::resilience::run_resilient` and
  covered by a paused-clock test per branch, plus a Prometheus exporter
  smoke test in its own test binary (183 unit / 30 integration / 18 model
  / 1 smoke tests)

### Changed

- Client-shortened deadlines no longer feed the shared circuit breaker
  (a single client could previously open the circuit for everyone), and
  request-scoped views no longer leak their deadline into the global
  reconnect session or health probes (`Arc<Config>` + separate
  per-view deadline; scoped clones are cheap)
- All third-party GitHub Actions pinned to full 40-char commit SHAs with
  version comments; toolchain/tool selector tags converted to explicit
  `with:` inputs
- Durable-storage guide re-validated key-by-key against upstream
  `server-0.8.0`: the disk/S3 archiver section now documents a feature
  removed upstream, retention is correctly attributed to the
  default-disabled `data_maintenance.messages` cleaner, `message_saver`
  defaults corrected (30 s interval, 1024-message threshold), and the
  partitioning guide was reconciled to the same schema
- **Breaking**: `RequestTimeout`'s fields are private — `from_millis` is
  the sole constructor and `duration()` the accessor — and
  `metrics::record_circuit_breaker_rejection` takes a state label

### Removed

- **Breaking**: the unused `RequestTimeoutExt` trait and
  `RequestTimeout::original_ms` (handlers extract `Option<RequestTimeout>`
  directly via a new `OptionalFromRequestParts` impl)

### Fixed

- Half-open probes completing with non-connection errors no longer
  permanently consume probe tokens (recovery-starvation cycle against a
  healthy server)
- `force_open` on an already-open breaker no longer refreshes the open
  window or inflates the times-opened counter

## [0.2.0] - 2026-07-05

### Security

- Refreshed `Cargo.lock` to patch 10 RUSTSEC advisories in transitive
  dependencies: `bytes` (RUSTSEC-2026-0007), `time` (RUSTSEC-2026-0009),
  `quinn-proto` (RUSTSEC-2026-0037), `rustls-webpki` (RUSTSEC-2026-0049),
  `aws-lc-sys` (RUSTSEC-2026-0044 through 0048), and `rkyv`
  (RUSTSEC-2026-0001)
- `testcontainers` 0.27 bump upgrades `astral-tokio-tar` to patched 0.6.x
  and removes unmaintained `rustls-pemfile` from the dev-dependency tree
- `cargo audit` now reports zero vulnerabilities

### Changed

- Updated Apache Iggy Rust SDK from 0.8.0 to 0.10.0 (latest stable);
  no source changes required — the `Client` trait API is unchanged
- **Breaking**: MSRV raised 1.90 → 1.93: iggy 0.10's `compio-buf`
  dependency uses APIs stabilized in Rust 1.93 (and declares no
  rust-version, so cargo cannot catch this at resolution time)
- Pinned the `apache/iggy` server image to 0.8.0 (the release paired
  with the 0.10 SDK) in `docker-compose.yaml` and integration tests,
  replacing the floating `latest` tag
- Bumped direct dependencies: `tower-http` 0.7, `rand` 0.10,
  `metrics-exporter-prometheus` 0.18, `testcontainers` 0.27 (dev),
  `reqwest` 0.13 (dev); raised version floors for `tokio` (1.52),
  `uuid` (1.23), and `rust_decimal` (1.42)
- Migrated `deny.toml` to the current cargo-deny schema and pruned
  obsolete advisory ignores; allowed `Unicode-3.0` and
  `CDLA-Permissive-2.0` licenses required by new transitive deps
- Documented why the service integrates at the SDK `Client` trait level
  instead of the high-level `IggyProducer`/`IggyConsumer` clients
- **Breaking**: default app port changed from 3000 to 8000 — the old
  default collided with the Iggy server's HTTP API port under the
  documented docker-compose quick start; all docs, `.env.example`, and
  compose now agree on 8000
- CI now fails on `cargo deny check advisories licenses` (previously
  licenses-only and non-blocking); weekly stress tests pin
  `apache/iggy:0.8.0` instead of `latest`
- Crate marked `publish = false`: releases are repo-level only (GitHub
  Releases + GitHub Pages docs) - cargo itself refuses to publish, so
  the release pipeline's publish step is a harmless no-op
- Updated `docker-compose.yaml` with full observability stack configuration
- Simplified documentation section in README.md to reference `docs/`
  directory

### Fixed

Findings from the session-01 eight-agent double review
(`docs/code-reviews/`); deferred items carry tech-debt records with binding
triggers (`docs/tech-debt/`):

- **Resilience**: SDK connection errors are now classified into the
  wrapper's connection-aware variants, making the reconnect and
  circuit-breaker paths reachable (previously dead code); the background
  health check performs live pings so `/health` and `/ready` stay truthful
  during outages; reconnection no longer leaks the old client's heartbeat
  task, resets its attempt counter per session, uses saturating backoff
  arithmetic, and is bounded on the request path; `ensure_stream/topic` no
  longer swallow lookup errors and tolerate losing a concurrent creation
  race instead of crash-looping
- **Security**: `TRUSTED_PROXIES` is enforced against the actual peer
  address (spoofed forwarded headers from untrusted peers are ignored) and
  invalid entries fail startup; the auth brute-force limiter meters
  failures only, so valid-key clients are no longer throttled to the
  failure budget
- **Observability**: the Prometheus exporter is now actually started on
  `METRICS_PORT` and the message/reconnect/breaker metrics are recorded;
  Prometheus scrapes the correct port
- **API**: `count=0` polls return 400 instead of 500; all-digit resource
  names ("42") are treated as names, not numeric server IDs; removed the
  dead `PollMessagesRequest` type
- Added `issues: write` permission to CI security audit job to allow
  creating advisory issues

### Added

- **Observability Stack**: Complete Grafana-based monitoring setup
  - Prometheus metrics collection (port 9090) with 15-day retention
  - Grafana dashboards (port 3001) with pre-configured Prometheus datasource
  - Iggy Web UI integration (port 3050) for stream/topic/message management
  - Pre-built Iggy Overview dashboard (server status, request rates, throughput, latency)
- **Documentation Guides**:
  - Event-driven architecture guide (`docs/guide.md`): streams/topics/partitions, consumer groups, error handling patterns, production patterns (outbox, saga, idempotency)
  - Partitioning guide (`docs/partitioning-guide.md`): partition keys, ordering guarantees, selection strategies
  - Durable storage guide (`docs/durable-storage-guide.md`): storage architecture, fsync configuration, S3 backup/archiving, recovery procedures
  - Documentation index (`docs/README.md`) with topic-based navigation

## [0.1.0] - 2024-12-01

### Added

- Initial public release
- RESTful API for Apache Iggy message streaming
- True batch message sending (single network call for multiple messages)
- Graceful shutdown with SIGTERM/SIGINT handling
- Input validation and sanitization for resource names
- Comprehensive error handling with `Result` types
- Zero clippy warnings policy with strict lints
- Stream and topic management endpoints
- Health checks (`/health`, `/ready`) and service statistics (`/stats`)
- Domain-driven event modeling (User, Order, Generic events)
- Partition-based message routing with partition keys
- Connection resilience with automatic reconnection and exponential backoff
- Rate limiting with token bucket algorithm (Governor)
- API key authentication with constant-time comparison
- Request ID propagation for distributed tracing
- Configurable CORS with origin whitelist support
- Background stats caching
- Structured concurrency with TaskTracker and CancellationToken
- Background health checks for connection monitoring
- Docker and Docker Compose support
- Comprehensive test suite (unit, integration, fuzz tests)
- GitHub Actions CI/CD workflows
- Dependabot configuration for automated updates

### Security

- Constant-time API key comparison (timing attack resistant)
- Per-IP brute force protection
- Trusted proxy configuration for X-Forwarded-For validation
- Input validation to prevent injection attacks

[Unreleased]: https://github.com/mlevkov/iggy_sample/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/mlevkov/iggy_sample/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/mlevkov/iggy_sample/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/mlevkov/iggy_sample/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/mlevkov/iggy_sample/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/mlevkov/iggy_sample/releases/tag/v0.1.0
