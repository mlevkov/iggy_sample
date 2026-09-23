#!/usr/bin/env bash
# Verify a release.yml run step by step, not just its overall conclusion.
#
# Usage: scripts/verify-release.sh <tag> [<previous-tag>]
#
# Checks the latest release.yml run for <tag>: job conclusions, what the
# steps printed, the uploaded artifacts, the GitHub Release, the Pages
# deployment, and one published binary. <previous-tag> is where the changelog
# should start; by default it is what release.yml computes,
# `git describe --tags --abbrev=0 <tagged commit>^`.
#
# A tag with a hyphen (v0.4.1-ci.1) is a pre-release: the publish job must be
# skipped and the release must not become Latest. To exercise a release.yml
# change without releasing, tag the current Cargo.toml version with a
# pre-release suffix (the validate job compares the part before the hyphen),
# verify the run, then delete the release and its tag. A leftover pre-release
# tag becomes the changelog base of the next release: see CONTRIBUTING.md.
#
# Requires gh (authenticated), git, perl and tar. Exits 0 when every check
# passes (skips allowed), 1 when any fails, 2 on a usage or setup error. The
# work directory is kept when a check fails. Binaries before v0.4.0 are not
# executed: they accept a zero OPERATION_TIMEOUT_SECS and go on to open
# network listeners.

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

for tool in gh git perl tar; do
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
        echo "work directory kept for inspection: $WORK"
    fi
}
trap cleanup EXIT

pass() { printf 'PASS  %s\n' "$1"; passes=$((passes + 1)); }
fail() { printf 'FAIL  %s\n' "$1"; failures=$((failures + 1)); }
skip() { printf 'SKIP  %s\n' "$1"; skips=$((skips + 1)); }
note() { printf 'NOTE  %s\n' "$1"; }
# check <description> <command> [args...]: PASS when the command succeeds.
check() {
    local desc=$1
    shift
    if "$@"; then pass "$desc"; else fail "$desc"; fi
}
# Checks read files, never a pipe: under pipefail, `producer | grep -q` fails
# when grep exits early and SIGPIPEs the producer, which turns a negated
# check into a vacuous pass. For the same reason a negated check demands a
# non-empty file (grep exits 2 on an unreadable one), so a failed fetch
# cannot pass as "nothing found".
has_line() { grep -qxF -- "$1" "$2"; }
lacks_regex() {
    [ -s "$2" ] || return 1
    grep -qiE -- "$1" "$2"
    [ $? -eq 1 ]
}
first_line() { head -n 1 "$1" 2>/dev/null; }
# version_lt A B: A < B, comparing X.Y.Z numerically.
# shellcheck disable=SC2016 # perl's variables, not the shell's
version_lt() {
    "$PERL" -e '@a = split /\./, $ARGV[0]; @b = split /\./, $ARGV[1];
        for (0 .. 2) { exit 0 if ($a[$_] // 0) < ($b[$_] // 0); exit 1 if ($a[$_] // 0) > ($b[$_] // 0) }
        exit 1' "$1" "$2"
}

# --------------------------------------------------------------------------
# The run, and the commit it built
# --------------------------------------------------------------------------
runs=$(gh run list --workflow release.yml --branch "$TAG" --limit 20 \
    --json databaseId,headSha,status,attempt,url \
    -q '.[] | "\(.databaseId) \(.headSha) \(.status) \(.attempt) \(.url)"') ||
    { echo "listing release.yml runs for $TAG failed" >&2; exit 2; }
[ -n "$runs" ] || { echo "no release.yml run found for $TAG" >&2; exit 2; }
read -r RID SHA STATUS ATTEMPT URL <<<"${runs%%$'\n'*}"
nruns=$(printf '%s\n' "$runs" | wc -l | tr -d ' ')
[ "$STATUS" = completed ] || { echo "run $RID is $STATUS; verify it once it completes" >&2; exit 2; }

git fetch --quiet --tags origin || { echo "fetching from origin failed" >&2; exit 2; }
git cat-file -e "$SHA^{commit}" 2>/dev/null ||
    { echo "commit ${SHA:0:7} is not in this clone after fetching" >&2; exit 2; }

# The workflow and manifest the run built are the oracle, not the working tree.
git show "$SHA:.github/workflows/release.yml" >"$WORK/release.yml" 2>/dev/null ||
    { echo "no .github/workflows/release.yml at ${SHA:0:7}" >&2; exit 2; }
git show "$SHA:Cargo.toml" >"$WORK/Cargo.toml" || exit 2
CRATE=$(awk -F'"' '/^name = / { print $2; exit }' "$WORK/Cargo.toml")
[ -n "$CRATE" ] || { echo "cannot read the package name from Cargo.toml at ${SHA:0:7}" >&2; exit 2; }
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
echo "run $RID, attempt $ATTEMPT: $URL"
echo "changelog base: $PREV"
[ "$nruns" -gt 1 ] && note "$nruns release.yml runs exist for $TAG; verifying the newest"
[ -s "$WORK/local-only.txt" ] && note "ignored tags that exist only in this clone: $(tr '\n' ' ' <"$WORK/local-only.txt")"
echo

git ls-remote --tags origin "refs/tags/$TAG" "refs/tags/$TAG^{}" >"$WORK/tag.raw" 2>/dev/null
tag_commit=$(awk -v ref="refs/tags/$TAG" '
    $2 == ref "^{}" { peeled = $1 } $2 == ref { plain = $1 }
    END { print (peeled != "" ? peeled : plain) }' "$WORK/tag.raw")
if [ -z "$tag_commit" ]; then
    skip "tag $TAG names the commit the run built (the tag is no longer on origin)"
else
    check "tag $TAG names the commit the run built (${SHA:0:7})" test "$tag_commit" = "$SHA"
fi

# --------------------------------------------------------------------------
# Jobs
# --------------------------------------------------------------------------
jobs_ok=false
if gh run view "$RID" --json jobs --jq '.jobs[] | "\(.name)\t\(.conclusion)"' \
    >"$WORK/jobs.tsv" 2>"$WORK/jobs.err" && [ -s "$WORK/jobs.tsv" ]; then
    jobs_ok=true
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
if gh run view "$RID" --log >"$WORK/raw.log" 2>"$WORK/raw.err" && [ -s "$WORK/raw.log" ]; then
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

    if [ "$jobs_ok" = true ]; then
        logged=$(cut -f1 "$WORK/output.log" | sort -u | wc -l | tr -d ' ')
        ran=$(awk -F'\t' '$2 != "skipped"' "$WORK/jobs.tsv" | wc -l | tr -d ' ')
        check "log covers every job that ran ($ran)" test "$logged" -eq "$ran"
    fi
    check "validate parsed the tag as $VERSION" has_line "Validate Release${TAB}Version: $VERSION" "$WORK/output.log"
    check "validate matched Cargo.toml version $BASE" has_line "Validate Release${TAB}Version verified: $BASE" "$WORK/output.log"
    check "changelog starts at $PREV" has_line "Create Release${TAB}Generating changelog since $PREV" "$WORK/output.log"
    check "changelog did not fall back to the last 20 commits" lacks_regex 'No previous tag found' "$WORK/output.log"
    if [ "$PRERELEASE" = false ]; then
        case $PREV in
            *-*) fail "changelog base $PREV is a pre-release tag (a leftover exercise tag truncates the notes)" ;;
            *) pass "changelog base $PREV is a stable tag" ;;
        esac
    fi
    check "no artifact digest mismatch" lacks_regex 'digest.*mismatch|hash mismatch' "$WORK/output.log"

    # A step under continue-on-error fails without failing its job, and the
    # API reports that step's conclusion as success too. Its "##[error]" line
    # is the only trace, so any such line means a failure the job status hides.
    if grep -F '##[error]' "$WORK/output.log" >"$WORK/errors.log"; then
        fail "no step failed silently; failures hidden behind a successful job:"
        sed 's/^/        /' "$WORK/errors.log"
    else
        pass "no step failed silently"
    fi
else
    fail "fetch the run log ($(first_line "$WORK/raw.err"))"
    skip "step-output checks (no log)"
fi

# --------------------------------------------------------------------------
# Artifacts
# --------------------------------------------------------------------------
if gh api "repos/$REPO/actions/runs/$RID/artifacts" --jq '.artifacts[].name' \
    >"$WORK/artifacts.txt" 2>"$WORK/artifacts.err"; then
    missing=$(while IFS= read -r t; do grep -qxF -- "$CRATE-$t" "$WORK/artifacts.txt" || printf '%s ' "$t"; done <"$WORK/targets.txt")
    check "a build artifact for every target${missing:+ (missing: $missing)}" test -z "$missing"
    check "Pages artifact uploaded" grep -qxF github-pages "$WORK/artifacts.txt"
else
    fail "list the run's artifacts ($(first_line "$WORK/artifacts.err"))"
fi

# --------------------------------------------------------------------------
# The GitHub Release
# --------------------------------------------------------------------------
release_ok=false
if gh release view "$TAG" --json isPrerelease,isDraft,publishedAt,assets \
    --jq '"\(.isPrerelease)\t\(.isDraft)\t\(.publishedAt)", (.assets[] | .name)' \
    >"$WORK/release.txt" 2>"$WORK/release.err"; then
    release_ok=true
    IFS=$TAB read -r is_pre is_draft published <"$WORK/release.txt"
    pass "release $TAG exists"
    check "release is published, not a draft" test "$is_draft" = false
    check "release pre-release flag is $PRERELEASE" test "$is_pre" = "$PRERELEASE"
    while IFS= read -r target; do
        case $target in *-windows-*) ext=zip ;; *) ext=tar.gz ;; esac
        check "asset $CRATE-$VERSION-$target.$ext" grep -qxF -- "$CRATE-$VERSION-$target.$ext" "$WORK/release.txt"
    done <"$WORK/targets.txt"
else
    fail "release $TAG exists ($(first_line "$WORK/release.err"))"
fi

latest=$(gh release list --limit 50 --json tagName,isLatest --jq '.[] | select(.isLatest) | .tagName')
latest_is_other_stable() {
    [ -n "$latest" ] && [ "$latest" != "$TAG" ] && case $latest in *-*) false ;; *) true ;; esac
}
if [ "$PRERELEASE" = true ]; then
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
# Pages
# --------------------------------------------------------------------------
# Reachability proves nothing: the site already served the last release's
# docs. The deployment record says what this run deployed. A later deploy
# turns this one inactive, so look for success anywhere in its history.
dep=$(gh api "repos/$REPO/deployments?environment=github-pages&sha=$SHA&per_page=1" --jq '.[0].id // empty')
if [ -n "$dep" ]; then
    check "Pages deployed from ${SHA:0:7}" \
        test "$(gh api "repos/$REPO/deployments/$dep/statuses" --jq '[.[].state] | index("success") != null')" = true
else
    fail "Pages deployed from ${SHA:0:7} (no deployment record)"
fi

# --------------------------------------------------------------------------
# One published binary, for the host platform
# --------------------------------------------------------------------------
case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) host=aarch64-apple-darwin ;;
    Darwin-x86_64) host=x86_64-apple-darwin ;;
    Linux-x86_64) host=x86_64-unknown-linux-gnu ;;
    Linux-aarch64 | Linux-arm64) host=aarch64-unknown-linux-gnu ;;
    *) host= ;;
esac
asset=$CRATE-$VERSION-$host.tar.gz
if [ "$release_ok" != true ]; then
    skip "host binary (no release to download from)"
elif [ -z "$host" ] || ! grep -qxF -- "$asset" "$WORK/release.txt"; then
    skip "host binary (no asset for $(uname -s)-$(uname -m))"
elif version_lt "$BASE" 0.4.0; then
    skip "host binary ($BASE predates the zero-timeout check it relies on)"
elif ! gh release download "$TAG" --pattern "$asset" --dir "$WORK/bin" >/dev/null 2>"$WORK/download.err"; then
    fail "download $asset ($(head -n 1 "$WORK/download.err"))"
else
    tar -tzf "$WORK/bin/$asset" >"$WORK/contents.txt"
    # shellcheck disable=SC2016 # $1 and $2 are the inner shell's arguments
    check "archive holds $CRATE, README.md and a LICENSE" \
        sh -c 'grep -qx "$1" "$2" && grep -qx README.md "$2" && grep -q "^LICENSE" "$2"' _ "$CRATE" "$WORK/contents.txt"
    tar -xzf "$WORK/bin/$asset" -C "$WORK/bin"
    # Config::validate rejects a zero OPERATION_TIMEOUT_SECS, so the binary
    # logs its version and exits with EX_CONFIG (78) before any network I/O.
    # Run it from an empty directory (the app loads a .env from its working
    # directory) with an empty environment (a verifier should not hand a
    # freshly downloaded binary the caller's tokens), and give it 30 seconds.
    mkdir -p "$WORK/run"
    (cd "$WORK/run" && env -i OPERATION_TIMEOUT_SECS=0 RUST_LOG=info \
        "$PERL" -e 'alarm 30; exec @ARGV or exit 127' "$WORK/bin/$CRATE") >"$WORK/bin.raw" 2>&1
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
fi
[ "$failures" -eq 0 ]
