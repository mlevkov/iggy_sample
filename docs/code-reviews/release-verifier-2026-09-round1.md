# Release Verifier and Dependabot-Wave TD Records — Code Review, Round 1

**Target:** branch `chore/release-verifier-and-td-records` vs `main`
(`b7aba82`), 6 commits at review time: `scripts/verify-release.sh` with its
CONTRIBUTING section and changelog entry, TD-2026-09-03 to -06, a trigger
check in TD-2026-07-02 and a pointer in TD-2026-07-07. Four of those commit
messages were later corrected in place (T6, T13) with every tree kept
byte-identical, so commits are cited by subject, not SHA.

**Provenance:** config: the session ran Opus 5.5 at maximum effort with
`feature-dev` and `pr-review-toolkit` installed, and the five review agents
and the verification agent were pinned to the strongest available model
(Fable). The skill's config gate covers full-suite runs, so it was not
asked; the attestation is the session's own settings. Cadence:
tier-graduated (CLAUDE.md records none; v0.4.1 used four lenses), so a
multi-file change got five. The type-design, architecture and
simplification lenses were not run: a bash script and prose give them
little to grip. Fallback, surfaced: [reviewer] has no shell in its agent
type, so it got the diff, the commit messages and an evidence sheet as
files. Verification (step 4.5): one agent re-read the source for all 48
consolidated claims: 45 present, 3 drifted (corrected in T7, T10 and T11),
0 absent. The parent session separately re-measured the two claims it had
most reason to doubt (the Codecov recount in T6, and GitHub's merge-queue
availability in T13) and read gh's source for the premise in T8. Findings:
51 from five lenses, all kept; one premise that three of them shared was
false (T8). They are grouped into 17 themes.

Reviewers cited in brackets: [reviewer] feature-dev:code-reviewer, [silent]
silent-failure-hunter, [comments] comment-analyzer, [tests] pr-test-analyzer,
[consistency] general-purpose (cross-cutting consistency).

Evidence for the remediation, measured on the remediated verifier:
macOS (bash 3.2): v0.4.1 39 passed / 1 failed / 1 skipped, v0.4.0 38/1/2,
v0.3.0 36/1/3, v0.4.1-ci.1 16/1/4 (its release was deleted after the
exercise); Linux (Ubuntu 24.04 container, arm64): v0.4.1 39/1/1. The one
FAIL on each stable release is TD-2026-09-04's masked publish step. Every
new check was also shown to fail under a mutation (listed per theme).

---

## T1 — HIGH — Nothing ran the verifier, so the record that relies on it depended on memory

*[silent] — 1 of 5.*

The PR's thesis is that a green run hides failures because nobody looks, and
TD-2026-09-04's trigger said "the verifier fails on the next stable
release". But nothing ran the verifier: no workflow, job or target in the
repository invoked it (verified: V1), only a CONTRIBUTING paragraph pointed
at it. The trigger would fire only if someone remembered.

**Remediation:** `ci: run the release verifier after every green Release
run`. `verify-release.yml` runs it on `workflow_run` after each green
Release run from a tag push, with a read-only token, a 20-minute limit, and
the tag name passed through the environment rather than interpolated into
the script; it checks out the default branch, so the reviewed verifier runs,
never code from the tag. It cannot run before it lands on main, so its first
run is the next release or exercise tag. TD-2026-09-04 now cites it.

## T2 — HIGH — The smoke test ran a downloaded binary with the caller's environment, from the repo root, unbounded

*[reviewer] [silent] [tests] — 3 of 5.*

The verifier executed a binary it had just downloaded from the release under
review with the maintainer's full environment (`GH_TOKEN`, any cloud
credentials), from the repository root, where the app's `dotenvy` loads the
gitignored `.env` (config.rs:168; V6, V7). It had no timeout. And its
"exits before any network I/O" rested on `Config::validate` rejecting a zero
`OPERATION_TIMEOUT_SECS`, which exists only from v0.4.0 (commit `671a27a`;
V8): run against v0.3.0 or v0.2.0, the binary opened a Prometheus listener
on 0.0.0.0:9090 and tried to connect before exiting 69 [tests] [silent].

**Remediation:** `ci(verify-release): run the binary isolated and
time-bounded`. It now runs under `env -i` from an empty directory, is killed
after 30 seconds (its own FAIL), and is not run for versions before 0.4.0.
Checked directly: a hung stand-in exits 142 after the alarm (it survives
`exec`), `/usr/bin/env` sees only the two variables, `/bin/pwd` is the empty
directory, and a missing binary exits 127.

## T3 — HIGH — Four of five assets were checked by name only, and nothing tied an asset to its build

*[tests] [reviewer] — 2 of 5.*

Only the host platform's asset was downloaded and opened (V9). The Windows
zip had never been opened anywhere, and the aarch64-linux binary, built with
`cross` installed from an unpinned git HEAD, was checked by name alone. The
Package steps swallow errors by design (`cp README.md LICENSE* dist/
2>/dev/null || true`), so a dropped LICENSE or a wrong-architecture binary
would pass. Nothing compared a release asset with what its build job
produced either, so an asset replaced after the run passed every check
[reviewer].

**Remediation:** `ci(verify-release): check every asset, the notes, Pages
and crates.io`. Each target's artifact is downloaded and must match the
release asset's sha256 exactly (the API's `digest`, V10), hold its binary,
README.md and a LICENSE, and carry a binary for its target according to
`file`, with patterns that accept both macOS's and libmagic's word order.
All five matched for v0.4.1 on both hosts. Mutations: a corrupted artifact,
a wrong expected architecture and a wrong binary name each FAIL. The Linux
run also executed the cross-built aarch64 binary for the first time: it
reported 0.4.1 and exited 78.

## T4 — HIGH — The run was never tied to the commit its tag names

*[silent] [tests] — 2 of 5.*

The run was selected by tag name, and its head SHA was never compared with
what the tag points at (V17). v0.2.0 already has two runs for one tag (V18).
If a tag were re-pushed and its new run were missing, the verifier would
verify the old commit's run and exit 0.

**Remediation:** `ci(verify-release): tie the run to its tag and fail
closed on fetches`. The tag's commit on origin (peeled, for annotated tags)
must equal the run's head; the check is skipped only once the tag is gone,
as for a deleted exercise tag. The header shows the run attempt and notes
when a tag has several runs. Mutation: selecting v0.2.0's older run
(`826d495`) FAILs the check.

## T5 — HIGH — "Exercise without releasing" omitted the Pages deployment it leaves behind

*[consistency] [tests] — 2 of 5.*

CONTRIBUTING called a pre-release tag a way to exercise release.yml "without
releasing", and the script's header described a pre-release only by the
skipped publish job and the unchanged Latest. But the docs job has no
pre-release guard (release.yml's Deploy Documentation job; V26): the
v0.4.1-ci.1 run deployed `b7aba82`'s docs to Pages, and deleting the tag did
not undo it (V27). It was harmless this time, since those docs match v0.4.1's,
but the documented procedure generalizes to commits with unreleased API
changes.

**Remediation:** `docs: say what exercising release.yml leaves behind`.
CONTRIBUTING lists both things that outlive an exercise, the Pages
deployment (with how to restore it: re-run the latest stable release's
Deploy Documentation job) and the tag, and the script's header and closing
message say the same. Guarding the docs job would stop an exercise from
testing the deploy path, which is what PR #37's exercise needed, so the
workflow is unchanged.

## T6 — HIGH — Miscount: 15 retained runs, not 14

*[comments] — 1 of 5.*

TD-2026-09-03 said 14 main push runs had a retained log and a rejected
upload; there are 15, all rejected. The count had filtered on run
conclusion and dropped 35910387482, the #40 merge's run, which ci.yml's
concurrency group cancelled after its upload step had already run (V28;
the parent session re-counted: 15 of 15). This is the class of error the
project has been burned by before: a count in a permanent record,
asserted from a filtered query.

**Remediation:** `docs(tech-debt): correct the counts and wording review
found` corrects the record, and the commit message that filed it now says
15. The CHANGELOG's matching "every upload has been rejected" was narrowed
before commit to the uploads whose logs are retained, and the record's
title no longer claims the upload "has never authenticated".

## T7 — MEDIUM — Failed fetches left checks to pass on nothing, and exits misreported causes

*[reviewer] [silent] — 2 of 5.*

After a failed log fetch the script continued with an empty log, and
`lacks_regex` succeeded on grep's exit 2 as well as 1 (V12), so three
negated checks, including "no step failed silently", printed PASS over a
log that did not exist. Logs expire after 90 days, so every old release
showed that pattern. The artifacts listing exited 2 mid-run, suppressing
the summary (drifted, V13: only that one of the two cited exits was
mid-run); a failing `gh run list` was reported as "no run found" (V14); an
empty job listing reached the "publish job (not in this run)" SKIP and ran
zero asset checks (V15); and no tool was checked before use (V16).

**Remediation:** `ci(verify-release): tie the run to its tag and fail
closed on fetches`. A failed log fetch FAILs with gh's reason and skips the
step-output checks; negated checks demand a non-empty file; job and
artifact listing failures FAIL instead of exiting mid-run; the run listing's
failure is reported as such; tools are checked up front. The crate name, the
build targets and whether a publish job exists come from Cargo.toml and
release.yml at the run's commit, so a missing build or publish job FAILs
rather than shrinking the check list. Mutations: an expired log (FAIL with
the 410), a broken job listing, a broken run listing (exit 2 with gh's
error).

## T8 — MEDIUM — The changelog base came from this clone's tags

*[consistency] [reviewer] [tests] [silent] — 4 of 5.*

The default base was `git describe` over local tags after a non-pruning
fetch (V19), while release.yml's checkout sees only origin's. From a clone
holding a tag that origin lacks, the verifier computed the wrong base and
produced two false FAILs that blamed the release. [silent] demonstrated it
with a clone-only `v0.3.0-stale.1`.

Premise corrected: [consistency], [reviewer] and [silent] said the
documented cleanup, `gh release delete --cleanup-tag`, leaves the local tag
behind. gh's source deletes it too when no `-R` is given (cli/cli
`pkg/cmd/release/delete/delete.go:105-107`; V20), so the documented in-clone
usage is clean. The gap is narrower: another clone, a deletion from the web
UI, or `-R`. [tests] had it right.

**Remediation:** `ci(verify-release): tie the run to its tag and fail
closed on fetches`. The base is computed excluding tags that exist only in
this clone, which are named in a NOTE. From the stale clone, verifying
v0.4.0 now computes `v0.3.0` instead of two false FAILs.

## T9 — MEDIUM — The release notes and the served docs were never read; Pages was looked up by commit

*[tests] [reviewer] [silent] — 3 of 5.*

The release body, the changelog's end product, was never read (V21), and
release.yml's own count cannot expose an empty changelog. The Pages check
proved a deployment record, not what the site serves (V23), and it looked
the record up by commit, so a release and an exercise sharing a commit could
see each other's record (V22).

**Remediation:** `ci(verify-release): check every asset, the notes, Pages
and crates.io`. The notes must list one entry per commit since the base
(31 for v0.4.1), newest first at the tagged commit, and GitHub's own
"Full Changelog" link must name the same base. The Pages deployment is
found by tag (`ref=`), and while it is the live one the served root
redirect and rustdoc version are checked, with a query string to bypass the
CDN's cache. Mutations: a wrong base FAILs the count and the compare base; a
wrong expected version FAILs the served-docs check.

## T10 — MEDIUM — The digest-mismatch check duplicated two others and missed the message it targeted

*[tests] — 1 of 5.*

`digest.*mismatch|hash mismatch` could not match download-artifact v8's
per-artifact message ("digest validation failed"), which v8 prints only in
its non-default warn and info modes. Drifted (V11): in the default error
mode the failing step's `##[error]` line does match, so the check was
redundant rather than dead; it fired only together with the
job-conclusion and `##[error]` checks.

**Remediation:** removed in `ci(verify-release): check every asset, the
notes, Pages and crates.io`, where T3's asset-against-artifact digest
comparison covers the whole chain.

## T11 — MEDIUM — The publish outcome rested on one log marker

*[silent] — 1 of 5.*

With the Actions API masking the step, a change in GitHub's `##[error]`
marker would silently remove the only signal for TD-2026-09-04, and the
record's "nothing can be published by accident" was asserted, not checked.
The header also listed curl, which nothing called (V38).

**Remediation:** `ci(verify-release): check every asset, the notes, Pages
and crates.io`. crates.io must lack the version while Cargo.toml at the
run's commit says `publish = false`, and have it otherwise, queried with a
User-Agent (crates.io answers 403 without one; drifted, V25). Mutation:
`publish = true` FAILs it. curl is now used, for this and the served docs.

## T12 — MEDIUM — Binding triggers that nothing points at

*[silent] [consistency] — 2 of 5.*

TD-2026-09-03 and TD-2026-09-04 bind to "the next edit" of the Codecov step,
the publish job and `publish = false`, but no comment marked those sites
(V2), while this repo marks TD-2026-09-01's tripwires in ci.yml. Two
records bind to "the next release", and CONTRIBUTING's release steps never
mentioned the registry (V3). TD-2026-09-05's second trigger was an upstream
release nothing here observes (V4).

**Remediation:** `ci: mark the lines the verifier and two TD records depend
on` names the record at each site. CONTRIBUTING's release steps include
resolving records bound to the next release (`docs: say what exercising
release.yml leaves behind`). TD-2026-09-05 binds to the next Dependabot PR
that moves either pin (`docs(tech-debt): correct the counts and wording
review found`).

## T13 — MEDIUM — Record prose that said more, or less, than the evidence

*[comments] [consistency] — 2 of 5.*

- TD-2026-09-06 said Dependabot rebases "only when it conflicts"; its
  documentation also lists scheduled runs, reopening and target-branch
  changes (V29). The observation about #39 stands.
- TD-2026-09-06 left merge-queue availability to be checked; GitHub's
  documentation says merge queues are for organization-owned repositories
  only (V33; the parent session read the sentence on the page).
- TD-2026-07-02 said the ungrouped pass "ended with" `Requirements to unlock
  update_not_possible`; it closes two lines later with `No update possible
  for iggy 0.10.0` (V30). It also did not say that the MSRV raise fires
  TD-2026-09-02 (V35), and its Status line lacked "by hand" (V36).
- TD-2026-09-04 said the verifier fails on an `##[error]` line "under a
  successful job"; it fails on any (V32).
- CONTRIBUTING said a leftover tag drops commits from "the next release's
  notes"; it truncates the changelog release.yml writes, and GitHub appends
  its own notes over its own range (V31).
- TD-2026-07-07's pointer called v2.9.2 (released 2026-08-06) available
  "today" and did not say PR #37 had already moved the pin (V34).
- Commit messages: the parenthetical in the verifier's commit (V46), the
  "ends with" in TD-2026-07-02's (V47) and "rebases only on conflict" in
  TD-2026-09-06's (V48).

**Remediation:** the records in `docs(tech-debt): correct the counts and
wording review found`, CONTRIBUTING in `docs: say what exercising
release.yml leaves behind`, and the four commit messages (with T6's)
corrected in place.

## T14 — MEDIUM — Docs the PR left stale

*[consistency] [reviewer] [comments] [tests] — 4 of 5.*

The README still said "Coverage: Uploaded to Codecov" (V37) while
TD-2026-09-03 recorded that every retained upload was rejected. CONTRIBUTING
said "a `v*` tag" while release.yml triggers only on `vX.Y.Z` and `vX.Y.Z-*`
(V39). SECURITY.md and the README described Dependabot without the MSRV gate
TD-2026-07-02 now documents (V40). The README described release.yml without
the docs deploy and its tree lacked `scripts/` (V41). The release.yml lines
the verifier matches carried no hint of that (V42).

**Remediation:** `docs: say what exercising release.yml leaves behind`
(README, SECURITY.md, CONTRIBUTING) and `ci: mark the lines the verifier
and two TD records depend on` (release.yml).

## T15 — LOW — One FAIL for every hidden failure, and warnings dropped

*[tests] — 1 of 5.*

Every stable release fails the silent-failure check until TD-2026-09-04 is
resolved, and a second hidden failure in another job would have appeared
only as an extra indented line under the same FAIL, with the count
unchanged (V24). `##[warning]` lines were discarded: v0.4.1's run carries
nine Node 20 deprecation warnings.

**Remediation:** `ci(verify-release): check every asset, the notes, Pages
and crates.io`. One FAIL per job with a hidden failure, and warnings are
listed as notes. Mutation: an injected error in a second job adds a FAIL.

## T16 — LOW — Nothing linted scripts/

*[tests] — 1 of 5.*

shellcheck ran only by hand (V43).

**Remediation:** `ci: lint shell scripts with shellcheck`. A Shellcheck job
lints `scripts/*.sh` as bash, and CI Success depends on it.

## T17 — LOW — Unexercised: a run whose jobs come from different attempts

*[tests] — 1 of 5.*

Every recorded run was either a single attempt or "Re-run all jobs" (v0.2.0's
four attempts, V44). After "Re-run failed jobs", the run-level log and job
listing may hold jobs from different attempts, and "log covers every job
that ran" could FAIL spuriously.

**Remediation:** not code. The script's header names the gap, and the next
exercise tag can test it by failing or cancelling one build job and
choosing "Re-run failed jobs".

---

## Declined, with reasons

- **Flip the main ruleset now, or add the merge procedure to CONTRIBUTING**
  [silent]: the ruleset is an admin decision, recorded with its fix in
  TD-2026-09-06, whose trigger (a Dependabot wave) arrives weekly and
  visibly.
- **Give TD-2026-07-02 a date or session bound** [silent]: the repository
  records no session plan for it, so a bound is the maintainer's call.
- **Re-pin rust-cache in this PR** [silent]: this PR files TD-2026-09-05;
  its trigger places the re-pin in the next github-actions PR.
- **Check Cargo.toml's version at the run's commit independently** [tests]:
  the validate job prints the comparison, and the verifier checks that line.
- **Restore v0.4.1's Pages deployment now**: the exercise deployed docs
  identical to v0.4.1's (same crate version, no source change since), so
  restoring changes nothing a reader sees.
