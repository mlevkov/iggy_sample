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

TAG=$1
VERSION=${TAG#v}
BASE=${VERSION%%-*}
case $VERSION in
    *-*) PRERELEASE=true ;;
    *) PRERELEASE=false ;;
esac
TAB=$(printf '\t')
PERL=$(command -v perl) || { echo "required tool not found: perl" >&2; exit 2; }

ROOT=$(git rev-parse --show-toplevel) || exit 2
cd "$ROOT" || exit 2
REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner) || exit 2
CRATE=$(awk -F'"' '/^name = / { print $2; exit }' Cargo.toml)
[ -n "$CRATE" ] || { echo "cannot read the package name from Cargo.toml" >&2; exit 2; }

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
# check <description> <command> [args...]: PASS when the command succeeds.
check() {
    local desc=$1
    shift
    if "$@"; then pass "$desc"; else fail "$desc"; fi
}
# Exact whole-line match against a file. Checks read files, never a pipe:
# under pipefail, `producer | grep -q` fails when grep exits early and
# SIGPIPEs the producer, which turns every negated check into a vacuous pass.
has_line() { grep -qxF -- "$1" "$2"; }
lacks_regex() { ! grep -qiE -- "$1" "$2"; }
# version_lt A B: A < B, comparing X.Y.Z numerically.
# shellcheck disable=SC2016 # perl's variables, not the shell's
version_lt() {
    "$PERL" -e '@a = split /\./, $ARGV[0]; @b = split /\./, $ARGV[1];
        for (0 .. 2) { exit 0 if ($a[$_] // 0) < ($b[$_] // 0); exit 1 if ($a[$_] // 0) > ($b[$_] // 0) }
        exit 1' "$1" "$2"
}

# --------------------------------------------------------------------------
# The run
# --------------------------------------------------------------------------
run=$(gh run list --workflow release.yml --branch "$TAG" --limit 1 \
    --json databaseId,headSha,status,url \
    -q '.[] | "\(.databaseId) \(.headSha) \(.status) \(.url)"')
[ -n "$run" ] || { echo "no release.yml run found for $TAG" >&2; exit 2; }
read -r RID SHA STATUS URL <<<"$run"
[ "$STATUS" = completed ] || { echo "run $RID is $STATUS; verify it once it completes" >&2; exit 2; }

git cat-file -e "$SHA^{commit}" 2>/dev/null || git fetch --quiet origin
git fetch --quiet --tags origin
PREV=${2:-$(git describe --tags --abbrev=0 "$SHA^" 2>/dev/null)}
[ -n "$PREV" ] || { echo "cannot determine the previous tag; pass it as the second argument" >&2; exit 2; }

echo "repo $REPO, tag $TAG ($([ "$PRERELEASE" = true ] && echo pre-release || echo stable)), commit ${SHA:0:7}"
echo "run $RID: $URL"
echo "changelog base: $PREV"
echo

# --------------------------------------------------------------------------
# Jobs
# --------------------------------------------------------------------------
gh run view "$RID" --json jobs --jq '.jobs[] | "\(.name)\t\(.conclusion)"' >"$WORK/jobs.tsv" || exit 2
builds=$(grep -c '^Build (' "$WORK/jobs.tsv")
check "build jobs ran ($builds)" test "$builds" -gt 0
# shellcheck disable=SC2016 # $1 and $2 are awk fields
check "every job except publish succeeded" \
    awk -F'\t' '$1 != "Publish to crates.io" && $2 != "success" { bad = 1 } END { exit bad }' "$WORK/jobs.tsv"
publish=$(awk -F'\t' '$1 == "Publish to crates.io" { print $2 }' "$WORK/jobs.tsv")
if [ -z "$publish" ]; then
    skip "publish job (not in this run)"
elif [ "$PRERELEASE" = true ]; then
    check "publish job skipped for a pre-release" test "$publish" = skipped
else
    check "publish job ran and reported success" test "$publish" = success
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
gh run view "$RID" --log >"$WORK/raw.log" 2>"$WORK/raw.err" || fail "fetch the run log"
perl -ne '
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

logged=$(cut -f1 "$WORK/output.log" | sort -u | wc -l | tr -d ' ')
ran=$(awk -F'\t' '$2 != "skipped"' "$WORK/jobs.tsv" | wc -l | tr -d ' ')
check "log covers every job that ran ($ran)" test "$logged" -eq "$ran"

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

# A step under continue-on-error fails without failing its job, and the API
# reports that step's conclusion as success too. Its "##[error]" line is the
# only trace, so any such line means a failure the job status hides.
if grep -F '##[error]' "$WORK/output.log" >"$WORK/errors.log"; then
    fail "no step failed silently; failures hidden behind a successful job:"
    sed 's/^/        /' "$WORK/errors.log"
else
    pass "no step failed silently"
fi

# --------------------------------------------------------------------------
# Artifacts
# --------------------------------------------------------------------------
gh api "repos/$REPO/actions/runs/$RID/artifacts" --jq '.artifacts[].name' >"$WORK/artifacts.txt" || exit 2
binaries=$(grep -c "^$CRATE-" "$WORK/artifacts.txt")
check "one binary artifact per build ($binaries of $builds)" test "$binaries" -eq "$builds"
check "Pages artifact uploaded" grep -qx 'github-pages' "$WORK/artifacts.txt"

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
    while IFS=$TAB read -r job _; do
        case $job in "Build ("*) ;; *) continue ;; esac
        target=${job#Build (}
        target=${target%)}
        case $target in *-windows-*) ext=zip ;; *) ext=tar.gz ;; esac
        check "asset $CRATE-$VERSION-$target.$ext" grep -qxF -- "$CRATE-$VERSION-$target.$ext" "$WORK/release.txt"
    done <"$WORK/jobs.tsv"
else
    fail "release $TAG exists ($(head -n 1 "$WORK/release.err"))"
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
