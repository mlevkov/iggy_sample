# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

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

[Unreleased]: https://github.com/mlevkov/iggy_sample/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/mlevkov/iggy_sample/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/mlevkov/iggy_sample/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/mlevkov/iggy_sample/releases/tag/v0.1.0
