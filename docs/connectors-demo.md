# Connectors Demo

Apache Iggy's connector runtime with upstream's HTTP source and HTTP sink,
wired to the gateway in both directions:

- **Inbound:** an external system POSTs a signed webhook to the HTTP source.
  It lands in a `persisted` topic, and the gateway's poll API reads it.
- **Outbound:** an event sent through the gateway comes back to it through
  the HTTP sink, at the gateway's own receiver, `POST /sink/inbound`.

The whole loop runs inside this repository, so it can be tested end to end
with no third-party service. ADR-0001 planned this as PR 4; ADR-0002 pinned
it to connectors 0.6.0. Everything here was verified against
`apache/iggy:0.10.0`, `apache/iggy-connect:0.6.0` and plugins built at
`91e02063` on 2026-11-08.

## Contents

1. [Running It](#running-it)
2. [Layout](#layout)
3. [Inbound: Webhooks](#inbound-webhooks)
4. [Outbound: The Sink](#outbound-the-sink)
5. [Delivery Semantics](#delivery-semantics)
6. [Secrets and Exposure](#secrets-and-exposure)
7. [Metrics](#metrics)
8. [The Connector Image](#the-connector-image)
9. [Known Issues](#known-issues)
10. [Tests](#tests)
11. [Limitations](#limitations)

## Running It

```bash
docker compose -f docker-compose.yaml -f docker-compose.connectors.yaml up -d --build --wait

# Both flows, with what each step returned
scripts/connectors-demo.sh

# Stop, keeping volumes (and the generated secrets); add -v to delete them
docker compose -f docker-compose.yaml -f docker-compose.connectors.yaml down
```

`docker-compose.connectors.yaml` is an overlay on the main stack: same
project, same Iggy, gateway, Prometheus and Grafana, plus the connector
runtime. `COMPOSE_FILE=docker-compose.yaml:docker-compose.connectors.yaml`
saves repeating both `-f` flags. Nothing needs configuring: a one-shot
container generates every secret on first start. The first build compiles
the two plugins, which takes about two minutes.

Without the overlay, the main stack runs as before: no connectors, and the
gateway's `/sink/inbound` routes do not exist.

A run of the demo script:

```
== Inbound: a signed webhook into sample-stream/webhooks
forged signature:  HTTP 401
signed webhook:    HTTP 200 {"status":"queued"}
read back via GET /streams/sample-stream/topics/webhooks/messages:
  {"offset":0,"id":56870877571974037698542652037181104946,"event_type":"webhook.received"}

== Outbound: POST /messages back to the gateway through the HTTP sink
POST /messages #1: HTTP 201
POST /messages #2: HTTP 201
POST /messages #3: HTTP 201
delivered to POST /sink/inbound, reported by GET /sink/inbound:
  {"partition_id":0,"offset":3,"id":"6ce22808cd362896954d39d160bb4ddc",...}
  {"partition_id":2,"offset":2,"id":"4dbf73a7d5c91483f24c03c06a484994",...}
  {"partition_id":1,"offset":2,"id":"c4f3622ca54915a1a34c7c0058d198b1",...}

== Summary
inbound:   webhook readable through the gateway 1s after it was sent
outbound:  three events back at the gateway 0s after the first was sent
PASS
```

## Layout

```
                  host :8085                                     host :8000
webhook sender ──────────────► HTTP source ─┐            ┌────── gateway (app) ◄── clients
  (HMAC-SHA256)                 (plugin)    │            │         │      ▲
                                            ▼            │         ▼      │ POST /sink/inbound
                              sample-stream/webhooks     │  sample-stream/events  (bearer token)
                                            │   Iggy     │         │      │
                                            └─► gateway poll API   └─► HTTP sink (plugin)
                                                                          │
                     iggy-connectors container: runtime + both plugins ───┘
```

| Service | Role |
|---|---|
| `connectors-init` | One-shot (`connectors/init.sh`, in the `apache/iggy` image for its CLI). It generates the secrets and the runtime's personal access token, and keeps both across restarts |
| `connectors` | The runtime (`connectors/Dockerfile`). It renders its configuration from `connectors/templates/` and the secrets, then runs both plugins as uid 10001 |
| `app` | The gateway, now with `SINK_INBOUND_TOKEN_FILE` set, which routes `/sink/inbound` |
| `prometheus` | Gains two scrape jobs (`observability/prometheus/connectors.yml`) |

## Inbound: Webhooks

The HTTP source accepts `POST /e/<endpoint id>` on port 8085:

- **The endpoint id is a secret path**: 128 random bits.
- **The body must carry an HMAC-SHA256 signature** in `X-Signature-256`, as
  `sha256=<hex>`, computed over the raw bytes.
- **Accepted bodies go to `sample-stream/webhooks`**, which the runtime
  creates with one partition and `durability = persisted`. The source writes
  to no other durability.
- **The source answers:**
  - `200 {"status":"queued"}`;
  - `401` for a missing or wrong signature;
  - `404` for any other path;
  - `413` above 1 MiB;
  - `429` when its buffer is full.

To send one by hand:

```bash
endpoint=$(docker exec iggy-connectors cat /run/secrets/connectors/webhook_endpoint_id)
secret=$(docker exec iggy-connectors cat /run/secrets/connectors/webhook_hmac_secret)
body='{"id":"6f1c2c1e-7d1b-4c8e-9a51-0b7c1d2e3f40","event_type":"webhook.received","timestamp":"2026-11-08T00:00:00Z","payload":{"type":"Generic","data":{"from":"curl"}}}'
signature=$(printf '%s' "$body" | openssl dgst -sha256 -hmac "$secret" -r | cut -d' ' -f1)

curl -i "http://localhost:8085/e/$endpoint" \
  -H "Content-Type: application/json" \
  -H "X-Signature-256: sha256=$signature" \
  -H "X-Webhook-Delivery: delivery-1" \
  --data-binary "$body"

curl -s "http://localhost:8000/streams/sample-stream/topics/webhooks/messages?partition_id=0&offset=0&count=10" | jq
```

`openssl -hmac` takes the secret on its command line, where other users of
the host can see it while it runs; sign elsewhere on a shared machine.

The webhook topic stores any body. The gateway's poll API returns the
messages that are gateway `Event`s and leaves the rest out, counting them in
`skipped`; `current_offset` still moves past them. A GitHub-style body stays
in the topic but never appears in `messages`:

```json
{"count": 0, "skipped": 1, "current_offset": 2, ...}
```

The source also stores, as message headers, the receiving instance, the
peer address, the receive time and the sender's `X-Webhook-Delivery`. The
gateway's poll API does not return message headers.

## Outbound: The Sink

The HTTP sink consumes `sample-stream/events` in its own consumer group,
`gateway_inbound_sink`. It POSTs each polled batch to
`http://app:8000/sink/inbound` as newline-delimited JSON (`ndjson`), one
envelope per message:

```json
{"metadata": {"iggy_id": "5d2c2d84efa748be944336a298797d1c", "iggy_offset": 3,
              "iggy_timestamp": 1762587065916187, "iggy_stream": "sample-stream",
              "iggy_topic": "events", "iggy_partition_id": 2},
 "payload": {"id": "d88ee8e5-...", "event_type": "sink.demo", ...}}
```

The receiver (`src/handlers/sink.rs`, `src/services/sink_inbox.rs`):

- **It exists only when `SINK_INBOUND_TOKEN` or `SINK_INBOUND_TOKEN_FILE` is
  set.** Otherwise both routes answer `404`. The token must be 32 to 1024
  printable characters.
- **`POST` needs `Authorization: Bearer <token>`.** The token is checked in
  constant time, before the body is read. A missing or wrong token answers
  `401` and is counted.
- **A batch is kept whole or not at all.** A line that is not an envelope
  answers `400`, naming the line. A content type other than
  `application/x-ndjson` answers `415`. Success answers
  `200 {"accepted": <n>}`.
- **`GET /sink/inbound` reports the receiver.** It returns lifetime counters
  and the newest deliveries, newest first: at most 100 of them, and at most
  1 MiB of envelopes. Like every other read, it is guarded by `API_KEY` when
  that is set.

```json
{
  "messages_received": 3,
  "batches_accepted": 3,
  "batches_rejected": 0,
  "unauthorized_requests": 0,
  "last_received_at": "2026-11-08T07:31:05.919449256Z",
  "retained": 3,
  "recent": [
    {"received_at": "2026-11-08T07:31:05.919449256Z", "stream": "sample-stream",
     "topic": "events", "partition_id": 2, "offset": 3,
     "id": "5d2c2d84efa748be944336a298797d1c", "timestamp": "2026-11-08T07:31:05.916187Z",
     "payload": {"id": "d88ee8e5-1149-499a-b7a9-ddb64beef9c8", "event_type": "sink.demo", "...": "..."}}
  ]
}
```

If you set `API_KEY` on the gateway, the sink must send `X-API-Key` as well:
add it to `[plugin_config.headers]` in
`connectors/templates/connectors/gateway_sink.toml`.

## Delivery Semantics

Neither direction is exactly-once, and only the sender's retries make the
inbound side more than best-effort.

**Inbound (HTTP source): best-effort.**
- **A `200` means the body is buffered in the runtime's memory, not stored
  in Iggy.** A crash before the next send loses it, with only a log line and
  a metric to show for it. A failed send is retried from the buffer.
- **Duplicates happen.** A sender that never saw the `200` (a timeout, a
  proxy retry) sends the body again, and it is stored twice under a new
  message id. Deduplicate on the sender's delivery id, which the source
  copies into the `X-Webhook-Delivery` message header.

**Outbound (HTTP sink): at most once per batch.**
- **The runtime stores the consumer group's offsets as it polls**
  (`CommitPolicy::OnRead`), before the plugin delivers. A batch the gateway
  refuses (`4xx`), or that fails after the plugin's own retries, is not
  delivered again. Its messages count as errors on the runtime's metrics.
- **The plugin retries a `429`, a `5xx` or a network error** up to three
  times, with backoff. A batch the gateway accepted, but whose answer was
  lost, therefore arrives twice. The receiver does not deduplicate; stream,
  topic, partition and offset identify a message.

## Secrets and Exposure

`connectors-init` writes these files once, from `/dev/urandom`:

| File | Volume | Read by |
|---|---|---|
| `webhook_endpoint_id` (16 bytes, hex) | `connectors_secrets` | connectors only (0400, uid 10001) |
| `webhook_hmac_secret` (32 bytes, hex) | `connectors_secrets` | connectors only (0400, uid 10001) |
| `sink_inbound_token` (32 bytes, hex) | `connectors_secrets` | connectors and the gateway (0444) |
| `connectors_api_key` (32 bytes, hex) | `iggy_secrets` | connectors and Prometheus (0444) |
| `connectors_pat` (personal access token) | `iggy_secrets` | connectors only (0400, uid 10001) |

How they are handled:

- **Secrets go into files, not environment variables.** The runtime cannot
  take plugin secrets from the environment: its env overrides reach only
  top-level plugin fields, and the webhook endpoint's HMAC secret and the
  sink's `Authorization` header are nested. So `connectors/entrypoint.sh`
  renders the TOML templates at start. It checks that each secret is
  lowercase hex of the expected length before substituting it, and writes
  the result where only uid 10001 can read it.
- **No password is stored.** The runtime signs in to Iggy with the
  `connectors` personal access token, which it reads from the file.
- **Restarts keep the secrets.** `connectors-init` keeps a token that still
  signs in, so a runtime holding it can reconnect.
- **`down -v` starts over**, with new secrets.

Network exposure:

| Port | Published | What |
|---|---|---|
| 8085 | yes (`WEBHOOK_PORT`) | HTTP source: `/e/<id>` and `/health` only |
| 8081 | no | Runtime API: `/metrics`, `/stats`, and every connector's configuration *including plugin secrets*. It requires the `api-key` header on every path but `/` and `/health` |
| 8086 | no | HTTP source admin listener: health and metrics, unauthenticated. No `management_token` is set, so the endpoint management API is not mounted |

This is a local demo. **In production:**
- terminate TLS in front of 8085 and put a reverse proxy there, since the
  source sets no read timeout or connection cap;
- enable TLS on the runtime API;
- keep 8081 and 8086 off any shared network.

Reading the secrets with `docker exec` needs Docker access, which is root
on the host anyway.

## Metrics

Prometheus gains two jobs, `iggy-connectors` (the runtime, scraped with the
generated `api-key` header) and `iggy-connectors-webhooks` (the source's
admin listener). The base `prometheus.yml` includes
`/etc/prometheus/scrape.d/*.yml`, where the overlay mounts them; without the
overlay the include matches nothing.

Grafana's **Iggy Connectors** dashboard
(`observability/grafana/provisioning/dashboards/iggy-connectors.json`) shows:
- connectors running;
- webhook requests by status;
- messages sent, consumed and processed per connector;
- the gateway receiver's requests by outcome;
- per-stage latency;
- connector errors;
- the webhook buffer.

The gateway exports `iggy_sink_inbound_requests_total{outcome}` (`accepted`,
`rejected`, `unauthorized`) and `iggy_sink_inbound_messages_total`.

## The Connector Image

`apache/iggy-connect` ships the runtime binary only; upstream publishes no
plugin builds. `connectors/Dockerfile` therefore:

1. **Fetches apache/iggy at exactly `91e0206367aad2d30259f7d45203832b08ae84e7`**
   by its full hash, and checks the checkout.
2. **Builds `iggy_connector_http_source` and `iggy_connector_http_sink`**
   with Rust 1.99.1 on Debian trixie, the runtime image's glibc.
3. **Copies them onto `apache/iggy-connect:0.6.0`**, adds `ca-certificates`
   and `curl`, and runs as a non-root user with a healthcheck. The container
   counts as healthy once the runtime reports both connectors `running`.

Runtime and plugins exchange configuration and messages across a C ABI
defined by the connector SDK, so both must come from the same commit.
**Bumping connectors means changing `IGGY_COMMIT` and the runtime tag
together.**

The image's `org.opencontainers.image.revision` label reads `1fb58a67`, the
branch head when upstream's release workflow ran, not `91e02063`. The binary
is release code nonetheless. It contains strings that exist at `91e02063`,
and none of the sink delivery changes merged after it (checked 2026-11-08),
which changed the sink ABI.

## Known Issues

Found while building this demo against the pinned releases, and filed
upstream:

- **An `OnRead` consumer group holds messages next to an empty partition**
  ([apache/iggy#4784](https://github.com/apache/iggy/issues/4784)).
  - **What happens:** SDK 0.12.0's `IggyConsumer`, in a consumer group
    with `CommitPolicy::OnRead` and a `Next` start, does not fetch a
    partition while another partition in its readiness group (as the server
    reports them) has never held a message. The runtime's sink consumer is
    one of these: on master, a sink's default `at_most_once` guarantee polls
    this way.
  - **Why:** the reader checks each new partition to resolve where `Next`
    starts. On an empty partition that check repeats every `max_wait`, and
    while it is pending the reader does not start the fetch that would
    deliver the group's other partitions.
  - **When it clears:** once the empty partition receives a message. From
    then on, delivery is prompt. If it never does, the messages next to it
    are never delivered.
  - **What it does not affect:** a single-partition topic, partitions that
    all hold data, and the server's deferred read: a raw `poll_messages`
    with `max_wait` returns the message at once.
  - **Here:** on a fresh stack, `events` has three empty partitions, so the
    demo and the tests send three events, which the gateway spreads over
    all three. A single event can wait indefinitely.
- **The runtime image has no CA certificates**
  ([apache/iggy#4787](https://github.com/apache/iggy/issues/4787)).
  - **What happens:** reqwest, with rustls' platform verifier, refuses to
    build a client without system roots, even for an `http://` URL ("No CA
    certificates were loaded from the system").
  - **Effect:** the HTTP sink cannot open, and the runtime itself will not
    start with `[state] storage = "http"`. The image here installs
    `ca-certificates` for that reason.
- **A plugin's `open()` error is not logged**
  ([apache/iggy#4786](https://github.com/apache/iggy/issues/4786)). The
  connector SDK turns a failed `open()` into a status code. The runtime then
  logs only `Plugin initialization failed`, and the HTTP sink does not log
  the reason itself.
- **Errors from the server print placeholder values**
  ([apache/iggy#4785](https://github.com/apache/iggy/issues/4785)). A reply
  carries only a status code, and the SDK fills the error's fields with
  defaults, such as "Stream with ID: 0 was not found." This is why the
  gateway checks `max_topic_size` against the segment size itself
  ([Durable Storage Guide](durable-storage-guide.md)).

## Tests

- **`tests/integration_tests.rs`** (every PR) covers the receiver against a
  real Iggy container:
  - a batch is accepted and reported newest first;
  - a missing or wrong token answers `401`;
  - a malformed batch answers `400` and another content type `415`;
  - without a token, the routes do not exist.
- **`tests/connectors_tests.rs`** is `#[ignore]`d, because it needs the
  overlay stack and the docker CLI. It covers:
  - a signed webhook is stored, readable through the gateway, and in a
    `persisted` topic;
  - a bad signature answers `401` and an unknown endpoint `404`;
  - a non-Event body is counted in `skipped`;
  - three events come back through the sink;
  - the receiver refuses a request without the token;
  - Prometheus scrapes both connector jobs.
- **The weekly Extended Tests workflow** (`.github/workflows/extended-tests.yml`,
  "Connectors Tests") brings the stack up on a GitHub runner, runs those
  tests, then `scripts/connectors-demo.sh`, which exits non-zero on a failed
  step.

To run them locally:

```bash
docker compose -f docker-compose.yaml -f docker-compose.connectors.yaml up -d --build --wait
cargo test --test connectors_tests -- --ignored --test-threads=1
scripts/connectors-demo.sh
```

## Limitations

- **HTTP only.** Other connectors (databases, object stores, MQTT) are out
  of scope (ADR-0001).
- **Static endpoints.** The webhook endpoint lives in the TOML. Runtime
  registration through the management API is not demonstrated.
- **Signatures over the body only.** The source verifies a hex HMAC of the
  raw body (GitHub-style). Schemes that sign a composed string, such as
  Stripe's or Twilio's, need verification downstream.
- **No message encryption.** It is out of scope (ADR-0001). Since 0.10,
  each encrypted stream has its own data key, chosen when the stream is
  created; enabling encryption later covers only new streams.
- **Single host, single runtime.** Endpoints registered on one runtime
  process are not shared with another.
