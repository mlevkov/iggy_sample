#!/usr/bin/env bash
# Verify a release.yml run step by step, not just its overall conclusion.
#
# Usage: scripts/verify-release.sh <tag> [<previous-tag>]
#
# Checks the newest release.yml run for <tag>, at its latest attempt, against
# the workflow file and Cargo.toml at the commit it built: that the tag names
# that commit; the job conclusions; what the steps printed; the GitHub
# Release, its notes and its assets; that every asset is byte-identical to its
# build job's artifact and holds a binary for its target; the Pages
# deployment and, while it is the live one, the served docs; that crates.io
# agrees with Cargo.toml's `publish`; and that the host platform's binary
# runs. <previous-tag> is where the changelog should start. By default it is
# what release.yml computes, `git describe --tags --abbrev=0 <tagged
# commit>^`, over the tags on origin. VERIFY_RUN_ID picks another of <tag>'s
# runs instead, and VERIFY_RUN_ATTEMPT an earlier attempt.
#
# .github/workflows/verify-release.yml runs this after every green Release
# run, for the run and attempt that triggered it, and on demand. Until
# TD-2026-09-04 is resolved every stable release gets one FAIL, for its
# crates.io publish step, which fails under continue-on-error.
#
# A tag with a hyphen (v0.4.1-ci.1) is a pre-release: the publish job must be
# skipped and the release must not become Latest. The docs job still deploys
# the tagged commit's docs to Pages, and deleting the tag does not undo that.
# CONTRIBUTING.md ("Releasing") describes exercising release.yml this way.
#
# Re-running failed jobs, or one job (CONTRIBUTING's Pages restore), makes a
# new attempt of the run that lists every job but runs only the chosen ones.
# The others carry over with their conclusions and their original logs, which
# gh 2.75 or later fetches for them (checked on v0.2.0's run 28759754600,
# attempts 3 and 4). A NOTE names the jobs an attempt re-ran.
#
# Requires gh 2.75 or later (authenticated), git, perl, tar and curl; unzip
# and file are used when present. Exits 0 when every check passes (skips
# allowed), 1 when any fails, 2 when it cannot start. The work directory is
# kept on failure. Binaries before v0.4.0 are not executed: they accept a
# zero OPERATION_TIMEOUT_SECS and go on to open network listeners.

set -uo pipefail

usage() {
    echo "usage: $0 <tag> [<previous-tag>]" >&2
    exit 2
}

case ${1:-} in
    -h | --help)
        awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
        exit 0
        ;;
    "") usage ;;
esac
[ $# -le 2 ] || usage

for tool in gh git perl tar curl; do
    command -v "$tool" >/dev/null 2>&1 || { echo "required tool not found: $tool" >&2; exit 2; }
done
PERL=$(command -v perl)

TAG=$1
VERSION=${TAG#v}
BASE=${VERSION%%-*}
case $VERSION in
    *-*) PRERELEASE=true ;;
    *) PRERELEASE=false ;;
esac
TAB=$(printf '\t')

ROOT=$(git rev-parse --show-toplevel) || exit 2
cd "$ROOT" || exit 2
REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner) || exit 2

tmp=${TMPDIR:-/tmp}
WORK=$(mktemp -d "${tmp%/}/verify-release.XXXXXX") || exit 2
failures=0
passes=0
skips=0
cleanup() {
    if [ "$failures" -eq 0 ]; then
        rm -rf "$WORK"
    else
        # Logs and archives stay for inspection; the extracted binaries (over
        # 100 MB each for Linux) do not.
        rm -rf "$WORK"/assets/*/x "$WORK/run"
        echo "work directory kept for inspection: $WORK ($(du -sh "$WORK" 2>/dev/null | cut -f1))"
    fi
}
trap cleanup EXIT

pass() { printf 'PASS  %s\n' "$1"; passes=$((passes + 1)); }
fail() { printf 'FAIL  %s\n' "$1"; failures=$((failures + 1)); }
skip() { printf 'SKIP  %s\n' "$1"; skips=$((skips + 1)); }
note() { printf 'NOTE  %s\n' "$1"; }
# join_lines: stdin's lines as one "a, b, c" line (job names hold spaces).
join_lines() { awk 'NR > 1 { printf ", " } { printf "%s", $0 } END { if (NR) print "" }'; }
# check <description> <command> [args...]: PASS when the command succeeds.
check() {
    local desc=$1
    shift
    if "$@"; then pass "$desc"; else fail "$desc"; fi
}
# Checks read files, never a pipe: under pipefail, `producer | grep -q` fails
# when grep exits early and SIGPIPEs the producer, which turns a negated
# check into a vacuous pass. And no check passes on the absence of text: an
# empty or failed fetch would pass it too, and so would a reworded message.
has_line() { grep -qxF -- "$1" "$2"; }
first_line() { head -n 1 "$1" 2>/dev/null; }
# fetch_has <url> <text> <file>: fetch until the page contains text, three
# tries 10 seconds apart, since a fresh Pages deploy can lag behind the CDN.
fetch_has() {
    local try
    for try in 1 2 3; do
        curl -fsSL --max-time 30 "$1" >"$3" 2>>"$WORK/fetch.err" && grep -qF -- "$2" "$3" && return 0
        [ "$try" -lt 3 ] && sleep 10
    done
    return 1
}
is_prefix_of() { case $2 in "$1"*) [ -n "$1" ] ;; *) false ;; esac; }
# shellcheck disable=SC2016 # perl's variables, not the shell's
sha256() { "$PERL" -MDigest::SHA -e 'print Digest::SHA->new(256)->addfile($ARGV[0])->hexdigest' "$1"; }
# version_lt A B: A < B, comparing X.Y.Z numerically.
# shellcheck disable=SC2016 # perl's variables, not the shell's
version_lt() {
    "$PERL" -e '@a = split /\./, $ARGV[0]; @b = split /\./, $ARGV[1];
        for (0 .. 2) { exit 0 if ($a[$_] // 0) < ($b[$_] // 0); exit 1 if ($a[$_] // 0) > ($b[$_] // 0) }
        exit 1' "$1" "$2"
}

# gh 2.75 is the first to fetch the logs of jobs a re-run carries over.
GH_VERSION=$(gh --version 2>/dev/null | awk 'NR == 1 { print $3 }')
if version_lt "${GH_VERSION:-0}" 2.75.0; then
    echo "gh ${GH_VERSION:-of unknown version} is too old: 2.75 or later is required" >&2
    exit 2
fi

# --------------------------------------------------------------------------
# The run, and the commit it built
# --------------------------------------------------------------------------
runs=$(gh run list --workflow release.yml --branch "$TAG" --limit 20 \
    --json databaseId,headSha,attempt,url \
    -q '.[] | "\(.databaseId) \(.headSha) \(.attempt) \(.url)"') ||
    { echo "listing release.yml runs for $TAG failed" >&2; exit 2; }
[ -n "$runs" ] || { echo "no release.yml run found for $TAG" >&2; exit 2; }
nruns=$(printf '%s\n' "$runs" | wc -l | tr -d ' ')
if [ -n "${VERIFY_RUN_ID:-}" ]; then
    # shellcheck disable=SC2016 # $1 is an awk field
    run=$(printf '%s\n' "$runs" | awk -v id="$VERIFY_RUN_ID" '$1 == id')
    [ -n "$run" ] || { echo "run $VERIFY_RUN_ID is not one of $TAG's release.yml runs" >&2; exit 2; }
else
    run=${runs%%$'\n'*}
fi
read -r RID SHA LATEST URL <<<"$run"
ATTEMPT=${VERIFY_RUN_ATTEMPT:-$LATEST}
case $ATTEMPT in
    '' | *[!0-9]* | 0*) echo "VERIFY_RUN_ATTEMPT is not an attempt number: $ATTEMPT" >&2; exit 2 ;;
esac
# The attempt's own record decides, not the run listing: a newer attempt may
# be running while this one is complete, and the listing can lag behind the
# attempt whose completion triggered the workflow.
attempt_info=$(gh api "repos/$REPO/actions/runs/$RID/attempts/$ATTEMPT" \
    --jq '"\(.status) \(.run_started_at)"' 2>"$WORK/attempt.err") ||
    { echo "reading attempt $ATTEMPT of run $RID failed ($(first_line "$WORK/attempt.err"))" >&2; exit 2; }
read -r STATUS STARTED <<<"$attempt_info"
[ "$STATUS" = completed ] || { echo "run $RID attempt $ATTEMPT is $STATUS; verify it once it completes" >&2; exit 2; }
[ "$ATTEMPT" -gt "$LATEST" ] && LATEST=$ATTEMPT

git fetch --quiet --tags origin || { echo "fetching from origin failed" >&2; exit 2; }
git cat-file -e "$SHA^{commit}" 2>/dev/null ||
    { echo "commit ${SHA:0:7} is not in this clone after fetching" >&2; exit 2; }

# The workflow and manifest the run built are the oracle, not the working tree.
git show "$SHA:.github/workflows/release.yml" >"$WORK/release.yml" 2>/dev/null ||
    { echo "no .github/workflows/release.yml at ${SHA:0:7}" >&2; exit 2; }
git show "$SHA:Cargo.toml" >"$WORK/Cargo.toml" || exit 2
CRATE=$(awk -F'"' '/^name = / { print $2; exit }' "$WORK/Cargo.toml")
[ -n "$CRATE" ] || { echo "cannot read the package name from Cargo.toml at ${SHA:0:7}" >&2; exit 2; }
PUBLISH=$(awk '/^publish = / { print $3; exit }' "$WORK/Cargo.toml")
awk '/^ *- target: / { print $3 }' "$WORK/release.yml" | LC_ALL=C sort >"$WORK/targets.txt"
[ -s "$WORK/targets.txt" ] || { echo "no build targets in release.yml at ${SHA:0:7}" >&2; exit 2; }
if grep -q '^ *name: Publish to crates.io$' "$WORK/release.yml"; then HAS_PUBLISH=true; else HAS_PUBLISH=false; fi

# release.yml's checkout sees only origin's tags, so a tag that exists only in
# this clone (an exercise tag deleted from another clone or the web UI) must
# not become the changelog base here.
git ls-remote --tags --refs origin >"$WORK/remote-tags.raw" ||
    { echo "listing origin's tags failed" >&2; exit 2; }
sed 's|.*refs/tags/||' "$WORK/remote-tags.raw" | LC_ALL=C sort >"$WORK/remote-tags.txt"
git tag | LC_ALL=C sort >"$WORK/local-tags.txt"
LC_ALL=C comm -23 "$WORK/local-tags.txt" "$WORK/remote-tags.txt" >"$WORK/local-only.txt"
if [ -n "${2:-}" ]; then
    PREV=$2
else
    set --
    while IFS= read -r t; do set -- "$@" "--exclude=$t"; done <"$WORK/local-only.txt"
    PREV=$(git describe --tags --abbrev=0 "$@" "$SHA^" 2>/dev/null)
fi
[ -n "$PREV" ] || { echo "cannot determine the previous tag; pass it as the second argument" >&2; exit 2; }

echo "repo $REPO, tag $TAG ($([ "$PRERELEASE" = true ] && echo pre-release || echo stable)), commit ${SHA:0:7}"
echo "run $RID, attempt $ATTEMPT of $LATEST: $URL"
echo "changelog base: $PREV"
[ "$nruns" -gt 1 ] && note "$nruns release.yml runs exist for $TAG; verifying $([ -n "${VERIFY_RUN_ID:-}" ] && echo "the one asked for" || echo "the newest")"
[ "$ATTEMPT" -lt "$LATEST" ] && note "attempt $ATTEMPT is not the latest: the release, its assets and Pages may be a later attempt's"
[ -s "$WORK/local-only.txt" ] && note "ignored tags that exist only in this clone: $(join_lines <"$WORK/local-only.txt")"
echo

# Only a tag missing from origin's (fail-closed) listing may skip; for one
# that is there, the lookup of its commit must succeed.
if ! grep -qxF -- "$TAG" "$WORK/remote-tags.txt"; then
    skip "tag $TAG names the commit the run built (the tag is no longer on origin)"
elif git ls-remote --tags origin "refs/tags/$TAG" "refs/tags/$TAG^{}" >"$WORK/tag.raw" 2>"$WORK/tag.err"; then
    tag_commit=$(awk -v ref="refs/tags/$TAG" '
        $2 == ref "^{}" { peeled = $1 } $2 == ref { plain = $1 }
        END { print (peeled != "" ? peeled : plain) }' "$WORK/tag.raw")
    check "tag $TAG names the commit the run built (${SHA:0:7})" test "$tag_commit" = "$SHA"
else
    fail "read tag $TAG's commit on origin ($(first_line "$WORK/tag.err"))"
fi

# --------------------------------------------------------------------------
# Jobs
# --------------------------------------------------------------------------
jobs_ok=false
if gh api --paginate "repos/$REPO/actions/runs/$RID/attempts/$ATTEMPT/jobs" \
    --jq '.jobs[] | "\(.name)\t\(.conclusion)\t\(.started_at)"' \
    >"$WORK/jobs.tsv" 2>"$WORK/jobs.err" && [ -s "$WORK/jobs.tsv" ]; then
    jobs_ok=true
    # An attempt's jobs that started before it did are carried over from an
    # earlier attempt (under new IDs, with their original start times).
    # shellcheck disable=SC2016 # $1 and $3 are awk fields
    awk -F'\t' -v s="$STARTED" '$3 >= s { print $1 }' "$WORK/jobs.tsv" >"$WORK/reran.txt"
    reran=$(wc -l <"$WORK/reran.txt" | tr -d ' ')
    total=$(wc -l <"$WORK/jobs.tsv" | tr -d ' ')
    [ "$reran" -lt "$total" ] &&
        note "attempt $ATTEMPT re-ran $reran of the $total jobs ($(join_lines <"$WORK/reran.txt")); the others carry over from earlier attempts, with their conclusions and logs"
    awk -F'\t' '$1 ~ /^Build \(/ { t = $1; sub(/^Build \(/, "", t); sub(/\)$/, "", t); print t }' \
        "$WORK/jobs.tsv" | LC_ALL=C sort >"$WORK/built.txt"
    check "one build job per release.yml target ($(wc -l <"$WORK/targets.txt" | tr -d ' '))" \
        cmp -s "$WORK/targets.txt" "$WORK/built.txt"
    # shellcheck disable=SC2016 # $1 and $2 are awk fields
    check "every job except publish succeeded" \
        awk -F'\t' '$1 != "Publish to crates.io" && $2 != "success" { bad = 1 } END { exit bad }' "$WORK/jobs.tsv"
    publish=$(awk -F'\t' '$1 == "Publish to crates.io" { print $2 }' "$WORK/jobs.tsv")
    if [ "$HAS_PUBLISH" = false ]; then
        skip "publish job (release.yml at ${SHA:0:7} has none)"
    elif [ -z "$publish" ]; then
        fail "publish job is in the run"
    elif [ "$PRERELEASE" = true ]; then
        check "publish job skipped for a pre-release" test "$publish" = skipped
    else
        check "publish job ran and reported success" test "$publish" = success
    fi
else
    fail "fetch the run's jobs ($(first_line "$WORK/jobs.err"))"
fi

# --------------------------------------------------------------------------
# What the steps printed
# --------------------------------------------------------------------------
# GitHub echoes each step's script source, inputs and env inside a
# "##[group]Run ..." block, so a grep over the raw log matches what a script
# says (echo "No previous tag found") rather than what it printed. Keep step
# output only, as "<job>\t<message>", without timestamps or colors. gh writes
# a log's escape sequences in caret notation ("^[[1m") when not on a
# terminal, so strip both that and the raw ESC form.
#
# An attempt's log archive holds only the jobs it ran; gh fetches each other
# job's log through the API, which serves a carried-over job's original log.
# A job with no log at all fails the whole fetch (v0.2.0's attempt 2 re-ran
# a job that failed within two seconds and left none). An empty cache
# directory makes gh download the archive rather than reuse a copy cached
# earlier on this machine, whose layout may differ from what CI sees.
XDG_CACHE_HOME=$WORK/gh-cache gh run view "$RID" --attempt "$ATTEMPT" --log \
    >"$WORK/raw.log" 2>"$WORK/raw.err" || : >"$WORK/raw.log"
: >"$WORK/output.log"
if [ -s "$WORK/raw.log" ]; then
    # shellcheck disable=SC2016 # perl's variables, not the shell's
    "$PERL" -ne '
        s/(?:\e|\^\[)\[[0-9;?]*[A-Za-z]//g;
        s/\r//g;
        my ($job, $step, $msg) = split /\t/, $_, 3;
        next unless defined $msg;
        $msg =~ s/^\xEF\xBB\xBF//;
        $msg =~ s/^\S+Z ?//;
        $msg =~ s/\s+$//;
        $skip = 0 if !defined $prev || $job ne $prev;
        $prev = $job;
        if ($msg =~ /^##\[group\]Run /) { $skip = 1; next }
        if ($skip && $msg =~ /^##\[endgroup\]/) { $skip = 0; next }
        print "$job\t$msg\n" unless $skip;
    ' "$WORK/raw.log" >"$WORK/output.log"
fi
if [ ! -s "$WORK/raw.log" ]; then
    fail "fetch the run log ($(first_line "$WORK/raw.err"))"
    skip "step-output checks (no log)"
elif [ ! -s "$WORK/output.log" ]; then
    fail "parse the run log (no step output left after filtering; has its format changed?)"
    skip "step-output checks (no step output)"
else
    # Set when the silent-failure check below cannot vouch for every job.
    partial=
    if [ "$jobs_ok" = true ]; then
        # shellcheck disable=SC2016 # $1 and $2 are awk fields
        awk -F'\t' '$2 != "skipped" { print $1 }' "$WORK/jobs.tsv" | LC_ALL=C sort -u >"$WORK/ran.txt"
        cut -f1 "$WORK/output.log" | LC_ALL=C sort -u >"$WORK/logged.txt"
        unlogged=$(LC_ALL=C comm -23 "$WORK/ran.txt" "$WORK/logged.txt" | join_lines)
        # gh prints no log for a skipped job, so a run in which no job ran
        # never gets here: an empty list means building it failed.
        if [ ! -s "$WORK/ran.txt" ]; then
            fail "log covers every job that ran (no job is listed as having run)"
        else
            check "log covers every job that ran ($(wc -l <"$WORK/ran.txt" | tr -d ' ')${unlogged:+; missing: $unlogged})" \
                test -z "$unlogged"
        fi
        [ -n "$unlogged" ] && partial=" (in the jobs with a log)"
    else
        skip "log covers every job that ran (no job list)"
        partial=" (in the jobs with a log)"
    fi
    check "validate parsed the tag as $VERSION" has_line "Validate Release${TAB}Version: $VERSION" "$WORK/output.log"
    check "validate matched Cargo.toml version $BASE" has_line "Validate Release${TAB}Version verified: $BASE" "$WORK/output.log"
    # This one line also rules out release.yml's fallback to the last 20
    # commits, which prints something else instead. A negated grep for the
    # fallback's message would pass on any rewording of it.
    check "changelog starts at $PREV (not the last-20-commits fallback)" \
        has_line "Create Release${TAB}Generating changelog since $PREV" "$WORK/output.log"
    if [ "$PRERELEASE" = false ]; then
        case $PREV in
            *-*) fail "changelog base $PREV is a pre-release tag (a leftover exercise tag truncates the notes)" ;;
            *) pass "changelog base $PREV is a stable tag" ;;
        esac
    fi

    # A step under continue-on-error fails without failing its job, and the
    # API reports that step's conclusion as success too. Its "##[error]" line
    # is the only trace. One FAIL per job, so a second hidden failure moves
    # the count instead of hiding under the first.
    awk -F'\t' 'index($2, "##[error]") == 1 { print $1 }' "$WORK/output.log" | sort -u >"$WORK/error-jobs.txt"
    if [ -s "$WORK/error-jobs.txt" ]; then
        while IFS= read -r job; do
            fail "no step failed silently in \"$job\":"
            awk -F'\t' -v j="$job" '$1 == j && index($2, "##[error]") == 1 { print "        " $2 }' "$WORK/output.log"
        done <"$WORK/error-jobs.txt"
    else
        pass "no step failed silently$partial"
    fi
    awk -F'\t' 'index($2, "##[warning]") == 1 { print substr($2, 12, 150) }' "$WORK/output.log" |
        sort | uniq -c >"$WORK/warnings.txt"
    if [ -s "$WORK/warnings.txt" ]; then
        note "the run logged warnings (not failures):"
        sed 's/^/        /' "$WORK/warnings.txt"
    fi
fi

# --------------------------------------------------------------------------
# Artifacts
# --------------------------------------------------------------------------
# artifact_field <name> <field#>: a field of the named artifact's line.
artifact_field() { awk -F'\t' -v n="$1" -v f="$2" '$1 == n { print $f; exit }' "$WORK/artifacts.tsv"; }
if gh api --paginate "repos/$REPO/actions/runs/$RID/artifacts" \
    --jq '.artifacts[] | "\(.name)\t\(.expired)"' >"$WORK/artifacts.tsv" 2>"$WORK/artifacts.err"; then
    missing=$(while IFS= read -r t; do [ -n "$(artifact_field "$CRATE-$t" 1)" ] || echo "$t"; done <"$WORK/targets.txt" | join_lines)
    check "a build artifact for every target${missing:+ (missing: $missing)}" test -z "$missing"
    check "Pages artifact uploaded" test -n "$(artifact_field github-pages 1)"
else
    : >"$WORK/artifacts.tsv"
    fail "list the run's artifacts ($(first_line "$WORK/artifacts.err"))"
fi

# --------------------------------------------------------------------------
# The GitHub Release
# --------------------------------------------------------------------------
release_ok=false
if gh release view "$TAG" --json isPrerelease,isDraft,publishedAt \
    --jq '"\(.isPrerelease)\t\(.isDraft)\t\(.publishedAt)"' \
    >"$WORK/release.tsv" 2>"$WORK/release.err"; then
    release_ok=true
    IFS=$TAB read -r is_pre is_draft published <"$WORK/release.tsv"
    pass "release $TAG exists"
    check "release is published, not a draft" test "$is_draft" = false
    check "release pre-release flag is $PRERELEASE" test "$is_pre" = "$PRERELEASE"
    # The REST API carries each asset's digest whatever gh version runs this.
    gh api "repos/$REPO/releases/tags/$TAG" --jq '.assets[] | "\(.name)\t\(.digest // "")"' \
        >"$WORK/assets.tsv" 2>"$WORK/assets.err" || fail "list the release's assets ($(first_line "$WORK/assets.err"))"
    gh release view "$TAG" --json body --jq .body >"$WORK/body.md" || fail "read the release notes"

    # release.yml writes one "- <subject> (<hash>)" line per commit since the
    # changelog base, newest first; GitHub appends its own notes, whose
    # "Full Changelog" link names the base it chose independently.
    expected=$(git rev-list --count "$PREV..$SHA" 2>/dev/null)
    entries=$(grep -cE '^- .* \([0-9a-f]{7,}\)$' "$WORK/body.md")
    check "release notes list the ${expected:-?} commits since $PREV ($entries)" test "$entries" = "${expected:-x}"
    newest=$(grep -m 1 -E '^- .* \([0-9a-f]{7,}\)$' "$WORK/body.md" | sed -E 's/.*\(([0-9a-f]+)\)$/\1/')
    check "release notes start at the tagged commit (${newest:-none})" is_prefix_of "$newest" "$SHA"
    # shellcheck disable=SC2016 # perl's variables, not the shell's
    gh_base=$("$PERL" -ne 'print "$1\n" and exit if m{\*\*Full Changelog\*\*: \S*/compare/(\S+?)\.\.\.\S+}' "$WORK/body.md")
    if [ -n "$gh_base" ]; then
        check "GitHub's notes compare from $PREV too ($gh_base)" test "$gh_base" = "$PREV"
    elif grep -qF -- "**Full Changelog**: " "$WORK/body.md"; then
        skip "GitHub's notes compare from $PREV (their link has no base: a first release)"
    else
        fail "GitHub's generated notes are in the release (no \"Full Changelog\" link)"
    fi
else
    fail "release $TAG exists ($(first_line "$WORK/release.err"))"
fi

latest_is_other_stable() {
    [ -n "$latest" ] && [ "$latest" != "$TAG" ] && case $latest in *-*) false ;; *) true ;; esac
}
if ! latest=$(gh release list --limit 50 --json tagName,isLatest --jq '.[] | select(.isLatest) | .tagName' 2>"$WORK/latest.err"); then
    fail "find the Latest release ($(first_line "$WORK/latest.err"))"
elif [ "$PRERELEASE" = true ]; then
    check "Latest is still a stable release (${latest:-none})" latest_is_other_stable
elif [ "$latest" = "$TAG" ]; then
    pass "release is marked Latest"
elif [ "$release_ok" = true ] && [ -n "$latest" ] &&
    [[ $(gh release view "$latest" --json publishedAt -q .publishedAt) > $published ]]; then
    skip "release is marked Latest (superseded by $latest)"
else
    fail "release is marked Latest (Latest is ${latest:-none})"
fi

# --------------------------------------------------------------------------
# Assets: each is the file its build job uploaded, and holds its target
# --------------------------------------------------------------------------
arch_pattern() {
    case $1 in
        x86_64-unknown-linux-*) echo '^ELF 64-bit .*x86-64' ;;
        aarch64-unknown-linux-*) echo '^ELF 64-bit .*aarch64' ;;
        x86_64-apple-darwin) echo '^Mach-O 64-bit .*x86_64' ;;
        aarch64-apple-darwin) echo '^Mach-O 64-bit .*arm64' ;;
        x86_64-pc-windows-*) echo '^PE32\+ executable.*x86-64' ;;
    esac
}
has_release_files() {  # has_release_files <listing> <binary>
    grep -qxF -- "$2" "$1" && grep -qxF README.md "$1" && grep -q '^LICENSE' "$1"
}
if [ "$release_ok" = true ]; then
    # The loop reads targets on fd 3, so a command in its body that reads
    # stdin cannot swallow the remaining targets.
    while IFS= read -r target <&3; do
        case $target in *-windows-*) ext=zip bin=$CRATE.exe ;; *) ext=tar.gz bin=$CRATE ;; esac
        name=$CRATE-$VERSION-$target.$ext
        # shellcheck disable=SC2016 # $1 and $2 are awk fields
        if ! awk -F'\t' -v n="$name" '$1 == n { found = 1 } END { exit !found }' "$WORK/assets.tsv"; then
            fail "asset $name"
            continue
        fi
        # shellcheck disable=SC2016 # $1 and $2 are awk fields
        digest=$(awk -F'\t' -v n="$name" '$1 == n { print $2 }' "$WORK/assets.tsv")
        dir=$WORK/assets/$target
        file=$dir/$name
        # Only an expired artifact excuses comparing nothing: the release asset
        # is then inspected on its own. Any other download failure, or an
        # artifact without the asset in it, is a FAIL, and an artifact missing
        # from the listing has already failed above.
        expired=$(artifact_field "$CRATE-$target" 2)
        if [ "$expired" = false ]; then
            if ! gh run download "$RID" -n "$CRATE-$target" -D "$dir" >/dev/null 2>"$WORK/art.err" </dev/null; then
                fail "download artifact $CRATE-$target ($(first_line "$WORK/art.err"))"
                continue
            fi
            if [ ! -f "$file" ]; then
                fail "artifact $CRATE-$target holds $name (it holds: $(find "$dir" -type f -exec basename {} \; | tr '\n' ' '))"
                continue
            fi
            if [ -z "$digest" ]; then
                # No digest from the API: hash the release asset itself.
                if gh release download "$TAG" --pattern "$name" --dir "$dir/release" >/dev/null 2>"$WORK/dl.err" </dev/null; then
                    digest=sha256:$(sha256 "$dir/release/$name")
                else
                    fail "download $name ($(first_line "$WORK/dl.err"))"
                    continue
                fi
            fi
            check "asset $name is its build job's artifact" test "sha256:$(sha256 "$file")" = "$digest"
        else
            if ! gh release download "$TAG" --pattern "$name" --dir "$dir" >/dev/null 2>"$WORK/dl.err" </dev/null; then
                fail "download $name ($(first_line "$WORK/dl.err"))"
                continue
            fi
            if [ "$expired" = true ]; then
                skip "asset $name is its build job's artifact (the artifact has expired)"
            else
                skip "asset $name is its build job's artifact (no artifact; that failed above)"
            fi
        fi
        mkdir -p "$dir/x"
        pattern=$(arch_pattern "$target")
        if [ "$ext" = zip ] && ! command -v unzip >/dev/null 2>&1; then
            skip "contents of $name (no unzip)"
            skip "$bin in $name is built for $target (no unzip)"
            continue
        fi
        if [ "$ext" = zip ]; then
            unzip -Z1 "$file" >"$dir/list.txt" 2>/dev/null && unzip -q -o "$file" "$bin" -d "$dir/x" 2>/dev/null
        else
            tar -tzf "$file" >"$dir/list.txt" 2>/dev/null && tar -xzf "$file" -C "$dir/x" "$bin" 2>/dev/null
        fi
        check "$name holds $bin, README.md and a LICENSE" has_release_files "$dir/list.txt" "$bin"
        if [ -z "$pattern" ]; then
            fail "$bin in $name is built for $target (no architecture pattern for $target: add one to arch_pattern)"
        elif ! command -v file >/dev/null 2>&1; then
            skip "$bin in $name is built for $target (no file command)"
        else
            file -b "$dir/x/$bin" >"$dir/file.txt" 2>&1
            check "$bin in $name is built for $target" grep -qE -- "$pattern" "$dir/file.txt"
        fi
    done 3<"$WORK/targets.txt"
else
    skip "assets (no release)"
fi

# --------------------------------------------------------------------------
# Pages
# --------------------------------------------------------------------------
# Reachability proves nothing: the site already served the last release's
# docs. The deployment record for this tag says what the run deployed; look
# for success anywhere in its status history. The served pages are checked
# only while this deployment is the live one, the newest that succeeded.
deployment_succeeded() {  # deployment_succeeded <id>: its statuses include success
    [ "$(gh api "repos/$REPO/deployments/$1/statuses" --jq '[.[].state] | index("success") != null' \
        2>>"$WORK/dep.err" </dev/null)" = true ]
}
if ! dep=$(gh api "repos/$REPO/deployments?environment=github-pages&ref=$TAG&per_page=1" \
    --jq '.[] | "\(.id) \(.sha)"' 2>"$WORK/dep.err"); then
    fail "look up the Pages deployment for $TAG ($(first_line "$WORK/dep.err"))"
elif [ -z "$dep" ]; then
    fail "Pages deployment for $TAG (none recorded)"
else
    read -r dep_id dep_sha <<<"$dep"
    check "Pages deployment for $TAG built ${SHA:0:7}" test "$dep_sha" = "$SHA"
    check "Pages deployment for $TAG succeeded" deployment_succeeded "$dep_id"
    live=""
    if gh api "repos/$REPO/deployments?environment=github-pages&per_page=10" --jq '.[].id' \
        >"$WORK/deployments.txt" 2>"$WORK/deployments.err"; then
        while IFS= read -r id; do
            if deployment_succeeded "$id"; then live=$id && break; fi
        done <"$WORK/deployments.txt"
    fi
    if [ -z "$live" ]; then
        fail "find the live Pages deployment ($(first_line "$WORK/deployments.err"))"
    elif [ "$live" != "$dep_id" ]; then
        skip "served docs (a later deployment, $live, is live)"
    elif ! site=$(gh api "repos/$REPO/deployments/$dep_id/statuses" \
        --jq 'map(select(.state == "success")) | .[0].environment_url // empty' 2>"$WORK/site.err") ||
        [ -z "$site" ]; then
        fail "find the deployment's site URL ($(first_line "$WORK/site.err"))"
    else
        # A query string sidesteps the CDN's cached copy of the old site.
        check "Pages root redirects to $CRATE/index.html" \
            fetch_has "${site%/}/?verify=$RID" "url=$CRATE/index.html" "$WORK/site-index.html"
        check "Pages serves the $BASE docs" \
            fetch_has "${site%/}/$CRATE/index.html?verify=$RID" "\"version\">$BASE<" "$WORK/site-crate.html"
    fi
fi

# --------------------------------------------------------------------------
# crates.io
# --------------------------------------------------------------------------
if [ "$PRERELEASE" = true ]; then
    skip "crates.io (pre-release)"
else
    status=$(curl -s --max-time 30 -o /dev/null -w '%{http_code}' -A "verify-release ($REPO)" \
        "https://crates.io/api/v1/crates/$CRATE/$BASE")
    if [ "$PUBLISH" = false ]; then
        check "crates.io does not have $CRATE $BASE (publish = false; HTTP $status)" test "$status" = 404
    else
        check "crates.io has $CRATE $BASE (HTTP $status)" test "$status" = 200
    fi
fi

# --------------------------------------------------------------------------
# The host platform's binary runs
# --------------------------------------------------------------------------
case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) host=aarch64-apple-darwin ;;
    Darwin-x86_64) host=x86_64-apple-darwin ;;
    Linux-x86_64) host=x86_64-unknown-linux-gnu ;;
    Linux-aarch64 | Linux-arm64) host=aarch64-unknown-linux-gnu ;;
    *) host= ;;
esac
bin=$WORK/assets/$host/x/$CRATE
if [ -z "$host" ]; then
    skip "host binary (no build target for $(uname -s)-$(uname -m))"
elif version_lt "$BASE" 0.4.0; then
    skip "host binary ($BASE predates the zero-timeout check it relies on)"
elif [ ! -e "$bin" ]; then
    skip "host binary (no $host asset was extracted above)"
elif [ ! -x "$bin" ]; then
    # shellcheck disable=SC2016 # perl's variables, not the shell's
    fail "$host binary is executable (its archive has mode $("$PERL" -e 'printf "%o", (stat $ARGV[0])[2] & 07777' "$bin"))"
else
    # Config::validate rejects a zero OPERATION_TIMEOUT_SECS, so the binary
    # logs its version and exits with EX_CONFIG (78) before any network I/O.
    # Run it from an empty directory under the temp dir (the app loads a .env
    # from its working directory or any parent) with an empty environment (a
    # verifier should not hand a freshly downloaded binary the caller's
    # tokens), and give it 30 seconds. `exec { $ARGV[0] } @ARGV` forces perl's
    # list form, so the path is never re-parsed by a shell.
    mkdir -p "$WORK/run"
    # shellcheck disable=SC2016 # perl's variables, not the shell's
    (cd "$WORK/run" && env -i OPERATION_TIMEOUT_SECS=0 RUST_LOG=info \
        "$PERL" -e 'alarm 30; exec { $ARGV[0] } @ARGV or exit 127' "$bin") >"$WORK/bin.raw" 2>&1
    code=$?
    "$PERL" -pe 's/\e\[[0-9;]*m//g' "$WORK/bin.raw" >"$WORK/bin.log"
    if [ "$code" -eq 142 ]; then
        fail "$host binary exits on an invalid config (killed after 30s; does Config::validate still reject a zero timeout?)"
    else
        check "$host binary exits 78 on an invalid config (got $code)" test "$code" -eq 78
    fi
    check "$host binary reports version $BASE" grep -qF "Starting Iggy Sample Application v$BASE" "$WORK/bin.log"
fi

echo
echo "$passes passed, $failures failed, $skips skipped"
if [ "$PRERELEASE" = true ] && [ "$release_ok" = true ]; then
    echo
    echo "If $TAG only exercised the workflow, delete it before the next release:"
    echo "  gh release delete $TAG --cleanup-tag --yes"
    echo "Its docs job deployed ${SHA:0:7}'s docs to Pages, which deleting the tag does"
    echo "not undo: CONTRIBUTING.md (Releasing) says how to restore them."
fi
[ "$failures" -eq 0 ]
