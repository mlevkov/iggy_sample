# Session State

A rolling handoff between working sessions, newest block first. Each
session adds a block at the top and renames the previous `CURRENT` heading
to its date; earlier blocks stay as written. The kickoff phrase at the end
of the current block is what to paste into a new session. Decisions live in
`docs/adr/`, debt in `docs/tech-debt/`, and stable project facts in
`CLAUDE.md`. This file records what was in flight and what not to
re-derive.

---

## CURRENT: State 1, 2026-11-08: the Iggy 0.10 sequence is complete

### What shipped

This session (2026-11-07 to 2026-11-08) merged eight PRs. Every merge was
an admin merge the user ran at a verified head.

| PR | Merge | Head | What |
|---|---|---|---|
| #77 | `f093b2a` | `211ec98` | Iggy server 0.10.0 and SDK 0.12.0 (ADR-0002 PR 1b), lane settling |
| #78 | `ffe651c` | `6fc219b` | Stress job: the curl loop that hung on 0.10 removed |
| #79 | `5387522` | `0556010` | Per-topic options on create and in reads, `IGGY_TOPIC_*` (ADR-0001 PR 2) |
| #80 | `39507d4` | `99a3e3f` | Three-node cluster demo, seed list, `GET /cluster`, `replication_factor` (PR 3) |
| #81 | `a801815` | `991f3b7` | Connectors demo: HTTP source and sink, `/sink/inbound`, poll `skipped` (PR 4) |
| #82 | `74a1673` | `bfd099d` | Connectors known issues corrected and linked upstream |
| #83 | `8945269` | `e1fa60a` | ADR-0002 Corrections section, checked against the 0.10 release notes |
| #84 | `5b211e6` | `8d3b4db` | SDK reconnection retries capped at 3 by default (ADR-0003) |

**ADR-0001's PR sequence is finished.** Nothing from ADR-0001 or ADR-0002
remains open.

**Filed upstream on 2026-11-08,** at the user's request, as GitHub user
`mlevkov`. All four are open, with no labels and no comments
(`gh issue view <n> -R apache/iggy`):
- [apache/iggy#4784](https://github.com/apache/iggy/issues/4784): SDK, an
  `OnRead` consumer group stalls next to an empty partition;
- [#4785](https://github.com/apache/iggy/issues/4785): SDK, errors decoded
  from a status code print placeholder values;
- [#4786](https://github.com/apache/iggy/issues/4786): connectors, a
  plugin's `open()` error is never logged;
- [#4787](https://github.com/apache/iggy/issues/4787): connectors, the
  `apache/iggy-connect` image has no CA certificates.

**Local machine:**
- **Removed** at the user's request: the volumes `iggy_sample_iggy_data_0_9`
  and `iggy_sample_iggy_data`.
- **Nothing is running.**
- **Volumes kept:**
  - the compose stack's `iggy_sample_iggy_data_0_10`, `_iggy_secrets`,
    `_connectors_secrets`, `_connectors_state`, `_grafana_data` and
    `_prometheus_data`;
  - the cluster's `iggy-sample-cluster_iggy_node_{0,1,2}_0_10`.

### Gate

Two kinds of figure, measured differently.

**CI:**
- PR #84 passed 24 of 24 checks at head `8d3b4db`.
- `main` at `5b211e6` is that head merged, with no other code change:
  `git diff --stat 8d3b4db 5b211e6 -- src tests Cargo.toml Cargo.lock connectors scripts docker-compose.yaml docker-compose.cluster.yaml docker-compose.connectors.yaml`
  prints nothing. Rerun it to confirm the figures below still stand.
- The PR that adds this file changes one comment in
  `src/iggy_client/seeds.rs` and no code, and CI re-ran the suites on it.

**Local:**

| Suite | Result | When measured |
|---|---|---|
| `cargo test --locked --lib` | 289 passed | re-run at session end |
| `cargo test --locked --test integration_tests` | 43 passed | the #84 tree before commit, the same code |
| `model_tests` | 18 passed | the #84 tree |
| `metrics_smoke_test` | 1 passed | the #84 tree |
| `--doc` | 1 passed, 11 ignored | the #84 tree |
| clippy `-D warnings`, `cargo doc` `-D warnings`, `rustup run 1.95.0 cargo check --all-targets` | clean | the #84 tree |

**Ignored suites,** green locally on 2026-11-08:
- `tests/cluster_tests.rs` (3), against `docker-compose.cluster.yaml`;
- `tests/connectors_tests.rs` (6), against `docker-compose.yaml` plus
  `docker-compose.connectors.yaml`.

**Extended Tests** (`.github/workflows/extended-tests.yml`) runs on Sundays
at 02:00 UTC.
- The 2026-11-08 scheduled run, 42436822960, ran on `17ba85b`, before #77,
  so it ran neither new job.
- Before #84, the new jobs had passed only in dispatch runs: Cluster Tests
  in 42450453665 (at `c08070a`), Connectors Tests in 42459894138 (at
  `991f3b7`). Neither was on the capped-retries code.
- At session end, dispatch run 42474775570 ran both on `main` at `5b211e6`:
  both passed. Cluster Tests ran 11:13 to 11:20 UTC and Connectors Tests
  11:13 to 11:23 UTC, so the capped-retries code has now passed both jobs
  in CI.
- Their first scheduled run on `main` is 2026-11-15.

### THE OPEN WORK

Nothing is due before 2026-11-09 (the blog post's expected date) or
2026-11-15 (the scheduled run). If the user asks for nothing, ask; do not
invent work.

1. **Re-check ADR-0002 against the 0.10.0 release blog post** once it is
   published.
   - **How to tell it is out:**
     `gh api repos/apache/iggy-website/contents/content/blog/release-0.10.0.mdx --jq .name`
     answers 404 until the file exists. It did so on 2026-11-08, when
     apache/iggy-website `main` was at `331d0614`; the local checkout at
     `/Users/maxim/rust_projects/apache/iggy-website` matched it.
   - **The URL is not fixed.** The post's front-matter `date` sets it, as
     `/blogs/YYYY/MM/DD/release-0.10.0`, so do not poll a guessed date. A
     published post answers 301 to the trailing-slash URL.
   - **What to compare it with:** ADR-0002 and its Corrections section,
     which were already checked against the upgrade guide
     (`content/docs/server/upgrading.mdx`) and apache/iggy discussion #4671.
   - **If it adds something:** record it as a dated entry in ADR-0002's
     Corrections (`docs/adr/README.md` allows that). A changed decision
     needs a new ADR.
2. **Read the 2026-11-15 scheduled Extended Tests run,** the first
   scheduled run of Cluster Tests and Connectors Tests.
   - **Find it:**
     `gh run list --workflow extended-tests.yml --event schedule --limit 1`,
     then `gh run view <id> --json headSha,jobs`.
   - **Done when** both jobs are `success` on a `headSha` at or after
     `5b211e6`.
   - **On a failure,** read the job's "Cluster logs" or "Stack logs" step
     first.
3. **Upstream follow-up on #4784 to #4787.**
   - **Do not touch the issues themselves.** Labels are not ours to set.
     They were filed as the user, so post nothing on them without the
     user's OK.
   - **#4784 is fixed for us only through a connectors release.** The stall
     is in the consumer inside the connector runtime image, so bumping
     `iggy` in this repo's `Cargo.toml` changes nothing. Either of two
     releases would do:
     - a connectors release that carries the fix;
     - one that ships the sinks' `at_least_once` guarantee (apache/iggy#4734,
       on master, not in 0.6.0), which commits with `Manual` and avoids the
       stall.

     Then:
     1. bump `IGGY_COMMIT` and `RUNTIME_IMAGE` together in
        `connectors/Dockerfile`;
     2. go back to one event in `scripts/connectors-demo.sh` and
        `test_events_sent_through_the_gateway_come_back_through_the_sink`;
     3. run the connectors suite and an Extended Tests dispatch.
   - **The other three:**

     | Issue | Workaround | When it is fixed |
     |---|---|---|
     | #4785 | the gateway's own `max_topic_size` check in `src/models/topic_options.rs` | it can stay; its message is better |
     | #4786 | none | update the docs only |
     | #4787 | the `ca-certificates` layer in `connectors/Dockerfile` | it is harmless to keep |

   - **Update every reference.** `grep -rln 478[4-7]` finds the issues cited
     in `CHANGELOG.md`, `CLAUDE.md`, `connectors/Dockerfile`,
     `docs/connectors-demo.md` ("Known Issues"),
     `docs/durable-storage-guide.md`, `scripts/connectors-demo.sh`,
     `src/models/topic_options.rs` and `tests/connectors_tests.rs`.
4. **Release v0.5.0: not started, and the user's call.**
   - **What it would carry:** `CHANGELOG.md` `[Unreleased]` holds all of
     #77 to #84, including the breaking changes of #77 (`feat(deps)!`) and
     #79 (`feat(topics)!`). `Cargo.toml` is at 0.4.1, and the last tag is
     `v0.4.1` (`98f14d5`, 2026-09-23).
   - **Before tagging,** CONTRIBUTING's release steps require resolving or
     re-binding every tech-debt record bound to the next release. That is
     TD-2026-09-03, the Codecov token.
   - **The other open records,** TD-2026-07-08, TD-2026-09-01 and
     TD-2026-09-05, have triggers that are not met.
5. **Optional, only on the user's request:** the #81 review notes in
   CARRIED.

### Deliberate calls (do not re-litigate)

- **Reconnection retries are capped at 3** (ADR-0003, #84).
  - **How:** `seeds::expand` adds the transport's option unless the
    connection string sets it.
  - **To restore the SDK default:** `?reconnection_retries=unlimited`, or
    `?reconnection_max_retries=unlimited` for `iggy+quic://`. Only the
    transport's own option name counts. There is deliberately no
    environment variable.
  - **Why:** with `unlimited`, during a full outage each request waited
    until the server returned or the 30 s SDK deadline passed (mean 23.8 s).
    The breaker never opened, because `TransientNotAccepted` is correctly
    not counted.
- **The connectors demo is a compose overlay, not a profile.** It modifies
  `app` and `prometheus`, and `docker-compose.yaml` itself is unchanged.
  CI selects it with
  `COMPOSE_FILE=docker-compose.yaml:docker-compose.connectors.yaml`.
  - **The base configuration did change slightly** (#81). The base
    `prometheus.yml` gained a `scrape_config_files` include, which matches
    nothing without the overlay. Base Grafana provisioning gained a
    connectors dashboard, which shows no data without it.
- **The plugins are built from source** at `91e0206367aad2d30259f7d45203832b08ae84e7`.
  Upstream publishes none. The runtime image's revision label names
  `1fb58a67`, but its binary is release code (see the probes below).
  - **Bumping connectors:** change `IGGY_COMMIT` and the runtime tag
    together.
- **The connector image adds three things:**
  - `ca-certificates`: without them the HTTP sink cannot open (#4787);
  - a non-root user, uid 10001;
  - a healthcheck that passes once both connectors report running on the
    runtime API.
- **Plugin secrets are generated, not configured.**
  - **Generation:** `connectors/init.sh` writes them into the
    `connectors_secrets` and `iggy_secrets` volumes. Files read by one
    service are 0400 and owned by it.
  - **Rendering:** `connectors/entrypoint.sh` substitutes them into TOML
    templates after checking each is lowercase hex of the exact length. The
    runtime's env overrides cannot reach nested plugin fields.
  - **The runtime API (8081) serves those secrets.** It stays unpublished
    and requires `api-key`, which Prometheus sends from a file.
- **The demo and the tests send three events, not one.** This is the #4784
  workaround. Do not "simplify" it back to one event.
- **`/sink/inbound`.**
  - It exists only with `SINK_INBOUND_TOKEN[_FILE]`.
  - The `POST` bearer token is checked in constant time, by a layer on the
    POST route (`post(..).layer(..)` in `routes.rs`), before the body is
    read. The `GET` is guarded only by `API_KEY`, like
    every other read.
  - A batch is kept whole or not at all.
  - Retention is 100 deliveries and 1 MiB of JSON as received; that is not
    a heap bound.
- **The poll API's `skipped` count is additive.** The webhook topic takes
  any body, and the poll API returns only `Event`s. The sender's delivery id
  is kept as a message header, which the poll API does not expose.
- **Seed lists refuse more than one `@`,** and every address must be
  `host:port` before it is logged, so a password cannot leak into the logs
  (#80, `99a3e3f`).
- **The ADR index now allows a dated correction of a factual error** in a
  Corrections section (#84, `d06c328`).
  - **Why:** #83 had done exactly that to ADR-0002 before the rule allowed
    it.
  - **Still open to the user:** if they want strict immutability, move
    ADR-0002's Corrections into a new ADR.
- **The upstream issues were filed without** the "willing to submit a PR"
  checkbox, so as not to commit the user to anything. They can edit the
  issues.
- **Merging.** The Claude Code permission classifier blocks
  `gh pr merge --admin` ("Merge Without Review"), even after "merge N when
  green". Hand the user the exact command:

  ```
  ! gh pr merge <N> --merge --admin --match-head-commit <full head sha>
  ```

  This happened on #77 and #83; the user ran every merge.

### CARRIED (tracked nowhere else)

- **Review notes on #81 that were not applied:**
  - `connectors/init.sh` passes the PAT (`--token`) and the root password
    (`-p`) on the command line, visible only inside the one-shot init
    container. The CLI reads `IGGY_TOKEN` since 0.10 (apache/iggy#4478), so
    the token could move off the command line.
  - `/sink/inbound` checks the content type after the body is buffered.
    Only a caller holding the token can get that far.
  - The base images are pinned by tag, not by digest.
- **Docker Hub pull flake.** The integration suite hit it once, on #82's
  run 42463964399: `test_auth_required_without_api_key` failed with "bytes
  remaining on stream" while pulling `apache/iggy:0.10.0`, and a rerun
  passed. This is not tracked anywhere.
- **Upstream `master` has moved past 0.6.0.** Since #4734, its sinks can be
  set to `at_least_once`, which commits with `Manual` and so avoids #4784.
  `docs/connectors-demo.md` describes 0.6.0, which is at most once. Revisit
  its Delivery Semantics section when bumping connectors.
- **The local cluster's pre-shared key was lost** with the session
  scratchpad. Before the next cluster run, put a new one in `.env`:
  `IGGY_CLUSTER_SHARED_SECRET=$(openssl rand -hex 32)`. Every node reads the
  same variable.
- **`.ai/`** is an untracked directory that predates this session. It is
  not this work's.

### DO NOT RE-DERIVE (each probe names its input)

| Finding | Input | Result |
|---|---|---|
| The `apache/iggy-connect:0.6.0` binary is release code, despite its label | `grep -c -a` on `/usr/local/bin/iggy-connectors` copied from the image (arm64, `sha256:0b5a4c5356723f2c51d16002e9b75a9081a47c289b9e9d3c4b47e84710b5f9cc`), 2026-11-08 | **0 hits** each: four post-`91e02063` strings ("cannot deliver at least once", `iggy_sink_enable_at_least_once`, `iggy_sink_flush`, "stopped before a message it could not deliver"). **1 hit** each: three release strings ("must use durability=persisted", `iggy_source_register_stop_callback`, "Token file does not exist"). The label reads `1fb58a67` |
| The #4784 stall | A standalone SDK 0.12.0 program against `apache/iggy:0.10.0` (single node). Topic: three empty partitions. Consumer: `consumer_group`, `create_consumer_group_if_not_exists`, `PollingStrategy::next()`, `PollOptions { count: 100, commit: OnRead, ..wait(1s) }`. Sends: A to p0 at 1.5 s, B to p0 at 6 s, C to p1 at 9 s | A and B arrived at 9.0 s, C at 10.0 s. A one-partition topic, pre-seeded partitions, and a raw `poll_messages` with `max_wait` 1 s and `OnRead` all returned at once. The instrumented reader puts the cause in `worker.rs` `main_in_flight` and `complete_check`, unchanged on master `689436f` |
| #4787, no CA certificates | `reqwest` 0.13.5 (`json`, `rustls`) `Client::builder().build()`, run inside `apache/iggy-connect:0.6.0` (digest as above), then inside `iggy-sample-connectors:0.6.0` (built 2026-11-08 07:36 UTC from the `connectors/` tree that became `991f3b7`) | In the official image: "builder error", caused by "No CA certificates were loaded from the system". In this repo's image: the client builds. Runtime 0.6.0 with `[state] storage = "http"` exits with "Failed to build the state HTTP client: builder error" |
| #4785, placeholder errors | `IggyError::from_code(n)` with `iggy` 0.12.0, n = 1019, 1009, 2010, 1012, 3005. The count comes from `awk` over `pub enum IggyError` in `iggy_common` 0.12.0's `iggy_error.rs` | "Max topic size: server_default < segment size: 0 B", "Stream with ID: 0 was not found.", and so on. 113 of the 267 variants carry fields: 112 tuple variants and the struct variant `ProducerSendFailed`. The issue first said "112 of the 265", corrected the same day |
| ADR-0003's numbers | The compose base stack, with gateway image `iggy_sample-app` built 2026-11-08 07:10 UTC from the pre-#84 `feat/connectors-demo` tree (the same reconnect code as `8945269`). `docker stop -t 2 iggy-server` at t=5 s, `docker start` at t=65 s, `POST /messages` every 0.5 s (`curl --max-time 40`). One run with no option, so the SDK's `unlimited`; one with `?reconnection_retries=3`. Since #84 a string with no option is capped, so a re-run must set `?reconnection_retries=unlimited` explicitly | **Unlimited:** 116 sends during the outage, mean 23.8 s, max 30 s. Those started more than 30 s before the restart failed with `503 not_accepted` at about 30 s; later ones succeeded once the server was back. The breaker never opened. First success about 4 s after the restart. **Capped at 3:** all 115 sends during the outage answered 503, mean 4.0 s. Of the run's 130 503s, 116 were breaker rejections; the first 14 waited about 30 s for the wrapper's reconnect. First success at t=72 |
| A cluster works with the cap | `docker-compose.cluster.yaml` built from the #84 tree | `cluster_tests` 3 of 3. The failover script passed twice: new leader after 6 s; 41 and 39 sends ok, none failed; slowest 4.6 s and 5.4 s. A follower stopped for 30 s logged "Replica 2 stays down until a keepalive…". It was in sync 2 s after restarting, and 60 sends across its rejoin all answered 201, slowest 0.0 s |
| An HTTP request on a node that is not the primary is relayed, not refused | `core/server/src/http/forward.rs` (module doc) and `core/server/src/http/error.rs` at `91e02063`, read with `gh api repos/apache/iggy/contents/<path>?ref=91e0206367aad2d30259f7d45203832b08ae84e7` | It is re-issued to the primary: one hop, or one more when the contacted node's view lags. A retryable `503 not_accepted` comes back when a relay cannot complete: `forward_busy`, `relay_refused`, server busy, or `not_primary`. A separate 503 answers when no primary is reachable |
| "The HTTP sink README uses `poll_interval`" is **false** | `grep -rn poll_interval core/connectors --include='*.md' --include='*.toml'` at `91e02063` and master `689436f` | Only plugin-level settings (the postgres and influxdb sources, meilisearch's `task_poll_interval`) and the runtime README's note on what replaces it |
| 0.10.0 is official | apache/iggy discussion #4746; `gh release view server-0.10.0 -R apache/iggy` | Five binding +1 votes. Published 2026-11-07T09:32:58Z, not a prerelease |
| What the plugins cost to build | `docker build --target plugins` from scratch, Docker Desktop arm64; Extended Tests run 42459894138 | 1:43.64 locally. The CI job ran 07:42:42 to 07:50:34 UTC, both images included |

### Kickoff

```text
Continue iggy_sample (/Users/maxim/rust_projects/iggy_sample).
Start with: git switch main && git pull. Then read docs/SESSION-STATE.md,
block "CURRENT: State 1", before anything else. Treat its probe table as
settled; do not re-derive those findings.

State: main is at 5b211e6, or at the later docs-only merge that added
SESSION-STATE.md. ADR-0001's sequence is done: PRs #77-#84 merged, Iggy
server 0.10.0, SDK 0.12.0, connectors 0.6.0. There is no feature work in
flight. Nothing is due before 2026-11-09 or 2026-11-15. If the user asked
for nothing, ask; do not invent work.

Open work, in order:
1. Is the 0.10.0 release blog post published?
     gh api repos/apache/iggy-website/contents/content/blog/release-0.10.0.mdx --jq .name
   It answered 404 on 2026-11-08. Its front-matter date sets the URL, so do
   not poll a guessed date. Once it is out, compare it with ADR-0002 and its
   Corrections section. Add anything new as a dated Corrections entry; a
   changed decision needs a new ADR.
2. After 2026-11-15, read the first scheduled Extended Tests run of the
   Cluster Tests and Connectors Tests jobs:
     gh run list --workflow extended-tests.yml --event schedule --limit 1
     gh run view <id> --json headSha,jobs
   Done when both are success on a headSha at or after 5b211e6.
3. Check apache/iggy #4784-#4787 for progress. Do not label them or post on
   them without the user's OK: they were filed as the user. #4784 is fixed
   for us only by a connectors release that carries the fix, or the sinks'
   at_least_once (#4734). Then bump IGGY_COMMIT and RUNTIME_IMAGE together
   in connectors/Dockerfile, go back to one event in the demo script and the
   round-trip test, and update every file that grep -rln 478[4-7] finds.
   #4786 has no workaround: update the docs only.
4. Release v0.5.0 is not started and is the user's call. [Unreleased] holds
   #77-#84, including two breaking changes. Before tagging, resolve
   TD-2026-09-03 (the Codecov token), which is bound to the next release.
5. Only on request: the #81 review notes listed under CARRIED.

Do not re-litigate:
- reconnection retries capped at 3 (ADR-0003). The override is
  ?reconnection_retries=unlimited, or reconnection_max_retries for
  iggy+quic;
- three events in the connectors round trip (the #4784 workaround);
- plugins built from source at 91e02063 onto apache/iggy-connect:0.6.0,
  with ca-certificates added;
- secrets generated by connectors/init.sh, never set in .env;
- dated factual corrections are allowed in ADRs.

What will bite:
- The classifier blocks your `gh pr merge --admin`. Give the user the
  exact command: ! gh pr merge <N> --merge --admin --match-head-commit <sha>
- There is no .env. Before a cluster run:
    echo "IGGY_CLUSTER_SHARED_SECRET=$(openssl rand -hex 32)" >> .env
- If the integration suite fails pulling apache/iggy:0.10.0 ("bytes
  remaining on stream"), rerun it: that is a known Docker Hub flake.
- docs/connectors-demo.md describes connectors 0.6.0, which delivers at
  most once. On upstream master, sinks can be at_least_once (#4734).
```
