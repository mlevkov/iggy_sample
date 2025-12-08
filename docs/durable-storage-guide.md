# Apache Iggy Durable Storage Guide

A comprehensive guide to configuring Apache Iggy for durable, production-ready message storage.

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
├── Partition 0/
│   ├── segment-0000000000.log    (messages 0-999,999)
│   ├── segment-0000000000.idx    (offset index)
│   ├── segment-0000000000.tidx   (time index)
│   ├── segment-0001000000.log    (messages 1,000,000-1,999,999)
│   └── ...
├── Partition 1/
│   └── ...
└── Partition 2/
    └── ...
```

### Key Characteristics

| Property | Description |
|----------|-------------|
| **Immutable records** | Messages cannot be modified once written |
| **Sequential writes** | Optimized for append operations |
| **Indexed access** | Fast lookups by offset and timestamp |
| **Segment-based** | Data split into manageable chunks |

### Storage Location

By default, all data is persisted under the `local_data` directory. Configure the path in `server.toml`:

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

The `message_saver` section controls background persistence:

```toml
[system.partition.message_saver]
# Enable background message saving
enabled = true

# Force synchronous writes to disk
# true  = durable but slower (data guaranteed on disk before ack)
# false = faster but data may be lost on crash
enforce_fsync = true

# Interval between save operations (when not using fsync per message)
interval = "1s"
```

#### 2. Per-Message fsync (Maximum Durability)

For the strongest durability guarantee, force fsync after every message:

```toml
[system.partition]
# Force fsync on every partition write
enforce_fsync = true

# Flush after each message (set to 1 for maximum durability)
messages_required_to_save = 1
```

Or via environment variables:

```bash
export IGGY_SYSTEM_PARTITION_ENFORCE_FSYNC=true
export IGGY_SYSTEM_PARTITION_MESSAGES_REQUIRED_TO_SAVE=1
```

### Durability Levels

| Level | Configuration | Latency | Data Loss Risk |
|-------|---------------|---------|----------------|
| **Maximum** | `enforce_fsync=true`, `messages_required_to_save=1` | ~9ms P99 | None (survives power loss) |
| **High** | `enforce_fsync=true`, `messages_required_to_save=100` | ~1-2ms P99 | Up to 100 messages |
| **Balanced** | `enforce_fsync=true`, `interval="1s"` | ~0.5ms P99 | Up to 1 second of data |
| **Performance** | `enforce_fsync=false` | ~0.1ms P99 | Unbounded (OS-dependent) |

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
# Maximum segment size before rolling to new segment
# Larger = fewer files, potentially slower recovery
# Smaller = more files, faster recovery, more overhead
size = "1 GB"

# Cache offset indexes in memory (faster reads)
cache_indexes = true

# Cache time-based indexes in memory
cache_time_indexes = true

# Validate checksums when loading segments (integrity check)
validate_checksum = true
```

### Segment Lifecycle

```
1. ACTIVE: Current segment receiving writes
   └── segment-0000000000.log (< size limit)

2. ROLLED: Size limit reached, new segment created
   ├── segment-0000000000.log (closed, read-only)
   └── segment-0001000000.log (now active)

3. EXPIRED: All messages in segment past retention
   └── segment-0000000000.log (eligible for deletion/archival)

4. ARCHIVED/DELETED: Based on configuration
   └── (moved to archive location or deleted)
```

### Index Files

Each segment has associated index files:

| File | Purpose |
|------|---------|
| `.log` | Message data (binary format) |
| `.idx` | Offset index (find message by offset) |
| `.tidx` | Time index (find message by timestamp) |

### Checksum Validation

Enable checksum validation to detect data corruption:

```toml
[system.segment]
validate_checksum = true  # Validates on segment load
```

This adds overhead during segment loading but catches corruption early.

---

## Data Retention Policies

Iggy supports both time-based and size-based retention.

### Time-Based Retention (Message Expiry)

Messages are deleted after a specified duration:

```toml
[system.segment]
# Human-readable format
message_expiry = "7 days"           # 7 days
message_expiry = "2 days 4 hours"   # 2 days + 4 hours
message_expiry = "none"             # Never expire (default)
```

Expiry is evaluated per-segment. The entire segment is removed when all messages within it have expired.

### Size-Based Retention (Max Topic Size)

Limit the total size of a topic:

```toml
[topic]
# Maximum topic size across all partitions
max_topic_size = "10 GB"

# Delete oldest segments when limit reached
delete_oldest_segments = true
```

When the limit is reached, the oldest segments are deleted in whole-segment increments to make room for new messages.

### Retention Strategy by Use Case

| Use Case | Time Retention | Size Limit | Rationale |
|----------|----------------|------------|-----------|
| Real-time processing | 1-7 days | 10-50 GB | Data processed quickly, no need to keep |
| Audit logs | 1-7 years | Unlimited | Compliance requirements |
| Event sourcing | Never expire | Unlimited | Events are source of truth |
| Metrics/telemetry | 30-90 days | 100 GB | Balance history vs storage cost |
| Dev/test environments | 1 day | 1 GB | Quick turnover, limited resources |

### Configuring Per-Topic Retention

When creating topics via the SDK:

```rust
use iggy::prelude::*;

// 7-day retention
client.create_topic(
    "mystream",
    "events",
    3,  // partitions
    None,
    None,
    Some(IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?)),
    Some(IggyByteSize::from_str("10GB")?),  // max size
).await?;

// Never expire (event sourcing)
client.create_topic(
    "mystream",
    "event-store",
    10,
    None,
    None,
    Some(IggyExpiry::NeverExpire),
    None,  // no size limit
).await?;
```

---

## Backup and Archiving

Iggy supports archiving expired segments before deletion, either to local disk or S3-compatible cloud storage.

### Disk Archiving

Archive segments to a local directory:

```toml
[data_maintenance.archiver]
enabled = true
kind = "disk"

# Archive expired segments before deletion
archive_expired = true

# Local path for archived data
path = "/var/lib/iggy/archive"
```

### S3-Compatible Cloud Storage

Archive to AWS S3, MinIO, or any S3-compatible storage:

```toml
[data_maintenance.archiver]
enabled = true
kind = "s3"

# Archive expired segments before deletion
archive_expired = true

# S3 configuration
[data_maintenance.archiver.s3]
# AWS credentials (or use IAM roles/environment)
access_key_id = "your-access-key"
secret_access_key = "your-secret-key"

# S3 bucket and optional prefix
bucket = "iggy-backups"
key_prefix = "production/iggy/"

# Region
region = "us-east-1"

# For non-AWS S3 (MinIO, etc.)
endpoint = "https://minio.example.com"

# Temporary staging directory for uploads
tmp_upload_dir = "/tmp/iggy-upload"
```

### Archive Workflow

```
1. Segment expires (all messages past retention)
   └── segment-0000000000.log

2. If archiver enabled:
   ├── Upload to S3: s3://bucket/prefix/stream/topic/partition/segment-0000000000.log
   └── Verify upload success

3. After successful archive:
   └── Delete local segment (if archive_expired = true)

4. If archiver disabled:
   └── Delete segment immediately
```

### MinIO Example (Self-Hosted S3)

For on-premises deployments using MinIO:

```toml
[data_maintenance.archiver]
enabled = true
kind = "s3"
archive_expired = true

[data_maintenance.archiver.s3]
access_key_id = "minioadmin"
secret_access_key = "minioadmin"
bucket = "iggy-archive"
endpoint = "http://minio:9000"
region = "us-east-1"  # Required even for MinIO
tmp_upload_dir = "/tmp/iggy-s3-staging"
```

---

## Recovery and Data Integrity

### Automatic State Recovery

If metadata becomes corrupted but segment files are intact, Iggy can rebuild:

```toml
[system.recovery]
# Recreate stream/topic/partition metadata from existing data files
recreate_missing_state = true
```

This scans the data directory and rebuilds the logical structure from physical segment files.

### Manual Recovery Steps

1. **Identify corruption**: Check logs for checksum errors or missing segments

2. **Stop the server**: Ensure no writes during recovery

3. **Verify segment files**: Check for truncated or zero-byte files
   ```bash
   find /var/lib/iggy/data -name "*.log" -size 0
   ```

4. **Restore from backup** (if using archiver):
   ```bash
   aws s3 sync s3://iggy-backups/production/iggy/ /var/lib/iggy/data/
   ```

5. **Restart with recovery enabled**:
   ```bash
   export IGGY_SYSTEM_RECOVERY_RECREATE_MISSING_STATE=true
   iggy-server
   ```

### Preventing Data Corruption

| Measure | Configuration | Purpose |
|---------|---------------|---------|
| Checksums | `validate_checksum = true` | Detect corruption on read |
| fsync | `enforce_fsync = true` | Prevent partial writes |
| S3 backup | `archiver.enabled = true` | Off-site redundancy |
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

[system.partition.message_saver]
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

[system.partition.message_saver]
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

[system.partition.message_saver]
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
# server.toml - Production Configuration

[system]
path = "/var/lib/iggy/data"

[system.partition]
enforce_fsync = true
messages_required_to_save = 100  # Balance: durability with reasonable latency

[system.partition.message_saver]
enabled = true
enforce_fsync = true
interval = "1s"

[system.segment]
size = "1 GB"
cache_indexes = true
cache_time_indexes = true
validate_checksum = true
message_expiry = "7 days"  # Adjust per your needs

[system.recovery]
recreate_missing_state = true

[data_maintenance.archiver]
enabled = true
kind = "s3"
archive_expired = true

[data_maintenance.archiver.s3]
bucket = "your-iggy-backups"
region = "us-east-1"
key_prefix = "production/"
tmp_upload_dir = "/tmp/iggy-upload"
```

### Infrastructure Checklist

- [ ] **Storage**: Use NVMe SSD or provisioned IOPS EBS
- [ ] **Filesystem**: ext4 or XFS with `noatime` mount option
- [ ] **Backup**: Enable S3 archiver or EBS snapshots
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

As of Iggy v0.6.0 (December 2025):

### Single-Node Architecture

Iggy currently runs as a **single node only**. There is no built-in replication or clustering.

**Implications:**
- No automatic failover
- Single point of failure for availability
- Rely on S3 archiving + infrastructure (EBS snapshots) for redundancy

**Roadmap:**
- Clustering via Viewstamped Replication is planned for future releases
- Will provide automatic failover and data replication

### Mitigation Strategies

Until clustering is available:

1. **S3 Archiving**: Archive all data to durable cloud storage
2. **EBS Snapshots**: If on AWS, use automated EBS snapshots
3. **Cold standby**: Keep a replica populated from S3 archives
4. **Infrastructure HA**: Use container orchestration for process restarts

```
Primary ──writes──→ Iggy ──archives──→ S3
                      │
                      └── EBS Snapshot (every hour)
                      
On failure:
1. Launch new instance
2. Attach latest EBS snapshot (or restore from S3)
3. Start Iggy with recreate_missing_state = true
4. Resume operations (some data loss possible)
```

---

## Configuration Reference

### Environment Variables

All configuration can be set via environment variables using the pattern `IGGY_<SECTION>_<KEY>`:

| Variable | Default | Description |
|----------|---------|-------------|
| `IGGY_SYSTEM_PATH` | `local_data` | Data directory |
| `IGGY_SYSTEM_PARTITION_ENFORCE_FSYNC` | `false` | Enable fsync on writes |
| `IGGY_SYSTEM_PARTITION_MESSAGES_REQUIRED_TO_SAVE` | `10000` | Messages before flush |
| `IGGY_SYSTEM_SEGMENT_SIZE` | `1073741824` | Segment size in bytes (1GB) |
| `IGGY_SYSTEM_SEGMENT_CACHE_INDEXES` | `true` | Cache offset indexes |
| `IGGY_SYSTEM_SEGMENT_VALIDATE_CHECKSUM` | `false` | Validate on load |
| `IGGY_SYSTEM_SEGMENT_MESSAGE_EXPIRY` | `none` | Default message TTL |
| `IGGY_SYSTEM_RECOVERY_RECREATE_MISSING_STATE` | `false` | Auto-rebuild metadata |
| `IGGY_DATA_MAINTENANCE_ARCHIVER_ENABLED` | `false` | Enable archiving |
| `IGGY_DATA_MAINTENANCE_ARCHIVER_KIND` | `disk` | `disk` or `s3` |

### Complete server.toml Template

```toml
# Apache Iggy Server Configuration
# Full reference for durable storage settings

[system]
# Base path for all data
path = "local_data"

[system.partition]
# Synchronous writes for durability
enforce_fsync = true

# Messages to buffer before flushing (1 = immediate, higher = batched)
messages_required_to_save = 100

[system.partition.message_saver]
# Background persistence
enabled = true
enforce_fsync = true
interval = "1s"

[system.segment]
# Max segment size (bytes or human-readable)
size = "1 GB"

# Index caching for performance
cache_indexes = true
cache_time_indexes = true

# Data integrity
validate_checksum = true

# Default message retention (overridable per-topic)
message_expiry = "7 days"

[system.recovery]
# Rebuild metadata from segment files if missing
recreate_missing_state = true

[data_maintenance.archiver]
# Enable archiving of expired segments
enabled = true

# "disk" or "s3"
kind = "s3"

# Archive before deleting expired segments
archive_expired = true

# Disk archiver settings (if kind = "disk")
[data_maintenance.archiver.disk]
path = "/var/lib/iggy/archive"

# S3 archiver settings (if kind = "s3")
[data_maintenance.archiver.s3]
access_key_id = ""       # Or use IAM roles
secret_access_key = ""   # Or use IAM roles
bucket = "iggy-backups"
key_prefix = "production/"
region = "us-east-1"
endpoint = ""            # Custom endpoint for MinIO, etc.
tmp_upload_dir = "/tmp/iggy-upload"
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

*Last updated: December 2025*
*Iggy version: 0.6.0*
