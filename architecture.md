# Architecture

This document describes the high-level architecture of the Iggy Sample Application.

## Overview

The application is an HTTP gateway in front of Apache Iggy message streaming
(server 0.10, Rust SDK 0.12). It demonstrates how to build a resilient,
observable and secure Rust service on the SDK's trait-level `Client` API, on a
single Iggy node or a three-node cluster.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Client Requests                                │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                     Middleware Stack (outermost first)                      │
│  Rate Limit → Auth → Request ID → Timeout → Tracing → CORS → Body Limit     │
│    (429)    (401/429)  (header)  (deadline)  (log)          (413)           │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                               Axum Router                                   │
│   /health, /ready, /stats, /cluster        → health and cluster handlers    │
│   /messages, /messages/batch               → message handlers (default      │
│   /streams/{s}/topics/{t}/messages           topic, or a named one)         │
│   /streams, /streams/{name}                → stream handlers                │
│   /streams/{s}/topics, /streams/{s}/topics/{t} → topic handlers (options)   │
│   /sink/inbound (only with SINK_INBOUND_TOKEN) → HTTP sink receiver         │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                            Application State                                │
│  IggyClientWrapper          ProducerService /    SinkInbox (optional)       │
│  - seeds, reconnect         ConsumerService      Stats cache                │
│  - lanes (SDK 0.12)                              Background tasks:          │
│  - circuit breaker                               - stats refresh            │
│  - per-request deadline                          - health check (ping)      │
│                                                  - SDK connection events    │
└─────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ Apache Iggy 0.10: one node (TCP 8090 / QUIC 8080 / HTTP 3000), or the       │
│ three-node cluster demo (TCP only, docker-compose.cluster.yaml)             │
└─────────────────────────────────────────────────────────────────────────────┘
```

Prometheus metrics are served on a separate listener (`METRICS_PORT`, 9090 by
default). The connectors demo (`docker-compose.connectors.yaml`) adds the Iggy
connector runtime with an HTTP source and an HTTP sink that delivers back to
`/sink/inbound`.

## Directory Structure

```
src/
├── main.rs              # Entry point: logging, metrics exporter, startup, exit codes
├── lib.rs               # Library exports for testing
├── config.rs            # Environment-based configuration
├── error.rs             # Error types with HTTP status mapping
├── state.rs             # Shared state, background tasks, shutdown
├── routes.rs            # Router construction with the middleware stack
├── metrics.rs           # Prometheus metrics
├── validation.rs        # Input validation (names, IDs, counts)
├── utils.rs             # Shutdown-signal handling
│
├── iggy_client/         # Iggy SDK wrapper
│   ├── mod.rs           # IggyClientWrapper: every SDK call the gateway makes
│   ├── connection.rs    # Connection state and the reconnect wait (Notify)
│   ├── seeds.rs         # Comma-separated seeds, reconnection retry cap
│   ├── lanes.rs         # SDK 0.12 lane settling (settled, send_settled)
│   ├── resilience.rs    # run_resilient: breaker check, deadline, one retry
│   ├── circuit_breaker.rs # Closed / open / half-open breaker
│   ├── events.rs        # SDK diagnostic events → connection state
│   ├── helpers.rs       # classify_iggy_error, identifiers, jitter
│   ├── params.rs        # PollParams
│   └── scopeguard.rs    # Drop guards
│
├── middleware/          # Custom Tower middleware
│   ├── mod.rs           # Module exports
│   ├── rate_limit.rs    # Token bucket rate limiting (Governor)
│   ├── auth.rs          # API key authentication, brute-force guard
│   ├── ip.rs            # Client IP extraction (trusted proxies)
│   ├── request_id.rs    # Request ID generation/propagation
│   └── timeout.rs       # X-Request-Timeout parsing
│
├── models/              # Domain and API types
│   ├── mod.rs           # Module exports
│   ├── event.rs         # Domain events (User, Order, Generic)
│   ├── api.rs           # Request/response DTOs
│   ├── topic_options.rs # Per-topic options: validation and read-back
│   └── sink.rs          # GET /sink/inbound response types
│
├── services/            # Business logic
│   ├── mod.rs           # Module exports
│   ├── producer.rs      # Message publishing
│   ├── consumer.rs      # Message consumption
│   └── sink_inbox.rs    # HTTP sink receiver: token, batch parsing, retention
│
└── handlers/            # HTTP request handlers
    ├── mod.rs           # Module exports
    ├── health.rs        # Health, readiness, stats
    ├── cluster.rs       # GET /cluster
    ├── messages.rs      # Send/poll messages
    ├── streams.rs       # Stream CRUD
    ├── topics.rs        # Topic CRUD with options
    ├── sink.rs          # POST/GET /sink/inbound
    └── util.rs          # Shared handler helpers
```

## Key Components

### 1. Middleware Stack

Request processing flows through middleware in order:

1. **Rate Limiting** (`middleware/rate_limit.rs`)
   - Token bucket algorithm via Governor crate
   - Per client IP. With `TRUSTED_PROXIES` set, forwarded headers count only
     from a trusted peer, resolved with the rightmost-untrusted rule;
     otherwise the peer address is used (`middleware/ip.rs`, shared with auth)
   - Configurable RPS and burst capacity
   - Returns 429 with Retry-After header

2. **Authentication** (`middleware/auth.rs`)
   - API key in the `X-API-Key` header (the `api_key` query parameter still
     works, deprecated, with a warning)
   - Constant-time comparison (timing attack resistant)
   - Brute-force guard: failures are metered per IP. A verified IP that
     exhausts its budget is blocked (429, checked before the key); an
     unverified one is throttled but never blocked. `API_KEY` therefore
     requires `TRUSTED_PROXIES` unless `ALLOW_UNVERIFIED_CLIENT_IPS=true`
   - Configurable bypass paths (`/health`, `/ready`, exact match)

3. **Request ID** (`middleware/request_id.rs`)
   - Generates UUIDv4 for new requests
   - Propagates existing X-Request-Id header
   - Enables distributed tracing

4. **Request Timeout** (`middleware/timeout.rs`)
   - `X-Request-Timeout: <milliseconds>`, 100 ms to 5 minutes
   - Clamped to `OPERATION_TIMEOUT_SECS`: a client can shorten a request's
     deadline, never extend it
   - Handlers apply it through `AppState::{producer,consumer,iggy}_scoped`

5. **Tracing** (tower-http)
   - Logging of requests/responses
   - Integrates with tracing ecosystem

6. **CORS** (tower-http)
   - Configurable allowed origins
   - Supports wildcard or explicit origin list

7. **Body Limit** (`DefaultBodyLimit`)
   - `MAX_REQUEST_BODY_SIZE`, 10 MiB by default; larger bodies get 413

`POST /sink/inbound` adds two layers of its own, both before the body is
read: its bearer token (constant time), then the `application/x-ndjson`
content type (415).

### 2. SDK Integration

The gateway uses the SDK's trait-level `Client` API through
`IggyClientWrapper` (`iggy_client/mod.rs`), not `IggyProducer` or
`IggyConsumer`: it serves arbitrary streams, topics, partitions and offsets per
request, while those clients bind to one topic at build time.

- **Seeds.** `IGGY_CONNECTION_STRING` takes comma-separated seeds
  (`seeds.rs`). Startup tries each in order; reconnects start from the last
  one that worked. Once signed in, the SDK reads the roster and routes each
  partition call to its primary.
- **Reconnection retries are capped at 3** (`DEFAULT_RECONNECTION_RETRIES`,
  ADR-0003) unless the connection string sets them, so a full outage ends in
  `NotConnected` and an open breaker instead of 30 s waits.
- **Lanes.** SDK 0.12 admits one mutation at a time per lane and retains one
  whose outcome it cannot prove. Every lane-taking call goes through
  `lanes::settled` / `send_settled`, which abandon a retained operation and
  rerun a lane-refused call once. The gateway never resends an unknown
  outcome itself.
- **Per-topic options** (`models/topic_options.rs`): durability, segment,
  flush, retention and replication settings belong to each topic and are
  fixed at creation. `POST /streams/{s}/topics` takes them as `options`
  (unknown fields are refused with 422), topic reads report each option with
  its `source`, and `IGGY_TOPIC_<OPTION>` sets them for the default topic.

### 3. Connection Resilience

Four parts work together:

```
 request ──▶ run_resilient ──▶ circuit breaker open? ──yes──▶ 503 circuit_open
                │                     │ no
                │                     ▼
                │            attempt, bounded by the request deadline
                │                     │
                │      connection error (classified, never string-matched)
                │                     ▼
                │         record a breaker failure, reconnect (bounded),
                │         retry once ──▶ result or error
                ▼
   SDK: reconnects by itself (3 retries per seed by default)
   App: rebuilds the client from the current seed, then the next one;
        stops on an incompatible server (exit 76 at startup);
        gives up after MAX_RECONNECT_ATTEMPTS (0 = never)
   State: live ping (health check task) + SDK diagnostic events
```

- Backoff: exponential with jitter, base 1 s, max 30 s
  (`RECONNECT_BASE_DELAY_MS`, `RECONNECT_MAX_DELAY_MS`)
- Uses `tokio::sync::Notify` for efficient waiting (no busy-wait);
  concurrent requests wait for the reconnect to complete
- A timeout counts as a breaker failure only at the global deadline, not
  under a client-shortened one
- The SDK's `Disconnected` event starts a recorded outage at once and
  `Connected` triggers a ping that ends it; the ping stays the source of
  truth (`events.rs`)

### 4. Structured Concurrency

Background tasks use `TaskTracker` and `CancellationToken`
(`state.rs`, details in [docs/structured-concurrency.md](docs/structured-concurrency.md)):

```rust
// Startup (AppState::new)
state.spawn_stats_refresh_task();
state.spawn_health_check_task();
state.spawn_connection_events_task();

// Shutdown (AppState::shutdown), in this order
self.cancellation_token.cancel(); // Signal all tasks
self.task_tracker.close();        // Prevent new spawns
self.task_tracker.wait().await;   // Wait for completion
```

### 5. Error Handling

Errors use explicit enum variants (`error.rs`). SDK errors are mapped in
`helpers::classify_iggy_error`.

| Error Type | HTTP Status | Description |
|------------|-------------|-------------|
| `ConnectionFailed` | 503 | Initial connection failed |
| `Disconnected` | 503 | Lost connection during operation |
| `ConnectionReset` | 503 | Connection reset by peer |
| `CircuitOpen` | 503 | Circuit breaker open, failing fast |
| `NotAccepted` | 503 | Not admitted; safe to retry |
| `OutcomeUnknown` | 503 | The write may or may not have happened |
| `AlreadyApplied` | 202 | The server had already applied this request |
| `OperationTimeout` | 504 | Deadline passed |
| `IncompatibleServer` | 502 | Server speaks another protocol version |
| `StreamError` | 500 | Stream operation failed |
| `TopicError` | 500 | Topic operation failed |
| `SendError` | 500 | Message send failed |
| `PollError` | 500 | Message poll failed |
| `Internal` | 500 | Internal error |
| `ConfigError` | 500 | Configuration error |
| `NotFound` | 404 | Resource not found |
| `BadRequest` | 400 | Invalid request data, or topic options the server refused |
| `SerializationError` | 400 | Payload could not be (de)serialized |

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

`NotAccepted`, `OutcomeUnknown` and `AlreadyApplied` are deliberately not
connection errors: the SDK has already replayed those requests, so a reconnect
and retry here could resend a write the server applied.

### 6. Configuration

All configuration via environment variables (`src/config.rs` defines each one
and its default):

| Category | Variables |
|----------|-----------|
| Server | `HOST`, `PORT`, `RUST_LOG`, `METRICS_PORT` |
| Iggy | `IGGY_CONNECTION_STRING` (comma-separated seeds), `IGGY_STREAM`, `IGGY_TOPIC`, `IGGY_PARTITIONS`, `IGGY_TOPIC_<OPTION>` |
| Resilience | `MAX_RECONNECT_ATTEMPTS`, `RECONNECT_BASE_DELAY_MS`, `RECONNECT_MAX_DELAY_MS`, `OPERATION_TIMEOUT_SECS`, `HEALTH_CHECK_INTERVAL_SECS`, `CIRCUIT_BREAKER_FAILURE_THRESHOLD`, `CIRCUIT_BREAKER_SUCCESS_THRESHOLD`, `CIRCUIT_BREAKER_OPEN_DURATION_SECS` |
| Rate Limit | `RATE_LIMIT_RPS`, `RATE_LIMIT_BURST`, `TRUSTED_PROXIES` |
| Auth | `API_KEY`, `AUTH_BYPASS_PATHS`, `CORS_ALLOWED_ORIGINS`, `ALLOW_UNVERIFIED_CLIENT_IPS` (`API_KEY` requires `TRUSTED_PROXIES` unless this is `true`) |
| Limits | `BATCH_MAX_SIZE`, `POLL_MAX_COUNT`, `MAX_REQUEST_BODY_SIZE` |
| Caching | `STATS_CACHE_TTL_SECS` |
| Sink receiver | `SINK_INBOUND_TOKEN` or `SINK_INBOUND_TOKEN_FILE` |

## Data Flow

### Message Publishing

```
Client          Handler          Producer          IggyClientWrapper          Iggy
  │                │                 │                     │                   │
  │ POST /messages │                 │                     │                   │
  │───────────────▶│ validate event  │                     │                   │
  │                │────────────────▶│ timing, metrics     │                   │
  │                │                 │────────────────────▶│ serialize to JSON │
  │                │                 │                     │ run_resilient     │
  │                │                 │                     │  └ send_settled   │
  │                │                 │                     │──send_messages()─▶│
  │                │                 │                     │◀──────────────────│
  │                │                 │◀────────────────────│                   │
  │                │◀────────────────│                     │                   │
  │◀───────────────│ 201 Created     │                     │                   │
```

### Message Polling

```
Client          Handler          Consumer          IggyClientWrapper          Iggy
  │                │                 │                     │                   │
  │ GET /messages  │                 │                     │                   │
  │───────────────▶│ build PollParams│                     │                   │
  │                │────────────────▶│────────────────────▶│ auto_commit →     │
  │                │                 │                     │ CommitPolicy::    │
  │                │                 │                     │ OnRead; settled   │
  │                │                 │                     │──poll_messages()─▶│
  │                │                 │                     │◀──────────────────│
  │                │                 │◀────────────────────│                   │
  │                │                 │ deserialize events; │                   │
  │                │                 │ count non-events in │                   │
  │                │                 │ `skipped`           │                   │
  │                │◀────────────────│                     │                   │
  │◀───────────────│ 200 OK + events │                     │                   │
```

`auto_commit=true` stores the offset as part of the read: at most once, so a
lost HTTP response skips those messages.

## Security Architecture

### Defense in Depth

1. **Network Layer**: Deploy behind reverse proxy, block direct access
2. **Rate Limiting**: Per-IP token bucket prevents DoS
3. **Authentication**: API key with constant-time comparison
4. **Brute Force Protection**: verified client IPs are blocked after
   repeated failures; unverified ones are throttled, never blocked
5. **Input Validation**: Strict validation of all user inputs; a topic
   creation body and its `options` refuse unknown fields (422)
6. **CORS**: Configurable origin whitelist
7. **Sink receiver**: its own constant-time bearer token and content-type
   check, both before the body is read; at most 100 deliveries and 1 MiB kept

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

### Test Suites

| Suite | Where | What it covers |
|-------|-------|----------------|
| Unit tests | `src/` | Individual components |
| Model tests | `tests/model_tests.rs` | Serialization and validation |
| Integration tests | `tests/integration_tests.rs` | The full API against a real Iggy 0.10 container (testcontainers) |
| Metrics smoke test | `tests/metrics_smoke_test.rs` | The Prometheus exporter |
| Cluster tests | `tests/cluster_tests.rs` | Three-node failover; `#[ignore]`d, weekly |
| Connectors tests | `tests/connectors_tests.rs` | The connectors round trip; `#[ignore]`d, weekly |
| Fuzz tests | `fuzz/` | Validation functions never panic (cargo-fuzz) |

### Running Tests

```bash
# Unit tests
cargo test --lib

# Integration tests (requires Docker)
cargo test --test integration_tests

# Model tests
cargo test --test model_tests

# Cluster and connectors suites, with their compose stack up
cargo test --test cluster_tests -- --ignored --test-threads=1
cargo test --test connectors_tests -- --ignored --test-threads=1

# Fuzz tests
cargo +nightly fuzz run fuzz_validation
```

## Performance Considerations

1. **Batch Operations**: Use `/messages/batch` for high throughput
2. **Partitioning**: Configure partitions for parallelism
3. **Stats Caching**: Background refresh avoids expensive queries
4. **One shared client**: one SDK client serves every request; on a cluster
   it keeps a link to every node and sends each partition call to its
   primary
5. **Lanes queue per partition**: the SDK admits one mutation at a time per
   partition, so concurrent sends to one partition queue inside the SDK

## Observability

### Logging

Logging via `tracing`, in text format, filtered by `RUST_LOG`:

```bash
RUST_LOG=info,iggy_sample=debug cargo run
```

### Request Tracing

Each request gets a unique ID (X-Request-Id header) for correlation.

### Metrics

Prometheus metrics on `METRICS_PORT` (9090 by default; 0 disables it). The
exporter starts before the client connects, and a bind failure fails startup.

### Health Endpoints

- `GET /health` - Detailed health with Iggy connection status (live ping)
- `GET /ready` - Kubernetes readiness probe (200 if ready, 503 if not)
- `GET /stats` - Cached server totals (streams, topics, messages, bytes),
  uptime, cache age and staleness
- `GET /cluster` - Cluster nodes and roles (node `status` is not liveness on
  server 0.10; a topic read's `in_sync_members` is)

## Deployment

### Container

```bash
docker build -t iggy-sample .
docker run --network iggy_sample_iggy-network -p 8000:8000 \
  -e IGGY_CONNECTION_STRING=iggy://iggy:iggy@iggy:8090 \
  iggy-sample
```

The host in the connection string must be the address the server
advertises: after sign-in the SDK switches to it. The compose server
advertises `iggy`, its name on the compose network (`iggy_sample_iggy-network`
when the compose project is named after the checkout; `docker network ls` shows
it).

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
            port: 8000
        readinessProbe:
          httpGet:
            path: /ready
            port: 8000
```

## Exit Codes

The application uses BSD sysexits-compatible exit codes:

| Code | Name | Description |
|------|------|-------------|
| 0 | OK | Successful shutdown |
| 69 | UNAVAILABLE | Iggy connection, default stream/topic setup, port binding, metrics exporter |
| 70 | SOFTWARE | Internal software error |
| 76 | PROTOCOL | The Iggy server speaks an incompatible protocol version |
| 78 | CONFIG | Configuration error, including router setup (e.g. an invalid `TRUSTED_PROXIES` CIDR) |
