# Iggy Sample Application

A comprehensive demonstration of Apache Iggy message streaming with Axum.

## Project Overview

This application showcases how to build a production-ready message streaming service using:
- **Apache Iggy 0.6.0-edge**: High-performance message streaming with io_uring shared-nothing architecture
- **Iggy SDK 0.8.0-edge.6**: Latest edge SDK for compatibility with edge server features
- **Axum 0.8**: Ergonomic and modular Rust web framework
- **Tokio**: Async runtime for Rust

### Key Features
- True batch message sending (single network call for multiple messages)
- Graceful shutdown with SIGTERM/SIGINT handling
- Input validation and sanitization for resource names
- Comprehensive error handling with `Result` types (no `unwrap()`/`expect()` in production code)
- **Zero clippy warnings** - strict lints enforced, no `#[allow(...)]` in production code
- **Connection resilience** with automatic reconnection and exponential backoff
- **Rate limiting** with token bucket algorithm (configurable RPS and burst)
- **API key authentication** with constant-time comparison (timing attack resistant)
- **Request ID propagation** for distributed tracing
- **Configurable CORS** with origin whitelist support
- **Background stats caching** to avoid expensive queries on each request
- **Structured concurrency** with proper task lifecycle management
- **Background health checks** for early connection issue detection

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Axum HTTP Server                       │
│                      (src/main.rs)                          │
├─────────────────────────────────────────────────────────────┤
│  Middleware Stack (src/middleware/)                         │
│  - rate_limit.rs: Token bucket rate limiting                │
│  - auth.rs: API key authentication                          │
│  - request_id.rs: Request ID propagation                    │
│  + tower_http: Tracing, CORS                                │
├─────────────────────────────────────────────────────────────┤
│  Handlers (src/handlers/)                                   │
│  - health.rs: Health/readiness checks, stats                │
│  - messages.rs: Send/poll messages                          │
│  - streams.rs: Stream CRUD operations                       │
│  - topics.rs: Topic CRUD operations                         │
├─────────────────────────────────────────────────────────────┤
│  Services (src/services/)                                   │
│  - producer.rs: Message publishing logic                    │
│  - consumer.rs: Message consumption logic                   │
├─────────────────────────────────────────────────────────────┤
│  IggyClientWrapper (src/iggy_client.rs)                     │
│  High-level wrapper with automatic reconnection             │
│  + PollParams builder for cleaner polling API               │
├─────────────────────────────────────────────────────────────┤
│  Background Tasks (managed by TaskTracker)                  │
│  - Stats refresh task (periodic cache update)               │
│  - Health check task (connection monitoring)                │
├─────────────────────────────────────────────────────────────┤
│  Apache Iggy Server (TCP/QUIC/HTTP)                         │
│  Persistent message streaming                               │
└─────────────────────────────────────────────────────────────┘
```

## Directory Structure

```
.github/
├── workflows/
│   ├── ci.yml              # Main CI (fmt, clippy, tests, coverage, audit)
│   ├── pr.yml              # PR checks (size, commits, docs, semver)
│   ├── release.yml         # Release builds and publishing
│   └── extended-tests.yml  # Weekly stress/memory/benchmark tests
├── dependabot.yml          # Automated dependency updates
└── pull_request_template.md

.commitlintrc.json            # Conventional commits configuration

observability/
├── prometheus/
│   └── prometheus.yml      # Prometheus scrape configuration
└── grafana/
    └── provisioning/
        ├── datasources/
        │   └── datasources.yml    # Prometheus datasource
        └── dashboards/
            ├── dashboards.yml     # Dashboard provisioning
            └── iggy-overview.json # Pre-built Iggy dashboard

src/
├── main.rs           # Application entry point
├── lib.rs            # Library exports
├── config.rs         # Configuration from environment
├── error.rs          # Error types with HTTP status codes
├── state.rs          # Shared application state with stats caching
├── routes.rs         # Route definitions and middleware stack
├── iggy_client.rs    # Iggy SDK wrapper with auto-reconnection
├── validation.rs     # Input validation utilities
├── middleware/
│   ├── mod.rs        # Middleware exports
│   ├── ip.rs         # Client IP extraction (shared by rate_limit and auth)
│   ├── rate_limit.rs # Token bucket rate limiting (Governor)
│   ├── auth.rs       # API key authentication
│   └── request_id.rs # Request ID propagation
├── models/
│   ├── mod.rs        # Model exports
│   ├── event.rs      # Domain event types (uses rust_decimal for money)
│   └── api.rs        # API request/response types
├── services/
│   ├── mod.rs        # Service exports
│   ├── producer.rs   # Message producer service
│   └── consumer.rs   # Message consumer service
└── handlers/
    ├── mod.rs        # Handler exports
    ├── health.rs     # Health endpoints
    ├── messages.rs   # Message endpoints
    ├── streams.rs    # Stream management
    └── topics.rs     # Topic management

tests/
├── integration_tests.rs  # End-to-end API tests with testcontainers
│   ├── Standard fixture tests (basic CRUD, messages)
│   └── Security boundary tests (auth, rate limiting)
└── model_tests.rs        # Unit tests for models

fuzz/
├── Cargo.toml            # Fuzz testing configuration
└── fuzz_targets/
    └── fuzz_validation.rs # Validation function fuzz tests

deny.toml                 # License and security policy for cargo-deny

docs/                     # Documentation and guides
├── README.md             # Guide index and navigation
├── guide.md              # Event-driven architecture guide
├── partitioning-guide.md # Partitioning strategies
├── durable-storage-guide.md # Storage and durability configuration
└── structured-concurrency.md # Task lifecycle management
```

## Documentation

See the [docs/](docs/) directory for comprehensive guides covering event-driven architecture, partitioning strategies, durable storage configuration, and more.

## API Endpoints

### Health & Status
- `GET /health` - Health check with Iggy connection status
- `GET /ready` - Kubernetes readiness probe
- `GET /stats` - Service statistics

### Messages (Default Stream/Topic)
- `POST /messages` - Send a single message
- `GET /messages` - Poll messages
- `POST /messages/batch` - Send multiple messages

### Messages (Specific Stream/Topic)
- `POST /streams/{stream}/topics/{topic}/messages` - Send to specific topic
- `GET /streams/{stream}/topics/{topic}/messages` - Poll from specific topic

### Stream Management
- `GET /streams` - List all streams
- `POST /streams` - Create a new stream
- `GET /streams/{name}` - Get stream details
- `DELETE /streams/{name}` - Delete a stream

### Topic Management
- `GET /streams/{stream}/topics` - List topics in stream
- `POST /streams/{stream}/topics` - Create a topic
- `GET /streams/{stream}/topics/{topic}` - Get topic details
- `DELETE /streams/{stream}/topics/{topic}` - Delete a topic

## Configuration

Environment variables (see `.env.example`):

### Server Configuration
| Variable | Default | Description |
|----------|---------|-------------|
| `HOST` | `0.0.0.0` | Server bind address |
| `PORT` | `3000` | Server port |
| `RUST_LOG` | `info` | Log level |

### Iggy Connection
| Variable | Default | Description |
|----------|---------|-------------|
| `IGGY_CONNECTION_STRING` | `iggy://iggy:iggy@localhost:8090` | Iggy connection string |
| `IGGY_STREAM` | `sample-stream` | Default stream name |
| `IGGY_TOPIC` | `events` | Default topic name |
| `IGGY_PARTITIONS` | `3` | Partitions for default topic |

### Connection Resilience
| Variable | Default | Description |
|----------|---------|-------------|
| `MAX_RECONNECT_ATTEMPTS` | `0` | Max reconnect attempts (0 = infinite) |
| `RECONNECT_BASE_DELAY_MS` | `1000` | Base delay for exponential backoff |
| `RECONNECT_MAX_DELAY_MS` | `30000` | Max delay between reconnects |
| `HEALTH_CHECK_INTERVAL_SECS` | `30` | Connection health check interval |
| `OPERATION_TIMEOUT_SECS` | `30` | Timeout for Iggy operations |

### Rate Limiting
| Variable | Default | Description |
|----------|---------|-------------|
| `RATE_LIMIT_RPS` | `100` | Requests per second (0 = disabled) |
| `RATE_LIMIT_BURST` | `50` | Burst capacity above RPS limit |

### Message Limits
| Variable | Default | Description |
|----------|---------|-------------|
| `BATCH_MAX_SIZE` | `1000` | Max messages per batch send |
| `POLL_MAX_COUNT` | `100` | Max messages per poll |
| `MAX_REQUEST_BODY_SIZE` | `10485760` | Max request body size in bytes (10MB) |

### Security
| Variable | Default | Description |
|----------|---------|-------------|
| `API_KEY` | (none) | API key for authentication (disabled if not set) |
| `AUTH_BYPASS_PATHS` | `/health,/ready` | Comma-separated paths that bypass auth |
| `CORS_ALLOWED_ORIGINS` | `*` | Comma-separated allowed origins |
| `TRUSTED_PROXIES` | (none) | Comma-separated CIDR ranges for trusted reverse proxies |

#### Trusted Proxy Configuration

The `TRUSTED_PROXIES` variable configures IP spoofing mitigation for the rate limiter.
When set, X-Forwarded-For headers are validated against trusted proxy networks.

**Format**: Comma-separated CIDR notation

**Common values**:
- Private networks: `10.0.0.0/8,172.16.0.0/12,192.168.0.0/16`
- Kubernetes pod network: `10.0.0.0/8`
- Docker bridge: `172.17.0.0/16`
- Localhost: `127.0.0.0/8`

**Example**:
```bash
# Trust all RFC 1918 private networks
TRUSTED_PROXIES="10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"

# Trust only specific proxy IPs
TRUSTED_PROXIES="10.0.1.5,10.0.1.6"
```

When empty (default), all X-Forwarded-For headers are trusted. **This is not recommended for production.**

### Observability
| Variable | Default | Description |
|----------|---------|-------------|
| `STATS_CACHE_TTL_SECS` | `5` | Stats cache refresh interval |

#### Log Levels

Background task logs use tiered log levels to reduce noise:

| Level | What's Logged |
|-------|---------------|
| `info` | Startup, shutdown, significant events |
| `debug` | Task lifecycle (cancellation, shutdown) |
| `trace` | Routine success ("Stats cache refreshed", "Health check OK") |

**Recommended settings:**
```bash
# Production (quiet)
RUST_LOG=info

# Development (see task events)
RUST_LOG=debug

# Debugging background tasks
RUST_LOG=trace
```

## Observability Stack

The project includes a complete Grafana-based observability stack for monitoring Iggy and the sample application.

### Components

| Service | Port | Description |
|---------|------|-------------|
| **Iggy** | 3000 | Message streaming server (also serves `/metrics`) |
| **Iggy Web UI** | 3050 | Dashboard for streams, topics, messages, and users |
| **Prometheus** | 9090 | Metrics collection and storage |
| **Grafana** | 3001 | Visualization and dashboards |

### Quick Start with Observability

```bash
# Start the full stack (Iggy + Prometheus + Grafana)
docker-compose up -d

# Access the services:
# - Iggy HTTP API: http://localhost:3000
# - Iggy Web UI: http://localhost:3050 (iggy/iggy)
# - Prometheus: http://localhost:9090
# - Grafana: http://localhost:3001 (admin/admin)
```

### Iggy Web UI

The Iggy Web UI provides a comprehensive dashboard for managing the Iggy server:

- **Streams & Topics**: Create, browse, and delete streams and topics
- **Messages**: Browse and inspect messages in topics
- **Users**: Manage users and permissions
- **Server Health**: Monitor server status and connections

Access at http://localhost:3050 with credentials `iggy/iggy`.

### Grafana Dashboards

Pre-configured dashboards are automatically provisioned:

- **Iggy Overview**: Server status, request rates, message throughput, latency percentiles

### Prometheus Metrics

Iggy exposes Prometheus-compatible metrics at `/metrics`:

```bash
# View raw metrics
curl http://localhost:3000/metrics
```

### Customizing Dashboards

1. Log into Grafana at http://localhost:3001
2. Navigate to Dashboards → Iggy folder
3. Edit existing dashboards or create new ones
4. Export JSON and save to `observability/grafana/provisioning/dashboards/`

### Adding OpenTelemetry (Optional)

Iggy supports OpenTelemetry for distributed tracing. Add to docker-compose:

```yaml
environment:
  - IGGY_TELEMETRY_ENABLED=true
  - IGGY_TELEMETRY_SERVICE_NAME=iggy
  - IGGY_TELEMETRY_LOGS_TRANSPORT=grpc
  - IGGY_TELEMETRY_LOGS_ENDPOINT=http://otel-collector:4317
  - IGGY_TELEMETRY_TRACES_TRANSPORT=grpc
  - IGGY_TELEMETRY_TRACES_ENDPOINT=http://otel-collector:4317
```

---

## Development

### Prerequisites
- Rust 1.90+ (edition 2024, MSRV: 1.90.0)
- Docker & Docker Compose

### Quick Start

1. Start Iggy server (with observability):
   ```bash
   docker-compose up -d iggy prometheus grafana
   ```

2. Run the application:
   ```bash
   cargo run
   ```

3. Test the API:
   ```bash
   # Health check
   curl http://localhost:3000/health

   # Send a message
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
             "email": "test@example.com",
             "name": "Test User"
           }
         }
       }
     }'

   # Poll messages (partition_id is 0-indexed, 0 = first partition)
   curl "http://localhost:3000/messages?partition_id=0&count=10"
   ```

### Running Tests

```bash
# Unit tests (93 tests)
cargo test --lib

# Integration tests (24 tests, requires Docker for testcontainers)
cargo test --test integration_tests

# Model tests (20 tests)
cargo test --test model_tests

# All tests
cargo test

# Security-specific tests
cargo test --test integration_tests -- test_auth
cargo test --test integration_tests -- test_rate_limit
```

### Fuzz Testing

Fuzz tests are available in the `fuzz/` directory for validation functions:

```bash
# Install cargo-fuzz (requires nightly)
cargo +nightly install cargo-fuzz

# Run the validation fuzz target
cargo +nightly fuzz run fuzz_validation

# Run with a time limit (e.g., 60 seconds)
cargo +nightly fuzz run fuzz_validation -- -max_total_time=60

# View coverage
cargo +nightly fuzz coverage fuzz_validation
```

The fuzz tests verify that validation functions never panic on any input.

### Building for Production

```bash
# Build release binary
cargo build --release

# Or use Docker
docker-compose up -d
```

## Message Format

Events follow a structured format:

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

1. **User Events**: Created, Updated, Deleted, LoggedIn
2. **Order Events**: Created, Updated, Cancelled, Shipped
3. **Generic Events**: Any JSON payload

## Error Handling

All errors return structured JSON responses:

```json
{
  "error": "error_type",
  "message": "Human-readable message",
  "details": "Optional additional context"
}
```

Error types and HTTP status codes:
- `connection_failed` (503): Initial connection to Iggy server failed
- `disconnected` (503): Lost connection during operation
- `connection_reset` (503): Connection was reset by peer
- `stream_error` (500): Stream operation failed
- `topic_error` (500): Topic operation failed
- `send_error` (500): Message send failed
- `poll_error` (500): Message poll failed
- `not_found` (404): Resource not found
- `bad_request` (400): Invalid request data

### Configuration Errors

`RateLimitError` is returned during router construction if rate limiting configuration is invalid:

```rust
pub enum RateLimitError {
    ZeroRps,  // RPS cannot be 0; use disabled() instead
}
```

The `build_router()` function returns `Result<Router, RateLimitError>` to propagate configuration errors at startup rather than panicking.

### Connection Error Detection

Connection-related errors use explicit enum variants for reliable reconnection logic:

```rust
// Pattern matching for connection errors (no string matching)
fn is_connection_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::ConnectionFailed(_)
            | AppError::Disconnected(_)
            | AppError::ConnectionReset(_)
    )
}
```

## Performance Considerations

- Use batch endpoints for high-throughput scenarios
- Configure appropriate partition count for parallelism
- Use partition keys for ordered processing within a partition
- Enable auto-commit for at-least-once delivery

## Partition Indexing

Iggy uses **0-indexed partitions**:
- A topic with `partitions: 3` has partitions `0`, `1`, and `2`
- When polling, `partition_id=0` refers to the first partition
- The poll query defaults to `partition_id=0` when not specified

## Dependencies

Key dependencies (see `Cargo.toml`):
- `iggy 0.8.0-edge.6`: Iggy Rust SDK (edge version for latest server features)
- `axum 0.8`: Web framework
- `tokio 1.48`: Async runtime
- `tokio-util 0.7`: Task tracking and cancellation tokens
- `serde 1.0`: Serialization
- `tracing 0.1`: Structured logging
- `thiserror 2.0`: Error handling
- `governor 0.8`: Rate limiting with token bucket algorithm
- `subtle 2.6`: Constant-time comparison for security
- `tower-http 0.6`: HTTP middleware (CORS, tracing, request ID)
- `rust_decimal 1.37`: Exact decimal arithmetic for monetary values
- `testcontainers 0.24`: Integration testing with containerized Iggy

## Structured Concurrency

The application uses structured concurrency patterns for proper task lifecycle management.

### Background Task Management

Background tasks (stats refresh, health checks) are managed using:
- `tokio_util::task::TaskTracker`: Tracks all spawned tasks
- `tokio_util::sync::CancellationToken`: Signals tasks to stop

```rust
// In src/state.rs
pub struct AppState {
    // ...
    task_tracker: TaskTracker,
    cancellation_token: CancellationToken,
}

impl AppState {
    pub async fn shutdown(&self) {
        self.cancellation_token.cancel();  // Signal all tasks
        self.task_tracker.close();          // Prevent new spawns
        self.task_tracker.wait().await;     // Wait for completion
    }
}
```

### Graceful Shutdown Flow

```
1. SIGTERM/SIGINT received
2. HTTP server stops accepting connections
3. In-flight requests complete
4. state.shutdown() called
5. CancellationToken signals all background tasks
6. TaskTracker waits for tasks to complete
7. Process exits cleanly
```

### Reconnection Coordination

Uses `tokio::sync::Notify` instead of busy-wait for efficient reconnection:

```rust
// In src/iggy_client.rs - ConnectionState
struct ConnectionState {
    reconnect_complete: Notify,  // Efficient wake-up
    // ...
}

// Waiting tasks sleep efficiently:
async fn wait_for_reconnection(&self) {
    if self.is_reconnecting() {
        self.reconnect_complete.notified().await;
    }
}
```

### PollParams Builder

The `PollParams` struct provides a cleaner API for message polling:

```rust
use iggy_sample::PollParams;

// Builder pattern for poll parameters
let params = PollParams::new(1, 1)  // partition_id, consumer_id
    .with_count(50)
    .with_offset(100)
    .with_auto_commit(true);

// Use with the cleaner API
let messages = client.poll_with_params("stream", "topic", params).await?;
```

## Middleware Stack

Request flow (applied in order):
```
Request → Rate Limit → Auth → Request ID → Tracing → CORS → Handler
```

### Client IP Extraction (`src/middleware/ip.rs`)
- Shared IP extraction logic used by both rate limiting and authentication
- Header priority: `X-Forwarded-For` (first IP) → `X-Real-IP` → "unknown" fallback
- Uses `Cow<'static, str>` for zero-allocation on the "unknown" fallback path
- `#[inline]` hints on hot paths for potential inlining
- See module-level docs for security warnings about IP spoofing

### Rate Limiting (`src/middleware/rate_limit.rs`)
- Token bucket algorithm via Governor crate (GCRA)
- Configurable RPS and burst capacity
- Per-IP rate limiting via shared `extract_client_ip_with_validation()` function
- Returns `429 Too Many Requests` with `Retry-After` header
- Fallible construction: `RateLimitLayer::new()` returns `Result<Self, RateLimitError>`

### API Key Authentication (`src/middleware/auth.rs`)
- Constant-time comparison to prevent timing attacks
- Per-IP brute force protection via shared `extract_client_ip()` function
- Accepts key via `X-API-Key` header or `api_key` query parameter
- Bypasses `/health` and `/ready` for health checks (exact path matching)

## Deployment Security

### Reverse Proxy Configuration (Required)

Both rate limiting and authentication brute force protection use client IP addresses
extracted from `X-Forwarded-For` or `X-Real-IP` headers. **These headers can be
spoofed by clients if the service is directly accessible.**

#### Required Configuration

1. **Deploy behind a trusted reverse proxy** (nginx, HAProxy, cloud LB, Kubernetes ingress)
2. **Block direct access** to this service from the internet
3. **Configure proxy to overwrite (not append)** client IP headers:

```nginx
# nginx example - overwrites any client-provided header
proxy_set_header X-Real-IP $remote_addr;
proxy_set_header X-Forwarded-For $remote_addr;
```

For multi-hop scenarios:
```nginx
# Trust only your proxy network
set_real_ip_from 10.0.0.0/8;
real_ip_header X-Forwarded-For;
real_ip_recursive off;
```

#### Security Risks Without Proper Proxy Configuration

Without proper proxy configuration, attackers can:
- **Bypass rate limiting** by rotating spoofed IP addresses
- **Bypass brute force protection** by rotating IPs during attacks
- **Frame innocent IPs** for abuse or lockout
- **Exhaust quotas for legitimate users** (DoS attack)

#### Kubernetes/Docker Deployment

When running in containers:
- Use an Ingress controller (nginx-ingress, Traefik, etc.)
- Configure the ingress to set `X-Real-IP` from the client connection
- Ensure the service is only accessible via the ingress (ClusterIP, not NodePort/LoadBalancer)

### Request ID (`src/middleware/request_id.rs`)
- Generates UUIDv4 for requests without `X-Request-Id`
- Propagates existing IDs for distributed tracing
- Adds ID to response headers

## CI/CD

GitHub Actions workflows provide automated quality assurance:

### Main CI (`ci.yml`)
- **Formatting**: `cargo fmt --check`
- **Linting**: `cargo clippy -- -D warnings`
- **Tests**: Matrix across Ubuntu/macOS/Windows × stable/beta/MSRV
- **Integration tests**: With Docker for testcontainers
- **Coverage**: Uploaded to Codecov
- **Documentation**: Build with `-D warnings`
- **Security audit**: `cargo-audit` for vulnerabilities
- **License check**: `cargo-deny` for license compliance
- **Scheduled runs**: Weekly Monday 2:00 AM UTC to catch dependency issues

### PR Checks (`pr.yml`)
- PR size limits (warning >500 lines, error >1000)
- Conventional commit format enforcement
- Semver breaking change detection
- Code complexity analysis
- Coverage diff reporting

### Release (`release.yml`)
Triggered on version tags (`v*.*.*`):
- Multi-platform builds (Linux x86/ARM, macOS x86/ARM, Windows)
- GitHub Release with changelog
- Documentation deployment to GitHub Pages
- Optional crates.io publishing

### Extended Tests (`extended-tests.yml`)
Weekly scheduled runs:
- Performance benchmarks
- Stress tests with real Iggy server
- Valgrind memory leak detection
- Miri undefined behavior detection
- Feature matrix testing
- Documentation coverage

### Dependabot
- Weekly Cargo dependency updates
- Weekly GitHub Actions updates
- Grouped minor/patch updates

## License

MIT License
