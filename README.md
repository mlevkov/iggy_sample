# Iggy Sample Application

[![CI](https://github.com/mlevkov/iggy_sample/actions/workflows/ci.yml/badge.svg)](https://github.com/mlevkov/iggy_sample/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.90%2B-blue.svg)](https://www.rust-lang.org)

A comprehensive demonstration of [Apache Iggy](https://github.com/apache/iggy) message streaming integrated with [Axum](https://github.com/tokio-rs/axum) web framework in Rust.

## Overview

This project showcases how to build a production-ready message streaming service using:

- **Apache Iggy 0.6.0-edge** - High-performance message streaming with io_uring shared-nothing architecture
- **Iggy SDK 0.8.0-edge.6** - Latest edge SDK for compatibility with edge server features
- **Axum 0.8** - Ergonomic and modular Rust web framework
- **Tokio** - Async runtime for Rust

Apache Iggy is capable of processing millions of messages per second with ultra-low latency, supporting TCP, QUIC, WebSocket, and HTTP transport protocols.

## Features

### Core Functionality
- RESTful API for message publishing and consumption
- **True batch message sending** (single network call for multiple messages)
- **Graceful shutdown** with SIGTERM/SIGINT handling
- **Input validation and sanitization** for resource names
- **Comprehensive error handling** with `Result` types (no `unwrap()`/`expect()` in production code)
- **Zero clippy warnings** - strict lints enforced, no `#[allow(...)]` in production code
- Stream and topic management endpoints
- Health checks and service statistics
- Domain-driven event modeling (User, Order, Generic events)
- Partition-based message routing

### Production-Ready Features
- **Connection resilience** with automatic reconnection and exponential backoff
- **Rate limiting** with token bucket algorithm (configurable RPS and burst)
- **API key authentication** with constant-time comparison (timing attack resistant)
- **Request ID propagation** for distributed tracing
- **Configurable CORS** with origin whitelist support
- **Background stats caching** to avoid expensive queries on each request

### Development & Testing
- Docker Compose setup for local development
- Comprehensive test suite (93 unit tests, 24 integration tests, 20 model tests)
- Integration tests with testcontainers (auto-spins Iggy server)
- Fuzz testing for input validation functions

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Axum HTTP Server                       │
│                        (Port 8000)                          │
├─────────────────────────────────────────────────────────────┤
│  Middleware Stack                                           │
│  Rate Limit → Auth → Request ID → Tracing → CORS            │
├─────────────────────────────────────────────────────────────┤
│  Handlers                                                   │
│  ├── health.rs    - Health/readiness checks, stats          │
│  ├── messages.rs  - Send/poll messages                      │
│  ├── streams.rs   - Stream CRUD operations                  │
│  └── topics.rs    - Topic CRUD operations                   │
├─────────────────────────────────────────────────────────────┤
│  Services                                                   │
│  ├── producer.rs  - Message publishing logic                │
│  └── consumer.rs  - Message consumption logic               │
├─────────────────────────────────────────────────────────────┤
│  IggyClientWrapper (with auto-reconnection)                 │
│  High-level wrapper around Iggy SDK                         │
├─────────────────────────────────────────────────────────────┤
│  Observability Stack                                        │
│  Prometheus (9090) → Grafana (3001) + Iggy Web UI (3050)    │
├─────────────────────────────────────────────────────────────┤
│  Apache Iggy Server                                         │
│  TCP (8090) / QUIC (8080) / HTTP (3000) / Metrics (/metrics)│
└─────────────────────────────────────────────────────────────┘
```

## Prerequisites

- Rust 1.90+ (edition 2024, MSRV: 1.90.0)
- Docker & Docker Compose
- curl or httpie (for testing)

## Quick Start

### 1. Clone and Setup

```bash
cd iggy_sample
cp .env.example .env
```

### 2. Start the Full Stack

```bash
# Start Iggy server with observability stack
docker-compose up -d
```

This starts:
- **Iggy Server** - Message streaming (ports 8090, 8080, 3000)
- **Iggy Web UI** - Dashboard for managing streams/topics (port 3050)
- **Prometheus** - Metrics collection (port 9090)
- **Grafana** - Visualization dashboards (port 3001)

### 3. Run the Application

```bash
cargo run
```

The server will start on `http://localhost:8000` (or the port specified in `.env`).

### 4. Verify It's Working

```bash
curl http://localhost:8000/health
```

Expected response:
```json
{
  "status": "healthy",
  "iggy_connected": true,
  "version": "0.1.0",
  "timestamp": "2024-01-15T10:30:00Z"
}
```

### 5. Access the Dashboards

| Service | URL | Credentials |
|---------|-----|-------------|
| Sample App API | http://localhost:8000 | - |
| Iggy HTTP API | http://localhost:3000 | - |
| Iggy Web UI | http://localhost:3050 | iggy / iggy |
| Prometheus | http://localhost:9090 | - |
| Grafana | http://localhost:3001 | admin / admin |

## API Reference

### Health & Status

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check with Iggy connection status |
| `/ready` | GET | Kubernetes readiness probe (200 if ready) |
| `/stats` | GET | Service statistics (streams, messages, uptime) |

### Messages (Default Stream/Topic)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/messages` | POST | Send a single message |
| `/messages` | GET | Poll messages |
| `/messages/batch` | POST | Send multiple messages |

### Messages (Specific Stream/Topic)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/streams/{stream}/topics/{topic}/messages` | POST | Send to specific topic |
| `/streams/{stream}/topics/{topic}/messages` | GET | Poll from specific topic |

### Stream Management

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/streams` | GET | List all streams |
| `/streams` | POST | Create a new stream |
| `/streams/{name}` | GET | Get stream details |
| `/streams/{name}` | DELETE | Delete a stream |

### Topic Management

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/streams/{stream}/topics` | GET | List topics in stream |
| `/streams/{stream}/topics` | POST | Create a topic |
| `/streams/{stream}/topics/{topic}` | GET | Get topic details |
| `/streams/{stream}/topics/{topic}` | DELETE | Delete a topic |

## Usage Examples

### Send a User Event

```bash
curl -X POST http://localhost:3000/messages \
  -H "Content-Type: application/json" \
  -d '{
    "event": {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "event_type": "user.created",
      "timestamp": "2024-01-15T10:30:00Z",
      "payload": {
        "type": "User",
        "data": {
          "action": "Created",
          "user_id": "550e8400-e29b-41d4-a716-446655440001",
          "email": "user@example.com",
          "name": "John Doe"
        }
      }
    }
  }'
```

### Send an Order Event

```bash
curl -X POST http://localhost:3000/messages \
  -H "Content-Type: application/json" \
  -d '{
    "event": {
      "id": "550e8400-e29b-41d4-a716-446655440002",
      "event_type": "order.created",
      "timestamp": "2024-01-15T10:31:00Z",
      "payload": {
        "type": "Order",
        "data": {
          "action": "Created",
          "order_id": "550e8400-e29b-41d4-a716-446655440003",
          "user_id": "550e8400-e29b-41d4-a716-446655440001",
          "items": [
            {
              "product_id": "550e8400-e29b-41d4-a716-446655440004",
              "quantity": 2,
              "unit_price": 29.99
            }
          ],
          "total_amount": 59.98
        }
      }
    },
    "partition_key": "user-550e8400"
  }'
```

### Send a Generic Event

```bash
curl -X POST http://localhost:3000/messages \
  -H "Content-Type: application/json" \
  -d '{
    "event": {
      "id": "550e8400-e29b-41d4-a716-446655440005",
      "event_type": "custom.event",
      "timestamp": "2024-01-15T10:32:00Z",
      "payload": {
        "type": "Generic",
        "data": {
          "custom_field": "any value",
          "nested": {"key": "value"}
        }
      }
    }
  }'
```

### Poll Messages

```bash
# Poll from partition 1, starting at offset 0
curl "http://localhost:3000/messages?partition_id=1&count=10&offset=0"

# Poll with auto-commit
curl "http://localhost:3000/messages?partition_id=1&count=10&auto_commit=true"
```

### Send Batch Messages

```bash
curl -X POST http://localhost:3000/messages/batch \
  -H "Content-Type: application/json" \
  -d '{
    "events": [
      {
        "id": "550e8400-e29b-41d4-a716-446655440006",
        "event_type": "batch.event.1",
        "timestamp": "2024-01-15T10:33:00Z",
        "payload": {"type": "Generic", "data": {"index": 1}}
      },
      {
        "id": "550e8400-e29b-41d4-a716-446655440007",
        "event_type": "batch.event.2",
        "timestamp": "2024-01-15T10:33:01Z",
        "payload": {"type": "Generic", "data": {"index": 2}}
      }
    ]
  }'
```

### Create a Stream

```bash
curl -X POST http://localhost:3000/streams \
  -H "Content-Type: application/json" \
  -d '{"name": "my-stream"}'
```

### Create a Topic

```bash
curl -X POST http://localhost:3000/streams/my-stream/topics \
  -H "Content-Type: application/json" \
  -d '{"name": "my-topic", "partitions": 3}'
```

### List Streams

```bash
curl http://localhost:3000/streams
```

### Get Statistics

```bash
curl http://localhost:3000/stats
```

## Configuration

Configuration is loaded from environment variables. See `.env.example` for all options.

### Server & Iggy Connection
| Variable | Default | Description |
|----------|---------|-------------|
| `HOST` | `0.0.0.0` | Server bind address |
| `PORT` | `3000` | Server port |
| `IGGY_CONNECTION_STRING` | `iggy://iggy:iggy@localhost:8090` | Iggy connection string |
| `IGGY_STREAM` | `sample-stream` | Default stream name |
| `IGGY_TOPIC` | `events` | Default topic name |
| `IGGY_PARTITIONS` | `3` | Partitions for default topic |
| `RUST_LOG` | `info` | Log level (trace, debug, info, warn, error) |

### Connection Resilience
| Variable | Default | Description |
|----------|---------|-------------|
| `MAX_RECONNECT_ATTEMPTS` | `0` | Max reconnect attempts (0 = infinite) |
| `RECONNECT_BASE_DELAY_MS` | `1000` | Base delay for exponential backoff |
| `RECONNECT_MAX_DELAY_MS` | `30000` | Max delay between reconnection attempts |
| `HEALTH_CHECK_INTERVAL_SECS` | `30` | Connection health check interval |

### Rate Limiting & Security
| Variable | Default | Description |
|----------|---------|-------------|
| `RATE_LIMIT_RPS` | `100` | Requests per second (0 = disabled) |
| `RATE_LIMIT_BURST` | `50` | Burst capacity above RPS limit |
| `API_KEY` | (none) | API key for authentication (disabled if not set) |
| `AUTH_BYPASS_PATHS` | `/health,/ready` | Comma-separated paths that bypass auth |
| `CORS_ALLOWED_ORIGINS` | `*` | Comma-separated allowed origins |

### Message Limits & Observability
| Variable | Default | Description |
|----------|---------|-------------|
| `BATCH_MAX_SIZE` | `1000` | Max messages per batch send |
| `POLL_MAX_COUNT` | `100` | Max messages per poll |
| `STATS_CACHE_TTL_SECS` | `5` | Stats cache refresh interval |

### Connection String Format

```
iggy://username:password@host:port
```

Example:
```
iggy://iggy:iggy@localhost:8090
```

## Project Structure

```
iggy_sample/
├── .github/
│   ├── workflows/
│   │   ├── ci.yml              # Main CI (tests, lint, coverage)
│   │   ├── pr.yml              # PR checks (size, commits, docs)
│   │   ├── release.yml         # Multi-platform release builds
│   │   └── extended-tests.yml  # Weekly stress/benchmark tests
│   ├── dependabot.yml          # Automated dependency updates
│   └── pull_request_template.md
├── Cargo.toml              # Dependencies and metadata
├── Dockerfile              # Multi-stage build for production
├── docker-compose.yaml     # Local development setup
├── deny.toml               # License/security policy (cargo-deny)
├── .env.example            # Environment variable template
├── CLAUDE.md               # Project documentation for AI assistants
├── README.md               # This file
├── src/
│   ├── main.rs             # Application entry point
│   ├── lib.rs              # Library exports
│   ├── config.rs           # Configuration from environment
│   ├── error.rs            # Error types with HTTP status codes
│   ├── state.rs            # Shared application state
│   ├── routes.rs           # Route definitions
│   ├── iggy_client.rs      # Iggy SDK wrapper
│   ├── validation.rs       # Input validation utilities
│   ├── middleware/
│   │   ├── mod.rs          # Middleware exports
│   │   ├── rate_limit.rs   # Token bucket rate limiting
│   │   ├── auth.rs         # API key authentication
│   │   └── request_id.rs   # Request ID propagation
│   ├── models/
│   │   ├── mod.rs          # Model exports
│   │   ├── event.rs        # Domain events (uses rust_decimal)
│   │   └── api.rs          # API request/response types
│   ├── services/
│   │   ├── mod.rs          # Service exports
│   │   ├── producer.rs     # Message producer service
│   │   └── consumer.rs     # Message consumer service
│   └── handlers/
│       ├── mod.rs          # Handler exports
│       ├── health.rs       # Health endpoints
│       ├── messages.rs     # Message endpoints
│       ├── streams.rs      # Stream management
│       └── topics.rs       # Topic management
├── tests/
│   ├── integration_tests.rs # End-to-end API tests
│   └── model_tests.rs       # Unit tests for models
└── fuzz/
    ├── Cargo.toml           # Fuzz testing configuration
    └── fuzz_targets/
        └── fuzz_validation.rs # Validation function fuzz tests
```

## Testing

### Run Unit Tests

```bash
cargo test
```

### Run Integration Tests

Integration tests require a running server:

```bash
# Terminal 1: Start services
docker-compose up -d iggy
cargo run &

# Terminal 2: Run integration tests
cargo test --test integration_tests -- --ignored
```

### Run with Coverage

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --out Html
```

## Docker

### Build and Run with Docker Compose

```bash
# Start everything (Iggy + App)
docker-compose up -d

# View logs
docker-compose logs -f app

# Stop
docker-compose down
```

### Build Docker Image Manually

```bash
docker build -t iggy-sample .
docker run -p 8000:8000 -e IGGY_CONNECTION_STRING=iggy://iggy:iggy@host.docker.internal:8090 iggy-sample
```

## Observability Stack

The project includes a complete observability stack for monitoring and managing Iggy.

### Components

| Component | Port | Description |
|-----------|------|-------------|
| **Iggy Server** | 3000 | HTTP API + Prometheus metrics at `/metrics` |
| **Iggy Web UI** | 3050 | Dashboard for streams, topics, messages, users |
| **Prometheus** | 9090 | Metrics collection with 15-day retention |
| **Grafana** | 3001 | Pre-configured dashboards for visualization |

### Iggy Web UI

The Iggy Web UI provides a comprehensive dashboard:

- **Streams & Topics**: Create, browse, and delete streams and topics
- **Messages**: Browse and inspect messages in real-time
- **Users**: Manage users and permissions
- **Server Health**: Monitor connections and server status

Access at http://localhost:3050 with credentials `iggy/iggy`.

### Grafana Dashboards

Pre-configured dashboards are automatically provisioned:

- **Iggy Overview**: Server status, request rates, message throughput, latency percentiles

Access at http://localhost:3001 with credentials `admin/admin`.

### Prometheus Metrics

Iggy exposes Prometheus-compatible metrics:

```bash
# View raw metrics from Iggy
curl http://localhost:3000/metrics

# Query via Prometheus
curl 'http://localhost:9090/api/v1/query?query=up{job="iggy"}'
```

### Configuration Files

```
observability/
├── prometheus/
│   └── prometheus.yml           # Scrape configuration
└── grafana/
    └── provisioning/
        ├── datasources/
        │   └── datasources.yml  # Prometheus datasource
        └── dashboards/
            ├── dashboards.yml   # Dashboard provisioning
            └── iggy-overview.json
```

### Adding OpenTelemetry (Optional)

Iggy supports OpenTelemetry for distributed tracing:

```yaml
# Add to iggy service in docker-compose.yaml
environment:
  - IGGY_TELEMETRY_ENABLED=true
  - IGGY_TELEMETRY_SERVICE_NAME=iggy
  - IGGY_TELEMETRY_LOGS_TRANSPORT=grpc
  - IGGY_TELEMETRY_LOGS_ENDPOINT=http://otel-collector:4317
  - IGGY_TELEMETRY_TRACES_TRANSPORT=grpc
  - IGGY_TELEMETRY_TRACES_ENDPOINT=http://otel-collector:4317
```

## Event Schema

Events follow a structured format with type-safe payloads:

```json
{
  "id": "uuid",
  "event_type": "domain.action",
  "timestamp": "ISO8601",
  "payload": {
    "type": "User|Order|Generic",
    "data": { ... }
  },
  "correlation_id": "optional-uuid",
  "source": "optional-service-name"
}
```

### Supported Event Types

#### User Events
- `Created` - New user registration
- `Updated` - User profile update
- `Deleted` - User account deletion
- `LoggedIn` - User authentication

#### Order Events
- `Created` - New order placed
- `Updated` - Order status change
- `Cancelled` - Order cancellation
- `Shipped` - Order shipment

#### Generic Events
- Any JSON payload for flexible use cases

## Error Handling

All errors return structured JSON responses:

```json
{
  "error": "error_type",
  "message": "Human-readable message",
  "details": "Optional additional context"
}
```

| Error Type | HTTP Status | Description |
|------------|-------------|-------------|
| `connection_failed` | 503 | Iggy server unavailable |
| `stream_error` | 500 | Stream operation failed |
| `topic_error` | 500 | Topic operation failed |
| `send_error` | 500 | Message send failed |
| `poll_error` | 500 | Message poll failed |
| `not_found` | 404 | Resource not found |
| `bad_request` | 400 | Invalid request data |

## Security

This application implements multiple security layers suitable for production deployment.

### Implemented Security Features

| Feature | Location | Description |
|---------|----------|-------------|
| CORS | `src/routes.rs` | Configurable origin whitelist via `CORS_ALLOWED_ORIGINS` using `tower-http` |
| API Key Authentication | `src/middleware/auth.rs` | Constant-time comparison to prevent timing attacks |
| Rate Limiting | `src/middleware/rate_limit.rs` | Token bucket algorithm via Governor, configurable RPS and burst |
| Brute Force Protection | `src/middleware/auth.rs` | Per-IP tracking of failed authentication attempts |
| Input Validation | `src/validation.rs` | Sanitization of stream names, topic names, and event types |
| Trusted Proxy Support | `src/middleware/ip.rs` | X-Forwarded-For validation against configurable CIDR ranges |
| Request ID Propagation | `src/middleware/request_id.rs` | UUIDv4 generation for distributed tracing |
| Security Audit | `.github/workflows/ci.yml` | Automated `cargo-audit` vulnerability scanning in CI |
| Vulnerability Reporting | `SECURITY.md` | Responsible disclosure policy |

### Not Included (by design)

| Feature | Reason |
|---------|--------|
| JWT | API keys are appropriate for service-to-service auth; JWT adds complexity without benefit for this use case |
| RBAC/ABAC | Single-purpose demonstration application, not a multi-tenant system |
| Encrypted cache | Stats cache is internal, read-only, and contains no sensitive data |

For deployment guidelines (reverse proxy configuration, trusted proxies), see [CLAUDE.md](CLAUDE.md#deployment-security).

## Performance Considerations

- **Batch Operations**: Use `/messages/batch` for high-throughput scenarios
- **Partitioning**: Configure appropriate partition count for parallelism
- **Partition Keys**: Use consistent keys for ordered processing within a partition
- **Auto-Commit**: Enable for at-least-once delivery semantics
- **Connection Pooling**: The client maintains persistent connections

## Dependencies

Key dependencies (see `Cargo.toml` for full list):

| Crate | Version | Purpose |
|-------|---------|---------|
| `iggy` | 0.8.0-edge.6 | Iggy Rust SDK (edge) |
| `axum` | 0.8 | Web framework |
| `tokio` | 1.48 | Async runtime |
| `serde` | 1.0 | Serialization |
| `tracing` | 0.1 | Structured logging |
| `thiserror` | 2.0 | Error handling |
| `governor` | 0.8 | Rate limiting (token bucket) |
| `subtle` | 2.6 | Constant-time comparison |
| `tower-http` | 0.6 | HTTP middleware (CORS, tracing) |
| `rust_decimal` | 1.37 | Exact decimal arithmetic for money |
| `uuid` | 1.18 | UUID generation |
| `chrono` | 0.4 | Date/time handling |
| `testcontainers` | 0.24 | Integration testing |

## CI/CD

This project uses GitHub Actions for continuous integration and deployment:

| Workflow | Trigger | Description |
|----------|---------|-------------|
| `ci.yml` | Push, PR | Tests, linting, coverage, security audit |
| `pr.yml` | PR | Size checks, conventional commits, semver |
| `release.yml` | Tag `v*` | Multi-platform builds, GitHub release |
| `extended-tests.yml` | Weekly | Benchmarks, stress tests, memory checks |

### Automated Checks
- **Formatting**: `cargo fmt --check`
- **Linting**: `cargo clippy -- -D warnings`
- **Tests**: Matrix across 3 OSes × 3 Rust versions
- **Coverage**: Uploaded to Codecov
- **Security**: `cargo-audit` vulnerability scanning
- **Licenses**: `cargo-deny` compliance checking

### Dependabot
Automatically creates PRs for:
- Cargo dependency updates (weekly)
- GitHub Actions updates (weekly)

## Documentation

See the [docs/](docs/) directory for comprehensive guides covering event-driven architecture, partitioning strategies, durable storage configuration, and more.

## Resources

- [Apache Iggy GitHub](https://github.com/apache/iggy)
- [Apache Iggy Documentation](https://iggy.apache.org/docs/introduction/getting-started/)
- [Iggy Rust SDK on crates.io](https://crates.io/crates/iggy)
- [Axum Documentation](https://docs.rs/axum/latest/axum/)

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'feat: add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request
