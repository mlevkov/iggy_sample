# Three-Node Cluster Demo

A three-node Apache Iggy cluster with the gateway in front of it. The demo
shows what clustering buys and costs:
- leader failover under live traffic;
- per-topic replication factors;
- a gateway that starts on whichever node is up.

ADR-0001 planned it as PR 3, and ADR-0002 added the per-topic replication
factor. Everything here was verified against `apache/iggy:0.10.0` and SDK
0.12.0 on 2026-11-08.

## Contents

1. [Running It](#running-it)
2. [Layout](#layout)
3. [Reading the Cluster](#reading-the-cluster)
4. [Replication Factor](#replication-factor)
5. [Failover](#failover)
6. [Seeds](#seeds)
7. [Tests](#tests)
8. [Limitations](#limitations)

## Running It

```bash
# A pre-shared key for the replica links: at least 32 random bytes
echo "IGGY_CLUSTER_SHARED_SECRET=$(openssl rand -hex 32)" >> .env

docker compose -f docker-compose.cluster.yaml up -d --build --wait
curl http://localhost:8001/cluster

# Kill the leader under traffic and watch the cluster recover
scripts/cluster-failover-demo.sh

# Stop, keeping the node volumes; add -v to delete them
docker compose -f docker-compose.cluster.yaml down
```

The stack is its own compose project (`iggy-sample-cluster`), so it shares
nothing with `docker-compose.yaml` and can run beside it. Its gateway
listens on **8001**.

## Layout

```
host ──8001──► gateway (iggy-cluster-app) ── TCP ──► iggy-node-0  10.29.0.10
                10.29.0.128/25                   ├─► iggy-node-1  10.29.0.11
                                                 └─► iggy-node-2  10.29.0.12
                                  replica links (VSR, PSK-authenticated) between nodes
```

| Choice | Why |
|---|---|
| **Fixed node IPs** on `10.29.0.0/24` | The roster names nodes by literal IP. `IGGY_CLUSTER_SUBNET` (first three octets) moves the subnet if it clashes with a local network |
| **No Iggy port published** | After sign-in, the SDK dials every node at the address the roster advertises. Only the gateway, inside the network, talks to Iggy, so each node advertises its roster IP and nothing needs host-specific addresses |
| **All three nodes as seeds** | See [Seeds](#seeds) |
| **`IGGY_CLUSTER_SHARED_SECRET`**, required | Replica links authenticate with a pre-shared key of at least 32 bytes. Compose refuses to start without it |
| **TCP only** | HTTP, QUIC and WebSocket are off. The gateway needs only TCP, and HTTP in cluster mode would need a cluster-wide JWT key besides |
| **One volume per node** (`iggy_node_<N>_0_10`) | Each node keeps its own copy of the data it holds |
| **Two unpinned shards per node** | Three servers share one host |
| **`restart: "no"` for nodes** | A node the demo kills stays down until it is started again |

The cluster name (`iggy-sample-cluster`) is stamped into each node's data on
first boot. Renaming it later means `down -v` and a fresh start.

## Reading the Cluster

`GET /cluster` reports the roster as the connected node sees it:

```json
{
  "name": "iggy-sample-cluster",
  "leader": 0,
  "nodes": [
    {"replica_id": 0, "name": "iggy-node-0", "address": "10.29.0.10:8090", "role": "leader", "status": "healthy"},
    {"replica_id": 1, "name": "iggy-node-1", "address": "10.29.0.11:8090", "role": "follower", "status": "healthy"},
    {"replica_id": 2, "name": "iggy-node-2", "address": "10.29.0.12:8090", "role": "follower", "status": "healthy"}
  ]
}
```

- **`role`** is the node's role in metadata consensus, which handles stream,
  topic and user changes. Each partition has its own primary, placed
  separately.
- **`status` is not liveness.** Server 0.10 reports `healthy` for every
  roster node, even one that has been down for minutes.
- **Use `in_sync_members` instead.** A topic read reports, for each
  partition, the nodes that hold it (`members`) and the ones currently
  caught up (`in_sync_members`). A dead node drops out of `in_sync_members`
  within seconds and returns once it has caught up after a restart:

```bash
curl -s http://localhost:8001/streams/sample-stream/topics/events | jq -c '.partitions[]'
{"id":0,"messages_count":42,"members":[0,1,2],"in_sync_members":[0,1]}   # node 2 stopped
```

Against a server with clustering off, `GET /cluster` reports one node in a
cluster named `single-node`.

## Replication Factor

Since server 0.10, each topic chooses how many nodes store each of its
partitions. `replication_factor` is a topic option: set it on create, or
with `IGGY_TOPIC_REPLICATION_FACTOR` for the default topic. It runs from 1
to 32 and at most the node count: on three nodes, `4` answers
`400 Invalid replication factor`. Omitted, the server's
`default_replication_factor` applies, which is `"all"` (every node).

Writes commit on a quorum of a partition's members, so on three nodes:

| Factor | Copies | While one node is down |
|---|---|---|
| omitted (`"all"`) or `3` | 3 | Partitions keep accepting writes on the other two |
| `2` | 2 | A partition with a copy on the dead node stops accepting writes until it returns |
| `1` | 1 | A partition on the dead node is unavailable until it returns |

```bash
curl -X POST http://localhost:8001/streams/sample-stream/topics \
  -H "Content-Type: application/json" \
  -d '{"name": "two-copies", "partitions": 3, "options": {"replication_factor": 2}}'

curl -s http://localhost:8001/streams/sample-stream/topics/two-copies |
  jq -c '{rf: .options.replication_factor, partitions: [.partitions[] | .members]}'
{"rf":{"value":2,"source":"explicit"},"partitions":[[0,1],[1,2],[0,1]]}
```

A topic read reports the factor that admission resolved and stored. The
default topic reads `{"value": 3, "source": "default"}`. On a single node,
every topic reads `1`.

## Failover

`scripts/cluster-failover-demo.sh` runs the failover end to end:
1. sends through the gateway five times a second;
2. SIGKILLs the metadata leader's container;
3. waits for a new leader and a successful send;
4. restarts the node and waits until it is back in sync on every partition.

A typical run on Docker Desktop:

```
== Killing the leader, iggy-node-0 (SIGKILL)
new leader: node 1, 5s after the kill
...
  partition 0  in sync: 1,2
== Starting iggy-node-0 again
iggy-node-0 back in sync on every partition 4s after starting
== Summary
leader:        node 0 killed, node 1 elected after 5s
sends:         44 ok, 0 failed
slowest send:  0.0s before the kill, 4.3s around and after it
PASS
```

What happens, and why:

- **The election takes about the heartbeat timeout.** Followers elect a new
  leader once the old one has been silent for `heartbeat_timeout`, 5 s by
  default (`IGGY_CLUSTER_HEARTBEAT_TIMEOUT` in the compose file). Raise it on
  an overloaded host, where scheduling stalls can look like a dead leader.
- **The switch shows as delay, not errors.** SDK 0.12 sends each message to
  its partition's primary over that node's own link. When the primary dies,
  the SDK holds the call and retries until a new primary answers, within a
  30 s deadline per call. In the runs above, no send failed; the slowest
  waited about as long as the election.
- **What a caller sees if a send does fail.** A send can still fail if the
  election outlasts the gateway's request deadline (`X-Request-Timeout`, or
  `OPERATION_TIMEOUT_SECS`), or if its outcome became uncertain:
  - `504 timeout`;
  - `503 not_accepted`, which is safe to retry;
  - `503 outcome_unknown`, where the write may or may not have happened.

  Upstream's own failover test allows any error until the first success,
  within 10 s.
- **The gateway stays connected.** Losing one node of three does not end the
  SDK's session: it keeps links to the other nodes, so `/health` and
  `/ready` stay up throughout.
- **The killed node catches up on its own.** It repairs its log from its
  peers, and rejoins `in_sync_members` a few seconds after it starts.
- **Then partition primaries move back to it, also on their own.** Server
  0.10 keeps primaries spread over the nodes, with no command or setting
  for it (apache/iggy#4619). The metadata primary probes the nodes every
  second. Once they have been ready for 5 s, it moves a partition whose
  primary leads at least two more partitions than another member. With
  the default topic's three partitions led one per node, the node that
  took over the killed node's partition leads two and the rejoined node
  none, so that partition moves back. Moves go one at a time: the old
  primary drains for up to 1 s, then a view change hands the partition
  over. Sends to that partition can wait during the move, like a short
  failover. The script stops sending once the node is back in sync, so
  its figures do not include this. This is upstream's design, read from
  the source at `91e02063` (`core/shard/src/primary_rebalance.rs` and the
  scan in `core/metadata/src/impls/placement.rs`); the demo does not
  measure it.

## Seeds

An SDK 0.12 connection string names one server. Once signed in, the client
learns the roster and keeps links to every node, but it has to reach that
one server first. That happens at startup, and again whenever the gateway's
reconnect logic builds a fresh client.

`IGGY_CONNECTION_STRING` therefore accepts several comma-separated
addresses:

```
iggy://iggy:iggy@10.29.0.10:8090,10.29.0.11:8090,10.29.0.12:8090
```

- **Startup** tries the seeds in order, each with an equal share of
  `OPERATION_TIMEOUT_SECS`. With the first node down, the gateway starts on
  the second about 10 s later (a third of the default 30 s).
- **Reconnects** start from the seed that worked last and move to the next
  one on a failure.
- **A protocol mismatch** stops the search at once, since every node runs
  the same release.

The gateway logs seed addresses, never the connection string, which holds
the credentials.

## Tests

The weekly Extended Tests workflow (`.github/workflows/extended-tests.yml`,
"Cluster Tests") brings the stack up on a GitHub runner, runs
`tests/cluster_tests.rs`, and then runs the failover script, which exits
non-zero if a step misses its budget.

The tests are `#[ignore]`d, because they need the stack and the docker CLI.
They check:
- the roster: three nodes, exactly one leader;
- replication-factor placement: two members per partition at factor 2, all
  three by default;
- that the gateway starts with its first seed stopped.

To run them locally:

```bash
docker compose -f docker-compose.cluster.yaml up -d --build --wait
cargo test --test cluster_tests -- --ignored --test-threads=1
scripts/cluster-failover-demo.sh
```

## Limitations

- **Failover, not elasticity.** The roster is static, and a partition's
  members are fixed when it is created. Adding or removing nodes, and
  moving a partition's replicas between them, is not part of this demo.
  Only primaries move, by themselves, among a partition's members (see
  [Failover](#failover)).
- **One host.** All three nodes share a machine, so the demo shows the
  protocol, not independence from hardware faults.
- **Node `status` is not liveness** (see [Reading the Cluster](#reading-the-cluster)).
- **Upgrades are cluster-wide.** Server 0.10 nodes cannot sustain a
  primary's quorum checks against nodes of another version, so all nodes
  move together (ADR-0002).
