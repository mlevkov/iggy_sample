#!/bin/sh
# One-shot for docker-compose.connectors.yaml: generate the connectors
# demo's secrets and the connector runtime's personal access token.
#
# Runs in the apache/iggy image (for its CLI) as root, so it can hand each
# file to the user that reads it. A secret that exists already is kept, so a
# restart does not invalidate what the running services hold; removing the
# volumes (`down -v`) starts over.
#
#   /secrets/connectors (connectors_secrets volume)
#     webhook_endpoint_id   16 random bytes, hex: the webhook URL's secret path
#     webhook_hmac_secret   32 random bytes, hex: signs each webhook body
#     sink_inbound_token    32 random bytes, hex: the sink's bearer token on
#                           the gateway's POST /sink/inbound
#   /secrets/iggy (iggy_secrets volume, shared with Prometheus)
#     connectors_api_key    32 random bytes, hex: the runtime API's api-key
#     connectors_pat        personal access token the runtime signs in with
#
# Readers: the connectors container (uid 10001) reads everything; the
# gateway (uid 1000) reads sink_inbound_token; Prometheus (uid 65534) reads
# connectors_api_key. Files only the runtime reads are 0400 and owned by it.
set -eu

server="${IGGY_SERVER_ADDRESS:-iggy:8090}"
username="${IGGY_USERNAME:-iggy}"
password="${IGGY_PASSWORD:-iggy}"
token_name="${IGGY_PAT_NAME:-connectors}"
connectors_uid="${CONNECTORS_UID:-10001}"
connector_secrets=/secrets/connectors
iggy_secrets=/secrets/iggy

fail() {
    echo "connectors-init: $*" >&2
    exit 1
}

# ensure_secret FILE BYTES OWNER MODE: create FILE with BYTES random bytes
# as lowercase hex, unless it holds one already. A file that is not exactly
# that (truncated, edited) is replaced, since the entrypoint would refuse it
# on every start.
ensure_secret() {
    # stderr first, so a missing file (first start) opens quietly
    existing=$(tr -d '\n' 2>/dev/null <"$1" || true)
    case "$existing" in
        "" | *[!0-9a-f]*) existing="" ;;
    esac
    if [ "${#existing}" -ne $(($2 * 2)) ]; then
        od -An -tx1 -N"$2" /dev/urandom | tr -d ' \n' >"$1.tmp"
        [ "$(wc -c <"$1.tmp")" -eq $(($2 * 2)) ] || fail "could not generate $1"
        mv "$1.tmp" "$1"
        echo "connectors-init: generated $1"
    fi
    chown "$3" "$1"
    chmod "$4" "$1"
}

umask 077
ensure_secret "$connector_secrets/webhook_endpoint_id" 16 "$connectors_uid" 0400
ensure_secret "$connector_secrets/webhook_hmac_secret" 32 "$connectors_uid" 0400
ensure_secret "$connector_secrets/sink_inbound_token" 32 0 0444
ensure_secret "$iggy_secrets/connectors_api_key" 32 0 0444

# Credentials reach the CLI (0.15) through its environment, never its
# arguments: any user who can list a process can read its arguments (on a
# Linux host, host users too), while its environment is readable only by
# its own user and root. IGGY_TOKEN takes precedence over the
# IGGY_USERNAME/IGGY_PASSWORD pair, and a token that fails does not fall
# back to the pair; an empty IGGY_TOKEN counts as unset.
iggy_cli() {
    IGGY_TOKEN="" IGGY_USERNAME="$username" IGGY_PASSWORD="$password" \
        iggy --tcp-server-address "$server" "$@"
}

# Keep a token that still signs in. A token is shown only when it is
# created, so replacing it means deleting and recreating it, and a runtime
# holding the old one could not sign in again after a reconnect.
pat_file="$iggy_secrets/connectors_pat"
if [ -s "$pat_file" ] &&
    IGGY_TOKEN="$(cat "$pat_file")" iggy --tcp-server-address "$server" stream list >/dev/null 2>&1; then
    echo "connectors-init: kept the $token_name token"
else
    # The delete may exit non-zero even when it succeeds: the CLI then tries
    # to update a keyring the container does not have (iggy-pat-init.sh).
    iggy_cli pat delete "$token_name" >/dev/null 2>&1 || true
    token=$(iggy_cli -q pat create "$token_name" 2>/dev/null) ||
        fail "could not create the $token_name token"
    [ -n "$token" ] || fail "the CLI printed no token"
    printf '%s' "$token" >"$pat_file.tmp"
    mv "$pat_file.tmp" "$pat_file"
    echo "connectors-init: created the $token_name token"
fi
chown "$connectors_uid" "$pat_file"
chmod 0400 "$pat_file"
