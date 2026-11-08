# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A three-node cluster demo (ADR-0001 PR 3, with ADR-0002's per-topic
  replication factor), described in `docs/cluster-demo.md`:
  - `docker-compose.cluster.yaml` runs three nodes on a fixed subnet, with
    the roster shared through a YAML anchor, a volume per node, the replica
    links' pre-shared key from `.env`, and the gateway inside the network on
    port 8001. No Iggy port is published.
  - `scripts/cluster-failover-demo.sh` SIGKILLs the metadata leader under
    live traffic, then reports the election time, failed sends, the slowest
    send and the restarted node's return to sync. It exits non-zero on a
    missed budget. In local runs the election took 5-6 s and no send failed;
    the SDK held sends for up to about 5 s until a new primary answered.
  - `tests/cluster_tests.rs` (ignored by default) checks the roster, that
    each partition sits on as many nodes as its replication factor, that a
    factor above the node count answers `400`, and that the gateway starts
    with its first seed down.
- `GET /cluster` reports the roster with each node's role and status, from
  cluster metadata. Server 0.10 reports every node `healthy`, even one that
  is down, and the docs say so; the live signal is `in_sync_members`
- The single-topic read reports each partition with the nodes that hold it
  (`members`) and those currently in sync (`in_sync_members`)
- The `replication_factor` topic option (1 to 32; the server refuses more
  than its node count) and `IGGY_TOPIC_REPLICATION_FACTOR`. Reads report the
  stored factor, which is the node count under the default `"all"`
- `IGGY_CONNECTION_STRING` accepts several comma-separated server addresses
  as seeds. Startup tries each in order with an equal share of
  `OPERATION_TIMEOUT_SECS`, and reconnects start from the seed that worked
  last. An SDK 0.12 connection string names one server; once signed in, the
  SDK follows the cluster roster itself. Logs name seed addresses only,
  never the connection string, which holds the credentials
- PR Checks validates `.github/dependabot.yml` against Dependabot's schema
  (`check-jsonschema==0.38.2`, "Dependabot Config" job), and Dependabot now
  proposes the Dockerfile's base-image updates (TD-2026-09-02)
- The SDK's diagnostic events drive the tracked connection state
  (`src/iggy_client/events.rs`, resolving TD-2026-07-02): `Connected` triggers
  an immediate ping, so an outage ends as soon as the SDK reconnects, and
  `Disconnected` marks the connection down. SDK 0.12 publishes `Disconnected`
  as soon as its last link closes, even with default (unlimited)
  connection-string retries, so `/health` and `/ready` drop at once instead
  of at the next ping
- Iggy request outcomes get their own responses instead of a generic 500:
  `TransientNotAccepted`, lane refusals and `LifecycleBusy` ->
  `503 not_accepted` (nothing applied, safe to retry);
  `TransientNotCommitted`, `OperationUncertain`, `FetchProgressUncertain` and
  `RequestTooOld` -> `503 outcome_unknown` (resending may duplicate); and
  `RequestAlreadyApplied` -> `202 Accepted` with `{"status":"applied"}`. None
  of them triggers the wrapper's reconnect-and-retry or counts as a
  circuit-breaker failure, since the SDK has already replayed each one
- SDK 0.12 lane settling (`src/iggy_client/lanes.rs`, ADR-0002). The SDK
  keeps a mutation whose outcome it cannot prove, including one cancelled
  after its request left the client, retained in its lane, and refuses every
  later call on that lane until the application abandons it. Without this,
  one send cut off by the per-request timeout made every later send to that
  partition fail with `Lane blocked by operation N` for the life of the
  client (reproduced against server 0.10.0). Every lane-taking SDK call now
  abandons a retained operation it meets and runs a lane-refused call once
  more, which is safe because a refused call was never sent
- `IncompatibleProtocolVersion` maps to `502 incompatible_broker`, ends a
  reconnect session instead of retrying it forever, and makes startup exit
  with `EX_PROTOCOL` (76) rather than `EX_UNAVAILABLE`
- The startup connection-timeout message says to check the server version,
  the likeliest cause once the address is known to be right
- Per-topic options on topic creation (ADR-0001 PR 2, on SDK 0.12 per
  ADR-0002): `POST /streams/{stream}/topics` takes an optional `options`
  object with `durability`, `consumer_offset_durability`, `message_expiry`,
  `max_topic_size`, `segment_size`, `messages_required_to_save`,
  `size_of_messages_required_to_save` and `preallocate_segments`. Each is
  checked against the server's bounds before anything is sent, and an
  invalid one answers `400` naming it (`options.segment_size`). A finite
  `max_topic_size` below the segment size is refused with the floor stated;
  the server refuses it too, with a message that prints neither value
- Topic reads (`GET /streams/{s}/topics`, `/topics/{t}`) report each stored
  option's value, in the form `POST` accepts, and its `source`: `explicit`
  (sent by the creating client), `default` (filled by the server) or `unset`.
  SDK 0.12 always sends both durability options, so they read `explicit` on
  every topic created through the SDK
- `IGGY_TOPIC_<OPTION>` environment variables set the default topic's options
  (e.g. `IGGY_TOPIC_DURABILITY=persisted`); an invalid value fails startup
  naming the variable. They apply when the gateway creates the topic, and for
  an existing topic each option that differs is logged as a warning
- `scripts/verify-release.sh` checks a release.yml run step by step instead
  of trusting its conclusion: that the tag names the commit the run built;
  what each step printed; the release notes; that every asset is
  byte-identical to its build job's artifact and holds a binary for its
  target; the Pages deployment and the served docs; crates.io against
  Cargo.toml's `publish`; and that the host platform's binary runs, isolated
  and time-bounded. It fails on a step that failed under `continue-on-error`,
  which the Actions API reports as a success: the crates.io publish step
  failed that way on every stable release through v0.4.1 (TD-2026-09-04).
  It handles a run whose jobs were re-run, where the jobs an attempt did not
  re-run carry over with their logs, and needs gh 2.75 or later to fetch
  those
- The Verify Release workflow runs it after every green Release run,
  re-runs included, on exactly the run and attempt that triggered it, so a
  hidden failure fails a run instead of waiting for someone to look; it can
  also be run by hand for any tag. It fails on the stable releases up to
  v0.4.1, by design, for that masked publish step
- CI lints shell scripts with shellcheck, as part of the required CI Success
  check
- CONTRIBUTING.md describes releasing, and exercising `release.yml` with a
  pre-release tag, including the two things that outlive the exercise: the
  Pages deployment (with how to restore it, and the limits of doing so) and
  the tag itself

### Changed

- Extended Tests' stress job is replaced by Cluster Tests, which runs the
  cluster tests and the failover script against `docker-compose.cluster.yaml`
  (workflow input `run_cluster_tests`). The stress job never ran a test: its
  `cargo test -- --ignored` matched none, its fallback ran only on failure,
  and its Iggy service container went unused, since the integration tests
  start their own
- A malformed `IGGY_CONNECTION_STRING` (no scheme or credentials, an empty
  or repeated address) now fails configuration validation, without
  repeating the string. A failed startup connection lists each seed's error
- **Breaking:** `POST /streams/{stream}/topics` refuses unknown fields with
  `422`, at the top level and inside `options`. Unknown fields were ignored,
  so a misspelt or misplaced option would have left the topic on a server
  default without any error
- Topics created without options no longer send `message_expiry` (never) and
  `max_topic_size` (unlimited) explicitly. The server's defaults are the same
  values, so retention is unchanged; topic reads report them as `default`
- The server's option admission errors (`InvalidOptionValue`,
  `UnsupportedOptionKey`, `OptionsBlockTooLarge`, `InvalidTopicSize`,
  `InvalidSegmentSize`, `InvalidReplicationFactor`) answer `400` instead of
  `500`
- `docs/durable-storage-guide.md` is rewritten for server 0.10: per-topic
  `replicated`/`persisted` durability, flush scheduling, segments and
  preallocation, retention and the cleaner (on by default since 0.9), backup
  with hard links preserved, recovery, and a table mapping each removed 0.8
  setting to its replacement. The partitioning guide's 0.8 server
  configuration section is replaced the same way (TD-2026-07-06)
- **Breaking:** Apache Iggy server 0.8.0 -> 0.10.0 and Rust SDK 0.10.0 ->
  0.12.0, together, by way of 0.9.0 and 0.11.0 (ADR-0001, ADR-0002). Each SDK
  speaks only its own release's wire protocol, so a mismatched server fails
  sign-in, and no server reads the previous release's data (0.10 refuses 0.9
  data at startup): the compose data volume is now `iggy_data_0_10`, and the
  old `iggy_data` and `iggy_data_0_9` volumes can be removed
- MSRV 1.93.0 -> 1.95.0, which the SDK requires since 0.11
- Topic and stream creation use the SDK's `TopicCreateOptions` and
  `StreamCreateOptions`; behaviour is unchanged (no expiry, unlimited size)
- Polls use SDK 0.12's `PollOptions`: `auto_commit=true` becomes
  `CommitPolicy::OnRead`, which stores the offset as part of the read. A
  poll now returns at most about 1 MiB of messages (the SDK default), so a
  large `count` can return fewer messages than before; `current_offset` says
  where to resume
- The integration fixture runs `apache/iggy:0.10.0` with explicit `0.0.0.0`
  binds, `IGGY_NODE_ADVERTISED_ADDRESS` (since 0.9 the server refuses a
  wildcard bind without it) and `IGGY_WEBSOCKET_ENABLED=false` (on by default,
  bound to loopback, and unused). 0.9 also needed
  `IGGY_SHARDING_CPU_ALLOCATION=all` on hosts without a NUMA topology, such
  as Docker Desktop; 0.10 boots there with the default
- SDK 0.11 no longer pulls in `paste`, so `deny.toml` drops the
  RUSTSEC-2024-0436 exception
- docker-compose runs `apache/iggy:0.10.0` with explicit binds,
  `IGGY_NODE_ADVERTISED_ADDRESS=iggy` and `IGGY_WEBSOCKET_ENABLED=false`, the
  io_uring capability set upstream uses (`SYS_NICE`, `seccomp:unconfined`,
  unlimited memlock) instead of `privileged`, and an explicit `IGGY_PATH`.
  Data lives in the `iggy_data_0_10` volume: the old mount at `/iggy/data`
  never matched the default data path, and no release reads its
  predecessor's data
- Prometheus scrapes Iggy's `/metrics`, which requires a bearer credential
  since 0.9, with a personal access token. A new one-shot `iggy-pat-init`
  service recreates it on every `up` (`observability/iggy-pat-init.sh`);
  Prometheus starts only once it succeeds, so a failure is loud rather than
  a blank dashboard
- CI's shellcheck step also lints `observability/*.sh`, as POSIX sh
- The Iggy Web UI is served by the server itself at `http://localhost:3000/ui`
  (`IGGY_HTTP_WEB_UI=true`); the separate `iggy-web-ui` container and its
  port 3050 are gone
- CI pins `Swatinem/rust-cache` to its v2.9.3 release commit in all 16
  uses (TD-2026-09-05). The pin had been an untagged `master` head under a
  `# v2` comment, so every github-actions Dependabot PR moved it to whatever
  upstream merged last; with a tagged SHA Dependabot proposes releases
- The `main` ruleset requires a pull request to be up to date with main
  before it merges, and requires the Dependabot Config check
  (TD-2026-09-06). A pull request whose checks passed against an older
  main could previously merge untested against the main it landed on

### Removed

- release.yml's crates.io publish job. The crate is `publish = false`, and
  its releases are repo-level only; the job could not succeed and failed on
  every stable release, hidden by `continue-on-error` (TD-2026-09-04)

### Fixed

- `docs/durable-storage-guide.md` said the compose stack keeps its data at
  `/app/local_data`; it sets `IGGY_PATH=/iggy/data`
- The quick-reference create-topic example in `docs/guide.md` sent fields
  the API never read (`topic_id`, `partitions_count`), so its topic got one
  partition instead of three; with unknown fields now refused it would fail
- The integration fixture advertised `127.0.0.1` with the in-container port
  8090. The SDK dials a node's advertised endpoint on failover, so a test
  whose container stopped signed in to whatever Iggy listened on the host's
  8090 (a local docker-compose stack). The container's listener now binds the
  same free port the host maps
- The README's error table listed `operation_timeout` as 503; the service
  answers `timeout` with 504
- The weekly stress-test job's Iggy service container had stayed on
  `apache/iggy:0.8.0` through the 0.9 upgrade, against its own "keep in
  lockstep" comment; it now runs 0.10.0 with the advertised address it needs
- The weekly stress-test job hung on 0.10: its readiness step ran `curl`
  without a timeout against Iggy's binary TCP port, and 0.10 holds such a
  connection open without answering. The step is gone (the service health
  check already gates the job's steps), and the job now times out after
  30 minutes
- The provisioned Grafana "Iggy Overview" dashboard charted metric names
  the server does not export (`iggy_messages_bytes_total`,
  `http_request_duration_seconds_bucket`, `iggy_streams_count`), so four of
  its six panels never showed data. They now chart what 0.9 exports: request
  rate (`http_requests_total`), net stored-message rate (`deriv(messages)`),
  connected `clients`, and `streams`/`topics`/`partitions`. The status panel
  no longer renders `up` as a percentage
- The README no longer says coverage is uploaded to Codecov: every upload
  whose log is still retained was rejected for want of a token
  (TD-2026-09-03)
- CI's cargo-deny check, part of the required CI Success check, failed on
  every run from 2026-11-03, when RUSTSEC-2026-0440 marked
  `astral-tokio-tar` unmaintained with no patched version. The crate is
  dev-only (pulled in by `testcontainers`, whose latest release still uses
  it), so `deny.toml` now accepts the advisory with that reasoning

### Security

- The Iggy client's tracing span recorded the whole connection string,
  password included, at `info` level on every startup. It no longer
  records it
- The auth brute-force block is now checked before the API key is compared.
  The key used to be compared first, so a blocked IP still got an answer to
  every guess (200 for the right key, 429 for a wrong one), and the block
  did not slow guessing below the general rate limit. With `TRUSTED_PROXIES`
  set, every request from a blocked IP now gets the same 429 until the
  failure budget refills, and a client that shares an IP with an attacker is
  locked out along with it. Without `TRUSTED_PROXIES` the client IP comes
  from spoofable headers, so failures are still throttled but valid keys are
  never blocked; otherwise anyone could lock out every header-less client
- **Breaking**: startup refuses `API_KEY` without `TRUSTED_PROXIES`.
  Brute-force protection would then be advisory, since rotating
  `X-Forwarded-For` gives an attacker a fresh failure budget, so an API key
  on its own looked protected without being so. Set `TRUSTED_PROXIES` to
  your reverse proxy's ranges, or `ALLOW_UNVERIFIED_CLIENT_IPS=true` for
  local development, which starts with a warning
- Bumped transitive `quinn-udp` 0.5.15 -> 0.5.16 (lockfile-only) to patch
  GHSA-6pp4-4cxf-xf88 (high; no CVE or RustSec ID yet, so `cargo audit` does
  not flag 0.5.15): after a truncated datagram, the Unix `recv_single`
  fallback could keep `MSG_TRUNC` set and spin on non-blocking receives,
  before any QUIC authentication, occupying the endpoint's runtime thread.
  Narrowly reachable here: `quinn` comes in only through the iggy SDK's QUIC
  transport, used only when `IGGY_CONNECTION_STRING` is `iggy+quic://`; the
  affected path is the fallback used on Apple and BSD targets, not Linux's
  batched receive; and it needs a datagram larger than the receive buffer.
  The same lockfile refresh takes the patch releases of `jiff`, `log`,
  `toml_edit`, `toml_datetime`, `sketches-ddsketch` and `spinning_top`

## [0.4.1] - 2026-09-23

Security patch for four RustSec advisories: one reachable on the public API
listener (h2; the listener has been accepting cleartext HTTP/2,
TD-2026-09-01), one exercised only on a TLS or QUIC Iggy connection
(rustls), and two not reachable at all. The root cause of the drift is
repaired too:
Dependabot had never run, and its version updates could not have reached
these transitive crates anyway. Reviewed with a four-agent double review
(tier-graduated cadence); artifacts under
`docs/code-reviews/rustsec-2026-09-round{1,2}.md`. TD-2026-07-02's trigger
fired during this release (iggy 0.11.0), and it is deliberately not in it.
TD-2026-09-02 records the check that would have caught the broken
Dependabot config: validating the file in CI.

### Added

- Tripwires for TD-2026-09-01. CI checks that `hyper-util`'s `http2`
  feature is still in the production dependency graph, the one thing that
  makes the API listener serve h2c (test builds get it from dev-dependencies
  too, so no test can see production lose it); tests pin that the API
  listener serves an h2c prior-knowledge request and the metrics listener
  refuses one. A dependency bump that flips either surface fails CI instead
  of passing unnoticed
- CI fails when the Dockerfile's Rust image falls below `rust-version`, the
  gap that left the image unbuildable from 2026-07-04 (see Fixed)

### Changed

- Replaced three yanked lockfile entries with their successors:
  `chacha20` 0.10.2 (via `rand`), `spin` 0.9.9 (via the iggy SDK) and
  `num-bigint` 0.4.8 (built only for tests, via `testcontainers`)
- Dependabot's cargo updates cover transitive dependencies
  (`allow: dependency-type: all`), so the weekly grouped PR doubles as a
  lockfile refresh. Version updates otherwise touch only what `Cargo.toml`
  names, and all four of this release's advisories sat in transitive
  crates: three needed lockfile-only bumps version updates never make, and
  rkyv went only through the direct `rust_decimal` bump. The GitHub
  Advisory Database carried none of the four, so no Dependabot mode would
  have flagged them
- `deny.toml` fails on yanked crates (`yanked = "deny"`) and on unsound
  advisories in any crate (`unsound = "all"`). cargo-deny's defaults only
  warned on the three yanked crates replaced above and never reported the
  transitive event-listener unsoundness at all. Run against the pre-fix
  lockfile, the stricter policy turns the unsoundness and all three yanked
  crates into errors. The yanked check reads cargo's local index cache, so
  the Dependency Policy job no longer restores one (an entry fetched before
  a yank would hide it), and an unreadable entry now fails the check
  (`-D index-failure`) instead of warning
- Every dependency-resolving cargo call in CI, the extended tests, the
  release build and the Dockerfile passes `--locked`. In CI's gating jobs
  and the release build, a `Cargo.lock` that does not match `Cargo.toml`
  now fails the job instead of being silently re-resolved on the runner
  (while `cargo audit` would still scan the committed file). In the pr.yml
  and extended-tests.yml steps that mask failures by design, it only stops
  the re-resolve
- The Security Audit job pins `rustsec/audit-check` to its Node 24 commit on
  `main`, which takes it out of that job's Node 20 deprecation annotation
  (`actions/checkout@v4` still triggers it until Dependabot bumps checkout).
  Upstream has cut no release since v2.0.0; the bundled action code is
  byte-identical

### Fixed

- Dependabot never ran: no run or PR appeared for either ecosystem after
  `.github/dependabot.yml` landed on 2025-12-01. The file fails schema
  validation on an ignore rule using the nonexistent
  `version-update:semver-prerelease` and on the `reviewers` option GitHub
  retired in 2025. Two settings that would have misfired once it ran are
  fixed too: the cargo
  commit prefix `deps(cargo)`, a type PR Checks rejects, is now
  `chore(deps)`; and custom labels that do not exist in the repository,
  which Dependabot silently drops, gave way to its auto-created defaults
- `SECURITY.md` listed only 0.1.x as supported; it now names 0.4.x and
  describes what each audit gate actually covers
- The Docker image could not be built since 2026-07-04: its builder stage
  used `rust:1.91.1` after `rust-version` rose to 1.93.0, and cargo refuses
  to build below the MSRV, so the compose quick start's `app` service
  failed at build time. The builder now uses `rust:1.98.1`, the current
  stable (the release binaries build on the floating stable channel), and
  builds with `--locked`
- A push to `main` during the Monday scheduled CI run would have cancelled
  it, since both shared one concurrency group, and with it that week's
  audit issue filing (only the scheduled run files issues); none of the 28
  scheduled runs so far was actually cancelled. CI's concurrency group now
  includes the event, so scheduled and push runs cannot cancel each other

### Security

- Bumped transitive `h2` 0.4.15 -> 0.4.19 (lockfile-only) to patch
  RUSTSEC-2026-0258: empty DATA frames were queued without limit, so a peer
  could grow memory on a stream that was not being drained, or overflow a
  length and panic, which this crate's `panic = "abort"` release profile
  turns into a process exit. Reachable here
  even though axum's `http2` feature is off: `metrics-exporter-prometheus`
  enables `hyper-util/server-auto`, which compiles HTTP/2 into the builder
  `axum::serve` uses, so the API listener accepts cleartext HTTP/2 (h2c)
  from any client that can reach it. Whether to keep that surface is
  tracked as TD-2026-09-01.
- Bumped transitive `rustls` 0.23.41 -> 0.23.45 (lockfile-only, with the
  `aws-lc-rs`, `aws-lc-sys` and `rustls-webpki` bumps it requires) to patch
  RUSTSEC-2026-0285: TLS 1.3 handshake messages were accepted at the wrong
  encryption level when packed into the same record as a key change. The
  transcript stays authenticated, so a peer cannot alter a handshake with
  it. Client-side only here: the service terminates no TLS, and rustls runs
  only on the Iggy connection when `IGGY_CONNECTION_STRING` selects TLS: a
  TLS-enabled transport, or `iggy+quic://`, which always uses it.
- Bumped transitive `event-listener` 5.4.1 -> 5.4.2 (lockfile-only) to patch
  RUSTSEC-2026-0221, an unsoundness: `StackSlot` was `Send + Sync`
  unconditionally, so a `!Send` tag set with `Event::with_tag` could cross
  threads through a `listener!` slot. Pulled in only under the iggy SDK:
  directly by `async-broadcast`, and by `iggy_common`'s `moka`, both
  directly and through `async-lock`. Not reachable: no crate in
  the graph calls `Event::with_tag`, so every event carries the default,
  `Send`, unit tag.
- Removed `rkyv` 0.7.46 from `Cargo.lock` (RUSTSEC-2026-0235: out-of-bounds
  reads through shared-pointer validation; only 0.8.17+ is patched) by
  raising `rust_decimal` to 1.43. It was never compiled: `rust_decimal`
  1.42 names it through the weak `rkyv?/std` feature, which pins an optional
  dependency in the lockfile without activating it. That is why
  `cargo deny` (resolved graph) stayed quiet while `cargo audit` (lockfile
  scan) flagged it. `rust_decimal` 1.43 drops the rkyv 0.7 bridge, taking
  `rkyv` and 13 crates that were in the lockfile only because of it.

## [0.4.0] - 2026-08-01

Session-03 tech-debt sweep: TD-2026-07-09 resolved. Plan review ran three
rounds before any code was written (25 agents; artifacts under
`docs/code-reviews/session-03-plan-round{1,2,3}.md`), which is why
TD-2026-07-08 moved to session 04 — round 2 established the two records were
mis-sequenced rather than merely mis-specified.

### Added

- Ownership of half-open circuit-breaker probe tokens. `admit()` returns a
  `ProbePermit` that returns its token on drop and is consumed when the
  outcome is recorded, closing the three accounting leaks TD-2026-07-09
  named: a request admitted while closed can no longer release a token it
  never took, a token from an expired probe window is discarded instead of
  credited to the live one, and a request future dropped mid-probe — a client
  disconnecting during an outage — returns its token instead of stranding it
  until the re-grant window
- `iggy_circuit_breaker_probe_dispositions_total{disposition}` — how probe
  tokens end, as `consumed` / `released` / `stale` / `abandoned` /
  `inconsistent`. The labels partition every admitted token, which is what
  makes `consumed` usable as a denominator; an abandoned-only counter could not
  distinguish a healthy system from a dead release path. `inconsistent` is
  separate on purpose — it means the token accounting is wrong, and it must not
  hide inside the routine `abandoned` volume

### Changed

- Circuit-breaker state is an enum whose variants own their own data, so a
  field belonging to another state is unrepresentable. Deletes two `Option`s,
  a window guard, a six-field hygiene reset and three defensive resets that
  were previously maintained by convention at each mutation site
- The breaker's state is guarded by `std::sync::Mutex` and its methods are
  synchronous. Not a preference: `Drop` cannot await, so a blocking guard is
  what makes the probe permit's release possible at all. Also removes the
  read-lock fast path and the read-to-write upgrade race it required
- Tracing and monotonic counters moved out of the breaker's critical section.
  The Prometheus state gauge deliberately stays inside it: it is
  last-writer-wins, and emitting it after releasing the guard would let racing
  transitions leave it permanently disagreeing with the breaker
- **Breaking**: `CircuitBreaker`, `CircuitBreakerConfig` and `CircuitState`
  are crate-internal, and `IggyClientWrapper`'s `circuit_breaker_state`,
  `circuit_breaker_metrics` and `force_close_circuit` accessors are removed.
  All had zero callers; narrowing the surface is what keeps the rest of this
  release non-breaking
- CI accepts the Conventional Commits `!` breaking-change marker, which its
  regex previously rejected outright

### Fixed

- `Config::validate` rejects a zero `CIRCUIT_BREAKER_OPEN_DURATION_SECS`, which
  disabled the breaker entirely — Open never rejected, and every admission past
  the budget re-granted — and a zero `OPERATION_TIMEOUT_SECS`, which opened the
  circuit on a healthy service and never closed it
- The 503 body for a rejected request now names the state that actually
  rejected. It previously re-read the breaker after the fact and could report a
  state a concurrent transition had already moved past

### Security

- Bumped transitive `crossbeam-epoch` 0.9.18 -> 0.9.20 (lockfile-only) to
  patch RUSTSEC-2026-0204: invalid pointer dereference in the
  `fmt::Pointer`/`Display` impls for `Atomic`/`Shared` on null pointers.
  Pulled in via the iggy SDK and `metrics-exporter-prometheus`; not used
  directly by this crate.

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

[Unreleased]: https://github.com/mlevkov/iggy_sample/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/mlevkov/iggy_sample/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/mlevkov/iggy_sample/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/mlevkov/iggy_sample/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/mlevkov/iggy_sample/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/mlevkov/iggy_sample/releases/tag/v0.1.0
