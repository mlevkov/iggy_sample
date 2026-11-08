# Session State

A rolling handoff between working sessions, newest block first. Each
session adds a block at the top and renames the previous `CURRENT` heading
to its date; earlier blocks stay as written. The kickoff phrase at the end
of the current block is what to paste into a new session. Decisions live in
`docs/adr/`, debt in `docs/tech-debt/`, and stable project facts in
`CLAUDE.md`. This file records what was in flight and what not to
re-derive.

---

## CURRENT: State 2, 2026-11-08: v0.5.0 is released

State 1 below still holds, except where this block supersedes it. Its open
item 4, the release, is done, and `main` has moved. Its "Deliberate calls",
"CARRIED" and "DO NOT RE-DERIVE" sections carry forward unchanged. Read
them too.

### What shipped

| PR | Merge | Head | What |
|---|---|---|---|
| #85 | `7adfd66` | `4ed97d0` | This file with State 1; corrected statements in ADR-0002 and ADR-0003 |
| #86 | `d24a269` | `03d5eaa` | v0.5.0: Coverage Diff fix, Codecov upload dropped (TD-2026-09-03), version 0.5.0 |

**v0.5.0 is released.**
- **Tag:** annotated `v0.5.0` on `d24a269`, with the message "v0.5.0: Iggy
  server 0.10 and SDK 0.12, per-topic options, cluster and connectors
  demos". v0.2.0 to v0.4.1 use the same annotated form; v0.1.0 is
  lightweight. I pushed it
  after the user merged #86. The classifier did not block the tag push.
- **Release run 42514350003:** success, all eight jobs (Validate Release,
  five builds, Create Release, Deploy Documentation).
- **GitHub release "Release v0.5.0":** Latest, not a draft or prerelease,
  published 2026-11-08T19:25:37Z, five assets.
- **Pages:** the `github-pages` deployment of `d24a269` (ref `v0.5.0`)
  succeeded at 19:26:53Z.
- **Verify Release run 42515088161** (`workflow_run`, on `d24a269`):
  success, 41 passed, 0 failed, 0 skipped. It is the first stable release
  to pass outright. The verifier's only earlier run on a stable tag,
  36068815000 on v0.4.1, failed one check, as CONTRIBUTING says it must for
  the masked publish step. This run reported "PASS no publish job, as Cargo.toml's
  publish = false wants" and "PASS crates.io does not have iggy_sample
  0.5.0 (publish = false; HTTP 404)".
- **CHANGELOG:** `[Unreleased]` is empty. `[0.5.0] - 2026-11-08` opens
  with a summary of the release.

**Three breaking changes, not two.** State 1 item 4 counted #77 and #79.
The third came from #68: startup refuses `API_KEY` without
`TRUSTED_PROXIES`. The 0.5.0 summary lists all three.

**TD-2026-09-03 is resolved** with option 2, which the user chose.
- ci.yml's Codecov step is gone. No workflow references `codecov-action`
  or `CODECOV_TOKEN` any more; only tech-debt records name them, as
  history.
- Code Coverage runs `cargo llvm-cov --summary-only` and writes the line
  total from the `TOTAL` row to its job summary.

**Coverage Diff had never worked before #86.** Checking the record's
mitigation found it.
- pr.yml grepped for a `Total coverage:` line that cargo-llvm-cov does not
  print, so every PR logged `unknown%` and the warning below 60% could not
  fire.
- It now reads the `TOTAL` row and warns when that row yields no number.
- Commit `a026dd9`'s message says "cargo-llvm-cov 0.6". CI installs the
  latest cargo-llvm-cov on every run (its log says `installing
  cargo-llvm-cov@latest`), which on 2026-11-08 was 0.9.1. 0.6.9 is the
  local version.

**Upstream:** apache/iggy #4784 to #4787 are open, with no comments and no
labels (`gh issue view <n> -R apache/iggy`, checked after the release). The
user says the Iggy maintainer will comment on them later. Post nothing.

### Gate

- #86 passed 24 of 24 checks at `03d5eaa`. CI on `main` at `d24a269`
  passed (run 42514315331).
- **The code has not changed since State 1's figures,** which come from
  the #84 tree.
  `git diff --stat 5b211e6 d24a269 -- src tests connectors scripts docker-compose.yaml docker-compose.cluster.yaml docker-compose.connectors.yaml Cargo.toml Cargo.lock`
  shows the version line in `Cargo.toml` and `Cargo.lock`, plus #85's
  doc-comment edit in `src/iggy_client/seeds.rs`, and no code. So State
  1's local figures still stand. None was re-run.
- **The release binary:** Verify Release ran it on Linux (exit 78 under
  `OPERATION_TIMEOUT_SECS=0`, version logged). A local debug build of the
  `03d5eaa` tree did the same before the tag was pushed.

### THE OPEN WORK

Nothing is due before 2026-11-09 (the blog post's expected date) or
2026-11-15 (the scheduled run). No tech-debt record is bound to the next
release: TD-2026-09-03 and TD-2026-09-04 are resolved. If the user asks for
nothing, ask; do not invent work.

1. **Re-check ADR-0002 against the 0.10.0 release blog post,** as State 1
   item 1 describes. It still answered 404 after the release, with
   apache/iggy-website `main` still at `331d0614`.
2. **Read the 2026-11-15 scheduled Extended Tests run,** as State 1 item 2
   describes. `d24a269` is after `5b211e6`, so a run on it counts.
3. **Upstream #4784 to #4787,** as State 1 item 3 describes. In addition,
   the Iggy maintainer said they will comment. Read their comments before
   anything else, and post, label or reply nothing without the user's OK.
   State 1's command for finding the references, `grep -rln 478[4-7]`,
   fails in zsh ("no matches found"). Use
   `grep -rln '478[4-7]' . --exclude-dir=target --exclude-dir=.git --exclude=Cargo.lock --exclude=SESSION-STATE.md`,
   which lists the eight files State 1 names. Without the exclusions it
   would also match a checksum in `Cargo.lock` and this file's history.
4. **Optional, only on the user's request:** the #81 review notes in State
   1's CARRIED.

### Deliberate calls (in addition to State 1's)

- **The Codecov upload is dropped, not waiting for a token.** Nothing read
  it, and it added a third-party action and a secret to manage. Re-adding
  it is TD-2026-09-03's option 1, and the user's call.
- **Coverage Diff warns and never fails.** It does not set `pipefail`, so
  a failed suite there shows as the "No line coverage total" warning.
  **Code Coverage sets `pipefail`** and fails with the suite, as it did
  before. Neither job is a required check.
- **The release order.** Merge the release PR (the user runs the admin
  merge), then I create the annotated tag on the merge commit and push it,
  then watch Release and Verify Release.

### CARRIED (tracked nowhere else)

- State 1's list, unchanged.
- State 1's kickoff is superseded by the one below.
- **CI's cargo-llvm-cov is not pinned.** `tool: cargo-llvm-cov` in ci.yml
  and pr.yml installs the latest release; only the install action is
  pinned (v2.90.1). Both coverage steps read the `TOTAL` row's tenth field,
  so a release that changes that row's layout trips the "No line coverage
  total" warning. Pinning it (`tool: cargo-llvm-cov@0.9.1`) is a possible
  follow-up, not done and not asked for.
- **A second integration-fixture flake: "Cannot bind to socket".**
  - **What fails:** the Iggy container exits at startup, and the test
    panics with "Failed to start Iggy container"
    (`tests/integration_tests.rs`, `IggyContainer::start`). The server logs
    "failed to start server listeners via replica_io
    replica_addr=0.0.0.0:0 client_addr=0.0.0.0:<port> error=Cannot bind to
    socket with addr: 0.0.0.0:<port>". `<port>` is the client port that
    `find_available_port` picked on the host.
  - **Seen twice:**
    - CI run 42419060930 on `main`, 2026-11-07: port 37485, in the required
      Integration Tests job;
    - #87's CI run 42515908933, attempt 1: port 37621, in Code Coverage.
      Attempt 2 passed.
  - **Cause, confirmed later the same day:** inside the container, the
    server binds its replica listener on port 0 just before the client
    listener on the fixed port. When the kernel gives the replica listener
    the client port, the second bind fails. The probe below reproduces it.
  - **Status: fixed,** at the user's request, by the PR that changed this
    entry: `find_container_port` picks the port from 20000-31999. It is
    distinct from the Docker Hub pull flake in State 1's CARRIED ("bytes
    remaining on stream"), which also struck Code Coverage on run
    42428510742.
- **The Docker Hub pull flake in State 1's CARRIED is fixed too,** at the
  user's request, by the PR that added this entry.
  - **Seen three times,** always on a fresh runner: runs 42463964399
    (Integration Tests, #82), 42428510742 (Code Coverage, `main`) and
    42518178286 (Code Coverage, #88, attempt 1). Each failed one test with
    `PullImage { descriptor: "apache/iggy:0.10.0", ... "bytes remaining on
    stream" }`.
  - **Cause:** testcontainers 0.28 pulls only after a 404 on container
    create, with no retry (probe below). The suite's tests start together,
    so on a runner without the image they all pulled it at once.
  - **Fix:** `ensure_iggy_image` in `tests/integration_tests.rs`. One pull
    per test process, up to three attempts with a 5 s and then 10 s
    backoff, and a shared outcome for the waiting tests. A cached image is
    found with `inspect_image` and not pulled.
  - **Not reproduced locally.** The compose stack was using the local
    image, so it was not removed. Only CI starts without the image.

### DO NOT RE-DERIVE (in addition to State 1's; each probe names its input)

| Finding | Input | Result |
|---|---|---|
| Coverage Diff never read a total before #86 | #85's PR Checks run 42475621793, Coverage Diff job 128716215642, its log | The `TOTAL` row reads 90.11% of lines. The step printed `Current test coverage: unknown%`. It grepped for `Total coverage:`, which the summary does not contain |
| The cargo-llvm-cov CI ran on 2026-11-08 | the same job's log, and #86's jobs 128832500251 and 128833120127 | Each log says `installing cargo-llvm-cov@latest`, then downloads v0.9.1. The action is pinned (v2.90.1); the tool is not. The local one is 0.6.9. The "No line coverage total" warning is the tripwire for a changed `TOTAL` row |
| The fix works | #86's PR Checks run 42513865804 (Coverage Diff job 128832500251), and CI run 42513865807 (Code Coverage job 128833120127) | Coverage Diff printed `Current test coverage: 90.11% of lines`. The Code Coverage log has the `TOTAL` row and no warning. The check-runs API does not return the job summary (`output.summary` is empty), so its "Line coverage: 90.11%" line was inferred from the missing warning |
| The "Cannot bind to socket" flake is a replica/client port collision inside the container | `start_on_shard_zero` in `core/message_bus/src/replica/io.rs` at `91e02063`, which binds the replica listener (line 208) before the client listener (line 209). Then `apache/iggy:0.10.0` (local image ID `sha256:176125a0…`), run locally with the fixture's environment, `--privileged` and `--sysctl net.ipv4.ip_local_port_range="37621 37621"` | With `IGGY_TCP_ADDRESS=0.0.0.0:37621` the container exits with CI's exact line: "failed to start server listeners via replica_io replica_addr=0.0.0.0:0 client_addr=0.0.0.0:37621 error=Cannot bind to socket". With port 20000 and the same range it boots. The container's default range is 32768-60999 |
| Concurrent pulls caused the pull flake, and the fix pulls once | testcontainers 0.28.0 `src/runners/async_runner.rs:343-361` (`start` pulls on a 404 from create, no retry). Then the suite with `ensure_iggy_image`, three ways, each a temporary edit restored afterwards: as written; with the `inspect_image` check skipped and an `eprintln` before the pull; with the pull's tag set to `0.10.0-does-not-exist`, filtered to `test_auth` | As written: 44 of 44, the cached image used. Check skipped: 44 of 44, and the pull ran **once** for the whole suite. Missing tag: 3 attempts in total (two "retrying" lines), and all 4 tests failed with the same "Failed to pull apache/iggy:0.10.0-does-not-exist after 3 attempts", in 18.8 s |
| The 0.5.0 binary passes the verifier's host check | a local debug build of the `03d5eaa` tree, run from an empty directory with `env -i OPERATION_TIMEOUT_SECS=0 RUST_LOG=info` | Exit 78, and "Starting Iggy Sample Application v0.5.0". Verify Release 42515088161 then passed on the release asset, with 0 skipped |

### Kickoff

```text
Continue iggy_sample (/Users/maxim/rust_projects/iggy_sample).
Start with: git switch main && git pull. Then read docs/SESSION-STATE.md:
block "CURRENT: State 2" first, then State 1's "THE OPEN WORK" items
1-3, "Deliberate calls", "CARRIED" and "DO NOT RE-DERIVE", which still
hold. Treat both probe tables as settled; do not re-derive those findings.

State: v0.5.0 is released. The annotated tag is on d24a269, and Release
(42514350003) and Verify Release (42515088161) are green. main is at
d24a269, or at the later docs-only merge that added State 2. Iggy server
0.10.0, SDK 0.12.0, connectors 0.6.0. There is no feature work in flight,
and no tech-debt record is bound to the next release. Nothing is due
before 2026-11-09 or 2026-11-15. If the user asked for nothing, ask; do
not invent work.

Open work, in order:
1. Is the 0.10.0 release blog post published?
     gh api repos/apache/iggy-website/contents/content/blog/release-0.10.0.mdx --jq .name
   It answered 404 on 2026-11-08, also after the release. Its front-matter
   date sets the URL, so do not poll a guessed date. Once it is out,
   compare it with ADR-0002 and its Corrections section. Add anything new
   as a dated Corrections entry; a changed decision needs a new ADR.
2. After 2026-11-15, read the first scheduled Extended Tests run of the
   Cluster Tests and Connectors Tests jobs:
     gh run list --workflow extended-tests.yml --event schedule --limit 1
     gh run view <id> --json headSha,jobs
   Done when both are success on a headSha at or after 5b211e6.
3. apache/iggy #4784-#4787: the Iggy maintainer said they will comment.
   Read their comments first. Do not label them, post on them or reply
   without the user's OK: they were filed as the user. #4784 is fixed
   for us only by a connectors release that carries the fix, or the sinks'
   at_least_once (#4734). Then bump IGGY_COMMIT and RUNTIME_IMAGE together
   in connectors/Dockerfile, go back to one event in the demo script and the
   round-trip test, then run the connectors suite and an Extended Tests
   dispatch. Update the files that this lists (quoted, or zsh refuses it):
     grep -rln '478[4-7]' . --exclude-dir=target --exclude-dir=.git --exclude=Cargo.lock --exclude=SESSION-STATE.md
   Keep #4785's max_topic_size check and #4787's ca-certificates layer
   even once fixed. #4786 has no workaround: update the docs only.
4. Only on request: the #81 review notes in State 1's CARRIED.

Do not re-litigate:
- reconnection retries capped at 3 (ADR-0003). The override is
  ?reconnection_retries=unlimited, or reconnection_max_retries for
  iggy+quic;
- three events in the connectors round trip (the #4784 workaround);
- plugins built from source at 91e02063 onto apache/iggy-connect:0.6.0,
  with ca-certificates added;
- secrets generated by connectors/init.sh, never set in .env;
- dated factual corrections are allowed in ADRs;
- the Codecov upload is dropped (TD-2026-09-03, option 2). Coverage Diff
  warns and never fails; Code Coverage fails with the suite.

What will bite:
- The classifier blocks your `gh pr merge --admin`. Give the user the
  exact command: ! gh pr merge <N> --merge --admin --match-head-commit <sha>
  Pushing a release tag after the user merges was not blocked.
- There is no .env. Before a cluster run:
    echo "IGGY_CLUSTER_SHARED_SECRET=$(openssl rand -hex 32)" >> .env
- Both integration-fixture flakes are fixed (State 2's CARRIED): "bytes
  remaining on stream" while pulling apache/iggy:0.10.0, and "Cannot bind
  to socket". If either happens again, the fix is incomplete: investigate
  it rather than rerunning. A pull that fails after three attempts is
  Docker Hub, not this repo.
- docs/connectors-demo.md describes connectors 0.6.0, which delivers at
  most once. On upstream master, sinks can be at_least_once (#4734).
```

---

## State 1, 2026-11-08: the Iggy 0.10 sequence is complete

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
