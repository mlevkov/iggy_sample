# Architecture

This document describes the high-level architecture of the Iggy Sample Application.

## Overview

The application is a production-ready HTTP API service that provides a REST interface
to Apache Iggy message streaming. It demonstrates best practices for building
resilient, observable, and secure Rust services.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Client Requests                                │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Middleware Stack                                  │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────┐  ┌─────────┐  ┌──────┐  │
│  │ Rate Limit  │→ │     Auth     │→ │ Request ID │→ │ Tracing │→ │ CORS │  │
│  │   (429)     │  │    (401)     │  │  (header)  │  │  (log)  │  │      │  │
│  └─────────────┘  └──────────────┘  └────────────┘  └─────────┘  └──────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Axum Router                                    │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │ Routes:                                                                │ │
│  │   /health, /ready, /stats     → Health Handlers                        │ │
│  │   /messages, /messages/batch  → Message Handlers                       │ │
│  │   /streams, /streams/{name}   → Stream Handlers                        │ │
│  │   /streams/{s}/topics/{t}     → Topic Handlers                         │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Application State                              │
│  ┌─────────────────┐  ┌─────────────────┐  ┌────────────────────────────┐  │
│  │  IggyClient     │  │  Stats Cache    │  │  Background Tasks          │  │
│  │  (with auto-    │  │  (Arc<RwLock>)  │  │  - Stats refresh           │  │
│  │   reconnect)    │  │                 │  │  - Health check            │  │
│  └─────────────────┘  └─────────────────┘  └────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                          Apache Iggy Server                                 │
│                     (TCP:8090 / QUIC:8080 / HTTP:3000)                      │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Directory Structure

```
src/
├── main.rs              # Entry point, logging init, graceful shutdown
├── lib.rs               # Library exports for testing
├── config.rs            # Environment-based configuration
├── error.rs             # Error types with HTTP status mapping
├── state.rs             # Shared application state, background tasks
├── routes.rs            # Router construction with middleware stack
├── validation.rs        # Input validation (names, IDs, counts)
│
├── iggy_client/         # Iggy SDK wrapper
│   ├── mod.rs           # Module exports
│   ├── wrapper.rs       # High-level client wrapper
│   ├── connection.rs    # Connection state and reconnection logic
│   └── helpers.rs       # Polling parameters builder
│
├── middleware/          # Custom Tower middleware
│   ├── mod.rs           # Module exports
│   ├── rate_limit.rs    # Token bucket rate limiting (Governor)
│   ├── auth.rs          # API key authentication
│   └── request_id.rs    # Request ID generation/propagation
│
├── models/              # Domain and API types
│   ├── mod.rs           # Module exports
│   ├── event.rs         # Domain events (User, Order, Generic)
│   └── api.rs           # Request/response DTOs
│
├── services/            # Business logic
│   ├── mod.rs           # Module exports
│   ├── producer.rs      # Message publishing
│   └── consumer.rs      # Message consumption
│
├── handlers/            # HTTP request handlers
│   ├── mod.rs           # Module exports
│   ├── health.rs        # Health, readiness, stats
│   ├── messages.rs      # Send/poll messages
│   ├── streams.rs       # Stream CRUD
│   └── topics.rs        # Topic CRUD
│
└── utils/               # Shared utilities
    ├── mod.rs           # Module exports
    └── shutdown.rs      # Signal handling
```

## Key Components

### 1. Middleware Stack

Request processing flows through middleware in order:

1. **Rate Limiting** (`middleware/rate_limit.rs`)
   - Token bucket algorithm via Governor crate
   - Per-IP limiting using X-Forwarded-For/X-Real-IP
   - Configurable RPS and burst capacity
   - Returns 429 with Retry-After header

2. **Authentication** (`middleware/auth.rs`)
   - API key validation (X-API-Key header or query param)
   - Constant-time comparison (timing attack resistant)
   - Per-IP brute force protection
   - Configurable bypass paths (/health, /ready)

3. **Request ID** (`middleware/request_id.rs`)
   - Generates UUIDv4 for new requests
   - Propagates existing X-Request-Id header
   - Enables distributed tracing

4. **Tracing** (tower-http)
   - Structured logging of requests/responses
   - Integrates with tracing ecosystem

5. **CORS** (tower-http)
   - Configurable allowed origins
   - Supports wildcard or explicit origin list

### 2. Connection Resilience

The `IggyClientWrapper` provides automatic reconnection:

```
┌─────────────────────────────────────────────────────────────┐
│                    Connection State Machine                  │
│                                                             │
│   ┌──────────┐    error    ┌──────────────┐                │
│   │Connected │────────────▶│ Reconnecting │                │
│   └──────────┘             └──────────────┘                │
│        ▲                          │                         │
│        │      success             │                         │
│        └──────────────────────────┘                         │
│                                                             │
│   Backoff: exponential with jitter                         │
│   Base: 1s, Max: 30s, Attempts: configurable (0=infinite)  │
└─────────────────────────────────────────────────────────────┘
```

- Uses `tokio::sync::Notify` for efficient waiting (no busy-wait)
- Concurrent requests wait for reconnection to complete
- Background health check detects issues proactively

### 3. Structured Concurrency

Background tasks use `TaskTracker` and `CancellationToken`:

```rust
// Startup
let tracker = TaskTracker::new();
let token = CancellationToken::new();

tracker.spawn(stats_refresh_task(token.clone()));
tracker.spawn(health_check_task(token.clone()));

// Shutdown
token.cancel();      // Signal all tasks
tracker.close();     // Prevent new spawns
tracker.wait().await; // Wait for completion
```

### 4. Error Handling

Errors are categorized by HTTP status and use explicit enum variants:

| Error Type | HTTP Status | Description |
|------------|-------------|-------------|
| `ConnectionFailed` | 503 | Initial connection failed |
| `Disconnected` | 503 | Lost connection during operation |
| `ConnectionReset` | 503 | Connection reset by peer |
| `StreamError` | 500 | Stream operation failed |
| `TopicError` | 500 | Topic operation failed |
| `SendError` | 500 | Message send failed |
| `PollError` | 500 | Message poll failed |
| `NotFound` | 404 | Resource not found |
| `BadRequest` | 400 | Invalid request data |

Connection errors use pattern matching (no string parsing):

```rust
fn is_connection_error(error: &AppError) -> bool {
    matches!(error,
        AppError::ConnectionFailed(_)
        | AppError::Disconnected(_)
        | AppError::ConnectionReset(_)
    )
}
```

### 5. Configuration

All configuration via environment variables:

| Category | Variables |
|----------|-----------|
| Server | `HOST`, `PORT`, `RUST_LOG` |
| Iggy | `IGGY_CONNECTION_STRING`, `IGGY_STREAM`, `IGGY_TOPIC`, `IGGY_PARTITIONS` |
| Resilience | `MAX_RECONNECT_ATTEMPTS`, `RECONNECT_BASE_DELAY_MS`, `RECONNECT_MAX_DELAY_MS` |
| Rate Limit | `RATE_LIMIT_RPS`, `RATE_LIMIT_BURST`, `TRUSTED_PROXIES` |
| Auth | `API_KEY`, `AUTH_BYPASS_PATHS`, `CORS_ALLOWED_ORIGINS` |
| Limits | `BATCH_MAX_SIZE`, `POLL_MAX_COUNT`, `MAX_REQUEST_BODY_SIZE` |

## Data Flow

### Message Publishing

```
Client                Handler              Producer             IggyClient          Iggy
  │                      │                    │                     │                 │
  │ POST /messages       │                    │                     │                 │
  │─────────────────────▶│                    │                     │                 │
  │                      │ validate event     │                     │                 │
  │                      │───────────────────▶│                     │                 │
  │                      │                    │ serialize to JSON   │                 │
  │                      │                    │────────────────────▶│                 │
  │                      │                    │                     │ send_messages() │
  │                      │                    │                     │────────────────▶│
  │                      │                    │                     │◀────────────────│
  │                      │                    │◀────────────────────│                 │
  │                      │◀───────────────────│                     │                 │
  │◀─────────────────────│ 201 Created        │                     │                 │
```

### Message Polling

```
Client                Handler              Consumer             IggyClient          Iggy
  │                      │                    │                     │                 │
  │ GET /messages        │                    │                     │                 │
  │─────────────────────▶│                    │                     │                 │
  │                      │ build poll params  │                     │                 │
  │                      │───────────────────▶│                     │                 │
  │                      │                    │ poll_with_params()  │                 │
  │                      │                    │────────────────────▶│                 │
  │                      │                    │                     │ poll_messages() │
  │                      │                    │                     │────────────────▶│
  │                      │                    │                     │◀────────────────│
  │                      │                    │ deserialize events  │                 │
  │                      │                    │◀────────────────────│                 │
  │                      │◀───────────────────│                     │                 │
  │◀─────────────────────│ 200 OK + events    │                     │                 │
```

## Security Architecture

### Defense in Depth

1. **Network Layer**: Deploy behind reverse proxy, block direct access
2. **Rate Limiting**: Per-IP token bucket prevents DoS
3. **Authentication**: API key with constant-time comparison
4. **Brute Force Protection**: Per-IP failure tracking with lockout
5. **Input Validation**: Strict validation of all user inputs
6. **CORS**: Configurable origin whitelist

### Trusted Proxy Configuration

When behind a reverse proxy:

```nginx
# nginx configuration
proxy_set_header X-Real-IP $remote_addr;
proxy_set_header X-Forwarded-For $remote_addr;

# Multi-hop
set_real_ip_from 10.0.0.0/8;
real_ip_header X-Forwarded-For;
real_ip_recursive off;
```

Set `TRUSTED_PROXIES` to validate proxy sources:

```bash
TRUSTED_PROXIES="10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"
```

## Testing Strategy

### Test Pyramid

```
         ┌──────────────┐
         │  Integration │  24 tests - Full API with real Iggy
         │    Tests     │  (testcontainers)
         └──────────────┘
        ┌────────────────┐
        │   Unit Tests   │  93 tests - Individual components
        │                │  (mock dependencies)
        └────────────────┘
       ┌──────────────────┐
       │   Model Tests    │  20 tests - Serialization/validation
       │                  │
       └──────────────────┘
      ┌────────────────────┐
      │    Fuzz Tests      │  Validation functions
      │                    │  (cargo-fuzz)
      └────────────────────┘
```

### Running Tests

```bash
# Unit tests
cargo test --lib

# Integration tests (requires Docker)
cargo test --test integration_tests

# Model tests
cargo test --test model_tests

# Fuzz tests
cargo +nightly fuzz run fuzz_validation
```

## Performance Considerations

1. **Batch Operations**: Use `/messages/batch` for high throughput
2. **Partitioning**: Configure partitions for parallelism
3. **Stats Caching**: Background refresh avoids expensive queries
4. **Connection Pooling**: Persistent TCP connections to Iggy
5. **Zero-Copy Where Possible**: Bytes passed through without copying

## Observability

### Logging

Structured JSON logging via `tracing`:

```bash
RUST_LOG=info,iggy_sample=debug cargo run
```

### Request Tracing

Each request gets a unique ID (X-Request-Id header) for correlation.

### Health Endpoints

- `GET /health` - Detailed health with Iggy connection status
- `GET /ready` - Kubernetes readiness probe (200 if ready, 503 if not)
- `GET /stats` - Service statistics (messages sent/received, uptime)

## Deployment

### Container

```bash
docker build -t iggy-sample .
docker run -p 3000:3000 \
  -e IGGY_CONNECTION_STRING=iggy://iggy:iggy@iggy-server:8090 \
  iggy-sample
```

### Kubernetes

Deploy with an Ingress controller for proper IP detection:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: iggy-sample
spec:
  template:
    spec:
      containers:
      - name: iggy-sample
        env:
        - name: TRUSTED_PROXIES
          value: "10.0.0.0/8"
        livenessProbe:
          httpGet:
            path: /health
            port: 3000
        readinessProbe:
          httpGet:
            path: /ready
            port: 3000
```

## Exit Codes

The application uses BSD sysexits-compatible exit codes:

| Code | Name | Description |
|------|------|-------------|
| 0 | OK | Successful shutdown |
| 69 | UNAVAILABLE | Service unavailable (Iggy connection, port binding) |
| 70 | SOFTWARE | Internal software error |
| 78 | CONFIG | Configuration error |
