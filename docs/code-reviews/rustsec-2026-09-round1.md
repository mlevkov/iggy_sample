# RustSec 2026-09 Remediation — Code Review, Round 1

**Target:** branch `fix/rustsec-2026-09-advisories` vs `main`, 10 commits at
review time: one lockfile commit per advisory (RUSTSEC-2026-0258 h2,
-0285 rustls, -0221 event-listener, -0235 rkyv), the yanked-crate
replacements, the Dependabot config repair, the audit-check Node 24 pin,
SECURITY.md, TD-2026-09-01 and the changelog. Four of those commit messages
were later corrected in place (T3, T11); every tree stayed byte-identical,
so commits are cited by subject, not SHA.

**Provenance:** config: the session ran Opus-class at maximum effort with
`feature-dev` and `pr-review-toolkit` installed; the four review agents were
pinned to the strongest available model. Cadence: tier-graduated (CLAUDE.md
records none), so a multi-file change got four lenses. The type-design,
simplification, architecture and test-coverage lenses were not run; the
change touches no source code for them to grip. Fallbacks, both surfaced:
[reviewer] has no shell in its agent type, so it could not read the bodies of
commits 1–9, which [comments] covered line by line; and the step-4.5
verification agent stalled after confirming one claim, so the parent session
re-verified every finding directly against its source (evidence per theme).
Findings: 25 kept / 1 discarded (see the false positive below), counted over
the 26 claims the verification pass checked (19 consolidated findings plus 7
follow-up items), which this artifact regroups into 13 themes.

Reviewers cited in brackets: [reviewer] feature-dev:code-reviewer, [silent]
silent-failure-hunter, [comments] comment-analyzer, [consistency]
general-purpose (cross-cutting consistency).

Maintainer decisions taken on the findings: Dependabot covers transitive
crates; CI passes `--locked`; deny.toml sets `yanked = "deny"` and
`unsound = "all"`; TD-2026-09-01 gets tripwire tests and stays open; v0.4.1 is
prepared in this PR; the Docker builder moves to rust 1.98.1 while the MSRV
stays 1.93.0 for this release.

---

## T1 — HIGH — No Dependabot mode could have caught these advisories, and the record said otherwise

*[silent] [comments] [consistency] — 3 of 4.*

The Dependabot commit said the invalid config was "why the advisories fixed
on this branch surfaced only as scheduled audit issues". But version updates
cover only what `Cargo.toml` names, and all four sat in transitive crates:
three needed lockfile-only bumps, and rkyv went only through the direct
`rust_decimal` bump. Security updates were no fallback: they are disabled here, and the
GitHub Advisory Database carries none of the four. `GET
/advisories/GHSA-q83h-524g-xf6h` (h2) and `GHSA-2mjx-qc3c-rqvc` (rustls)
return 404; neither of rkyv's two GHSAs covers 0.7.46 (GHSA-vfvv-c25p-m7mm is
a different bug in 0.8.0–0.8.15, GHSA-w5cr-frph-hw7f covers < 0.6.0);
event-listener has none. The repository's 26
Dependabot alerts are all `fixed`, none open.

**Remediated:** the cargo block gains `allow: dependency-type: all`
("ci: let dependabot refresh transitive cargo dependencies"); the commit
message was corrected before push; SECURITY.md says why Dependabot alerts are
not relied on for Rust crates.

## T2 — HIGH — Nothing in CI enforced the lockfile the branch edited by hand

*[silent] [reviewer] — 2 of 4.*

No workflow passed `--locked`, so an inconsistent `Cargo.lock` would be
re-resolved on the runner and pass, while `cargo audit` scanned the committed
file. This branch restores 12 lockfile entries by hand, which makes the
committed file's validity load-bearing.

**Remediated:** "ci: fail on a stale Cargo.lock with --locked" — every
resolving cargo call in all four workflows (and, later, the Dockerfile).
Negative-tested: with a manifest its lockfile cannot satisfy, `cargo build
--locked` and `cargo deny --locked check` both refuse.

## T3 — MEDIUM — The lockfile-restoration notes overclaimed one step and undercounted another

*[comments] [silent].*

The rustls commit said the update "re-picked the windows-sys edges of the
same 12 unchanged packages"; replaying `cargo update -p rustls` on its parent
reproduces the committed lockfile exactly. The rebuild script had reported 12
restorations at every step because its input for later steps was the old
chain's lockfile, which already carried h2's re-picks. The h2 commit said
Windows would compile "one more" windows-sys; measured on the raw `cargo
update -p h2` output with `--no-dedupe`, it is two more (0.52.0 and 0.59.0),
and cargo deny's duplicate warnings would rise from 15 to 18.

**Remediated:** both messages corrected before push. Replays confirm only the
h2 step needed restoring; the other four lockfile commits are exactly what
`cargo update -p …` produces.

## T4 — MEDIUM — The audit-check pin does not clear its job's Node 20 annotation

*[comments] [reviewer] [silent] — 3 of 4.*

The runner annotates per job, listing every Node 20 action; the Security
Audit job still runs `actions/checkout@v4` (`using: node20`). The commit
message was careful; the CHANGELOG entry was not.

**Remediated:** "docs(changelog): correct entries flagged by review".

## T5 — MEDIUM — SECURITY.md understated what audit-check gates

*[consistency] [silent].*

At the pinned commit, `reportCheck` (every non-`schedule` event) fails the job
when there is at least one vulnerability (`reporter.ts:214`, `:221`), so
cargo-audit gates pushes and PRs through `CI Success`, including for crates
that are only in the lockfile. Only the scheduled run files issues; it never
files one for a yanked crate (`:296-300`) and skips any advisory ID already in
an issue or PR title in any state (`:239-242`).

**Remediated:** "docs(security): describe the gates as this branch leaves
them".

## T6 — MEDIUM — deny.toml let both classes this branch fixed by hand pass

*[silent].*

cargo-deny defaults `unsound` to `workspace` and `yanked` to `warn`, so the
transitive event-listener unsoundness never reached the gate and three
yanked crates accumulated as warnings.

**Remediated:** "chore(deps): deny yanked crates and transitive unsound
advisories". Run against main's lockfile, the new policy reports
event-listener and the three yanked crates as errors beside h2 and rustls;
the branch passes. CI's pinned installer resolves cargo-deny 0.19.9, which
supports the `unsound` field (added in 0.19.0).

## T7 — MEDIUM — TD-2026-09-01's binding trigger could not fire on its own

*[silent]; wording [comments] [consistency].*

No test exercised HTTP/2, and `hyper-util` is transitive, so its bumps arrive
unnamed inside Dependabot's grouped PR. Separately, the record said the docs
"implied HTTP/1.1 only" (no doc mentions an HTTP version) and cited a
nonexistent "deployment guide".

**Remediated:** "test: pin both listeners' h2c behavior for TD-2026-09-01"
(mutation-checked both ways) and "docs(tech-debt): tripwire TD-2026-09-01,
record TD-07-02's fired trigger". A failing tripwire now leads the record's
trigger; the integration harness serves through the same `axum::serve` path
as `main.rs` (`tests/integration_tests.rs:200-207`). Round 2 found that the
same path is not the same surface: test builds enable `hyper-util/http2`
through dev-dependencies, so this test cannot see the production graph lose
it. A CI feature-graph check now covers that (round 2, R1).

## T8 — MEDIUM — Stale docs repeated the story the branch was correcting

*[consistency] [silent].*

README listed `rust_decimal` 1.42 against a 1.43 floor and described
cargo-audit as the CI scanner and cargo-deny as licenses only; CONTRIBUTING
said Rust 1.90+ against a 1.93.0 MSRV and called `cargo deny check` a license
check; deny.toml's header named `cargo deny check advisories licenses`, which
CI stopped running on 2026-07-03; release.yml claimed a manual-approval gate
on an environment with no protection rules; README's sample `/health`
response showed version 0.2.0.

**Remediated:** "docs: correct stale audit, MSRV and workflow references",
the deny.toml commit, and the release prep.

## T9 — MEDIUM — The Docker image had not built since 2026-07-04

*[consistency].*

The builder used `rust:1.91.1` after `rust-version` rose to 1.93.0, and cargo
refuses to build below the MSRV (reproduced locally with rustc 1.92.0: "rustc
1.92.0 is not supported by the following package: iggy_sample@0.4.0 requires
rustc 1.93.0"; the image's 1.91.1 fails the same check). No workflow builds
the Dockerfile.

**Remediated:** "fix(docker): build on rust 1.98.1, above the 1.93.0 MSRV".
The image builds on 1.93.0 and 1.98.1, and the 1.98.1 image, run against
`apache/iggy:0.8.0`, serves `/health` and `/metrics` and its HEALTHCHECK
reports healthy.

## T10 — HIGH — The fixes reach no binary user without a release

*[consistency].*

The v0.4.0 release ships five binaries built from a lockfile with h2 0.4.15
and rustls 0.23.41, and SECURITY.md now promises fixes on the 0.4 line.

**Remediated:** "chore(release): prepare v0.4.1". The tag is pushed after
merge, and before Dependabot's first github-actions PR is merged: that PR
bumps actions that only `release.yml` runs, which no PR CI can exercise.

## T11 — LOW — Accumulated precision fixes

*[reviewer] [comments] [silent] [consistency].*

- The h2 entry omitted the advisory's panic, which `panic = "abort"` turns
  into a process exit [reviewer].
- event-listener comes only from under the iggy SDK; `moka` is
  `iggy_common`'s dependency, not this crate's [comments] [reviewer].
- num-bigint is built only for tests, and production crates reach it only
  through an inactive weak feature [reviewer].
- The `reviewers` removal was scheduled for 2025-05-20 and confirmed on
  2025-08-08, and no GitHub source says a leftover key invalidates the file;
  the invalid `semver-prerelease` value alone does [reviewer] [silent].
- The "Conventional Commits" job the old prefix would have failed is not a
  required check [silent].
- The rkyv commit subject hid the manifest floor raise, and rust_decimal 1.43
  still weak-pins `borsh` and `rand` 0.8 the same way [reviewer] [silent].
- The untagged `# main` audit-check pin, like rust-cache's untagged `# v2`
  SHA, will float to its branch head under Dependabot [consistency] [silent].
- TD-2026-07-02 and TD-2026-07-07 assumed Dependabot had been running
  [consistency].

**Remediated:** commit messages corrected before push; the rest in the
changelog, tech-debt and docs commits. TD-2026-07-02's own trigger turned out
to have fired (iggy 0.11.0, 2026-09-18), now recorded there.

## T12 — LOW — A push to main could cancel the Monday issue-filing run

*[silent].*

`concurrency` was keyed on workflow and ref with `cancel-in-progress`, and
only the scheduled run files audit issues.

**Remediated:** "ci: keep scheduled and push runs from cancelling each
other".

## T13 — LOW — Nothing validates dependabot.yml

*[silent].*

**Deferred:** TD-2026-09-02, binding trigger "the next edit to
`.github/dependabot.yml`". It needs a new third-party tool in CI, which is the
maintainer's call.

## Not adopted

- A daily audit schedule [silent]: the weekly run matches Dependabot's
  cadence, and the PR-time gates already catch a vulnerability at merge.
- A separate Dependabot group for release-only actions [consistency]:
  sequencing the v0.4.1 tag first addresses the risk (T10).
- softprops/action-gh-release declares `node16`, not `node20`
  [consistency]: cosmetic; the runner reports it with the Node 20 set.

## One false positive, recorded

[reviewer] reported that Dependabot classifies a 0.x bump such as axum 0.8 to
0.9 as "minor", so breaking pre-1.0 updates would land in the grouped
minor/patch PR. For cargo, dependabot-core never takes the generic segment
comparison the finding reasoned from: `semver_rules_allow_grouping?`
branches to `cargo_semver_rules_allow_grouping?`
(`group_update_creation.rb:592`), which calls `Cargo::Version.update_type`,
and its `cargo_pre_1_0_type` returns "major" for a 0.y minor increase
(`version.rb:187-188`; both at dependabot-core `main` @ 9743eea,
2026-09-23). Discarded; SECURITY.md's "separate PR per major bump" stands,
now with the pre-1.0 note.

## Verdict

No CRITICAL findings. The advisories were correctly patched and the exposure
analyses held up under independent re-derivation; what round 1 found was the
branch's *records* overclaiming (T1, T3, T4, T5), gates that could still pass
silently (T2, T6, T7, T12), and pre-existing drift the branch sat next to (T8,
T9). All but T13 are remediated; T13 is deferred with a binding trigger.
Round 2 reviews the remediation.
