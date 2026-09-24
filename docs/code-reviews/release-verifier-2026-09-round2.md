# Release Verifier and Dependabot-Wave TD Records — Code Review, Round 2

**Target:** branch `chore/release-verifier-and-td-records` after round 1's
remediation: 15 commits on `main` (`b7aba82`), the last being
`docs(code-reviews): release verifier review round 1`. Four earlier commit
messages were corrected in place after this round (R2-T14) with every tree
kept byte-identical, so commits are cited by subject, as in round 1.

**Provenance:** the five lenses of round 1, pinned to the strongest
available model (Fable): [reviewer] feature-dev:code-reviewer, [silent]
silent-failure-hunter, [comments] comment-analyzer, [tests]
pr-test-analyzer, [consistency] general-purpose. [reviewer]'s first run died
on an API rate limit before reviewing anything and was relaunched with the
same brief. Findings: 46 (silent 12, comments 11, reviewer 9, consistency 7,
tests 7) plus one informational list, none CRITICAL, grouped into 15 themes.
Round 1's code themes held at their original sites; the fail-open class
recurred in round 1's own new fetches. Verification (step 4.5): one pass over
the 31 consolidated claims (W1-W31) found 29 present, 2 drifted (W13, W19;
corrected in R2-T2 and R2-T7) and 0 absent. The round paused mid-remediation
on 2026-09-23 and resumed in a new session on 2026-09-24, which re-measured
R2-T2's premise before acting on it: it holds only for gh before 2.75, and
W13's correction rested on it too. Because round 1's remediation had opened
new silent paths, two of the lenses, [silent] and [comments] (Fable), then
checked the resumed session's remediation: 15 findings, 13 distinct, none
above MEDIUM; 12 fixed and 1 declined (see "Remediation check").

Evidence for the remediation, measured on the remediated verifier (passed /
failed / skipped). macOS, gh 2.97.0: v0.4.1 38/1/1, v0.3.0 35/1/3,
v0.4.1-ci.1 15/1/4 (its release was deleted after the exercise), v0.2.0 at
attempt 4 32/4/3, at attempts 3 and 1 31/6/3, at attempt 2 26/5/4, and
v0.2.0's older run 20/15/8. Ubuntu 24.04 container (arm64): gh 2.97.0 gives
v0.4.1 38/1/1 (the cross-built aarch64 binary exits 78) and v0.2.0 32/4/3,
gh 2.75.0 v0.2.0 32/4/3, and gh 2.74.2 and 2.45 (Ubuntu's package) exit 2.
The FAIL on each stable release is TD-2026-09-04's masked publish step.
v0.2.0 adds three FAILs for release notes that are not release.yml's: its
body is now its CHANGELOG.md section, and the release's `updated_at` is six
minutes after its `published_at` (00:21:11Z, 00:27:25Z). Its attempts 1 to
3 add their failed Deploy Documentation jobs. Every new FAIL path was also
shown to fail under a mutation, as the remediation commits record.

---

## R2-T1 — HIGH — The remediation's new fetches failed open

*[silent] [reviewer] [tests] — 3 of 5.*

Round 1's new asset-digest comparison treated any artifact-download failure
as a SKIP, and a failed lookup of the live Pages deployment became a SKIP
with a false reason.

**Remediation:** `ci(verify-release): fail closed where round 2 found new
silent paths`. Only an artifact the listing marks expired may skip; another
download failure, or an artifact without the asset in it, FAILs, and a
missing API digest is replaced by hashing the release asset. Failed lookups
of the live deployment and its site URL FAIL with gh's reason.

## R2-T2 — MEDIUM — The documented Pages restore expires after 30 days, and it makes a re-run attempt the verifier had not been checked on

*[consistency] [reviewer] [comments] [tests] — 4 of 5.*

GitHub re-runs jobs only "up to 30 days after [a run's] initial run" (its
documentation), and the last two stable releases came 26 and 53 days after
the one before, so CONTRIBUTING's restore can be unavailable when it is
needed. And the restore, a re-run of one job, produces the run shape the
script's header called unexercised: the reviewers expected its log to lack
the other jobs, so that "no step failed silently" could pass without the
publish job's `##[error]`. Drifted (W13): "log covers every job that ran"
would still FAIL on such a log.

The resumed session measured it on v0.2.0's run 28759754600 before acting,
and the premise holds only for gh before 2.75. Round 1's V44 ("Re-run all
jobs") was wrong for all three later attempts, not only the second: each
re-ran only Deploy Documentation. Each attempt's job list names all 9 jobs,
the 8 carried over under new IDs with their original `started_at`, and its
log archive holds only the jobs it ran. Attempt 2's archive is empty (the
draft of this artifact said it held 6 of 9 jobs; gh printed 5 before
stopping at `log not found` for the re-run job, which never reached a
runner: no steps, no log). The per-job logs API serves a carried-over job's
original log, byte-identical across the four attempts, and gh 2.75 and later
fetch every job the archive lacks from it. gh 2.74.2 prints only Deploy
Documentation for attempt 4: there "no step failed silently" passes without
the publish error, and only the job-count check fails. So the planned
stitching of each job's log from the attempt that ran it was not needed.

The measurement found a second limit. deploy-pages refuses a run that holds
more than one `github-pages` artifact. v0.2.0's attempt 3, its second
re-run, failed with `Multiple artifacts named "github-pages" ... Artifact
count is 2` beside attempt 1's artifact, which upload-pages-artifact keeps
for a day, and attempt 4 succeeded once the earlier ones were gone (the run
lists only attempt 4's, while expired artifacts stay listed).

**Remediation:** `ci(verify-release): verify the run and attempt that
triggered it`. The workflow passes the triggering run and attempt
(`VERIFY_RUN_ID`, `VERIFY_RUN_ATTEMPT`), which the script checks against the
tag's runs and the run's attempts; jobs and log come from that attempt; a
NOTE names the jobs it re-ran; the job-coverage check names the jobs without
a log; and gh before 2.75 exits 2. Mutations: a gh stand-in that drops two
carried-over logs FAILs naming both; a foreign run ID, attempt 5 and a
non-numeric attempt each exit 2. `docs: say when the Pages restore expires
and how it can fail` gives CONTRIBUTING the 30-day limit, the fallback (a
pre-release tag on the stable release's commit), the `Multiple artifacts`
failure and its fix (delete the run's `github-pages` artifacts, re-run
again), and says that a re-run attempt that succeeds runs Verify Release
again. The remediation check's fixes to both followed (see below).

Open, both for the restore after merge to show (see the end): whether an
expired `github-pages` artifact also blocks a re-run, and the re-run
trigger itself, which rests on GitHub's documentation ("The `requested`
activity type does not occur when a workflow is re-run", which exempts no
other type), since Verify Release cannot run before it is on main.

## R2-T3 — MEDIUM — A failed tag lookup skipped with a false reason

*[silent] [reviewer] — 2 of 5.*

**Remediation:** `ci(verify-release): fail closed where round 2 found new
silent paths`. The tag check skips only when origin's fail-closed tag
listing lacks the tag; for a tag that is there, a failed lookup FAILs.

## R2-T4 — MEDIUM — A non-executable binary was skipped

*[silent] — 1 of 5.*

**Remediation:** the same commit. A host binary without its exec bit FAILs
with its archive mode.

## R2-T5 — MEDIUM — The workflow cannot run before merge, and its first run is red by design

*[tests] [silent] — 2 of 5.*

`workflow_run` uses the default branch's workflow file, so nothing could
exercise it before merge, and its first run would be the next release, red
until TD-2026-09-04 is resolved.

**Remediation:** `ci: let Verify Release run by hand and keep the token off
disk` adds a `workflow_dispatch` with a tag input, and the run's summary
page carries the script's output. After merge, a run for v0.4.1 on the
Linux runner should give 38 passed, 1 failed (the masked publish step) and 1
skipped, the x86_64 Linux binary exiting 78. The permanent red, and the
habituation it invites, ends only when TD-2026-09-04 is resolved: the
maintainer's decision.

## R2-T6 — MEDIUM — A release.yml comment said the fallback line is matched exactly

*[comments] — 1 of 5.*

The negated grep for the fallback's line would pass on any rewording of it.

**Remediation:** the check went in `ci(verify-release): fail closed where
round 2 found new silent paths`, since the positive "Generating changelog
since" line already rules the fallback out; the comment says so since `ci:
correct the verifier's changelog comment and mark two more sites`.

## R2-T7 — MEDIUM — Round 1's artifact over-claimed

*[comments] [consistency] — 2 of 5.*

The exercise's Pages docs are not "identical" to v0.4.1's: 25 files differ
(21 HTML, 4 JS; drifted, W19, from "25 HTML files"), all from the
Dependabot merges between `98f14d5` and `b7aba82` (one JS file gains a
method block from a bumped dependency), and a `.lock` exists only in
v0.4.1's artifact, because the upload-pages-artifact v5 that PR #37 brought
leaves out hidden files (found by the remediation check). So round 1's
reason to decline restoring v0.4.1's docs was false. And gh's local-tag
deletion is at `delete.go` lines 105-107 on trunk but 106-108 in v2.97.0,
the version used.

**Remediation:** `docs(code-reviews): correct round 1 where round 2 found it
wrong` corrects both in place, marked as round 2's, along with T17's V44
(R2-T2). The restore is reopened: proposed after merge (see the end).

## R2-T8 — LOW — Misattributed fail-closed reasons, and unbounded curls

*[silent] [tests] — 2 of 5.*

A failed `gh release list`, deployments lookup or Pages API call FAILed as a
defect of the release ("Latest is none", "none recorded", a served-docs
fetch of an empty URL), with gh's error left on unlabelled stderr.

**Remediation:** `ci(verify-release): fail closed where round 2 found new
silent paths`: each of those fetches FAILs as a fetch, with gh's reason in
the line, and the site URL comes from the deployment's status instead of a
separate Pages API call. curl calls are time-bounded, and the served-docs
fetches retry for the CDN.

## R2-T9 — LOW — "No step failed silently" passed on empty output

*[silent] [reviewer] — 2 of 5.*

**Remediation:** the same commit. A log with no step output left after
filtering FAILs and skips the step-output checks.

## R2-T10 — LOW — perl's scalar `exec` on a one-element list

*[reviewer] — 1 of 5.*

With one argument, `exec @ARGV` re-parsed the binary's path through a shell.

**Remediation:** the same commit uses `exec { $ARGV[0] } @ARGV`. A space in
the path gave exit 127 before and 0 after.

## R2-T11 — LOW — A missing architecture pattern, no-unzip bookkeeping, and absent GitHub notes

*[silent] [tests] — 2 of 5.*

**Remediation:** the same commit. A target without an architecture pattern
FAILs, a missing unzip skips both checks it blocks by name, and a release
without GitHub's generated notes FAILs.

## R2-T12 — LOW — The workflow verified the newest run, not the one that triggered it, and kept the token beside the executed binary

*[reviewer] — 1 of 5.*

**Remediation:** the token in `ci: let Verify Release run by hand and keep
the token off disk` (`persist-credentials: false`); the run, and its
attempt, in `ci(verify-release): verify the run and attempt that triggered
it` (R2-T2).

## R2-T13 — LOW — Prose precision

*[consistency] [comments] [silent] — 3 of 5.*

The README gave release.yml's trigger as `vX.Y.Z` only and did not list
shellcheck, nor did ci.yml's header; Cargo.toml's comment said `publish =
false` makes the publish job fail, when the missing token fails it first;
TD-2026-09-05 said rust-cache runs in "most CI jobs", and nothing at its
sites pointed to it; and nothing marked release.yml's `name: Release`, which
Verify Release's trigger matches.

**Remediation:** `ci: correct the verifier's changelog comment and mark two
more sites` (the `name: Release` marker, pointers at the first rust-cache pin
in each workflow, TD-2026-09-05's five jobs, ci.yml's header) and `docs: say
how the publish job fails, and name both tag patterns` (Cargo.toml, README).
The script's header and its `.env` comment were corrected in
`ci(verify-release): fail closed where round 2 found new silent paths`.

## R2-T14 — LOW — Commit messages

*[comments] — 1 of 5.*

- `ci: lint shell scripts with shellcheck` said the job runs "on every push
  and pull request"; it runs in every CI run, whose triggers are narrower.
- `docs(tech-debt): record why Dependabot proposes no iggy 0.11` said only
  `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=allow` moves iggy to 0.11.0;
  that holds under the loosened requirement (an exact one resolves too).
- `ci: add a script that verifies a release run step by step` said a
  leftover tag truncates "the next release's notes"; it truncates the
  changelog release.yml writes (round 1's T13 made the same correction in
  CONTRIBUTING).
- `ci: mark the lines the verifier and two TD records depend on` said the
  verifier matches four echo lines exactly; it required three, and only
  checked the fallback's for absence.

**Remediation:** all four corrected in place, trees byte-identical, pushed
with `--force-with-lease`. The two draft commits of this artifact, made at
the pause and never pushed, were folded into its final commit.

## R2-T15 — LOW — The digest depended on gh's JSON fields, and a failed run kept 320 MB

*[tests] — 1 of 5.*

**Remediation:** `ci(verify-release): fail closed where round 2 found new
silent paths` reads digests from the REST API, and a failed run keeps its
logs and archives but not the extracted binaries (67 MB kept, down from
about 320).

---

## Remediation check

*[silent] 7 LOW; [comments] 2 MEDIUM, 6 LOW. Two pairs overlap, leaving 13.*

- **MEDIUM:** CONTRIBUTING called the re-run that hit `Multiple artifacts`
  v0.2.0's first; it was the second (attempt 3), since attempt 2's re-run
  never reached a runner. And the script's header said gh fetched the
  carried-over logs on attempts 2 to 4; on attempt 2 the fetch aborts at the
  job without a log, so only attempts 3 and 4 show it.
- **LOW, in the script:** the run listing can lag the attempt that
  triggered the workflow, which failed the attempt bounds spuriously; gh
  reuses a run-log archive cached on the machine, and from one with
  per-step files it drops a step the archive lacks without a word; an empty
  list of jobs that ran passed the coverage check as "(0)"; and without a
  job list the coverage check vanished, while "no step failed silently"
  still claimed every job.
- **LOW, in prose:** "never started" for a job that failed in two seconds
  without reaching a runner; "six minutes after the run", which is six
  minutes after publication and before the re-runs; the re-run trigger
  stated as observed fact, and without its success condition; "kept for a
  day", implying that expiry clears the conflict; the `.lock`'s cause; and
  ci.yml's header omitting the dependency-policy job.

**Remediation:** `ci: close the gaps the remediation check found` (the
attempt's own record decides whether it exists; a fresh gh cache for each
run; an empty list FAILs; without a job list the coverage check SKIPs and
the silent-failure check says it covered only the jobs with a log; the
comments) and `docs: correct the restore steps and round 1's attribution`.
The two unpushed commit messages among the findings were corrected before
push. Mutations: a run listing one attempt behind now verifies attempt 4 of
4; a failed job listing gives the SKIP and the qualified PASS; a job list
of only skipped jobs FAILs the coverage check; attempt 5 exits 2 with gh's
404.

**Declined:** the re-ran NOTE misreads a null `run_started_at` (every job
would count as carried over), but it is informational, and a completed
attempt always has one.

## Declined, with reasons

- **CLAUDE.md edits** (its stale "Latest stable SDK" line, a pointer to
  CONTRIBUTING's Releasing section) [consistency]: CLAUDE.md is the
  maintainer's to edit; raised with him.
- **Run the other platforms' binaries** (Rosetta, macOS or Windows legs)
  [tests]: the workflow runs the Linux x86_64 binary; the other four are
  checked by digest, contents and architecture. A further CI leg is a cost
  decision.
- **Fail a stable release while release-bound TD records are open, or add a
  status badge** [silent]: a policy gate beyond the verifier; the badge
  would be red by design until TD-2026-09-04 is resolved.

## After merge

- Restore v0.4.1's docs by re-running its Release run's Deploy
  Documentation job (run 35888419705, within 30 days of 2026-09-23). That
  replaces the exercise's docs (R2-T7), shows whether the expired
  `github-pages` artifact blocks a re-run (R2-T2), and, if the attempt
  succeeds, whether it triggers Verify Release, which would then check
  attempt 2 on its carried-over logs. Expected on the Linux runner: 40
  passed, 1 failed, 0 skipped, since the served docs are then v0.4.1's.
  Otherwise, run Verify Release by hand for v0.4.1 (R2-T5).
- Resolve TD-2026-09-04 before the next stable tag, or Verify Release stays
  red by design.
