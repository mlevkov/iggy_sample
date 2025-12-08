# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
