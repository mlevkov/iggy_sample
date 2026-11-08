# Apache Iggy Durable Storage Guide

How Apache Iggy stores messages, what each durability and retention setting
guarantees, and how to set them through this gateway.

> **Validated against Apache Iggy server 0.10.0 and Rust SDK 0.12.0**
> (upstream tag `server-0.10.0`, commit `91e02063`): the shipped
> `core/server/config.toml`, the SDK's option catalog, and a running
> `apache/iggy:0.10.0` container, on 2026-11-08. The 0.8 edition of this guide
> is in git history; most of its configuration no longer exists (see
> [Removed since 0.8](#removed-since-08)).

## Table of Contents

1. [Storage Architecture Overview](#storage-architecture-overview)
2. [Durability Configuration](#durability-configuration)
3. [Segment Management](#segment-management)
4. [Data Retention Policies](#data-retention-policies)
5. [Backup and Archiving](#backup-and-archiving)
6. [Recovery and Data Integrity](#recovery-and-data-integrity)
7. [Performance vs Durability Trade-offs](#performance-vs-durability-trade-offs)
8. [Production Recommendations](#production-recommendations)
9. [Current Limitations](#current-limitations)
10. [Configuration Reference](#configuration-reference)
11. [Sources](#sources)

---

## Storage Architecture Overview

### Append-Only Log Model

Each partition is an append-only log. Messages are written in batches to the
partition's open *segment*; when the segment reaches the topic's
`segment_size`, it is sealed and a new one opens. Retention deletes whole
sealed segments, oldest first, and never touches the open one.

Since 0.9 every write also goes through consensus (VSR, viewstamped
replication), even on a single node. Each partition keeps a write-ahead log
(WAL) of *prepares*: an operation is journaled there, committed, then applied
to the segment. Recovery after a crash replays the WAL.

```
Producer ──► partition primary ──► WAL prepare ──► commit ──► segment append
                                    (prepares-1/)              (*.log, *.index)
```

### On-Disk Layout

Observed in a running `apache/iggy:0.10.0` container after creating one
stream, one topic and sending one message:

```
local_data/
├── storage-format              # format marker; 0.10 refuses other formats
├── metadata/journal.wal        # metadata WAL: streams, topics, users, ...
├── state/
├── partition-initialization/
├── runtime/current_config.toml # the effective server config
├── logs/iggy-server.log
└── streams/0/topics/0/partitions/0/
    ├── 00000000000000000000.log    # segment: message batches
    ├── 00000000000000000000.index  # sparse offset/time index
    ├── superblock.a                # partition consensus state
    ├── created.revision
    ├── offsets/                    # stored consumer offsets
    └── prepares-1/                 # partition WAL
        ├── prepares-0.wal
        ├── frontier
        └── segment-0-0.log         # hard link to the segment above
```

Stream, topic and partition directories are numbered from 0. The WAL holds
segment bodies by hard link until it reclaims them, so a segment and its WAL
reference are one file on disk, not two.

### Storage Location

| Setting | Value |
|---|---|
| Server key | `path` (environment override `IGGY_PATH`) |
| Shipped default | `local_data`, relative to the image's working directory `/app` |
| This repository's compose stack | volume `iggy_data_0_10` mounted at `/app/local_data` |

The volume name carries the release line because 0.10 refuses a 0.9 data
directory: "storage at /iggy/data does not match this build's storage format;
startup refused without modifying data. No in-place upgrade is supported"
([ADR-0002](adr/0002-iggy-0.10-before-topic-options.md)). Upgrading means
starting on an empty directory, or migrating data out and back in through
clients.

---

## Durability Configuration

### Durability Is Per Topic

Up to 0.8, durability was server-wide: `[system.partition] enforce_fsync`,
the background `[message_saver]`, and flush thresholds in `config.toml`. Since 0.9 these are
**per-topic options**, set when the topic is created and fixed from then on.
0.9 rejects `enforce_fsync` at boot, and 0.10's shipped config has no message
saver.

Two options decide what a successful call has waited for:

| Option | Covers | Values | Default |
|---|---|---|---|
| `durability` | Message sends | `replicated`, `persisted` | `replicated` |
| `consumer_offset_durability` | Explicit consumer-offset stores and deletes | `replicated`, `persisted` | `replicated` |

They are independent: neither inherits the other.

### The Two Policies

Both policies write data to storage. They differ in what has happened before
the server answers success:

- **`replicated`** waits for the VSR commit and its application, without an
  extra stable-storage barrier. After a crash, a replicated partition recovers
  up to the last WAL frontier it published: at checkpoints, durable operations
  and graceful shutdown. Writes acknowledged after that frontier can be lost
  in a crash.
- **`persisted`** also requires recoverable stable-storage copies at the
  replication quorum before success. On a single node the quorum is that one
  replica, so success follows local persistence.

Two caveats from the server's own documentation:

- A persisted prepare is limited to 64 MiB, including the consensus header.
  Larger operations are rejected permanently.
- **Poll auto-commit is asynchronous.** A successful poll does not confirm that
  the offset it submitted committed or became durable, whatever
  `consumer_offset_durability` says. This gateway's `auto_commit=true` polls
  use exactly that path (`CommitPolicy::OnRead`). Where an offset must survive
  a crash, store it explicitly.

### Flush Scheduling

| Option | Default | Range |
|---|---|---|
| `messages_required_to_save` | 1024 | 1 to 16777216 |
| `size_of_messages_required_to_save` | 1 MiB | 1 B to 1 GiB |

These thresholds schedule ordinary segment writes from the in-memory buffer.
They are **not** a durability setting: the WAL, not the segment, is what
recovery relies on, and required persistence, capacity pressure and lifecycle
operations flush sooner anyway. Under 0.8, `messages_required_to_save = 1` was
the way to get per-message durability. On 0.10, use `durability: persisted`
for that, and tune these only for throughput and latency.

### Setting Options Through This Gateway

Create a topic with an `options` object:

```bash
curl -X POST http://localhost:8000/streams/my-stream/topics \
  -H "Content-Type: application/json" \
  -d '{
    "name": "payments",
    "partitions": 3,
    "options": {
      "durability": "persisted",
      "consumer_offset_durability": "persisted"
    }
  }'
```

The default topic takes the same options from `IGGY_TOPIC_<OPTION>`
environment variables, e.g. `IGGY_TOPIC_DURABILITY=persisted`. They apply only
when the gateway creates the topic. For a topic that already exists, the
gateway logs a warning for each option that differs and leaves the topic as
it is.

The gateway validates every option against the server's bounds before sending
it. An invalid value answers `400` naming the option, and an unknown key
answers `422`, so a typo cannot silently leave a topic on `replicated`. The
full option table is in the [README](../README.md#create-a-topic).

### Reading Options Back

`GET /streams/{stream}/topics/{topic}` reports every option with its stored
value and where it came from:

```json
"options": {
  "durability": {"value": "persisted", "source": "explicit"},
  "consumer_offset_durability": {"value": "replicated", "source": "explicit"},
  "message_expiry": {"value": "unlimited", "source": "default"},
  "segment_size": {"value": "1 GiB", "source": "default"}
}
```

- `explicit`: the client that created the topic sent the value.
- `default`: the server filled it in when it admitted the topic.
- `unset`: the topic stores no value (`value` is `null`).

SDK 0.12 always sends both durability options, so on any topic created
through the SDK, this gateway included, they read `explicit` even when the
caller omitted them. An omitted one reads `replicated`. Check `value`, not
`source`, to know a topic's guarantee.

---

## Segment Management

### Segment Size

`segment_size` is per topic: 1 MiB to 1 GiB, in multiples of 512 B, with a
default of 1 GiB. It is a soft limit, since a segment may close one whole
batch past it.

| Smaller segments | Larger segments |
|---|---|
| Retention frees space in finer steps | Fewer files per partition |
| A small `max_topic_size` becomes possible (it must be at least one segment) | Less per-segment overhead |
| More files and more index entries to recover | Retention frees space in 1 GiB steps |

### Preallocation

`preallocate_segments: true` reserves each segment's full `segment_size` on
disk when it opens, where the filesystem supports it. It is off by default:
with the default 1 GiB segments it costs 1 GiB of real disk per partition as
soon as the topic is created. The server caps what one topic may reserve,
`segment_size` times the partition count, at 64 GiB, and the gateway checks
that cap before creating the topic.

### Index Files and Checksums

Each segment has a sparse `.index` file for offset and timestamp lookups.
Message batches carry checksums. `[partition] validate_checksum` (default
`true` in 0.10) verifies them on every read from segment storage, and a
mismatch is reported rather than served. Recovery validates batches against
their checksums and partition identity whatever this setting says.

### Partition WAL Budget

| Key (`[partition]`) | Default | Meaning |
|---|---|---|
| `wal_bytes_max` | `256 MiB` | Per-partition budget for the active WAL plus queued and in-flight prepares. Checkpointing starts at half. Full message payloads count against it under both policies |
| `wal_group_commit_delay_micros` | `0` (off) | For `persisted` partitions: wait up to this long for more prepares, so one storage barrier covers a group. Trades acknowledgment latency for fewer device writes; durability is unchanged |
| `prepare_queue_depth` | `32` | Uncommitted sends in flight per partition. Past it and its spill queue, the server answers `TransientNotAccepted`, which this gateway returns as a retryable `503` |

---

## Data Retention Policies

Retention is per topic, set at creation, and deletes whole sealed segments.
The active segment is never touched.

### Time-Based Retention (Message Expiry)

`message_expiry` is a duration such as `"7days"`, `"12h"` or `"1h 30m"`, or
`"unlimited"` (the default) to keep messages forever. Sealed segments older
than the expiry are deleted.

### Size-Based Retention (Max Topic Size)

`max_topic_size` is a size such as `"10 GiB"`, or `"unlimited"` (the default).
Past it, the oldest sealed segments are deleted.

- **It is best-effort, not a hard disk bound.** The cap is enforced per
  partition, as the cap divided by the partition count. The active segment is
  never trimmed, and a lagging committed consumer pins the sealed segments
  behind it.
- **A finite cap must be at least the topic's segment size.** With the default
  1 GiB segments, a cap below 1 GiB is refused. Set a smaller `segment_size`
  for a smaller cap. The server refuses such a topic too, but its 0.10 error
  message prints neither value, so the gateway checks first and says what the
  floor is.

Both policies can be active at once.

### The Cleaner

Retention runs in the server's segment cleaner:

```toml
[data_maintenance.messages]
cleaner_enabled = true   # default in 0.10
interval = "1 m"
```

It is on by default in 0.10. (Under 0.8 it was off by default, and retention
silently did nothing until it was enabled.)

### Retention Strategy by Use Case

| Use case | `options` |
|---|---|
| Event sourcing, audit log | `{"durability": "persisted", "message_expiry": "unlimited", "max_topic_size": "unlimited"}` |
| Payments, orders | `{"durability": "persisted", "consumer_offset_durability": "persisted", "message_expiry": "90days"}` |
| Application events | `{"message_expiry": "7days"}` |
| Metrics, logs | `{"message_expiry": "24h", "max_topic_size": "10 GiB", "segment_size": "256 MiB"}` |
| Ephemeral notifications | `{"message_expiry": "1h", "max_topic_size": "1 GiB", "segment_size": "64 MiB"}` |

---

## Backup and Archiving

There is no built-in archiver: it was removed before 0.8. Back up the data
directory, or stream data out through clients.

### Filesystem-Level Backup

- **Take a consistent copy.** Stop the server, or snapshot the volume (LVM,
  ZFS or a cloud disk snapshot) and copy from the snapshot. A copy of a live
  directory can catch a segment and its WAL mid-write.
- **Preserve hard links.** WAL references are hard links to segment files
  ([On-Disk Layout](#on-disk-layout)). `rsync -aH` or `cp -a` keep them as
  links. Copying them as separate files doubles their size in the backup.
- **Restore onto the same release line.** 0.10 refuses data from another
  storage format without modifying it. The server's own config notes that a
  rollback after an upgrade needs a consistent pre-upgrade backup.

For the compose stack:

```bash
docker compose stop iggy
# tar keeps hard links as links
docker run --rm -v iggy_sample_iggy_data_0_10:/data -v "$PWD":/backup \
  alpine tar czf /backup/iggy-data.tgz -C /data .
docker compose start iggy
```

### Streaming Data Out

Upstream's connectors runtime (`apache/iggy-connect`) can copy topics to
external sinks continuously. ADR-0001's connectors demo adds an HTTP source
and sink to this repository.

---

## Recovery and Data Integrity

### What Recovery Does

From the server's documented recovery behaviour at 0.10:

- Message batches are validated against their checksums and partition
  identity before any data is served.
- A genuinely incomplete tail can be truncated. Damage in the interior of the
  log, or valid records after a damaged range, are refused or repaired, never
  blindly truncated.
- **`replicated` partitions** check the log from byte zero, because background
  writeback can preserve later pages before earlier ones.
  **`persisted` partitions** can use the faster index-anchored path, since
  their completed flushes constrain the incomplete tail.
- Unrecoverable local segment data is fenced and repaired from peers when
  peers exist. **A single node cannot fetch a missing copy from anywhere.**
  Fenced files are kept for diagnosis.
- Recovery never replaces an acknowledged durable history with an empty
  partition.
- Index reconstruction across many segments can take time. A slow start is not
  by itself a stalled server.

### Upgrades

0.10 checks the `storage-format` marker at boot and refuses a directory it
does not match, without modifying it. There is no in-place upgrade from 0.9.
Keep the old volume until the new deployment is verified; this repository's
compose stack uses a new volume per release line for that reason.

### Preventing Data Loss

| Risk | Mitigation |
|---|---|
| Crash loses recent `replicated` writes | `durability: persisted` for data that must survive a crash |
| Auto-committed offset lost in a crash | Store offsets explicitly for consumers that must not reprocess or skip |
| Disk loss on a single node | Volume snapshots or backups; a cluster for replica repair |
| Disk full | `max_topic_size` and `message_expiry` on every unbounded topic; monitor free space |
| Silent corruption | Keep `validate_checksum = true` (the default) |

---

## Performance vs Durability Trade-offs

`persisted` adds a stable-storage barrier to every send's acknowledgment, so
its cost depends on the device's barrier latency, not on Iggy. Measure it on
your hardware before you choose; this guide quotes no figures, because none
were measured for 0.10.

| Profile | Topic options | Server tuning |
|---|---|---|
| **Throughput** (metrics, logs) | defaults (`replicated`), short `message_expiry` | none |
| **Balanced** (most applications) | `replicated` messages, `persisted` consumer offsets if offsets are stored explicitly | none |
| **Maximum durability** (payments, ledgers) | `persisted` for both | `wal_group_commit_delay_micros` near the measured barrier time, if throughput suffers |

Flush thresholds (`messages_required_to_save`,
`size_of_messages_required_to_save`) move cost between batching and latency.
They do not change what an acknowledgment guarantees.

### Hardware

- **NVMe SSDs** keep barrier latency low, which matters most for `persisted`.
- **Separate volumes** for data and the OS keep log I/O from competing.
- **Filesystem:** preallocation needs a filesystem that supports it (ext4 and
  XFS do).

---

## Production Recommendations

### Server Configuration

0.10's defaults are reasonable for most deployments. The keys that matter for
storage:

```toml
# Apache Iggy server 0.10.0 - storage-related keys (shipped defaults)
path = "local_data"                    # mount a dedicated volume here

[partition]
validate_checksum = true               # keep on
wal_bytes_max = "256 MiB"              # raise for high-throughput partitions
wal_group_commit_delay_micros = 0      # tune only for persisted topics

[data_maintenance.messages]
cleaner_enabled = true                 # retention runs only while this is on
interval = "1 m"

[metadata]
partitions_max = 0                     # 0 = no cap on total partitions
```

Each key has an environment override. Upstream documents `IGGY_PATH` and
`IGGY_PARTITION_CONSUMER_OFFSETS_MAX` by name, and the other keys follow the
same `IGGY_<SECTION>_<KEY>` pattern.

### Topic Defaults

Decide durability per topic rather than per server, and set it explicitly:

```bash
# The gateway's default topic, from the environment
IGGY_TOPIC_DURABILITY=persisted
IGGY_TOPIC_CONSUMER_OFFSET_DURABILITY=persisted
IGGY_TOPIC_MESSAGE_EXPIRY=30days
```

### Checklist

- [ ] The data directory is on a dedicated, monitored volume.
- [ ] Every topic that matters has `durability: persisted`, confirmed with a
      `GET` on the topic.
- [ ] Every topic has `message_expiry` or `max_topic_size`, unless it is meant
      to grow forever.
- [ ] Backups are consistent snapshots, keep hard links, and have been
      restored at least once.
- [ ] Consumers that must not lose position store offsets explicitly.

### Monitoring

The server's `/metrics` endpoint (it needs a bearer token since 0.9; the
compose stack's Prometheus uses a personal access token) exposes per-shard
WAL metrics, among them:

| Metric | Meaning |
|---|---|
| `partition_wal_retained_bytes`, plus the queued and in-flight byte gauges | Together, the WAL budget in use (against `wal_bytes_max`) |
| `partition_wal_disk_bytes` | WAL file length, excluding referenced segment bodies |
| `partition_wal_prepares_total` / `partition_wal_batches_total` | Prepares per durable group |
| `partition_wal_group_commit_waits_total` | Groups that took the optional commit wait |

Alert on disk usage and on sustained `TransientNotAccepted` (`503` from this
gateway), which means a partition's queues are full.

---

## Current Limitations

- **Single node in this repository.** The compose stack runs one node, so
  there is no replica to repair from and `persisted` means local persistence
  only. ADR-0001's clustering demo adds a three-node stack.
- **Options are fixed at creation here.** SDK 0.12 can update some topic
  options, but this gateway does not offer updates (ADR-0002). Changing a
  topic's durability or segment size means creating a new topic.
- **`compression_algorithm` is stored but not applied** by the server, so the
  gateway does not offer it.
- **Replication factor** is a per-topic option since 0.10
  (`replication_factor`), meaningful on a cluster.

---

## Configuration Reference

### Per-Topic Options

| Option | Values | Default |
|---|---|---|
| `durability` | `replicated`, `persisted` | `replicated` |
| `consumer_offset_durability` | `replicated`, `persisted` | `replicated` |
| `message_expiry` | duration (`"7days"`) or `"unlimited"` | `unlimited` |
| `max_topic_size` | size (`"10 GiB"`) or `"unlimited"`; at least `segment_size` | `unlimited` |
| `segment_size` | 1 MiB to 1 GiB, multiple of 512 B | `1 GiB` |
| `messages_required_to_save` | 1 to 16777216 | `1024` |
| `size_of_messages_required_to_save` | 1 B to 1 GiB | `1 MiB` |
| `preallocate_segments` | `true`, `false`; at most 64 GiB reserved per topic | `false` |
| `replication_factor` | 1 to 32, at most the node count | server's `default_replication_factor` (`"all"`) |

Sizes accept binary (`MiB`, `GiB`) and decimal (`MB`, `GB`) units. Iggy's
spellings for "server default" (`"server_default"`, `"0"`) are refused: omit
the option instead.

### Gateway Environment Variables

`IGGY_TOPIC_<OPTION>` for any option above, applied when the gateway creates
the default topic (`IGGY_STREAM`/`IGGY_TOPIC`, `IGGY_PARTITIONS` partitions).
Unset or empty leaves the option to the server, and an invalid value fails
startup naming the variable.

### Removed Since 0.8

| 0.8 setting | Since 0.9 |
|---|---|
| `[system.partition] enforce_fsync` | Rejected at boot by 0.9. Use per-topic `durability` |
| `[message_saver]` | No saver in 0.10's config. The WAL and the durability policy cover persistence |
| `[system.partition] messages_required_to_save`, `size_of_messages_required_to_save` | Per-topic options |
| `[system.segment] size` | Per-topic `segment_size` |
| `[system.topic]` default expiry and size | Per-topic options; the defaults are fixed by the option catalog |
| `[data_maintenance.messages] cleaner_enabled = false` | Now `true` by default |
| `IGGY_SYSTEM_*` variables | `[system]` was flattened, so `SYSTEM_` is dropped (`IGGY_SYSTEM_PATH` is now `IGGY_PATH`) |

---

## Sources

- Server configuration with inline documentation:
  `core/server/config.toml` at
  [`server-0.10.0`](https://github.com/apache/iggy/releases/tag/server-0.10.0)
  (`91e02063`), sections `[partition]`, `[metadata]` and
  `[data_maintenance.messages]`.
- SDK option catalog and bounds: `iggy_common` 0.12.0,
  `src/types/options/mod.rs` and `src/types/options/catalog.rs`.
- [ADR-0001](adr/0001-iggy-0.9-upgrade-and-demos.md) and
  [ADR-0002](adr/0002-iggy-0.10-before-topic-options.md): the upgrade path and
  what was verified for each release.
- [Apache Iggy documentation](https://iggy.apache.org/docs/).
