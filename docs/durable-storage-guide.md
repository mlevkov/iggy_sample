# Apache Iggy Durable Storage Guide

A comprehensive guide to configuring Apache Iggy for durable, production-ready message storage.

> **Validated key-by-key against Apache Iggy server 0.8.0** (upstream tag `server-0.8.0`,
> shipped default config `core/server/config.toml` and config sources in
> `core/configs/src/server_config/`) on 2026-07-05.

---

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

---

## Storage Architecture Overview

### Append-Only Log Model

Iggy uses an **append-only log** as its core storage abstraction. This is the same fundamental design used by Kafka, Apache BookKeeper, and other high-performance streaming systems.

```
Topic: "orders" (3 partitions)
├── partitions/0/
│   ├── 00000000000000000000.log     (messages from offset 0)
│   ├── 00000000000000000000.index   (offset + timestamp index)
│   ├── 00000000000001000000.log     (messages from offset 1,000,000)
│   └── ...
├── partitions/1/
│   └── ...
└── partitions/2/
    └── ...
```

Segment files are named by their 20-digit zero-padded start offset. Each segment
has a single `.index` companion file covering both positional and time-based lookups
(older Iggy versions used separate `.idx`/`.tidx` files).

### Key Characteristics

| Property | Description |
|----------|-------------|
| **Immutable records** | Messages cannot be modified once written |
| **Sequential writes** | Optimized for append operations |
| **Indexed access** | Fast lookups by offset and timestamp |
| **Segment-based** | Data split into manageable chunks |

### Storage Location

By default, all data is persisted under the `local_data` directory. Configure
the path in the server's `config.toml` (server 0.8.0 embeds a default config;
point `IGGY_CONFIG_PATH` at your own file to override it):

```toml
[system]
path = "local_data"  # Change to your preferred location
```

Or via environment variable:

```bash
export IGGY_SYSTEM_PATH=/var/lib/iggy/data
```

---

## Durability Configuration

Durability in Iggy is controlled by **fsync** settings. fsync forces the operating system to flush data from kernel buffers to the physical storage device, ensuring data survives crashes.

### The fsync Trade-off

```
Without fsync (default):
┌─────────┐    ┌─────────────┐    ┌──────────┐
│  Iggy   │ → │ OS Buffer   │ → │   Disk   │
│ Server  │    │   Cache     │    │          │
└─────────┘    └─────────────┘    └──────────┘
                     ↑
              Data lives here temporarily.
              Lost on power failure or crash.

With fsync enabled:
┌─────────┐    ┌─────────────┐    ┌──────────┐
│  Iggy   │ → │ OS Buffer   │ ═══│   Disk   │
│ Server  │    │   Cache     │ ↑  │          │
└─────────┘    └─────────────┘ │  └──────────┘
                               │
                    fsync() forces write to disk
                    before acknowledging message.
```

### Core Durability Settings

#### 1. Message Saver Configuration

The top-level `[message_saver]` section controls background persistence
(defaults shown are the shipped 0.8.0 defaults):

```toml
[message_saver]
# Enable the background process that saves buffered data to disk
enabled = true

# Force synchronous writes to disk
# true  = durable but slower (data guaranteed on disk)
# false = faster but data may be lost on crash
enforce_fsync = true

# Interval for running the message saver (default: 30 s)
interval = "30 s"
```

#### 2. Per-Message fsync (Maximum Durability)

For the strongest durability guarantee, force fsync after every message:

```toml
[system.partition]
# Force fsync on every partition write (default: false)
enforce_fsync = true

# Flush after each message (set to 1 for maximum durability; default: 1024)
messages_required_to_save = 1

# Companion size threshold - a save is also triggered once buffered
# messages reach this size (default: "1 MiB")
size_of_messages_required_to_save = "1 MiB"
```

Or via environment variables:

```bash
export IGGY_SYSTEM_PARTITION_ENFORCE_FSYNC=true
export IGGY_SYSTEM_PARTITION_MESSAGES_REQUIRED_TO_SAVE=1
```

### Durability Levels

| Level | Configuration | Latency | Data Loss Risk |
|-------|---------------|---------|----------------|
| **Maximum** | `[system.partition]` `enforce_fsync=true`, `messages_required_to_save=1` | ~9ms P99 | None (survives power loss) |
| **High** | `[system.partition]` `enforce_fsync=true`, `messages_required_to_save=100` | ~1-2ms P99 | Up to 100 messages |
| **Balanced** | `[system.partition]` `enforce_fsync=false` + `[message_saver]` `enforce_fsync=true`, `interval="1s"` | ~0.5ms P99 | Up to 1 saver interval of data |
| **Performance** | `enforce_fsync=false` everywhere | ~0.1ms P99 | Unbounded (OS-dependent) |

### Performance Benchmarks

With fsync-per-message enabled, Iggy achieves impressive durability performance:

| Metric | Value | Notes |
|--------|-------|-------|
| P99 Producer Latency | ~9.48ms | With `enforce_fsync=true`, `messages_required_to_save=1` |
| P9999 Producer Latency | ~9.48ms | Consistent even at tail |
| P99 Consumer Latency | <3ms | Reading from durable storage |

Single-digit millisecond latencies at P9999 for a fully durable workload is excellent performance, competitive with or exceeding many other streaming platforms.

---

## Segment Management

Iggy stores messages in **segments**, which are physical files on disk.

### Segment Configuration

```toml
[system.segment]
# Soft limit for segment size before rolling to a new segment (default: "1 GiB")
# Hard maximum is 1 GiB, and the size must be a multiple of 512 B.
# Larger = fewer files, potentially slower recovery
# Smaller = more files, faster recovery, more overhead
size = "1 GiB"

# Index caching strategy (single .index file covers offset + time lookups):
#   "all" (or true)    = keep all indexes in memory (fastest, most memory)
#   "open_segment"     = cache indexes only for the currently open segment (default)
#   "none" (or false)  = always read indexes from disk (least memory)
cache_indexes = "open_segment"

[system.partition]
# Validate checksums (CRC) when loading data (integrity check; default: false)
validate_checksum = true
```

> **Changed in 0.8.0**: `cache_indexes` is now a string enum (`"all"`,
> `"open_segment"`, `"none"`) rather than a boolean, the separate
> `cache_time_indexes` option was removed (one combined index file), and
> `validate_checksum` lives under `[system.partition]`, not `[system.segment]`.

### Segment Lifecycle

```
1. ACTIVE: Current segment receiving writes
   └── 00000000000000000000.log (< size limit)

2. SEALED: Size limit reached, new segment created
   ├── 00000000000000000000.log (closed, read-only)
   └── 00000000000001000000.log (now active)

3. EXPIRED: All messages in segment past retention
   └── 00000000000000000000.log (eligible for deletion)

4. DELETED: Removed by the message cleaner
   └── (the active segment is always protected)
```

### Index Files

Each segment has one associated index file:

| File | Purpose |
|------|---------|
| `.log` | Message data (binary format) |
| `.index` | Combined index (find message by offset or timestamp) |

### Checksum Validation

Enable checksum validation to detect data corruption:

```toml
[system.partition]
validate_checksum = true  # Validates CRCs when loading data (default: false)
```

This adds overhead during data loading but catches corruption early.

---

## Data Retention Policies

Iggy supports both time-based and size-based retention. Server-wide defaults
for new topics live under `[system.topic]`; both policies can be active
simultaneously and can be overridden per-topic via `CreateTopic`/`UpdateTopic`.

> **Important**: retention is enforced by the background **message cleaner**,
> which is **disabled by default** in 0.8.0. Enable it or expired data will
> never be deleted:
>
> ```toml
> [data_maintenance.messages]
> cleaner_enabled = true   # default: false
> interval = "1 m"         # how often the cleaner runs (default: "1 m")
> ```

### Time-Based Retention (Message Expiry)

Messages are deleted after a specified duration:

```toml
[system.topic]
# Human-readable format
message_expiry = "7 days"           # 7 days
message_expiry = "2 days 4 hours"   # 2 days + 4 hours
message_expiry = "none"             # Never expire (default)
```

Expiry is evaluated per-segment: a sealed segment is removed once all messages
within it have expired. The active (open) segment is always protected.

### Size-Based Retention (Max Topic Size)

Limit the total size of a topic:

```toml
[system.topic]
# Maximum topic size; "unlimited" or "0" = no size limit (default: "unlimited")
max_size = "10 GiB"
```

When a topic reaches **90% of `max_size`**, the oldest sealed segments are
deleted in whole-segment increments to make room for new messages. There is no
separate `delete_oldest_segments` switch in 0.8.0 — this behavior is automatic
whenever `max_size` is set and the message cleaner is enabled.

### Retention Strategy by Use Case

| Use Case | Time Retention | Size Limit | Rationale |
|----------|----------------|------------|-----------|
| Real-time processing | 1-7 days | 10-50 GB | Data processed quickly, no need to keep |
| Audit logs | 1-7 years | Unlimited | Compliance requirements |
| Event sourcing | Never expire | Unlimited | Events are source of truth |
| Metrics/telemetry | 30-90 days | 100 GB | Balance history vs storage cost |
| Dev/test environments | 1 day | 1 GB | Quick turnover, limited resources |

### Configuring Per-Topic Retention

When creating topics via the SDK (signature as of iggy 0.10.0):

```rust
use iggy::prelude::*;
use std::str::FromStr;

// 7-day retention with a 10 GiB size cap
client.create_topic(
    &"mystream".try_into()?,             // stream_id: &Identifier
    "events",                            // topic name
    3,                                   // partitions_count
    CompressionAlgorithm::default(),     // compression ("none")
    None,                                // replication_factor
    IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?),
    MaxTopicSize::Custom(IggyByteSize::from_str("10GiB")?),
).await?;

// Never expire (event sourcing)
client.create_topic(
    &"mystream".try_into()?,
    "event-store",
    10,
    CompressionAlgorithm::default(),
    None,
    IggyExpiry::NeverExpire,
    MaxTopicSize::Unlimited,             // no size limit
).await?;
```

---

## Backup and Archiving

> **Removed in 0.8.0**: the built-in segment archiver
> (`[data_maintenance.archiver]` with `disk` and `s3` backends) existed in the
> legacy 0.4.x server line but was **removed** in the io_uring server rewrite
> (0.7.x/0.8.x). Server 0.8.0 ships **no built-in archiving to disk or S3**.
> A `[system.segment] archive_expired` flag (default `false`) is still
> accepted by the config parser, but no archiver backend exists in 0.8.0, so
> the flag has no effect — expired segments are simply deleted by the message
> cleaner.

Until archiving returns upstream, handle backups at the infrastructure level.

### Filesystem-Level Backup

The entire server state lives under `system.path` (default `local_data`), so
any file-level backup tool works. For a consistent snapshot, either stop the
server or use filesystem/volume snapshots (LVM, ZFS, EBS):

```bash
# Simple sync of the data directory to S3 (run against a snapshot,
# or accept that the active segment may be mid-write)
aws s3 sync /var/lib/iggy/data s3://iggy-backups/production/iggy/
```

### Backup Directory

Iggy reserves a backup location inside the data directory, used by the server
for compatibility conversions (not a general-purpose archiver):

```toml
[system.backup]
# Path for storing backup data, relative to system.path (default: "backup")
path = "backup"

[system.backup.compatibility]
# Subpath for converted segment data after compatibility conversion
path = "compatibility"
```

### Streaming Data Out (Connectors)

For continuous off-server copies of message data, the Iggy **connectors
runtime** (a separate component from the server) can sink topics to external
systems (e.g., PostgreSQL, Elasticsearch, Iceberg, Quickwit). This is a
replication-style pipeline rather than a segment archive, but it is the
supported way to keep an external durable copy with 0.8.0.

---

## Recovery and Data Integrity

### Automatic State Recovery

The config schema retains a recovery flag:

```toml
[system.recovery]
# Recreate streams/topics/partitions if expected data for existing state
# is missing (default: false)
recreate_missing_state = true
```

> **Caveat (0.8.0)**: this key is accepted by the config parser, but as of
> server 0.8.0 it is not referenced anywhere in the server code — it is a
> holdover from the legacy server line and currently has **no effect**. The
> 0.8.0 server performs its own crash-recovery on startup (offset clamping,
> checksum-validated loading when `validate_checksum` is enabled), but it does
> not rebuild missing metadata from raw segment files. Do not rely on this
> flag; rely on backups.

### Manual Recovery Steps

1. **Identify corruption**: Check logs for checksum errors or missing segments

2. **Stop the server**: Ensure no writes during recovery

3. **Verify segment files**: Check for truncated or zero-byte files
   ```bash
   find /var/lib/iggy/data -name "*.log" -size 0
   ```

4. **Restore from backup** (if syncing the data directory to S3):
   ```bash
   aws s3 sync s3://iggy-backups/production/iggy/ /var/lib/iggy/data/
   ```

5. **Restart and verify**:
   ```bash
   iggy-server
   ```

### Preventing Data Corruption

| Measure | Configuration | Purpose |
|---------|---------------|---------|
| Checksums | `[system.partition]` `validate_checksum = true` | Detect corruption on load |
| fsync | `enforce_fsync = true` | Prevent partial writes |
| S3 backup | (infrastructure, e.g. `aws s3 sync`) | Off-site redundancy |
| EBS snapshots | (infrastructure) | Point-in-time recovery |

---

## Performance vs Durability Trade-offs

### Understanding the Trade-off

Every durability feature has a performance cost:

```
Performance ←────────────────────────────────→ Durability

[No fsync]      [Batched fsync]      [Per-message fsync]
 ~0.1ms              ~1ms                  ~9ms
 Data at risk    Some data risk         No data loss
```

### Choosing Your Configuration

#### High-Throughput, Lower Durability (Metrics, Logs)

```toml
[system.partition]
enforce_fsync = false

[message_saver]
enabled = true
enforce_fsync = false
interval = "5s"
```

- Throughput: Millions of messages/second
- Latency: Sub-millisecond
- Risk: May lose several seconds of data on crash

#### Balanced (Most Applications)

```toml
[system.partition]
enforce_fsync = true
messages_required_to_save = 100

[message_saver]
enabled = true
enforce_fsync = true
interval = "1s"
```

- Throughput: Hundreds of thousands of messages/second
- Latency: 1-2ms
- Risk: May lose up to 100 messages on crash

#### Maximum Durability (Financial, Critical Systems)

```toml
[system.partition]
enforce_fsync = true
messages_required_to_save = 1

[message_saver]
enabled = true
enforce_fsync = true
```

- Throughput: Tens of thousands of messages/second
- Latency: ~9ms
- Risk: No data loss (survives power failure)

### Hardware Considerations

fsync performance depends heavily on storage:

| Storage Type | fsync Latency | Recommendation |
|--------------|---------------|----------------|
| NVMe SSD | 0.1-1ms | Best for durable workloads |
| SATA SSD | 1-5ms | Good for balanced workloads |
| HDD | 5-15ms | Avoid for high-durability needs |
| Network (EBS gp3) | 1-3ms | Good, use provisioned IOPS |
| Network (EBS io2) | 0.5-1ms | Best for cloud deployments |

---

## Production Recommendations

### Recommended Production Configuration

```toml
# config.toml - Production Configuration (Iggy server 0.8.0)

[system]
path = "/var/lib/iggy/data"

[system.partition]
enforce_fsync = true
messages_required_to_save = 100  # Balance: durability with reasonable latency
validate_checksum = true

[message_saver]
enabled = true
enforce_fsync = true
interval = "1s"

[system.segment]
size = "1 GiB"
cache_indexes = "open_segment"

[system.topic]
message_expiry = "7 days"   # Adjust per your needs
max_size = "unlimited"      # Or e.g. "50 GiB"

# Required for retention to be enforced (disabled by default!)
[data_maintenance.messages]
cleaner_enabled = true
interval = "1 m"
```

> **Note**: the built-in S3/disk archiver from older Iggy versions does not
> exist in 0.8.0 — see [Backup and Archiving](#backup-and-archiving) for
> infrastructure-level alternatives.

### Infrastructure Checklist

- [ ] **Storage**: Use NVMe SSD or provisioned IOPS EBS
- [ ] **Filesystem**: ext4 or XFS with `noatime` mount option
- [ ] **Backup**: Sync the data directory to S3 and/or use EBS snapshots (no built-in archiver in 0.8.0)
- [ ] **Monitoring**: Alert on disk space, segment count, backup failures
- [ ] **Capacity**: Plan for 2x expected data volume
- [ ] **Recovery testing**: Regularly test restore from backups

### Monitoring Metrics

Key metrics to monitor for storage health:

| Metric | Alert Threshold | Action |
|--------|-----------------|--------|
| Disk usage | >80% | Add storage or reduce retention |
| Segment count | >1000 per partition | Review segment size settings |
| Backup failures | Any | Check S3 credentials/connectivity |
| fsync latency | >50ms | Check storage health |
| Checksum errors | Any | Investigate corruption source |

---

## Current Limitations

As of Iggy server v0.8.0 (April 2026):

### Single-Node Architecture

Iggy 0.8.0 is effectively a **single-node** system for production purposes.
The 0.8.0 config does introduce an **experimental `[cluster]` section**
(disabled by default) with node/peer definitions, but clustered replication is
still under active development and not production-ready in this release.

**Implications:**
- No automatic failover
- Single point of failure for availability
- Rely on filesystem backups + infrastructure (EBS snapshots) for redundancy

**Roadmap:**
- Clustering via Viewstamped Replication is planned for future releases
- Will provide automatic failover and data replication

### Mitigation Strategies

Until clustering is production-ready:

1. **S3 backup**: Periodically sync the data directory to durable cloud storage
2. **EBS Snapshots**: If on AWS, use automated EBS snapshots
3. **Cold standby**: Keep a replica populated from S3 backups
4. **Infrastructure HA**: Use container orchestration for process restarts

```
Primary ──writes──→ Iggy ──s3 sync──→ S3
                      │
                      └── EBS Snapshot (every hour)

On failure:
1. Launch new instance
2. Attach latest EBS snapshot (or restore from S3)
3. Start Iggy
4. Resume operations (some data loss possible)
```

---

## Configuration Reference

### Environment Variables

All configuration can be set via environment variables using the pattern
`IGGY_<CONFIG_PATH>` — the TOML path uppercased with `_` separators (e.g.
`system.partition.enforce_fsync` → `IGGY_SYSTEM_PARTITION_ENFORCE_FSYNC`).
The 0.8.0 server generates these mappings at compile time, so only real config
keys are accepted. A custom config file path can be given via
`IGGY_CONFIG_PATH` (otherwise the embedded default `core/server/config.toml`
is used).

| Variable | Default | Description |
|----------|---------|-------------|
| `IGGY_SYSTEM_PATH` | `local_data` | Data directory |
| `IGGY_SYSTEM_PARTITION_ENFORCE_FSYNC` | `false` | Enable fsync on partition writes |
| `IGGY_SYSTEM_PARTITION_MESSAGES_REQUIRED_TO_SAVE` | `1024` | Buffered messages before flush |
| `IGGY_SYSTEM_PARTITION_SIZE_OF_MESSAGES_REQUIRED_TO_SAVE` | `1 MiB` | Buffered size before flush |
| `IGGY_SYSTEM_PARTITION_VALIDATE_CHECKSUM` | `false` | Validate CRCs on load |
| `IGGY_SYSTEM_SEGMENT_SIZE` | `1 GiB` | Segment size (max 1 GiB, multiple of 512 B) |
| `IGGY_SYSTEM_SEGMENT_CACHE_INDEXES` | `open_segment` | `all`, `open_segment`, or `none` |
| `IGGY_SYSTEM_TOPIC_MESSAGE_EXPIRY` | `none` | Default message TTL for new topics |
| `IGGY_SYSTEM_TOPIC_MAX_SIZE` | `unlimited` | Default max topic size |
| `IGGY_MESSAGE_SAVER_ENABLED` | `true` | Background message saver |
| `IGGY_MESSAGE_SAVER_ENFORCE_FSYNC` | `true` | fsync on background saves |
| `IGGY_MESSAGE_SAVER_INTERVAL` | `30 s` | Saver interval |
| `IGGY_DATA_MAINTENANCE_MESSAGES_CLEANER_ENABLED` | `false` | Retention cleaner (enable for expiry/size limits!) |
| `IGGY_DATA_MAINTENANCE_MESSAGES_INTERVAL` | `1 m` | Cleaner interval |
| `IGGY_SYSTEM_STATE_ENFORCE_FSYNC` | `false` | fsync on metadata state updates |
| `IGGY_SYSTEM_RECOVERY_RECREATE_MISSING_STATE` | `false` | Accepted but has no effect in 0.8.0 |

### Complete config.toml Template (Durability-Related Sections)

```toml
# Apache Iggy Server 0.8.0 Configuration
# Durable-storage-related sections; values shown are recommended
# production settings (shipped defaults noted in comments)

[system]
# Base path for all data (default: "local_data")
path = "local_data"

[system.state]
# fsync on metadata state updates (default: false)
enforce_fsync = false

[system.partition]
# Synchronous writes for durability (default: false)
enforce_fsync = true

# Count of buffered messages before triggering a save
# (1 = immediate, higher = batched; soft limit; default: 1024)
messages_required_to_save = 100

# Size of buffered messages before triggering a save (default: "1 MiB")
size_of_messages_required_to_save = "1 MiB"

# Validate CRC checksums when loading data (default: false)
validate_checksum = true

[message_saver]
# Background persistence (defaults: enabled = true, enforce_fsync = true,
# interval = "30 s")
enabled = true
enforce_fsync = true
interval = "1s"

[system.segment]
# Soft segment size limit; max 1 GiB, multiple of 512 B (default: "1 GiB")
size = "1 GiB"

# Index caching: "all" | "open_segment" | "none" (default: "open_segment")
cache_indexes = "open_segment"

# Parsed but non-functional in 0.8.0 (no archiver backend exists)
archive_expired = false

[system.topic]
# Default message retention for new topics (default: "none" = keep forever)
message_expiry = "7 days"

# Default max topic size; oldest sealed segments deleted at 90% of this
# (default: "unlimited")
max_size = "unlimited"

# Retention enforcement - MUST be enabled for expiry/size limits to apply
[data_maintenance.messages]
# (default: false)
cleaner_enabled = true
# (default: "1 m")
interval = "1 m"

[system.recovery]
# Accepted by the parser but has no effect in server 0.8.0 (default: false)
recreate_missing_state = false
```

---

## Sources and Further Reading

### Official Documentation
- [Apache Iggy GitHub Repository](https://github.com/apache/iggy)
- [Iggy Architecture Documentation](https://iggy.apache.org/docs/introduction/architecture/)
- [Iggy Server Configuration](https://iggy.apache.org/docs/server/configuration)
- [Iggy Official Website](https://iggy.apache.org/)

### Related Concepts
- [Apache Kafka Storage Internals](https://kafka.apache.org/documentation/#design_filesystem) - Similar log-based architecture
- [fsync and Data Durability](https://lwn.net/Articles/457667/) - Linux kernel perspective
- [Write-Ahead Logging](https://en.wikipedia.org/wiki/Write-ahead_logging) - Foundational concept

### Performance References
- [Iggy Benchmarks](https://github.com/apache/iggy/tree/master/bench) - Official benchmark suite
- [io_uring and Modern I/O](https://iggy.apache.org/blogs/2025/11/17/websocket-io-uring/) - Iggy's I/O architecture

---

*Last updated: 2026-07-05*
*Iggy server version: 0.8.0 — every configuration key in this guide was
validated key-by-key against the upstream `server-0.8.0` tag
(`core/server/config.toml` + `core/configs/src/server_config/`).*
