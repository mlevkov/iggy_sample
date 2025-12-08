# Understanding Partitions in Message Streaming

A deep-dive guide into partitioning strategies, partition keys, and ordering guarantees in event-driven systems, with specific focus on Apache Iggy.

---

## Table of Contents

1. [What is a Partition?](#what-is-a-partition)
2. [Why Partitions Exist](#why-partitions-exist)
3. [The Ordering Problem](#the-ordering-problem)
4. [Partition Keys Explained](#partition-keys-explained)
5. [Iggy Partitioning Strategies](#iggy-partitioning-strategies)
6. [Choosing Partition Keys](#choosing-partition-keys)
7. [Common Partitioning Mistakes](#common-partitioning-mistakes)
8. [Partition Count Guidelines](#partition-count-guidelines)
9. [Iggy Partition Storage Architecture](#iggy-partition-storage-architecture)
10. [Iggy Server Configuration](#iggy-server-configuration)
11. [Rebalancing and Consumer Groups](#rebalancing-and-consumer-groups)
12. [Custom Partitioners](#custom-partitioners)
13. [Real-World Scenarios](#real-world-scenarios)
14. [Code Examples](#code-examples)
15. [Troubleshooting](#troubleshooting)

---

## What is a Partition?

A **partition** is a physical subdivision of a topic. Think of it as a separate, ordered log file within a topic.

### Visual Model

```
Topic: "orders" (3 partitions)

Partition 0: [msg1] → [msg4] → [msg7] → [msg10] → ...
Partition 1: [msg2] → [msg5] → [msg8] → [msg11] → ...
Partition 2: [msg3] → [msg6] → [msg9] → [msg12] → ...

Each partition:
- Is an independent, ordered sequence
- Can be read by one consumer (within a consumer group)
- Has its own offset counter
- Is stored separately on disk
```

### Key Properties

| Property | Description |
|----------|-------------|
| **Ordered** | Messages within a partition maintain strict order |
| **Independent** | Partitions don't share state or ordering with each other |
| **Parallelizable** | Different partitions can be processed simultaneously |
| **Persistent** | Each partition is an append-only log on disk |

---

## Why Partitions Exist

Partitions solve two fundamental problems in distributed systems:

### Problem 1: Scalability

Without partitions, a single consumer must process ALL messages:

```
Topic (no partitions):
[msg1] → [msg2] → [msg3] → [msg4] → [msg5] → ... → Consumer A (bottleneck!)

With partitions:
Partition 0: [msg1] → [msg4] → ... → Consumer A
Partition 1: [msg2] → [msg5] → ... → Consumer B  
Partition 2: [msg3] → [msg6] → ... → Consumer C

Throughput: 3x improvement
```

### Problem 2: Locality

Related data can be colocated for efficient processing:

```
Without partition keys:
Order 123 events scattered randomly:
  P0: [Order 123 Created]
  P2: [Order 123 Paid]      ← Different partition!
  P1: [Order 123 Shipped]   ← Different partition!

With partition key = order_id:
All Order 123 events together:
  P1: [Order 123 Created] → [Order 123 Paid] → [Order 123 Shipped]
```

### The Tradeoff

```
More Partitions = More Parallelism = Less Global Ordering

┌─────────────────────────────────────────────────────────────────┐
│  1 Partition          │  Perfect global order, no parallelism  │
├───────────────────────┼─────────────────────────────────────────┤
│  3 Partitions         │  Good balance for most applications    │
├───────────────────────┼─────────────────────────────────────────┤
│  100 Partitions       │  High parallelism, complex coordination │
└─────────────────────────────────────────────────────────────────┘
```

---

## The Ordering Problem

This is the most critical concept to understand.

### Scenario: E-commerce Order Processing

```rust
// These events MUST be processed in order for correctness
let events = vec![
    OrderEvent::Created { order_id: "123", total: 100.00 },
    OrderEvent::PaymentReceived { order_id: "123", amount: 100.00 },
    OrderEvent::Shipped { order_id: "123", tracking: "UPS123" },
    OrderEvent::Delivered { order_id: "123" },
];
```

### What Happens Without Partition Keys?

```
Producer sends events (no partition key):

Event 1: Created    → hash(random) % 3 = 0 → Partition 0
Event 2: Paid       → hash(random) % 3 = 2 → Partition 2
Event 3: Shipped    → hash(random) % 3 = 1 → Partition 1
Event 4: Delivered  → hash(random) % 3 = 0 → Partition 0

Consumer processing order (parallel consumption):

Consumer A (P0): Created → Delivered
Consumer B (P1): Shipped
Consumer C (P2): Paid

Actual processing order: Created → Shipped → Paid → Delivered
                                   ^^^^^^^^^^^^^^^^
                                   OUT OF ORDER! BUG!
```

### The Bug in Action

```rust
// Consumer processes "Shipped" before "Paid"
fn process_order_shipped(order_id: &str) {
    let order = db.get_order(order_id);
    
    if order.status != OrderStatus::Paid {
        // ERROR: "Cannot ship unpaid order!"
        // But the payment DID happen - we just processed out of order
    }
}
```

### Solution: Partition Keys

```
Producer sends events WITH partition key = order_id:

Event 1: Created    → hash("123") % 3 = 1 → Partition 1
Event 2: Paid       → hash("123") % 3 = 1 → Partition 1
Event 3: Shipped    → hash("123") % 3 = 1 → Partition 1
Event 4: Delivered  → hash("123") % 3 = 1 → Partition 1

All events for order "123" go to Partition 1.
Single consumer processes them in exact order.
```

---

## Partition Keys Explained

### How Partition Keys Work

```
partition_index = hash(partition_key) % number_of_partitions
```

Step by step:

```
partition_key = "order-123"

Step 1: Hash the key
        hash("order-123") = 2847623847 (example hash value)

Step 2: Modulo by partition count
        2847623847 % 3 = 1

Step 3: Route to partition
        Message → Partition 1
```

### Consistent Hashing Guarantee

The same key ALWAYS maps to the same partition (given fixed partition count):

```
hash("order-123") % 3 = 1  ← Always 1
hash("order-123") % 3 = 1  ← Always 1
hash("order-123") % 3 = 1  ← Always 1

hash("order-456") % 3 = 2  ← Always 2
hash("order-456") % 3 = 2  ← Always 2
```

### What Makes a Good Partition Key?

| Characteristic | Good | Bad |
|----------------|------|-----|
| **Cardinality** | High (many unique values) | Low (few values) |
| **Distribution** | Even spread across values | Skewed (hot keys) |
| **Stability** | Doesn't change for entity | Changes over time |
| **Business meaning** | Represents ordering boundary | Arbitrary |

---

## Iggy Partitioning Strategies

Apache Iggy provides three built-in partitioning strategies through the `Partitioning` struct and `PartitioningKind` enum.

### Strategy 1: Balanced (Round-Robin)

The server automatically distributes messages across partitions using round-robin:

```rust
use iggy::prelude::*;

// Messages distributed: P0 → P1 → P2 → P0 → P1 → P2 → ...
let partitioning = Partitioning::balanced();
```

**When to use:**
- Order of events doesn't matter
- Maximum throughput distribution
- Stateless event processing (logs, metrics)

**Trade-off:** No ordering guarantees across messages.

### Strategy 2: Partition ID (Direct Assignment)

Explicitly specify the target partition:

```rust
use iggy::prelude::*;

// Always send to partition 2
let partitioning = Partitioning::partition_id(2);
```

**When to use:**
- Priority queues (partition 0 = high priority)
- Manual load balancing
- Testing specific partitions
- Sticky routing based on external logic

**Trade-off:** You manage distribution manually.

### Strategy 3: Messages Key (Hash-Based)

The server calculates partition using **MurmurHash3** of your key:

```rust
use iggy::prelude::*;

// All messages with same key go to same partition
// partition = murmur3(key) % partition_count

// String keys
let partitioning = Partitioning::messages_key_str("order-123")?;

// Byte slice keys
let partitioning = Partitioning::messages_key(b"order-123")?;

// Numeric keys (efficient - no string conversion)
let partitioning = Partitioning::messages_key_u64(order_id);
let partitioning = Partitioning::messages_key_u128(uuid_as_u128);
```

**Key constraints:**
- Maximum key length: **255 bytes**
- Supports: `&[u8]`, `&str`, `u32`, `u64`, `u128`

**When to use:**
- Ordering required for related events
- Entity-based processing (orders, users, accounts)
- Session affinity

### Partitioning Strategy Comparison

| Strategy | Ordering | Distribution | Use Case |
|----------|----------|--------------|----------|
| `balanced()` | None | Even (round-robin) | Logs, metrics, stateless |
| `partition_id(n)` | Per-partition | Manual | Priority queues, testing |
| `messages_key(k)` | Per-key | Hash-based | Entity workflows, sessions |

### Understanding MurmurHash3

Iggy uses MurmurHash3 for key-based partitioning:

```
partition_id = murmur3_hash(key_bytes) % partition_count
```

**Properties of MurmurHash3:**
- Non-cryptographic (fast, not secure)
- Excellent distribution (minimal collisions)
- Deterministic (same key = same hash)
- Used by Kafka, Cassandra, and other distributed systems

**Why it matters:**
```
murmur3("order-123") = 0x8A3F2B1C  (example)
murmur3("order-124") = 0x2D7E9F8A  (completely different)

Even sequential keys distribute evenly across partitions.
```

---

## Choosing Partition Keys

### Decision Framework

Ask yourself: **"Which events MUST be processed in order?"**

```
If events for Entity X must be ordered:
    partition_key = Entity X's identifier
```

### Domain-Specific Examples

#### E-commerce

```
┌─────────────────┬─────────────────┬──────────────────────────────┐
│ Event Type      │ Partition Key   │ Reasoning                    │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Order events    │ order_id        │ Order lifecycle must be      │
│                 │                 │ processed sequentially       │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Cart events     │ user_id         │ User's cart modifications    │
│                 │                 │ must be ordered              │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Inventory       │ product_id      │ Stock changes for a product  │
│                 │                 │ must be sequential           │
└─────────────────┴─────────────────┴──────────────────────────────┘
```

#### Financial Services

```
┌─────────────────┬─────────────────┬──────────────────────────────┐
│ Event Type      │ Partition Key   │ Reasoning                    │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Transactions    │ account_id      │ Balance updates must be      │
│                 │                 │ strictly ordered             │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Trades          │ symbol          │ Price updates per symbol     │
│                 │                 │ must be sequential           │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Audit logs      │ transaction_id  │ Audit trail for each         │
│                 │                 │ transaction stays together   │
└─────────────────┴─────────────────┴──────────────────────────────┘
```

#### IoT / Telemetry

```
┌─────────────────┬─────────────────┬──────────────────────────────┐
│ Event Type      │ Partition Key   │ Reasoning                    │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Sensor readings │ device_id       │ Time-series per device       │
│                 │                 │ must be ordered              │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ GPS locations   │ vehicle_id      │ Route reconstruction         │
│                 │                 │ needs ordered points         │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Metrics         │ host_id         │ Per-host metrics stay        │
│                 │                 │ together for analysis        │
└─────────────────┴─────────────────┴──────────────────────────────┘
```

#### User Activity

```
┌─────────────────┬─────────────────┬──────────────────────────────┐
│ Event Type      │ Partition Key   │ Reasoning                    │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Clickstream     │ session_id      │ User journey must be         │
│                 │                 │ reconstructable in order     │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ User actions    │ user_id         │ User's actions stay          │
│                 │                 │ together                     │
├─────────────────┼─────────────────┼──────────────────────────────┤
│ Game events     │ match_id        │ Game state changes must      │
│                 │                 │ be ordered per match         │
└─────────────────┴─────────────────┴──────────────────────────────┘
```

---

## Common Partitioning Mistakes

### Mistake 1: Using Partitions as Categories

```
❌ WRONG: Treating partitions like separate queues

Topic: "events" (3 partitions)
  Partition 0 = "user events"      ← WRONG!
  Partition 1 = "order events"     ← WRONG!
  Partition 2 = "system events"    ← WRONG!

✅ CORRECT: Use separate topics for different event types

Topic: "user-events" (3 partitions)
Topic: "order-events" (3 partitions)
Topic: "system-events" (3 partitions)
```

**Why it's wrong:** Partitions should distribute load, not categorize data. Use topics for categorization.

### Mistake 2: Too Few Partitions

```
❌ WRONG: 1 partition for high-throughput topic

Topic: "transactions" (1 partition)
  → Max 1 consumer
  → Bottleneck at ~10K msg/sec
  → Cannot scale horizontally

✅ CORRECT: Plan for growth

Topic: "transactions" (10 partitions)
  → Up to 10 consumers
  → Can handle 100K+ msg/sec
  → Room to grow
```

### Mistake 3: Too Many Partitions

```
❌ WRONG: 1000 partitions "just in case"

Topic: "notifications" (1000 partitions)
  → Memory overhead for each partition
  → Increased leader election time
  → Most partitions sit idle
  → Harder to rebalance

✅ CORRECT: Start reasonable, increase as needed

Topic: "notifications" (10 partitions)
  → Monitor throughput
  → Add partitions when needed
```

### Mistake 4: Low-Cardinality Partition Keys

```
❌ WRONG: Using status as partition key

partition_key = order.status  // Only 5 possible values!

Distribution:
  Partition for "pending":    90% of messages  ← HOT!
  Partition for "shipped":     5% of messages
  Partition for "delivered":   3% of messages
  Partition for "cancelled":   2% of messages

✅ CORRECT: Use high-cardinality key

partition_key = order.id  // Millions of unique values

Distribution:
  Partition 0: ~33% of messages
  Partition 1: ~33% of messages
  Partition 2: ~33% of messages
```

### Mistake 5: Changing Partition Count

```
❌ DANGEROUS: Adding partitions to existing topic

Before: 3 partitions
  hash("order-123") % 3 = 1 → Partition 1

After: 5 partitions
  hash("order-123") % 5 = 2 → Partition 2  ← DIFFERENT!

Old events for "order-123" in Partition 1
New events for "order-123" in Partition 2
Ordering broken!

✅ SAFE APPROACH:
1. Create new topic with desired partition count
2. Migrate consumers to new topic
3. Dual-write during transition
4. Deprecate old topic
```

### Mistake 6: No Partition Key at All

```
❌ WRONG: Random distribution (no key)

producer.send(message);  // No partition key

Events randomly distributed:
  Order 123 Created   → Partition 0
  Order 123 Paid      → Partition 2  ← Out of order risk!
  Order 123 Shipped   → Partition 1  ← Out of order risk!

✅ CORRECT: Always use meaningful partition key

producer.send_with_key(order_id, message);

Events consistently routed:
  Order 123 Created   → Partition 1
  Order 123 Paid      → Partition 1  ← Same partition
  Order 123 Shipped   → Partition 1  ← Same partition
```

---

## Partition Count Guidelines

### Factors to Consider

```
Partition Count = max(
    expected_throughput / throughput_per_partition,
    max_consumer_parallelism,
    minimum_for_availability
)
```

### Rules of Thumb

| Throughput | Partitions | Notes |
|------------|------------|-------|
| < 1K msg/sec | 1-3 | Development, low-volume |
| 1K-10K msg/sec | 3-10 | Small production |
| 10K-100K msg/sec | 10-30 | Medium production |
| 100K+ msg/sec | 30-100+ | High-scale production |

### Capacity Planning Formula

```
Required Partitions = Peak Messages Per Second / Messages Per Partition Per Second

Example:
  Peak throughput: 50,000 msg/sec
  Per-partition throughput: 5,000 msg/sec (conservative estimate)
  Required: 50,000 / 5,000 = 10 partitions

  Add buffer: 10 * 1.5 = 15 partitions
```

### Starting Recommendations

| Use Case | Starting Partitions | Scale Trigger |
|----------|---------------------|---------------|
| Prototype/Dev | 1-3 | N/A |
| Small app | 3-6 | Consumer lag > 1 min |
| Medium app | 6-12 | Consumer lag > 30 sec |
| Large app | 12-30 | Consumer lag > 10 sec |
| Mission-critical | 30+ | Any lag increase |

---

## Iggy Partition Storage Architecture

Understanding how Iggy stores partition data helps you make better configuration decisions.

### Physical Storage Layout

```
data/                           # Base data directory
└── streams/
    └── {stream_id}/
        └── topics/
            └── {topic_id}/
                └── partitions/
                    ├── 0/                    # Partition 0
                    │   ├── 00000000000000000001/  # Segment 1
                    │   │   ├── .log              # Message data
                    │   │   ├── .index            # Offset index
                    │   │   └── .timeindex        # Timestamp index
                    │   └── 00000000000000001001/  # Segment 2
                    │       ├── .log
                    │       ├── .index
                    │       └── .timeindex
                    ├── 1/                    # Partition 1
                    └── 2/                    # Partition 2
```

### Segments

Each partition is divided into **segments** - fixed-size files that make up the append-only log:

```
Partition 0:
┌─────────────┐   ┌─────────────┐   ┌─────────────┐
│  Segment 1  │ → │  Segment 2  │ → │  Segment 3  │ → (active)
│  (closed)   │   │  (closed)   │   │  (writing)  │
│  1 GiB      │   │  1 GiB      │   │  0.3 GiB    │
└─────────────┘   └─────────────┘   └─────────────┘
    offset          offset            offset
    0-999K          1M-1.99M          2M-current
```

**Segment benefits:**
- Efficient deletion (drop entire segment files)
- Parallel reads from different segments
- Memory-mapped I/O for performance
- Archival/backup at segment granularity

### Indexes

Each segment has two indexes for fast lookups:

**Offset Index (`.index`):**
```
Maps message offset → file position
┌────────────┬───────────────┐
│ Offset     │ File Position │
├────────────┼───────────────┤
│ 1000000    │ 0             │
│ 1000100    │ 52480         │  ← Sparse index (every N messages)
│ 1000200    │ 104960        │
└────────────┴───────────────┘

Lookup: offset 1000150 → scan from 52480
```

**Time Index (`.timeindex`):**
```
Maps timestamp → offset
┌─────────────────────┬────────────┐
│ Timestamp           │ Offset     │
├─────────────────────┼────────────┤
│ 2024-01-15T10:00:00 │ 1000000    │
│ 2024-01-15T10:01:00 │ 1000500    │
│ 2024-01-15T10:02:00 │ 1001000    │
└─────────────────────┴────────────┘

Lookup: "messages after 10:01:30" → start at offset 1000500
```

### Memory and Caching

Iggy provides configurable index caching:

| Cache Mode | Behavior | Memory Usage | Read Performance |
|------------|----------|--------------|------------------|
| `all` | All indexes in memory | High | Fastest |
| `open_segment` | Only active segment | Medium | Fast for recent |
| `none` | Read from disk | Low | Slower |

---

## Iggy Server Configuration

These settings in `server.toml` affect partition behavior.

### Partition Settings

```toml
[system.partition]
# Directory for partition data (relative to topic.path)
path = "partitions"

# Force fsync after every write (durability vs performance)
# true = data safe on disk immediately (slower)
# false = OS manages write buffering (faster, small data loss risk on crash)
enforce_fsync = false

# Validate CRC checksums when loading data
# true = detect corruption on startup (slower startup)
# false = trust data integrity (faster startup)
validate_checksum = false

# Buffered messages before forced disk write
# Higher = better throughput, more memory, larger data loss window
messages_required_to_save = 1024  # default, minimum: 32

# Buffered bytes before forced disk write
# Triggers save when either this OR messages_required_to_save is reached
size_of_messages_required_to_save = "1 MiB"  # minimum: 512 B
```

### Segment Settings

```toml
[system.segment]
# Maximum segment file size before rolling to new segment
# Smaller = more files, faster deletion; Larger = fewer files, better sequential I/O
size = "1 GiB"  # maximum: 1 GiB

# Message expiration time (retention policy)
# "none" = keep forever
# Time format: "7 days", "24 hours", "30 minutes"
message_expiry = "none"

# What happens when segments expire
# true = move to archive directory
# false = delete permanently
archive_expired = false

# Index caching strategy
# "all" = cache all indexes (fastest reads, most memory)
# "open_segment" = cache only active segment (balanced)
# "none" = no caching (lowest memory, slowest reads)
cache_indexes = "open_segment"

# File system confirmation behavior
# "wait" = block until OS confirms write
# "no_wait" = return immediately (faster, less safe)
server_confirmation = "wait"
```

### Topic Settings (Affects All Partitions)

```toml
[system.topic]
# Maximum total size for all partitions in a topic
# "unlimited" or size like "100 GiB"
max_size = "unlimited"

# Auto-delete oldest segments when max_size is reached
# Only takes effect if max_size is set
delete_oldest_segments = false
```

### Message Deduplication

```toml
[system.message_deduplication]
# Enable server-side duplicate detection by message ID
enabled = false

# Maximum message IDs to track
max_entries = 10000

# How long to remember message IDs
expiry = "1 m"
```

### Configuration Trade-offs

| Goal | Configuration |
|------|---------------|
| **Maximum durability** | `enforce_fsync = true`, `validate_checksum = true` |
| **Maximum throughput** | `enforce_fsync = false`, `messages_required_to_save = 5000` |
| **Low memory** | `cache_indexes = "none"`, smaller `messages_required_to_save` |
| **Fast reads** | `cache_indexes = "all"` |
| **Auto-cleanup** | `message_expiry = "7 days"`, `delete_oldest_segments = true` |

---

## Rebalancing and Consumer Groups

### How Consumer Groups Use Partitions

```
Topic: "orders" (6 partitions)
Consumer Group: "order-processors"

Scenario 1: 2 consumers
┌─────────────────────────────────────────┐
│ Consumer A: P0, P1, P2                  │
│ Consumer B: P3, P4, P5                  │
└─────────────────────────────────────────┘

Scenario 2: 3 consumers (rebalance)
┌─────────────────────────────────────────┐
│ Consumer A: P0, P1                      │
│ Consumer B: P2, P3                      │
│ Consumer C: P4, P5                      │
└─────────────────────────────────────────┘

Scenario 3: 6 consumers (perfect distribution)
┌─────────────────────────────────────────┐
│ Consumer A: P0                          │
│ Consumer B: P1                          │
│ Consumer C: P2                          │
│ Consumer D: P3                          │
│ Consumer E: P4                          │
│ Consumer F: P5                          │
└─────────────────────────────────────────┘

Scenario 4: 8 consumers (2 idle)
┌─────────────────────────────────────────┐
│ Consumer A: P0                          │
│ Consumer B: P1                          │
│ Consumer C: P2                          │
│ Consumer D: P3                          │
│ Consumer E: P4                          │
│ Consumer F: P5                          │
│ Consumer G: IDLE (no partition)         │
│ Consumer H: IDLE (no partition)         │
└─────────────────────────────────────────┘
```

### Rebalancing Triggers

| Event | What Happens |
|-------|--------------|
| Consumer joins | Partitions redistributed |
| Consumer leaves/crashes | Partitions reassigned to remaining consumers |
| Consumer heartbeat timeout | Treated as crash, partitions reassigned |

### Rebalancing Impact

```
During rebalance:
1. All consumers stop processing (brief pause)
2. Coordinator recalculates assignments
3. Consumers receive new assignments
4. Processing resumes

Duration: Typically 1-30 seconds depending on group size
```

### Minimizing Rebalance Impact

```rust
// Use longer session timeout to prevent spurious rebalances
let consumer = client
    .consumer_group("stream", "topic", "group")?
    .session_timeout(Duration::from_secs(30))  // Default is often 10s
    .heartbeat_interval(Duration::from_secs(10))
    .build();

// Process batches, not single messages (reduces commit frequency)
let batch = consumer.poll(100).await?;
for message in batch {
    process(message)?;
}
consumer.commit().await?;  // One commit per batch
```

---

## Custom Partitioners

Iggy supports custom partitioning logic through the `Partitioner` trait. This is useful when built-in strategies don't meet your needs.

### The Partitioner Trait

```rust
use iggy::prelude::*;

/// Custom partitioner must implement this trait
pub trait Partitioner: Send + Sync + Debug {
    /// Calculate which partition a message should go to
    fn calculate_partition_id(
        &self,
        stream_id: &Identifier,
        topic_id: &Identifier,
        messages: &[IggyMessage],
    ) -> Result<u32, IggyError>;
}
```

### Example: Weighted Partitioner

Route more traffic to specific partitions (e.g., for heterogeneous hardware):

```rust
use iggy::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// Routes 70% to partitions 0-1, 30% to partitions 2-4
#[derive(Debug)]
struct WeightedPartitioner {
    counter: AtomicU64,
    high_capacity_partitions: Vec<u32>,  // [0, 1] - faster machines
    low_capacity_partitions: Vec<u32>,   // [2, 3, 4] - slower machines
    high_capacity_weight: u64,           // 70
}

impl Partitioner for WeightedPartitioner {
    fn calculate_partition_id(
        &self,
        _stream_id: &Identifier,
        _topic_id: &Identifier,
        _messages: &[IggyMessage],
    ) -> Result<u32, IggyError> {
        let count = self.counter.fetch_add(1, Ordering::Relaxed);
        
        // 70% to high capacity, 30% to low capacity
        if count % 100 < self.high_capacity_weight {
            let idx = (count as usize) % self.high_capacity_partitions.len();
            Ok(self.high_capacity_partitions[idx])
        } else {
            let idx = (count as usize) % self.low_capacity_partitions.len();
            Ok(self.low_capacity_partitions[idx])
        }
    }
}
```

### Example: Time-Based Partitioner

Route messages to partitions based on time windows:

```rust
use iggy::prelude::*;
use chrono::{Timelike, Utc};

/// Routes to different partitions by hour of day
/// Useful for time-bucketed analytics
#[derive(Debug)]
struct HourlyPartitioner {
    partitions_count: u32,
}

impl Partitioner for HourlyPartitioner {
    fn calculate_partition_id(
        &self,
        _stream_id: &Identifier,
        _topic_id: &Identifier,
        _messages: &[IggyMessage],
    ) -> Result<u32, IggyError> {
        let hour = Utc::now().hour();
        Ok(hour % self.partitions_count)
    }
}
```

### Example: Content-Based Partitioner

Route based on message content:

```rust
use iggy::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct EventEnvelope {
    priority: String,
    // ... other fields
}

/// Routes high-priority messages to partition 0
#[derive(Debug)]
struct PriorityPartitioner {
    high_priority_partition: u32,
    normal_partitions: Vec<u32>,
    counter: AtomicU64,
}

impl Partitioner for PriorityPartitioner {
    fn calculate_partition_id(
        &self,
        _stream_id: &Identifier,
        _topic_id: &Identifier,
        messages: &[IggyMessage],
    ) -> Result<u32, IggyError> {
        // Check first message for priority (batch typically has same priority)
        if let Some(msg) = messages.first() {
            if let Ok(envelope) = serde_json::from_slice::<EventEnvelope>(&msg.payload) {
                if envelope.priority == "high" {
                    return Ok(self.high_priority_partition);
                }
            }
        }
        
        // Round-robin for normal priority
        let count = self.counter.fetch_add(1, Ordering::Relaxed);
        let idx = (count as usize) % self.normal_partitions.len();
        Ok(self.normal_partitions[idx])
    }
}
```

### Using a Custom Partitioner

```rust
use iggy::prelude::*;
use std::sync::Arc;

// Create your custom partitioner
let partitioner = Arc::new(WeightedPartitioner {
    counter: AtomicU64::new(0),
    high_capacity_partitions: vec![0, 1],
    low_capacity_partitions: vec![2, 3, 4],
    high_capacity_weight: 70,
});

// Use with IggyClient
let client = IggyClient::builder()
    .with_tcp()
    .with_server_address("127.0.0.1:8090")
    .with_partitioner(partitioner)  // Inject custom partitioner
    .build()?;
```

### When to Use Custom Partitioners

| Use Case | Built-in Strategy | Custom Partitioner |
|----------|-------------------|-------------------|
| Simple key-based | `messages_key()` | Not needed |
| Round-robin | `balanced()` | Not needed |
| Weighted distribution | - | Yes |
| Time-bucketed | - | Yes |
| Content-based routing | - | Yes |
| A/B testing | - | Yes |
| Geographic routing | - | Yes |

---

## Real-World Scenarios

### Scenario 1: Order Processing Pipeline

```
Requirements:
- 10,000 orders/day (~0.1 orders/sec average, 10 orders/sec peak)
- Each order has 5-10 events in its lifecycle
- Order events MUST be processed in sequence
- 3 order processors for redundancy

Solution:
- Topic: "order-events" with 6 partitions
- Partition key: order_id
- Consumer group: "order-processors" with 3 instances

Why 6 partitions?
- 3 consumers = minimum 3 partitions
- 6 gives room to scale to 6 consumers
- Even distribution: 2 partitions per consumer
```

### Scenario 2: High-Volume Clickstream

```
Requirements:
- 1,000,000 clicks/hour (~280 clicks/sec)
- Need to reconstruct user sessions
- Session analysis must see events in order
- Scale processing as needed

Solution:
- Topic: "clickstream" with 20 partitions
- Partition key: session_id
- Consumer group: "session-analyzers"

Why 20 partitions?
- 280 msg/sec ÷ ~50 msg/sec per consumer = 6 consumers minimum
- 20 partitions allows scaling to 20 consumers
- High cardinality of session_id ensures even distribution
```

### Scenario 3: Financial Transactions

```
Requirements:
- Account balance must never go negative
- Transactions for same account must be strictly ordered
- Regulatory audit trail required
- Zero message loss tolerance

Solution:
- Topic: "transactions" with 12 partitions
- Partition key: account_id
- Consumer group: "transaction-processors"
- Replication factor: 3 (when available)
- acks: all (wait for all replicas)

Why account_id as key?
- Balance calculations require strict ordering
- Deposit → Withdrawal → Check must be in order
- Different accounts can be processed in parallel
```

### Scenario 4: Multi-Tenant SaaS

```
Requirements:
- 500 tenants, varying activity levels
- Tenant data isolation
- Fair resource allocation
- Scale per tenant activity

Solution:
- Topic: "tenant-events" with 50 partitions
- Partition key: tenant_id
- Consumer group per processing type

Why tenant_id?
- Natural isolation boundary
- Events for a tenant stay together
- Hot tenants spread across partitions via hashing
```

---

## Code Examples

### Basic Partitioned Producer

```rust
use iggy::prelude::*;
use uuid::Uuid;

struct OrderEvent {
    order_id: Uuid,
    event_type: String,
    data: serde_json::Value,
}

async fn publish_order_event(
    producer: &IggyProducer,
    event: &OrderEvent,
) -> Result<(), Error> {
    let payload = serde_json::to_vec(event)?;
    
    // Use order_id as partition key
    // All events for this order go to the same partition
    let partition_key = event.order_id.to_string();
    
    producer
        .send_with_key(partition_key, payload)
        .await?;
    
    Ok(())
}

// Usage: All these events will be in the same partition, in order
async fn process_order(producer: &IggyProducer, order_id: Uuid) {
    let events = vec![
        OrderEvent {
            order_id,
            event_type: "created".into(),
            data: json!({"items": 3, "total": 99.99}),
        },
        OrderEvent {
            order_id,
            event_type: "paid".into(),
            data: json!({"method": "credit_card"}),
        },
        OrderEvent {
            order_id,
            event_type: "shipped".into(),
            data: json!({"carrier": "UPS", "tracking": "1Z999"}),
        },
    ];
    
    for event in events {
        publish_order_event(producer, &event).await.unwrap();
    }
}
```

### Consumer with Partition Awareness

```rust
async fn run_consumer() -> Result<(), Error> {
    let mut consumer = client
        .consumer_group("ecommerce", "order-events", "processors")?
        .auto_commit(AutoCommit::IntervalOrWhen(
            IggyDuration::from_str("5s")?,
            AutoCommitWhen::ConsumingAllMessages,
        ))
        .create_consumer_group_if_not_exists()
        .auto_join_consumer_group()
        .build();
    
    consumer.init().await?;
    
    while let Some(message) = consumer.next().await {
        // message.partition_id tells you which partition this came from
        println!(
            "Processing message from partition {}: offset {}",
            message.partition_id,
            message.offset
        );
        
        let event: OrderEvent = serde_json::from_slice(&message.payload)?;
        
        // All events for the same order_id come from the same partition
        // So they arrive in order
        process_order_event(&event).await?;
    }
    
    Ok(())
}
```

### Manual Partition Selection (Advanced)

```rust
// Sometimes you need explicit control over partition assignment

async fn send_to_specific_partition(
    producer: &IggyProducer,
    partition_id: u32,
    message: &[u8],
) -> Result<(), Error> {
    // Use partition ID directly instead of key-based routing
    producer
        .send_to_partition(partition_id, message.to_vec())
        .await
}

// Use case: Priority processing
// High-priority orders go to partition 0 (dedicated fast consumer)
// Normal orders distributed across partitions 1-5

async fn send_order_by_priority(
    producer: &IggyProducer,
    order: &Order,
) -> Result<(), Error> {
    let payload = serde_json::to_vec(order)?;
    
    if order.priority == Priority::High {
        producer.send_to_partition(0, payload).await
    } else {
        // Normal priority: use order_id as key for distribution
        producer.send_with_key(order.id.to_string(), payload).await
    }
}
```

### Partition-Aware Batch Processing

```rust
use std::collections::HashMap;

async fn process_batch_by_partition(
    messages: Vec<Message>,
) -> Result<(), Error> {
    // Group messages by partition for efficient processing
    let mut by_partition: HashMap<u32, Vec<Message>> = HashMap::new();
    
    for msg in messages {
        by_partition
            .entry(msg.partition_id)
            .or_default()
            .push(msg);
    }
    
    // Process each partition's messages (they're already ordered within partition)
    for (partition_id, partition_messages) in by_partition {
        println!("Processing {} messages from partition {}", 
            partition_messages.len(), partition_id);
        
        for msg in partition_messages {
            // Messages within a partition are in order
            process_message(&msg).await?;
        }
    }
    
    Ok(())
}
```

---

## Troubleshooting

### Problem: Uneven Partition Distribution

**Symptoms:**
- Some partitions have much more data than others
- Some consumers are idle while others are overloaded

**Diagnosis:**
```bash
# Check partition sizes via Iggy CLI
iggy topic get stream-name topic-name

# Look for skew in message counts per partition
```

**Solutions:**

1. **Hot key problem**: A few keys produce most messages
   ```
   Bad: partition_key = "US"  (80% of users)
   Good: partition_key = user_id  (even distribution)
   ```

2. **Low cardinality**: Too few unique keys
   ```
   Bad: partition_key = status  (5 values)
   Good: partition_key = order_id  (millions of values)
   ```

3. **Add salting** for hot keys:
   ```rust
   // Instead of just user_id for power users
   let salt = random::<u8>() % 10;
   let partition_key = format!("{}-{}", user_id, salt);
   // Spreads hot user across 10 partitions
   // Trade-off: events for same user may be out of order
   ```

### Problem: Consumer Lag Increasing

**Symptoms:**
- Messages pile up
- Processing falls behind
- Offset lag grows

**Solutions:**

1. **Add consumers** (up to partition count)
   ```
   Current: 2 consumers, 6 partitions
   Solution: Scale to 6 consumers
   ```

2. **Increase partitions** (requires migration)
   ```
   Current: 3 partitions, 3 consumers at max
   Solution: Create new topic with 10 partitions, migrate
   ```

3. **Optimize consumer**
   ```rust
   // Process larger batches
   .batch_size(1000)  // Instead of 100
   
   // Reduce commit frequency
   .auto_commit(AutoCommit::IntervalOrWhen(
       IggyDuration::from_str("10s")?,  // Less frequent
       AutoCommitWhen::ConsumingAllMessages,
   ))
   ```

### Problem: Out-of-Order Processing

**Symptoms:**
- Events processed in wrong sequence
- State inconsistencies
- "Cannot find entity" errors

**Diagnosis:**
```rust
// Add logging to track event order
println!(
    "Processing event {} for order {} (partition {}, offset {})",
    event.event_type,
    event.order_id,
    message.partition_id,
    message.offset
);
```

**Solutions:**

1. **Add partition key** if missing
   ```rust
   // Before: no key, random partition
   producer.send(message).await;
   
   // After: consistent partitioning
   producer.send_with_key(order_id, message).await;
   ```

2. **Check for multiple producers** using different keys
   ```
   Producer A: partition_key = order.id
   Producer B: partition_key = order.user_id  ← Inconsistent!
   
   Fix: All producers must use same key strategy
   ```

3. **Verify consumer group** name is consistent
   ```
   Consumer A: group = "processors"
   Consumer B: group = "processor"  ← Different group!
   
   Different groups = both receive all messages
   ```

---

## Summary

### Key Takeaways

1. **Partitions enable parallelism**, not categorization
2. **Partition keys ensure ordering** for related events
3. **Choose high-cardinality keys** for even distribution
4. **Start with fewer partitions** and scale as needed
5. **Never change partition count** on active topics
6. **Monitor partition lag** and distribution

### Quick Decision Guide

```
Do I need ordering for related events?
├── Yes → Use partition key (entity ID)
└── No  → Random distribution is fine

How many partitions?
├── Low volume (<1K/sec)  → 3-6
├── Medium (<100K/sec)    → 6-20
└── High (>100K/sec)      → 20-100+

What should my partition key be?
└── The ID of the entity whose events must be ordered
    (order_id, user_id, account_id, device_id, etc.)
```

---

## Further Reading

### Apache Iggy Resources
- [Apache Iggy Documentation](https://iggy.apache.org/docs/) - Official documentation
- [Iggy Getting Started Guide](https://iggy.apache.org/docs/introduction/getting-started) - Partitioning strategies explained
- [Iggy Server Configuration](https://iggy.apache.org/docs/server/configuration) - Partition and segment settings
- [Iggy Rust SDK](https://docs.rs/iggy/latest/iggy/) - API reference for `Partitioning`, `Partitioner` trait

### Distributed Systems Fundamentals
- [Designing Data-Intensive Applications](https://dataintensive.net/) - Chapter 6: Partitioning (Martin Kleppmann)
- [Jay Kreps: The Log](https://engineering.linkedin.com/distributed-systems/log-what-every-software-engineer-should-know-about-real-time-datas-unifying) - Foundational paper on log-based systems
- [Martin Kleppmann's Blog](https://martin.kleppmann.com/) - Distributed systems deep dives

### Related Technologies
- [Kafka Partitioning](https://kafka.apache.org/documentation/#intro_concepts_and_terms) - Similar concepts (Iggy inspired by Kafka)
- [MurmurHash](https://en.wikipedia.org/wiki/MurmurHash) - Hash algorithm used for partition key routing
- [Cassandra Murmur3Partitioner](https://docs.datastax.com/en/cassandra-oss/3.x/cassandra/architecture/archPartitionerM3P.html) - Another system using murmur3

### Performance and Benchmarking
- [Iggy Benchmarks](https://benchmarks.iggy.apache.org) - Official performance metrics
- [Transparent Benchmarking Blog Post](https://iggy.apache.org/blogs) - Iggy's approach to benchmarking

---

*Last updated: December 2025*
