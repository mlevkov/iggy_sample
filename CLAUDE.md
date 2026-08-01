# Iggy Sample Application

A comprehensive demonstration of Apache Iggy message streaming with Axum.

## Project Overview

This application showcases how to build a production-ready message streaming service using:
- **Apache Iggy server 0.8.0**: High-performance message streaming with io_uring shared-nothing architecture
- **Iggy Rust SDK 0.10.0**: Latest stable SDK, paired with the server 0.8 release line (server image pinned in `docker-compose.yaml`)
- **Axum 0.8**: Ergonomic and modular Rust web framework
- **Tokio**: Async runtime for Rust

## Documentation

See the [docs/](docs/) directory for comprehensive guides covering event-driven architecture, partitioning strategies, durable storage configuration, and more.

## Configuration

Every environment variable and its default is defined in `src/config.rs`; see
also `.env.example`. Only the entries below carry semantics the code does not
convey on its own.

`RATE_LIMIT_BURST` sets the instantaneous token-bucket capacity and
**replaces** the default rather than adding to it.

### Security
| Variable | Default | Description |
|----------|---------|-------------|
| `API_KEY` | (none) | API key for authentication (disabled if not set) |
| `AUTH_BYPASS_PATHS` | `/health,/ready` | Comma-separated paths that bypass auth |
| `CORS_ALLOWED_ORIGINS` | `*` | Comma-separated allowed origins |
| `TRUSTED_PROXIES` | (none) | Comma-separated CIDR ranges for trusted reverse proxies |

#### Trusted Proxy Configuration

The `TRUSTED_PROXIES` variable configures IP spoofing mitigation for both the
rate limiter and the auth brute-force limiter. When set, forwarded headers
(`X-Forwarded-For`/`X-Real-IP`) are only honored if the direct peer address is
inside a trusted range; requests from untrusted peers are keyed by their actual
peer address. Honored `X-Forwarded-For` chains are resolved with the
**rightmost-untrusted** rule (walk from the right, skip trusted-range hops,
take the first untrusted address), so the guarantee holds for proxies that
append to a client-supplied header — the common default — as well as those
that overwrite it. Unparseable forwarded values fall back to the peer address.
Invalid entries fail startup (`RateLimitError::InvalidTrustedProxyCidr`)
instead of silently degrading to trust-all.

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

### Log Levels

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

## Development

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

## Error Handling

- `build_router()` returns `Result<Router, RateLimitError>` so invalid
  rate-limiting configuration (zero RPS, unparseable `TRUSTED_PROXIES` CIDR)
  fails at startup instead of panicking.
- Connection errors are detected by matching explicit `AppError` enum variants
  (`ConnectionFailed`/`Disconnected`/`ConnectionReset`), never by string
  matching on error text — the reconnection logic depends on this.

## Partition Indexing

Iggy uses **0-indexed partitions**:
- A topic with `partitions: 3` has partitions `0`, `1`, and `2`
- When polling, `partition_id=0` refers to the first partition
- The poll query defaults to `partition_id=0` when not specified

## Iggy SDK Integration

This service integrates with the SDK at the `Client` trait level (via
`IggyClientWrapper`) rather than through the higher-level `IggyProducer`/
`IggyConsumer` clients. This is deliberate: the HTTP gateway serves
*arbitrary* stream/topic routes with per-request partition, offset, and
consumer parameters. `IggyProducer` binds to a single stream/topic at build
time and batches in the background, and `IggyConsumer` is a long-lived
subscription iterator — neither maps onto stateless request/response
semantics. The high-level clients are the right choice for dedicated
pipeline workers; a protocol gateway belongs on the trait API.

Resilience is layered: the SDK's connection-string clients ship with
transport-level auto-reconnection (default on, unlimited retries), which
swallows most mid-operation connection failures into blocking retries. The
wrapper therefore treats **timeouts** as circuit-breaker failures, classifies
the SDK error variants that do escape (`classify_iggy_error`), and runs live
`ping` health probes so `/health` and `/ready` reflect reality during an
outage rather than a latched startup flag.

## Structured Concurrency

Background tasks (stats refresh, health checks) are owned by a
`tokio_util::task::TaskTracker` + `CancellationToken` pair held on `AppState`.
`AppState::shutdown()` cancels, then closes, then awaits the tracker — in that
order — so shutdown drains in-flight work instead of dropping it.

Reconnection waits on `tokio::sync::Notify`
(`ConnectionState::reconnect_complete` in `src/iggy_client/connection.rs`)
rather than polling, so tasks blocked on a reconnect sleep instead of spinning.

See [docs/structured-concurrency.md](docs/structured-concurrency.md) for the
full task-lifecycle and shutdown-ordering details.

## Middleware Stack

Request flow (applied in order):
```
Request → Rate Limit → Auth → Request ID → Timeout → Tracing → CORS → Handler
```

- Client IP extraction (`src/middleware/ip.rs`) is shared by rate limiting and
  auth. With `TRUSTED_PROXIES` set it applies peer-address gating plus
  rightmost-untrusted resolution; without it the header priority is
  `X-Forwarded-For` → `X-Real-IP` → `"unknown"`.
- `RateLimitLayer::new()` is fallible — it returns `Result<Self, RateLimitError>`
  so bad configuration surfaces at startup rather than being silently ignored.
- Auth meters authentication **failures** only: a valid-key request never
  consumes from the per-IP brute-force budget.
- Auth bypass for `/health` and `/ready` uses exact path matching.

### Request Timeout (`src/middleware/timeout.rs`)
- Clients can specify `X-Request-Timeout: <milliseconds>` header
- Bounded: 100ms minimum, 5 minutes maximum (header parse acceptance)
- Enforced end-to-end: all Iggy-touching handlers scope their client via
  `AppState::{producer,consumer,iggy}_scoped` →
  `IggyClientWrapper::with_timeout`, bounding every operation attempt by
  the request deadline (worst case ~3x the deadline on the reconnect
  path: first attempt + bounded reconnect wait + single retry)
- Timeouts under a client-shortened deadline are NOT circuit-breaker
  failures (a client's short deadline expiring is not outage evidence)
- The effective deadline is clamped to the global `OPERATION_TIMEOUT_SECS`
  — clients may shorten a request's bound, never extend it
- Requests without the header use the global timeout unchanged

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
