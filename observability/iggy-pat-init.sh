#!/bin/sh
# Create the personal access token Prometheus uses to scrape Iggy's /metrics.
#
# Since server 0.9, /metrics requires a bearer credential. A token is shown
# only when it is created, so every run deletes the previous "prometheus"
# token and creates a fresh one, writing it to /secrets/iggy_pat for
# Prometheus's `credentials_file`.
#
# The delete may exit non-zero even when it succeeds: the CLI then tries to
# update a keyring the container does not have. The create must succeed, so
# a failure here stops the stack (Prometheus waits for this service to
# complete successfully) instead of leaving the Iggy panels silently blank.
set -eu

server="${IGGY_SERVER_ADDRESS:-iggy:8090}"
username="${IGGY_USERNAME:-iggy}"
password="${IGGY_PASSWORD:-iggy}"
token_name="${IGGY_PAT_NAME:-prometheus}"
out="/secrets/iggy_pat"

iggy_cli() {
    iggy --tcp-server-address "$server" -u "$username" -p "$password" "$@"
}

iggy_cli pat delete "$token_name" >/dev/null 2>&1 || true

token=$(iggy_cli -q pat create "$token_name" 2>/dev/null)
if [ -z "$token" ]; then
    echo "iggy-pat-init: the CLI printed no token" >&2
    exit 1
fi

printf '%s' "$token" > "$out.tmp"
mv "$out.tmp" "$out"
echo "iggy-pat-init: wrote $out"
