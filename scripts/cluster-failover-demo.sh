#!/usr/bin/env bash
# Kill the leader of the three-node demo cluster under steady traffic, and
# show the cluster elect a new one while traffic resumes.
#
# Usage: scripts/cluster-failover-demo.sh
#
# Needs the stack from docker-compose.cluster.yaml running (see
# docs/cluster-demo.md), plus docker, curl and jq. It:
#   1. shows the cluster from the gateway's GET /cluster;
#   2. sends a message through the gateway every SEND_INTERVAL seconds, in
#      the background, to the default topic (replication factor "all", so
#      every node holds every partition);
#   3. kills the metadata leader's container (SIGKILL, no clean shutdown);
#   4. waits for the survivors to elect a new leader, and for sends to
#      succeed again;
#   5. starts the killed node and waits for it to rejoin: back in sync on
#      every partition of the default topic;
#   6. prints how long each step took, how many sends failed, and the
#      slowest send: the SDK holds a send and retries it while the new
#      primary takes over, so the switch mostly shows as delay, not errors.
#
# Rejoining is read from each partition's in_sync_members, not from node
# status: server 0.10 reports every node as healthy, even one that is down.
#
# It exits non-zero when no new leader appears, or sends do not resume,
# within RESUME_BUDGET seconds of the kill, or the killed node does not
# rejoin within REJOIN_BUDGET. The weekly Extended Tests workflow runs it
# that way. Sends that fail during the election are expected: upstream's own
# failover test allows every operation to fail until one succeeds, within
# 10 s. The gateway answers them 503 (retryable, or outcome unknown) or 504.
#
# Environment:
#   GATEWAY_URL     gateway base URL (default http://localhost:8001)
#   SEND_INTERVAL   seconds between sends (default 0.2)
#   WARMUP          seconds of traffic before the kill (default 5)
#   RESUME_BUDGET   seconds allowed for a new leader and a send (default 30)
#   REJOIN_BUDGET   seconds allowed for the killed node to rejoin (default 60)
#   API_KEY         sent as X-API-Key, if the gateway requires one

set -euo pipefail

GATEWAY_URL="${GATEWAY_URL:-http://localhost:8001}"
SEND_INTERVAL="${SEND_INTERVAL:-0.2}"
WARMUP="${WARMUP:-5}"
RESUME_BUDGET="${RESUME_BUDGET:-30}"
REJOIN_BUDGET="${REJOIN_BUDGET:-60}"

for tool in docker curl jq; do
    command -v "$tool" >/dev/null || { echo "error: $tool is required" >&2; exit 2; }
done

work_dir="$(mktemp -d)"
send_log="$work_dir/sends.log"
traffic_pid=""
cleanup() {
    if [ -n "$traffic_pid" ]; then
        kill "$traffic_pid" 2>/dev/null || true
        wait "$traffic_pid" 2>/dev/null || true
    fi
    rm -rf "$work_dir"
}
trap cleanup EXIT

# Guarded expansion below: macOS bash 3.2 treats an empty array as unset
# under set -u.
auth_args=()
if [ -n "${API_KEY:-}" ]; then
    auth_args=(-H "X-API-Key: $API_KEY")
fi

now() { date +%s; }

# GET /cluster, bounded: during an election the metadata call waits for a
# leader, so the gateway is asked to give up after 3 s.
cluster() {
    curl -fsS --max-time 5 -H "X-Request-Timeout: 3000" ${auth_args[@]+"${auth_args[@]}"} \
        "$GATEWAY_URL/cluster"
}

show_cluster() {
    cluster | jq -r '
        "cluster \(.name), leader: \(.leader // "none")",
        (.nodes[] | "  node \(.replica_id)  \(.name)  \(.address)  \(.role)  \(.status)")'
}

# The in-sync replicas of each partition of the default topic.
show_in_sync() {
    curl -fsS --max-time 5 -H "X-Request-Timeout: 3000" ${auth_args[@]+"${auth_args[@]}"} \
        "$GATEWAY_URL/streams/sample-stream/topics/events" |
        jq -r '.partitions[] | "  partition \(.id)  in sync: \(.in_sync_members | map(tostring) | join(","))"' \
        2>/dev/null || echo "  (topic read failed)"
}

# Whether replica $1 is in sync on every partition of the default topic.
in_sync_everywhere() {
    curl -fsS --max-time 5 -H "X-Request-Timeout: 3000" ${auth_args[@]+"${auth_args[@]}"} \
        "$GATEWAY_URL/streams/sample-stream/topics/events" 2>/dev/null |
        jq -e --argjson id "$1" \
            '(.partitions | length) > 0 and all(.partitions[]; .in_sync_members | index($id) != null)' \
            >/dev/null 2>&1
}

leader_id() {
    cluster 2>/dev/null | jq -r '.leader // empty' 2>/dev/null || true
}

send_once() {
    local id timestamp body
    id="$(uuidgen 2>/dev/null | tr '[:upper:]' '[:lower:]' || true)"
    [ -n "$id" ] || id="00000000-0000-4000-8000-$(printf '%012d' "$RANDOM")"
    timestamp="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    body=$(jq -cn --arg id "$id" --arg ts "$timestamp" '{
        event: {id: $id, event_type: "failover.demo", timestamp: $ts,
                payload: {type: "Generic", data: {demo: "failover"}}}}')
    curl -s -o /dev/null -w '%{http_code} %{time_total}' --max-time 10 \
        -H "Content-Type: application/json" ${auth_args[@]+"${auth_args[@]}"} \
        -d "$body" "$GATEWAY_URL/messages" || echo "000 10"
}

# One line per send: <epoch second it started> <HTTP status> <seconds taken>.
traffic() {
    while true; do
        printf '%s %s\n' "$(now)" "$(send_once)" >>"$send_log"
        sleep "$SEND_INTERVAL"
    done
}

# Successful sends (HTTP 2xx: 201 sent, 202 already applied) started at or
# after epoch second $1.
sends_ok_since() {
    awk -v since="$1" '$1 >= since && $2 ~ /^2/' "$send_log" | wc -l | tr -d ' '
}

sends_failed_since() {
    awk -v since="$1" '$1 >= since && $2 !~ /^2/' "$send_log" | wc -l | tr -d ' '
}

# The first successful send started after epoch second $1, and when it
# finished, in whole seconds after $1.
resumed_after() {
    awk -v since="$1" '$1 > since && $2 ~ /^2/ { printf "%d\n", $1 - since + $3; exit }' "$send_log"
}

# The slowest send started in [$1, $2) epoch seconds, in seconds.
slowest_between() {
    awk -v from="$1" -v to="$2" '$1 >= from && $1 < to && $3 > max { max = $3 }
        END { printf "%.1f\n", max }' "$send_log"
}

echo "== Cluster before the failure"
curl -fsS --max-time 5 ${auth_args[@]+"${auth_args[@]}"} "$GATEWAY_URL/ready" >/dev/null ||
    { echo "error: the gateway at $GATEWAY_URL is not ready" >&2; exit 2; }
show_cluster
show_in_sync
old_leader="$(leader_id)"
[ -n "$old_leader" ] || { echo "error: no leader reported" >&2; exit 1; }

echo
echo "== Sending a message every ${SEND_INTERVAL}s for ${WARMUP}s"
: >"$send_log"
traffic &
traffic_pid=$!
sleep "$WARMUP"
echo "sends so far: $(sends_ok_since 0) ok, $(sends_failed_since 0) failed"

victim="iggy-node-$old_leader"
echo
echo "== Killing the leader, $victim (SIGKILL)"
killed_at="$(now)"
docker kill "$victim" >/dev/null

new_leader=""
while [ $(( $(now) - killed_at )) -lt "$RESUME_BUDGET" ]; do
    candidate="$(leader_id)"
    if [ -n "$candidate" ] && [ "$candidate" != "$old_leader" ]; then
        new_leader="$candidate"
        break
    fi
    sleep 0.5
done
if [ -z "$new_leader" ]; then
    echo "FAIL: no new leader within ${RESUME_BUDGET}s" >&2
    exit 1
fi
elected_after=$(( $(now) - killed_at ))
echo "new leader: node $new_leader, ${elected_after}s after the kill"

while [ -z "$(resumed_after "$killed_at")" ]; do
    if [ $(( $(now) - killed_at )) -ge "$RESUME_BUDGET" ]; then
        echo "FAIL: no send started after the kill succeeded within ${RESUME_BUDGET}s" >&2
        exit 1
    fi
    sleep 0.5
done
resumed="$(resumed_after "$killed_at")"
echo "a send started after the kill succeeded ${resumed}s after it"
show_cluster
show_in_sync

echo
echo "== Starting $victim again"
docker start "$victim" >/dev/null
started_at="$(now)"
rejoined=""
while [ $(( $(now) - started_at )) -lt "$REJOIN_BUDGET" ]; do
    if in_sync_everywhere "$old_leader"; then
        rejoined="yes"
        break
    fi
    sleep 1
done
if [ -z "$rejoined" ]; then
    echo "FAIL: $victim was not back in sync within ${REJOIN_BUDGET}s" >&2
    exit 1
fi
echo "$victim back in sync on every partition $(( $(now) - started_at ))s after starting"
show_cluster
show_in_sync

kill "$traffic_pid" 2>/dev/null || true
wait "$traffic_pid" 2>/dev/null || true
traffic_pid=""

echo
echo "== Summary"
echo "leader:        node $old_leader killed, node $new_leader elected after ${elected_after}s"
echo "sends resumed: a send started after the kill succeeded ${resumed}s after it"
echo "sends:         $(sends_ok_since 0) ok, $(sends_failed_since 0) failed"
# A send in flight when the leader died started up to a second before the
# kill, so "around the kill" starts one second early.
echo "slowest send:  $(slowest_between 0 $(( killed_at - 1 )))s before the kill, $(slowest_between $(( killed_at - 1 )) "$(( $(now) + 1 ))")s around and after it"
echo "failed sends by status:"
awk '$2 !~ /^2/ { print "  HTTP " $2 }' "$send_log" | sort | uniq -c
echo "PASS"
