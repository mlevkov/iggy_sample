#!/usr/bin/env bash
# Run both flows of the connectors demo against a running stack, and show
# what each step returned.
#
# Usage: scripts/connectors-demo.sh
#
# Needs the stack from docker-compose.yaml plus docker-compose.connectors.yaml
# (see docs/connectors-demo.md), and docker, curl, jq and openssl.
#
#   inbound   signs a webhook with the endpoint's HMAC secret, POSTs it to
#             the HTTP source, then polls sample-stream/webhooks through the
#             gateway until the event is there. A copy with a forged
#             signature must be refused (401).
#   outbound  sends three events through the gateway's POST /messages,
#             which spreads them over the topic's three partitions, then
#             watches GET /sink/inbound until the HTTP sink has delivered all
#             three back. Three, not one: with SDK 0.12.0, an OnRead
#             consumer group does not fetch a partition while another in
#             its readiness group has never held a message, and on a fresh
#             stack every partition is empty (apache/iggy#4784;
#             docs/connectors-demo.md, Known Issues).
#
# The webhook's endpoint id and HMAC secret are read from the connectors
# container (connectors-init generated them) and never printed. The secret
# is on openssl's command line while it signs, so other users of this host
# could see it in the process list for that moment: fine for a local demo,
# not for a shared machine.
#
# Exits non-zero when a step fails or misses its budget. The weekly Extended
# Tests workflow runs it that way.
#
# Environment:
#   GATEWAY_URL   gateway base URL (default http://localhost:8000)
#   WEBHOOK_URL   HTTP source base URL (default http://localhost:8085)
#   BUDGET        seconds allowed for each flow to complete (default 30)
#   API_KEY       sent as X-API-Key to the gateway, if it requires one

set -euo pipefail

GATEWAY_URL="${GATEWAY_URL:-http://localhost:8000}"
WEBHOOK_URL="${WEBHOOK_URL:-http://localhost:8085}"
BUDGET="${BUDGET:-30}"
CONTAINER="iggy-connectors"

for tool in docker curl jq openssl; do
    command -v "$tool" >/dev/null || { echo "error: $tool is required" >&2; exit 2; }
done

# Guarded expansion below: macOS bash 3.2 treats an empty array as unset
# under set -u.
auth_args=()
if [ -n "${API_KEY:-}" ]; then
    auth_args=(-H "X-API-Key: $API_KEY")
fi

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

now() { date +%s; }

new_uuid() {
    local id
    id="$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid 2>/dev/null || true)"
    [ -n "$id" ] || fail "no uuidgen and no /proc/sys/kernel/random/uuid"
    printf '%s' "$id" | tr '[:upper:]' '[:lower:]'
}

# An event in the gateway's schema, so its poll API can read it back.
event_json() {
    jq -cn --arg id "$1" --arg type "$2" --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        --arg source "$3" '{
            id: $id, event_type: $type, timestamp: $ts,
            payload: {type: "Generic", data: {source: $source}}}'
}

gateway_get() {
    curl -fsS --max-time 5 ${auth_args[@]+"${auth_args[@]}"} "$GATEWAY_URL$1"
}

secret() {
    docker exec "$CONTAINER" cat "/run/secrets/connectors/$1" ||
        fail "cannot read $1 from the $CONTAINER container; is the connectors overlay running?"
}

# POST a webhook body; prints "<HTTP status> <response body>".
post_webhook() {
    local body="$1" signature="$2" delivery="$3" response_file status
    response_file="$(mktemp)"
    status="$(curl -sS -o "$response_file" -w '%{http_code}' --max-time 5 \
        -H "Content-Type: application/json" \
        -H "X-Signature-256: sha256=$signature" \
        -H "X-Webhook-Delivery: $delivery" \
        --data-binary "$body" "$WEBHOOK_URL/e/$endpoint_id" || echo 000)"
    printf '%s %s\n' "$status" "$(cat "$response_file")"
    rm -f "$response_file"
}

echo "== Preflight"
gateway_get /ready >/dev/null || fail "the gateway at $GATEWAY_URL is not ready"
gateway_get /sink/inbound >/dev/null ||
    fail "GET /sink/inbound failed: the gateway runs without the connectors overlay"
endpoint_id="$(secret webhook_endpoint_id)"
hmac_secret="$(secret webhook_hmac_secret)"
echo "gateway ready, webhook endpoint $WEBHOOK_URL/e/<secret id>"

echo
echo "== Inbound: a signed webhook into sample-stream/webhooks"
webhook_id="$(new_uuid)"
delivery_id="delivery-$(new_uuid)"
webhook_body="$(event_json "$webhook_id" "webhook.received" "connectors-demo")"
signature="$(printf '%s' "$webhook_body" | openssl dgst -sha256 -hmac "$hmac_secret" -r | cut -d' ' -f1)"

# A forged signature is refused before anything is buffered.
forged="$(post_webhook "$webhook_body" "$(printf '%064d' 0)" "$delivery_id")"
echo "forged signature:  HTTP ${forged%% *}"
[ "${forged%% *}" = "401" ] || fail "a forged signature was answered ${forged%% *}, not 401"

# The source publishes its routes once its poll task runs, so a fresh
# container answers 404 for a moment. A 404 stored nothing.
started="$(now)"
while :; do
    answer="$(post_webhook "$webhook_body" "$signature" "$delivery_id")"
    [ "${answer%% *}" = "404" ] || break
    [ $(( $(now) - started )) -lt "$BUDGET" ] || fail "the webhook endpoint answered 404 for ${BUDGET}s"
    sleep 1
done
echo "signed webhook:    HTTP $answer"
[ "${answer%% *}" = "200" ] || fail "the signed webhook was answered ${answer%% *}"

found=""
while [ $(( $(now) - started )) -lt "$BUDGET" ]; do
    found="$(gateway_get "/streams/sample-stream/topics/webhooks/messages?partition_id=0&offset=0&count=100" |
        jq -c --arg id "$webhook_id" '.messages[] | select(.event.id == $id)
            | {offset, id, event_type: .event.event_type}' 2>/dev/null | head -1 || true)"
    [ -n "$found" ] && break
    sleep 1
done
[ -n "$found" ] || fail "the webhook was not readable through the gateway within ${BUDGET}s"
inbound_after=$(( $(now) - started ))
echo "read back via GET /streams/sample-stream/topics/webhooks/messages:"
echo "  $found"

echo
echo "== Outbound: POST /messages back to the gateway through the HTTP sink"
event_ids=()
started="$(now)"
for n in 1 2 3; do
    event_id="$(new_uuid)"
    event_ids+=("$event_id")
    send_status="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 \
        -H "Content-Type: application/json" ${auth_args[@]+"${auth_args[@]}"} \
        -d "$(jq -cn --argjson event "$(event_json "$event_id" "sink.demo" "connectors-demo")" '{event: $event}')" \
        "$GATEWAY_URL/messages" || echo 000)"
    echo "POST /messages #$n: HTTP $send_status"
    case "$send_status" in
        2??) ;;
        *) fail "POST /messages answered $send_status" ;;
    esac
done

ids_json="$(printf '%s\n' "${event_ids[@]}" | jq -R . | jq -cs .)"
delivered=""
while [ $(( $(now) - started )) -lt "$BUDGET" ]; do
    delivered="$(gateway_get /sink/inbound |
        jq -c --argjson ids "$ids_json" '[.recent[] | select(.payload.id as $id | $ids | index($id))
            | {partition_id, offset, id, received_at}]' 2>/dev/null || true)"
    [ "$(printf '%s' "$delivered" | jq 'length' 2>/dev/null || echo 0)" -eq 3 ] && break
    sleep 1
done
[ "$(printf '%s' "$delivered" | jq 'length' 2>/dev/null || echo 0)" -eq 3 ] ||
    fail "the HTTP sink did not deliver all three events within ${BUDGET}s (got: ${delivered:-none})"
outbound_after=$(( $(now) - started ))
echo "delivered to POST /sink/inbound, reported by GET /sink/inbound:"
printf '%s' "$delivered" | jq -c '.[]' | sed 's/^/  /'
echo "receiver totals:   $(gateway_get /sink/inbound |
    jq -c '{messages_received, batches_accepted, batches_rejected, unauthorized_requests}')"

echo
echo "== Summary"
echo "inbound:   webhook readable through the gateway ${inbound_after}s after it was sent"
echo "outbound:  three events back at the gateway ${outbound_after}s after the first was sent"
echo "PASS"
