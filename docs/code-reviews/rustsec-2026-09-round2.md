# RustSec 2026-09 Remediation — Code Review, Round 2

**Target:** branch `fix/rustsec-2026-09-advisories` vs `main`, 22 commits at
review time: the 10 round-1 commits plus the 12 that remediated round 1
(Dependabot transitive coverage, `--locked`, the stricter deny policy, h2c
tripwire tests, the Docker builder, CI concurrency, TD-2026-09-02, the doc
corrections and the v0.4.1 release prep), with the round-1 artifact.

**Provenance:** config as in round 1: same four lenses, agents pinned to the
strongest available model. Round 1's fallback gap is closed: the commit
messages and diffs were exported to files, so every reviewer, including the
shell-less [reviewer], read all 22 commit bodies. Verification: the parent
session re-checked each finding against its source; the load-bearing ones by
experiment (the production-vs-test feature graphs with `cargo tree`, the stale
index cache in an isolated `CARGO_HOME`, the scheduled-run history, the iggy
hold-back with `cargo update -p iggy --dry-run`, cargo-deny 0.19.9's source).
Findings: 40 reported, consolidated into 14 themes; none was discarded, and
one proposed remediation was not adopted (see below). After remediating, one
further agent (silent-failure and prose lenses) checked only the round-2
fixes; see "Post-remediation check".

Reviewers cited in brackets: [reviewer] feature-dev:code-reviewer, [silent]
silent-failure-hunter, [comments] comment-analyzer, [consistency]
general-purpose (cross-cutting consistency).

---

## R1 — HIGH — The API h2c tripwire could not see the change it was written for

*[silent].*

Whether `axum::serve`'s `auto::Builder` serves HTTP/2 is decided by
`hyper-util`'s `http2` feature. In the production graph its only enabler is
metrics-exporter-prometheus (`_hyper-server` → `server-auto`); test builds also
get it from dev-dependencies (reqwest and hyper-rustls `http2`, tonic's
`server` re-enabling `server-auto`). So the exporter dropping `server-auto`
would make production HTTP/1-only while the integration test kept passing.
Verified with `cargo tree -e normal,build,features -i hyper-util` against the
same with `dev`.

**Remediated:** "ci: pin the production h2c surface and the Docker toolchain
floor" checks the production graph in the Dependency Policy job. Mutation:
with the exporter's `http-listener` removed and the lockfile regenerated, the
step fails with "hyper-util/http2 left the production graph" while the test
graph still carries the feature. The test's doc comment, TD-2026-09-01 and the
changelog now say which tripwire sees what.

## R2 — HIGH — The yanked gate read a cache CI kept warm

*[silent].*

cargo-deny reads yank status from cargo's local sparse-index cache, the
Dependency Policy job restored that cache through rust-cache, and `cargo fetch
--locked` does not refresh entries for locked packages. Reproduced in an
isolated `CARGO_HOME`: after flipping chacha20's cached `"yanked":true`, a
fetch made no index update and the check dropped chacha20. An unreadable
entry was also only a warning (`index-failure`).

**Remediated:** "ci: run the yanked check against a fresh index" removes
rust-cache from the job and passes `-D index-failure`; both the code and the
flag exist in cargo-deny 0.19.9, the version CI resolves.

## R3 — MEDIUM — Records asserted GitHub behavior GitHub does not document

*[consistency] [reviewer] [silent] [comments] — 4 of 4.*

The dependabot.yml header, TD-2026-09-02 and its commit said GitHub runs no
updates for an invalid file and reports it only on the Dependabot page; round
1 had already removed the same claim from the changelog for lack of a source.

**Remediated:** "docs: correct the records round 2 flagged" and a reworded
commit message; the records now state the evidence (no run, PR or failing
check while the file was invalid).

## R4 — MEDIUM — The `--locked` coverage was overclaimed

*[consistency] [reviewer] [silent] [comments] — 4 of 4.*

The changelog said every cargo invocation passes `--locked` and fails on a
stale lockfile. `fmt`, `install`, `publish`, `semver-checks` and `miri setup`
do not take it, and several informational pr.yml/extended-tests.yml steps mask
failures by design, which turns a stale lockfile there into a silent skip
[silent]. The commit also undercounted the extended-tests.yml changes
[comments].

**Remediated:** the changelog names dependency-resolving calls and says only
the gating jobs and the release build fail; the commit message was reworded.
Not adopted: making the informational steps fail-closed (see below).

## R5 — MEDIUM — A cancellation was recorded as having happened

*[reviewer] [comments].*

The concurrency fix's changelog entry and commit said a push "cancelled" the
scheduled run. None of the 28 scheduled CI runs was cancelled (13 failure,
15 success); round 1 had correctly said "could".

**Remediated:** both now say "would have".

## R6 — MEDIUM — The Docker toolchain still had no tripwire or updater

*[consistency] [reviewer] [comments] — 3 of 4.*

The Docker fix bumped a pin; nothing builds the image in CI and no Dependabot
ecosystem covers its `FROM` tags, and "the latest stable" in its comment would
stop being true with Rust 1.99.

**Remediated:** the Dependency Policy job fails when the Dockerfile's Rust
image drops below `rust-version` (checked against 1.92.0, 1.93.0, 1.98.1 and an
unparseable tag); the comment is dated. **Deferred:** a `docker` Dependabot
ecosystem and an image build job, added to TD-2026-09-02 because that edit is
its own trigger.

## R7 — MEDIUM — The tripwire oracles accepted unrelated failures

*[silent].*

The metrics side passed on any error, including a timeout or refused
connection; the API side blamed TD-2026-09-01 for any send error.

**Remediated:** "test: make the h2c tripwires tell a protocol change from an
outage". Mutation: aimed at a closed port, the metrics test now fails with
"unrelated reason" where the old assertion passed.

## R8 — MEDIUM — The changelog intro undercounted reachability

*[comments].*

"Four advisories, one of them reachable" contradicted the section's own rustls
entry, which is reachable on a TLS or QUIC Iggy connection.

**Remediated:** the intro separates the public listener (h2), the Iggy
connection (rustls) and the two unreachable advisories.

## R9 — MEDIUM — The yanked gate had no documented escape hatch

*[silent] [reviewer].*

**Remediated:** deny.toml names the `{ crate = "name@version", reason = "…" }`
ignore form for a yanked crate no parent requirement lets cargo update past.

## R10 — LOW — TD-2026-07-02's new section had the wrong mechanism

*[reviewer] [silent] [comments] [consistency].*

`cargo update` holds iggy at 0.10 because of the `^0.10.0` requirement, not
the MSRV (`cargo update -p iggy --dry-run` locks nothing); its Status line
lagged the registry; "0.11 will open as its own PR" was untested.

**Remediated:** corrected, with the six MSRV sites listed for the raise
(ci.yml's `env.MSRV` is read by no step).

## R11 — LOW — Commit-message precision

*[comments] [reviewer].*

Nine messages: the advisory's date versus issue #32's; wording the changelog
had already retracted (moka's provenance, "dev-only"); the extended-tests
undercount; a reproduction quote naming a toolchain the image never used;
"latest stable"; the cancellation; the undocumented GitHub behavior; and two
tripwire messages that implied the test alone could see the production graph.

**Remediated:** reworded in place before push; every tree is byte-identical
(verified per commit), and all messages end in a single newline.

## R12 — LOW — README and architecture drift

*[consistency] [reviewer].*

The SDK line called 0.10 the latest stable; ci.yml's triggers omitted the
weekly run; the test matrix was described as "3 × 3"; the project tree missed
`ip.rs`, `timeout.rs` and `metrics_smoke_test.rs`; architecture.md's test
pyramid said 24/93/20 against the measured 31/194/18.

**Remediated** in the round-2 docs commit.

## R13 — LOW — Rot-prone or imprecise records

*[comments] [silent].*

release.yml's environment comment stated a setting it cannot track;
SECURITY.md said the scheduled audit run "never fails" (it can fail on an API
error, just never on advisories); the round-1 artifact cited
`tests/integration_tests.rs:196-204` for a call at 202-207, left the
dependabot-core citations unpinned, quoted a 1.92.0 reproduction without
saying so, and gave a finding count without saying how it was counted.

**Remediated:** all corrected; the dependabot-core lines are pinned to `main`
@ 9743eea.

## R14 — LOW — The changelog cited artifacts that were not committed

*[consistency] [reviewer] [silent] [comments] — 4 of 4.*

**Remediated:** both artifacts are committed on the branch before merge, so
the v0.4.1 tag's changelog cites files in its own tree.

## Not adopted

- Making the informational pr.yml and extended-tests.yml steps fail-closed
  (pipefail, a valgrind step that fails without a binary) [silent]. They
  mask failures by design, and a stale lockfile cannot reach `main` now that
  the gating jobs fail on it.
- Editing CLAUDE.md's "Latest stable SDK" line [consistency]. It is the
  maintainer's project-instructions file, so it is flagged to them rather
  than changed on a reviewer's suggestion.

## Post-remediation check

One agent, with the silent-failure and prose lenses, reviewed only the round-2
fixes: the four commits after TD-2026-09-02's filing, the nine reworded
messages and the round-1 artifact corrections. All four fixes held under
adversarial checking. It traced `-D index-failure` through cargo-deny 0.19.9's
code path (the override applies per diagnostic, and the yank check is
local-only, via tame-index), and reqwest 0.13.4's `is_timeout`/`is_connect`
classification against hyper dropping the connection on an h2 preface. It
also confirmed the runner image starts with an empty cargo registry.

It found one MEDIUM, self-inflicted: TD-2026-07-02 counted six MSRV sites
while the same commit's README line added a seventh. The literal was removed,
so six stands. It also found five LOWs:

- The Docker floor step false-failed a two-part tag (`rust:1.93`); versions
  are now padded to X.Y.Z.
- The API oracle still blamed the TD on a slow listener; a timeout is now
  reported as such.
- The `--locked` changelog entry over-generalized which steps mask failures.
- "Three of the four sat in transitive crates" was wrong: all four did, and
  three needed lockfile-only bumps.
- CLAUDE.md's "Latest stable SDK" line is flagged to the maintainer.

All are remediated in three commits and two reworded messages. The
ambiguity of `cargo tree -i hyper-util` if a second version arrives is kept
as a loud failure, since matching any version could pass on the wrong one;
the step's comment says what to do.

## Verdict

Round 2 found what the double review exists to find: two round-1 fixes that
moved their holes rather than closing them (R1, R2), both silent and both
described as guarantees. Each is now closed and mutation-checked. The rest
were records overclaiming (R3–R5, R8, R11) and drift (R6, R10, R12, R13). No
CRITICAL findings; R6's updater half is deferred with a binding trigger. The
post-remediation check found the round-2 fixes sound and one miscount of
their own making, now corrected.
