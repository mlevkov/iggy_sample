# Iggy Sample Application

A comprehensive demonstration of Apache Iggy message streaming with Axum.

## Project Overview

This application showcases how to build a production-ready message streaming service using:
- **Apache Iggy server 0.10.0**: High-performance message streaming with io_uring shared-nothing architecture and optional VSR clustering
- **Iggy Rust SDK 0.12.0**: Paired with the server 0.10 release line (server image pinned in `docker-compose.yaml` and the integration fixture). SDK and server always move together, and 0.10 refuses to boot on 0.9 data directories, so the compose data volume is versioned. The SDK needs Rust 1.95, the declared MSRV (`rust-version = "1.95.0"`); cargo's MSRV-aware resolver keeps Dependabot from proposing any SDK that needs a newer Rust, so those upgrades raise the MSRV by hand. Plan and constraints: [ADR-0001](docs/adr/0001-iggy-0.9-upgrade-and-demos.md), amended by [ADR-0002](docs/adr/0002-iggy-0.10-before-topic-options.md)
- **Axum 0.8**: Ergonomic and modular Rust web framework
- **Tokio**: Async runtime for Rust

## Documentation

See the [docs/](docs/) directory for comprehensive guides covering event-driven architecture, partitioning strategies, durable storage configuration, and more.

Significant design decisions are recorded as ADRs in [docs/adr/](docs/adr/).
The Iggy upgrades and the clustering and connector demos follow
[ADR-0001](docs/adr/0001-iggy-0.9-upgrade-and-demos.md), which fixes their
PR sequence and scope, as amended by
[ADR-0002](docs/adr/0002-iggy-0.10-before-topic-options.md) (move to 0.10
before the per-topic options PR).
[ADR-0003](docs/adr/0003-cap-sdk-reconnection-retries.md) caps the SDK's
reconnection retries by default.

What was in flight at the end of the last session, and the findings not to
re-derive, are in [docs/SESSION-STATE.md](docs/SESSION-STATE.md) (newest
block first).

## Configuration

Every environment variable and its default is defined in `src/config.rs`; see
also `.env.example`. Only the entries below carry semantics the code does not
convey on its own.

`RATE_LIMIT_BURST` sets the instantaneous token-bucket capacity and
**replaces** the default rather than adding to it.

`IGGY_TOPIC_<OPTION>` (e.g. `IGGY_TOPIC_DURABILITY=persisted`) sets one
per-topic option of the default topic, with the names and values of the
`options` object on `POST /streams/{s}/topics`. Topic options are fixed at
creation, so these apply only when the gateway creates the topic; for an
existing topic `ensure_topic` logs each differing option as a warning and
changes nothing.

### Security
| Variable | Default | Description |
|----------|---------|-------------|
| `API_KEY` | (none) | API key for authentication (disabled if not set) |
| `AUTH_BYPASS_PATHS` | `/health,/ready` | Comma-separated paths that bypass auth |
| `CORS_ALLOWED_ORIGINS` | `*` | Comma-separated allowed origins |
| `TRUSTED_PROXIES` | (none) | Comma-separated CIDR ranges for trusted reverse proxies |
| `ALLOW_UNVERIFIED_CLIENT_IPS` | `false` | Accept `API_KEY` without `TRUSTED_PROXIES` (local development only) |
| `SINK_INBOUND_TOKEN` / `SINK_INBOUND_TOKEN_FILE` | (none) | Bearer token of the HTTP sink receiver; unset leaves `/sink/inbound` unrouted |

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
With `API_KEY` set, an empty `TRUSTED_PROXIES` fails startup (`Config::validate`)
unless `ALLOW_UNVERIFIED_CLIENT_IPS=true`: the brute-force block applies only to
verified client IPs, so an API key without trusted proxies would look protected
without being so. The flag accepts `true`/`false` in any case; empty means unset,
and any other value fails startup.

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
- Request outcomes map to `NotAccepted` (`TransientNotAccepted`, lane
  refusals, `LifecycleBusy`), `OutcomeUnknown` (`TransientNotCommitted`,
  `OperationUncertain`, `FetchProgressUncertain`, `RequestTooOld`) and
  `AlreadyApplied` (`RequestAlreadyApplied`), deliberately **not**
  connection errors: the SDK has already replayed them, and the wrapper's
  reconnect-and-retry would resend a write the server applied or may have
  applied. `AlreadyApplied` answers `202`.
- **SDK 0.12 lanes.** The SDK admits one mutation at a time per lane (one
  for metadata, one per partition, plus a progress lane per partition for
  auto-commit and `Next` polls) and keeps a mutation whose outcome it cannot
  prove *retained* there. Every later call on that lane then fails with
  `LaneBlocked` until the application abandons or resolves it. A call
  cancelled after its request left the client is retained too, and the
  wrapper's per-request timeout cancels calls exactly that way. So every
  lane-taking SDK call goes through `lanes::settled` / `send_settled`
  (`src/iggy_client/lanes.rs`), which abandons any retained operation it
  sees and runs a lane-refused call once more (a refusal takes no request
  id, so nothing was sent). The gateway never resends an unknown outcome
  itself. `IncompatibleProtocolVersion` maps to `IncompatibleServer`, which
  stops a reconnect session (otherwise infinite at `MAX_RECONNECT_ATTEMPTS=0`)
  and exits startup with `EX_PROTOCOL`.

## Per-Topic Options

Since server 0.9, durability, segment, flush and retention settings belong to
each topic and are fixed at creation (ADR-0001 PR 2, on SDK 0.12 per
ADR-0002). `src/models/topic_options.rs` holds the whole contract:

- `TopicOptionsInput` is the raw form, from the request body's `options`
  object or from `IGGY_TOPIC_*`. `validate()` checks each option against the
  server's bounds (the SDK's `iggy_common` constants and validators) and the
  rules that combine options: a finite `max_topic_size` must be at least the
  segment size (the server's own error for that reaches the client with
  placeholder values, apache/iggy#4785), and
  preallocation is capped across partitions. Iggy's spellings for "server
  default" (`"server_default"`, `"0"`) are refused: omitting the option says
  that, and keeps read-back provenance honest.
- `TopicOptionsView` reports a topic's stored options with each value in the
  form `POST` accepts (sizes in exact units, since the SDK's display rounds)
  and its `source`: `explicit`, `default` (filled by the server at
  admission) or `unset`.
- **SDK 0.12 always sends both durability keys** (`TopicCreateOptions` has no
  "unset" for them, and the trait API offers no way around it), so they read
  back `explicit` on every topic this gateway creates. Documented rather
  than worked around with hand-built wire requests.
- Request bodies refuse unknown fields at both levels, so a misspelt or
  misplaced option fails (`422`) instead of silently leaving a topic on a
  default. Server admission errors for options map to `400`
  (`classify_iggy_error`).
- `replication_factor` (1 to 32) is checked here only for range; the server
  refuses one above the node count (`InvalidReplicationFactor`, `400`).
  Reads report the factor admission stored: the node count for the
  default `"all"`, so `1` on a single node.
- Not offered: `compression_algorithm` (stored, not applied by the server),
  `partition_resize_policy`, and topic updates (ADR-0002).

## Clustering

`docker-compose.cluster.yaml` runs three nodes and the gateway (port 8001);
[docs/cluster-demo.md](docs/cluster-demo.md) explains it. Facts the code
depends on (SDK 0.12, server 0.10):

- **One address per SDK connection string.** After sign-in the SDK reads
  the roster and keeps a link to every node, routing each send and poll to
  its partition's primary and retrying through an election within a 30 s
  per-call deadline. The gateway only has to reach *one* node when it
  builds a client, so `IGGY_CONNECTION_STRING` takes comma-separated seeds
  (`src/iggy_client/seeds.rs`): startup tries each in order with an equal
  share of `OPERATION_TIMEOUT_SECS`, and reconnects start from the last seed
  that worked. Only seed addresses are logged; the connection string holds
  credentials.
- **Roster addresses must be reachable from the gateway.** The SDK dials
  each node at its advertised address (the roster IP when none is set). The
  demo keeps the gateway inside the cluster network and publishes no Iggy
  port.
- **Node `status` in cluster metadata is not liveness:** 0.10 reports
  `healthy` for a node that is down. `GET /cluster` passes it through with
  that caveat; the live signal is each partition's `in_sync_members` on a
  topic read, which the failover script and `tests/cluster_tests.rs` wait on.
- Losing one node of three does not disconnect the SDK, so `/health` stays
  up; the switch shows as send latency of about `heartbeat_timeout` (5 s).
- `tests/cluster_tests.rs` is `#[ignore]`d (it needs the stack and the
  docker CLI) and runs weekly in Extended Tests, followed by
  `scripts/cluster-failover-demo.sh`, which exits non-zero on a missed
  budget.

## Connectors

`docker-compose.connectors.yaml` is an overlay on `docker-compose.yaml` that
adds the connector runtime with upstream's HTTP source and sink;
[docs/connectors-demo.md](docs/connectors-demo.md) explains it. Facts the
code and the image depend on (runtime 0.6.0, plugins at `91e02063`):

- **Plugins are built from source.** `connectors/Dockerfile` compiles them
  at the exact commit of the runtime image: they share a C ABI, so bump
  `IGGY_COMMIT` and the runtime tag together. The runtime image's revision
  label names a later commit, but the binary is release code.
- **The runtime image needs `ca-certificates`** (apache/iggy#4787).
  Without system roots, the HTTP sink's client fails to build. The
  connector SDK does not log a plugin's `open()` error (apache/iggy#4786),
  so the runtime logs only "Plugin initialization failed".
- **Plugin secrets come from rendered TOML, not the environment.** Runtime
  env overrides reach top-level plugin fields only. `connectors/init.sh`
  generates the secrets into the `connectors_secrets` and `iggy_secrets`
  volumes, and `connectors/entrypoint.sh` substitutes them after checking
  each is hex of the expected length.
- **The runtime API (8081) serves plugin secrets.** It stays unpublished and
  requires an `api-key` header, which Prometheus sends from a file.
- **`POST /sink/inbound`** (`src/services/sink_inbox.rs`) exists only with
  `SINK_INBOUND_TOKEN[_FILE]`. Two layers on the POST route
  (`post(..).layer(..).layer(..)` in `routes.rs`) run before the body is
  read: the bearer token, checked in constant time, then the `ndjson`
  content type (`415`). A batch is kept whole or not
  at all. It keeps at most 100 deliveries and 1 MiB in memory.
- **Delivery.** The sink is at most once per batch (offsets are stored when
  polled), and its HTTP retries can duplicate a batch. A source `200` means
  buffered in memory.
- **The poll API counts non-`Event` payloads in `skipped`** rather than
  dropping them silently. The webhook topic takes any body.
- **Known SDK 0.12.0 stall** (apache/iggy#4784). An `OnRead` consumer
  group, such as the sink's, does not fetch a partition while another
  partition in its readiness group has never held a message. On a fresh
  stack every partition is empty, so the demo and tests send three events,
  one per partition.
- `tests/connectors_tests.rs` is `#[ignore]`d and runs weekly in Extended
  Tests with `scripts/connectors-demo.sh`.

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

Resilience is layered. The SDK's connection-string clients reconnect on
their own, and while they are still retrying they hold calls up to the SDK's
30 s deadline. The wrapper therefore treats **timeouts** as circuit-breaker
failures, classifies the SDK error variants that do escape
(`classify_iggy_error`), and runs live `ping` health probes, so `/health`
and `/ready` reflect reality during an outage rather than a latched startup
flag.

**Reconnection retries are capped at 3 by default** (ADR-0003,
`seeds::DEFAULT_RECONNECTION_RETRIES`). `seeds::expand` adds the transport's
option (`reconnection_retries`, or `reconnection_max_retries` for QUIC) to
each seed unless `IGGY_CONNECTION_STRING` sets it.
- **Why:** with the SDK default, `unlimited`, during a full outage every call
  waits until the server returns or the SDK's 30 s deadline passes, then
  fails as `TransientNotAccepted`. The breaker deliberately ignores that
  error, so requests keep waiting up to 30 s for as long as the outage
  lasts.
- **With the cap:** the SDK answers `NotConnected` once its retries run out.
  The breaker opens and requests get an immediate 503. The SDK's keepalive
  (every 5 s) restarts a client that ran out, with a fresh budget, so it
  recovers on its own.
- `tests/integration_tests.rs::test_a_call_to_a_stopped_server_ends_as_a_connection_error`
  fails without the cap.

The SDK's diagnostic events (`src/iggy_client/events.rs`, TD-2026-07-02)
end a recorded outage as soon as the SDK reconnects (`Connected` triggers an
immediate ping) and start one as soon as the last link closes: SDK 0.12
publishes `Disconnected` then, while it is still retrying (0.11 did so only
once a bounded retry sweep gave up). The ping stays the source of
truth and the backstop for a server that stops answering without closing the
connection.

**Advertised endpoint = dialled endpoint.** After sign-in the SDK records a
node's advertised address plus its own TCP port and fails over to it, even
against a single node. If that endpoint reaches a different server, a client
silently signs in there. docker-compose advertises `iggy` (the in-network
name, port 8090). The integration fixture binds the container's listener to
the same free port it maps on the host. It picks that port below the
kernel's ephemeral range (`find_container_port` in
`tests/integration_tests.rs`). Inside the container the server binds a
replica listener on port 0 just before the client port, and the replica
listener could otherwise take that port first.

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
  consumes from the per-IP brute-force budget. Once a **verified** IP
  (`TRUSTED_PROXIES` set and the peer address available,
  `ip::client_ip_is_verified`) exhausts the budget it is blocked, and the
  block is checked **before** the key is compared: every request from it gets
  the same 429, valid key or not, so the block cannot serve as a hit/miss
  oracle (`AuthFailureGuard` in `src/middleware/auth.rs`). Unverified IPs come
  from spoofable headers, so they are throttled on failures but never
  blocked: blocking them would buy no protection (rotate the header) and
  would let anyone lock out the shared `"unknown"` bucket. Startup therefore
  refuses `API_KEY` without `TRUSTED_PROXIES` unless
  `ALLOW_UNVERIFIED_CLIENT_IPS=true`, and warns while that opt-out is in use.
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
