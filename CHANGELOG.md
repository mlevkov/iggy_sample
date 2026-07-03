# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

### Changed

- Updated `docker-compose.yaml` with full observability stack configuration
- Simplified documentation section in README.md to reference `docs/` directory

### Fixed

- Added `issues: write` permission to CI security audit job to allow creating advisory issues

### Security

- Ignored unmaintained transitive dependency advisories in `deny.toml`:
  - `RUSTSEC-2024-0384` (instant): from iggy -> reqwest-retry -> parking_lot v0.11
  - `RUSTSEC-2025-0134` (rustls-pemfile): from testcontainers -> bollard (dev-dep only)

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

[Unreleased]: https://github.com/mlevkov/iggy_sample/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/mlevkov/iggy_sample/releases/tag/v0.1.0
