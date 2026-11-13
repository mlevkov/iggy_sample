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

Throughput: up to 3x (three consumers in parallel)
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
Producer sends events (no partition key, so balanced: each send
goes to the next partition):

Event 1: Created    → Partition 0
Event 2: Paid       → Partition 1
Event 3: Shipped    → Partition 2
Event 4: Delivered  → Partition 0

Consumer processing order (parallel consumption):

Consumer A (P0): Created → Delivered
Consumer B (P1): Paid
Consumer C (P2): Shipped

Actual processing order: Created → Shipped → Paid → Delivered
                                   ^^^^^^^^^^^^^^^^
                                   OUT OF ORDER! BUG!
```

### The Bug in Action

```rust
// Consumer processes "Shipped" before "Paid"
fn process_order_shipped(db: &Database, order_id: &str) {
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

In Iggy, `hash` is XxHash32 with seed 0 (see
[Understanding the Key Hash](#understanding-the-key-hash-xxhash32)).

Step by step:

```
partition_key = "order-123"

Step 1: Hash the key
        XxHash32("order-123") = 3483354390

Step 2: Modulo by partition count
        3483354390 % 3 = 0

Step 3: Route to partition
        Message → Partition 0
```

### Consistent Hashing Guarantee

The same key ALWAYS maps to the same partition (given fixed partition count):

```
hash("order-123") % 3 = 0  ← Always 0
hash("order-123") % 3 = 0  ← Always 0
hash("order-123") % 3 = 0  ← Always 0

hash("order-456") % 3 = 2  ← Always 2
hash("order-456") % 3 = 2  ← Always 2
```

This is plain modulo hashing, not a consistent-hashing ring: changing the
partition count moves most keys (see
[Mistake 5](#mistake-5-changing-partition-count)).

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

The strategy picks one partition per send request: every message of a
request goes to the same partition. (`IggyProducer` splits a large send into
requests of up to 1000 messages, and with `balanced` each request can go to
a different partition.) Over the binary transports (TCP, QUIC, WebSocket)
the SDK resolves `balanced` and `messages_key` itself and sends an explicit
partition ID. Over HTTP it sends the strategy, and the server resolves it at
admission with the same rules.

### Strategy 1: Balanced (Round-Robin)

Each send goes to the next partition in turn:

```rust
use iggy::prelude::*;

// Each send goes to the next partition: P0 → P1 → P2 → P0 → ...
let partitioning = Partitioning::balanced();
```

Over the binary transports the round-robin cursor belongs to the client,
one per topic, starting at partition 0: several producers each cycle on
their own, and many short-lived clients that send once each all hit
partition 0. Over HTTP the server keeps one counter per topic.

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

Partition IDs start at 0. An ID past the last partition fails the send
with `PartitionNotFound`.

**When to use:**
- Priority queues (partition 0 = high priority)
- Manual load balancing
- Testing specific partitions
- Sticky routing based on external logic

**Trade-off:** You manage distribution manually.

### Strategy 3: Messages Key (Hash-Based)

The partition is the **XxHash32** of your key modulo the partition count:

```rust
use iggy::prelude::*;

// All messages with same key go to same partition
// partition = XxHash32(key) % partition_count

// String keys
let partitioning = Partitioning::messages_key_str("order-123")?;

// Byte slice keys
let partitioning = Partitioning::messages_key(b"order-123")?;

// Numeric keys (efficient - no string conversion)
let partitioning = Partitioning::messages_key_u64(order_id);
let partitioning = Partitioning::messages_key_u128(uuid_as_u128);
```

**Key constraints:**
- Key length: **1 to 255 bytes** (`messages_key` and `messages_key_str` refuse an empty or longer key)
- Supports: `&[u8]`, `&str`, `u32`, `u64`, `u128`
- Numeric keys hash their little-endian bytes, so `messages_key_u64(123)` and
  `messages_key_str("123")` can land on different partitions. Every producer
  of a topic must use the same key form.

**When to use:**
- Ordering required for related events
- Entity-based processing (orders, users, accounts)
- Session affinity

### Partitioning Strategy Comparison

| Strategy | Ordering | Distribution | Use Case |
|----------|----------|--------------|----------|
| `balanced()` | None | Even per client (round-robin of sends) | Logs, metrics, stateless |
| `partition_id(n)` | Per-partition | Manual | Priority queues, testing |
| `messages_key(k)` | Per-key | Hash-based | Entity workflows, sessions |

### Partition Keys Through This Gateway

`POST /messages`, `POST /messages/batch` and
`POST /streams/{stream}/topics/{topic}/messages` take an optional
`partition_key` string. With it, the gateway sends
`Partitioning::messages_key_str(key)`; without it, `Partitioning::balanced()`
(`send_event` and `send_events_batch` in `src/iggy_client/mod.rs`).

- One key covers a whole `/messages/batch` request, and a batch without a
  key goes to one partition too.
- The key lands where `messages_key_str` with the same string lands from any
  binary client, so other producers stay consistent with the gateway if they
  key by the same string.
- The gateway passes the key through unchecked: an empty key, or one longer
  than 255 bytes, fails the send with `500` (`send_error`).
- A poll (`GET /messages`, `GET /streams/{stream}/topics/{topic}/messages`)
  reads one partition, `partition_id`, which is 0-based and defaults to `0`.

```bash
# The default topic has 3 partitions (IGGY_PARTITIONS)
curl -X POST http://localhost:8000/messages \
  -H "Content-Type: application/json" \
  -d '{
    "event": {
      "id": "550e8400-e29b-41d4-a716-446655440010",
      "event_type": "order.created",
      "timestamp": "2024-01-15T10:30:00Z",
      "payload": {"type": "Generic", "data": {"order_id": "order-123"}}
    },
    "partition_key": "order-123"
  }'

# XxHash32("order-123") % 3 = 0, so the event is in partition 0
curl "http://localhost:8000/messages?partition_id=0&offset=0&count=10"
```

### Understanding the Key Hash (XxHash32)

Iggy hashes keys with XxHash32, seed 0 (`calculate_32` in `iggy_common`):

```
partition_id = XxHash32(key_bytes, seed = 0) % partition_count
```

Over TCP, QUIC and WebSocket the SDK computes this before sending, with the
partition count it has cached for the topic. Over HTTP the server computes
the same hash. To predict where a key lands:

```rust
// What the SDK computes for a messages key over TCP, QUIC and WebSocket
let partition_id = iggy_common::calculate_32(b"order-123") % partitions_count;
```

**Properties of XxHash32:**
- Non-cryptographic (fast, not secure)
- Good distribution for well-spread keys
- Deterministic (same key = same hash)

**Why it matters:**
```
XxHash32("order-123") = 0xCF9FC516  → % 3 = 0
XxHash32("order-124") = 0xD9F3B84E  → % 3 = 2  (completely different)

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
  → Throughput capped by what that one consumer processes
  → Cannot scale horizontally

✅ CORRECT: Plan for growth

Topic: "transactions" (10 partitions)
  → Up to 10 consumers
  → Up to 10x the processing parallelism
  → Room to grow
```

### Mistake 3: Too Many Partitions

```
❌ WRONG: 1000 partitions "just in case"

Topic: "notifications" (1000 partitions, the most one create accepts)
  → Memory and file handles for each partition (its own WAL budget,
    consensus pipeline and segment read state)
  → On a cluster, one more consensus group per partition to fail over
  → Most partitions sit idle
  → Harder to rebalance

✅ CORRECT: Start reasonable, increase as needed

Topic: "notifications" (10 partitions)
  → Monitor throughput
  → Add partitions when needed (for a keyed topic, by migrating: see Mistake 5)
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
  hash("order-122") % 3 = 1 → Partition 1

After: 5 partitions
  hash("order-122") % 5 = 3 → Partition 3  ← DIFFERENT!

Old events for "order-122" in Partition 1
New events for "order-122" in Partition 3
Ordering broken!

From 3 to 5 partitions, a key keeps its partition only when
hash % 15 is 0, 1 or 2: about 1 key in 5 stays, 4 in 5 move.

✅ SAFE APPROACH:
1. Create new topic with desired partition count
2. Migrate consumers to new topic
3. Dual-write during transition
4. Deprecate old topic
```

Iggy can also fix a topic's partition count at creation
(`partition_resize_policy: fixed`). This gateway does not offer that option,
and it has no endpoint that adds partitions.

### Mistake 6: No Partition Key at All

```
❌ WRONG: Round-robin distribution (no key)

producer.send(vec![message]).await?;  // Partitioning::balanced() by default

Events spread over partitions, one partition per send:
  Order 123 Created   → Partition 0
  Order 123 Paid      → Partition 1  ← Out of order risk!
  Order 123 Shipped   → Partition 2  ← Out of order risk!

✅ CORRECT: Always use meaningful partition key

let key = Arc::new(Partitioning::messages_key_str(&order_id)?);
producer.send_with_partitioning(vec![message], Some(key)).await?;
// Through this gateway: "partition_key": "<order_id>" in the request body

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

The throughput bands below are illustrative, not Iggy measurements. What one
partition sustains depends on message size, the topic's durability policy
and, usually first, how fast its consumer processes. Measure it.

| Throughput | Partitions | Notes |
|------------|------------|-------|
| < 1K msg/sec | 1-3 | Development, low-volume |
| 1K-10K msg/sec | 3-10 | Small production |
| 10K-100K msg/sec | 10-30 | Medium production |
| 100K+ msg/sec | 30-100+ | High-scale production |

### Capacity Planning Formula

```
Required Partitions = Peak Messages Per Second / Messages Per Partition Per Second

Example (illustrative figures):
  Peak throughput: 50,000 msg/sec
  Per-partition throughput: 5,000 msg/sec (an assumed figure; measure yours)
  Required: 50,000 / 5,000 = 10 partitions

  Add buffer: 10 * 1.5 = 15 partitions
```

### Hard Limits

| Limit | Value | Source |
|-------|-------|--------|
| Partitions in one create request | 1000 | Server (`TooManyPartitions`), `MAX_PARTITIONS_COUNT` in `iggy_common`. This gateway refuses more with `400` (`MAX_PARTITIONS`, `src/validation.rs`) |
| Partitions with `preallocate_segments` | `segment_size` × partitions ≤ 64 GiB, so at most 64 partitions with the default 1 GiB segments | `MAX_PREALLOCATED_TOPIC_BYTES` in `iggy_common`; the gateway checks it before creating the topic |
| Partitions on a node, all topics | `[metadata] partitions_max`, default `0` (no cap) | Server config; past it, creates fail with `PartitionsLimitReached` |

This gateway creates a topic with 1 partition when `partitions` is left out
of `POST /streams/{stream}/topics`, and the default topic with
`IGGY_PARTITIONS` (3).

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
local_data/                     # Base data directory (`path`, env IGGY_PATH)
└── streams/
    └── {stream_id}/
        └── topics/
            └── {topic_id}/
                └── partitions/
                    ├── 0/                              # Partition 0
                    │   ├── 00000000000000000000.log    # Segment 1: message batches
                    │   ├── 00000000000000000000.index  # Segment 1: sparse index
                    │   ├── 00000000000001000000.log    # Segment 2, from offset 1000000
                    │   ├── 00000000000001000000.index
                    │   ├── offsets/                    # Stored consumer and group offsets
                    │   └── prepares-{revision}/        # Partition write-ahead log
                    ├── 1/                              # Partition 1
                    └── 2/                              # Partition 2
```

Segments are files in the partition directory, not directories: each is a
`.log` and an `.index` named after the segment's first offset, padded to 20
digits. Stream, topic and partition directories are numbered from 0. The
[durable-storage guide](durable-storage-guide.md#on-disk-layout) lists the
remaining files, observed in a running 0.10.0 container.

### Segments

Each partition is divided into **segments**, files of about `segment_size`
(a per-topic option, 1 GiB by default) that make up the append-only log:

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

`segment_size` is a soft limit: a segment closes after the batch that
crosses it, so a sealed segment can run one batch past it.

**Segment benefits:**
- Efficient deletion (retention drops whole sealed segments)
- Bounded memory for old data: at most 12 sealed segments per partition keep
  an open read handle, and a sealed segment's index is reloaded from its
  `.index` file when needed
- Asynchronous file I/O (compio over io_uring), not memory-mapped files
- No built-in archiver: back up the data directory, or stream data out
  through a client (see the
  [durable-storage guide](durable-storage-guide.md#backup-and-archiving))

### Indexes

Each segment has one sparse index, its `.index` file. There is no separate
time index: each entry carries both an offset and a timestamp.

**Sparse index (`.index`):**
```
One 24-byte entry per chunk of batches written to the segment:
offset u64 (absolute), timestamp u64 (microseconds), position u64,
all little-endian
┌────────────┬──────────────────┬───────────────┐
│ Offset     │ Timestamp        │ File Position │
├────────────┼──────────────────┼───────────────┤
│ 1000000    │ 1705312800000000 │ 0             │  ← 2024-01-15T10:00:00Z
│ 1001024    │ 1705312860000000 │ 1052672       │  ← 10:01:00, sparse
│ 1002048    │ 1705312920000000 │ 2105344       │  ← 10:02:00
└────────────┴──────────────────┴───────────────┘

Lookup by offset: offset 1001500 → nearest entry at or below
                  (1001024) → scan from position 1052672
Lookup by time:   "messages after 10:01:30" → nearest entry at or
                  below (10:01:00) → scan from position 1052672
```

---

## Iggy Server Configuration

Since server 0.9, the storage settings that shape partitions belong to each
topic, set when the topic is created: segment size, flush thresholds,
durability, message expiry and the size cap. They apply to every partition of
the topic. This gateway accepts them in the `options` object of
`POST /streams/{stream}/topics`:

```json
{
  "name": "orders",
  "partitions": 6,
  "options": {
    "segment_size": "256 MiB",
    "message_expiry": "7days",
    "max_topic_size": "60 GiB"
  }
}
```

What matters for partitioning:

- **`max_topic_size` is enforced per partition**, as the cap divided by the
  partition count: 60 GiB across 6 partitions retains about 10 GiB in each.
  A hot partition (a skewed key) reaches its share first and loses its
  oldest data first.
- **Each partition's share is floored at one sealed segment** (`segment_size`
  plus the maximum bus frame), so a small cap still keeps about partitions ×
  `segment_size`: a 1 GiB cap across 6 partitions with 1 GiB segments keeps
  about 6 GiB of sealed data, while a topic read still reports 1 GiB
  (`per_partition_size_budget` in the server's `segment_cleaner.rs`).
- **A finite cap must be at least one segment** (`segment_size`, 1 GiB by
  default), counted for the whole topic, not per partition.
- **`preallocate_segments` reserves `segment_size` per partition** when the
  topic is created, up to 64 GiB per topic, so many partitions multiply its
  disk cost.
- **Retention runs in the server's cleaner**, which 0.10 enables by default
  (`[data_maintenance.messages] cleaner_enabled = true`, every minute). It
  deletes whole sealed segments, never a partition's open one.

The 0.8 settings this section used to show (`[system.partition]`,
`[system.segment]`, `[system.topic]`, `cache_indexes`) are gone from 0.10's
configuration. The
[durable-storage guide](durable-storage-guide.md#data-retention-policies)
covers each option, its bounds, and the server-wide keys that remain.

### Retry Deduplication

0.8's message-ID deduplication (`[system.message_deduplication]`) is gone.
Since 0.9 each partition deduplicates **retries within a client session**:
when the SDK resends a request whose reply it lost, the partition recognises
it and does not append it twice (`RequestAlreadyApplied`, which this gateway
answers with `202`). `[partition] dedup_clients_max` (default 4096) sizes that
protection per partition. A full table refuses new writers with
`TransientNotAccepted` rather than evicting existing protection.

This covers the SDK's own retries, not an application that sends the same
message twice. Give messages stable IDs and deduplicate in consumers where
that matters.

### Configuration Trade-offs

| Goal | Topic `options` |
|------|---------------|
| **Maximum durability** | `"durability": "persisted"`, `"consumer_offset_durability": "persisted"` |
| **Maximum throughput** | defaults (`replicated`); raise `messages_required_to_save` to batch more per segment write |
| **Bounded disk** | `"max_topic_size"`, with a `segment_size` small enough to fit under it |
| **Auto-cleanup** | `"message_expiry": "7days"` (the cleaner runs by default) |

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

Scenario 2: 3 consumers (rebalance after C joins)
┌─────────────────────────────────────────┐
│ Consumer A: P0, P1                      │
│ Consumer B: P3, P4                      │
│ Consumer C: P2, P5    ← only these move │
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

The server keeps each member's partitions where it can and moves only those
above a member's fair share, as in Scenario 2. After several joins, which
member holds which partition depends on the order they joined, so read the
assignment rather than assume it.

### Rebalancing Triggers

| Event | What Happens |
|-------|--------------|
| Consumer joins | Partitions above each member's fair share move to it |
| Consumer leaves (`shutdown()` leaves the group) | Its partitions go to the remaining members at once |
| Consumer crashes or loses its connection | Its partitions are reassigned when its session ends: 1.2 × `[heartbeat] interval` (36 s at the default 30 s) after the server last heard from it |
| Partitions added or removed | The group rebalances over the new set; members pick it up on their next heartbeat |

### Rebalancing Impact

```
During rebalance (server 0.10):
1. Coordinator recalculates assignments, moving as few partitions
   as it can
2. A partition that moves enters pending revocation: its current
   owner stops reading it, so it can commit what it was served
3. The handoff completes once that owner has committed, or after
   [consumer_group] rebalancing_timeout (30 s by default)
4. The new owner reads on from the last committed offset

Partitions that do not move keep being consumed throughout.
A handoff forced by the timeout can deliver the uncommitted
messages again, to the new owner.
```

### Minimizing Rebalance Impact

```rust
use futures::StreamExt;
use iggy::prelude::*;

// No session timeout or heartbeat setting here: the server ends a
// member's session 1.2 x [heartbeat] interval (36 s by default) after
// it last heard from it, and the SDK pings every 5 s on its own.
let mut consumer = client
    .consumer_group("order-processors", "ecommerce", "orders")?
    .poll_options(PollOptions {
        count: 100,
        // Store the processed position once per batch, not per message.
        // The consumer also stores it before a rebalance takes a
        // partition away.
        commit: CommitPolicy::Auto(AutoCommit {
            every: Some(CommitEvery::Batch),
            ..AutoCommit::default()
        }),
        ..PollOptions::wait(IggyDuration::ONE_SECOND)
    })
    .build();
consumer.init().await?;

while let Some(received) = consumer.next().await {
    match received {
        Ok(received) => process(&received.message)?,
        // The reader stays usable after a read error and retries
        Err(error) => eprintln!("Read failed: {error}"),
    }
}

// Leave the group: its partitions move to the other members at once,
// not after the session timeout
consumer.shutdown().await?;
```

A longer session timeout (a larger `[heartbeat] interval`, set the same on
every node) rides out longer network outages without a reassignment, but
delays the reassignment after a real crash by the same amount.

---

## Custom Partitioners

Iggy supports custom partitioning logic through the `Partitioner` trait. This is useful when built-in strategies don't meet your needs.

### The Partitioner Trait

The trait lives in `iggy_common` and is re-exported by `iggy::prelude`:

```rust
use iggy::prelude::*;
use std::fmt::Debug;

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

Only `IggyProducer` calls it: once per batch it writes (one `send` call, or
adjacent sends that a background producer merged), with all messages of the
batch, and its result replaces any `Partitioning` the call or the producer
names. Plain `send_messages` calls on the client ignore it. With a
client-side encryptor, it sees the encrypted payloads.

### Example: Weighted Partitioner

Route more traffic to specific partitions (e.g., for heterogeneous hardware):

```rust
use iggy::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// Routes 70% of sends to partitions 0-1, 30% to partitions 2-4
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
use std::sync::atomic::{AtomicU64, Ordering};

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
use std::sync::atomic::AtomicU64;

// Create your custom partitioner
let partitioner = Arc::new(WeightedPartitioner {
    counter: AtomicU64::new(0),
    high_capacity_partitions: vec![0, 1],
    low_capacity_partitions: vec![2, 3, 4],
    high_capacity_weight: 70,
});

// Set it on the client builder, before choosing the transport
let client = IggyClient::builder()
    .with_partitioner(partitioner)
    .with_tcp()
    .with_server_address("127.0.0.1:8090".to_string())
    .build()?;
```

Every producer built from that client (`client.producer(..)`) then uses it.
To give it to one producer only, set it on the producer builder:

```rust
let producer = client
    .producer("ecommerce", "orders")?
    .partitioner(partitioner)
    .build();
producer.init().await?;
```

This gateway sends through the `Client` trait, not `IggyProducer`, so it
uses no partitioner: its routing is the `partition_key` described in
[Partition Keys Through This Gateway](#partition-keys-through-this-gateway).

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
- 280 msg/sec ÷ ~50 msg/sec per consumer (an assumed processing
  rate) = 6 consumers minimum
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
- Replication factor: 3 (the `replication_factor` topic option since server 0.10; see the [cluster demo](cluster-demo.md#replication-factor))
- Durability: "persisted". Iggy has no acks setting to raise: a
  successful send has already waited for the replication quorum
  to commit it, and "persisted" also requires stable-storage
  copies at that quorum
  (see the [durable-storage guide](durable-storage-guide.md#the-two-policies))

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
- Natural ordering boundary (not physical isolation: about
  10 tenants share each partition here)
- Events for a tenant stay together
- Tenants spread across partitions via hashing, but each tenant,
  however hot, stays on one partition (see salting under
  Troubleshooting)
```

---

## Code Examples

### Basic Partitioned Producer

```rust
use iggy::prelude::*;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Serialize)]
struct OrderEvent {
    order_id: Uuid,
    event_type: String,
    data: serde_json::Value,
}

async fn publish_order_event(
    producer: &IggyProducer,
    event: &OrderEvent,
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(event)?;
    let message = IggyMessage::builder().payload(payload.into()).build()?;

    // Use order_id as partition key
    // All events for this order go to the same partition
    let partitioning = Partitioning::messages_key_str(&event.order_id.to_string())?;

    producer
        .send_with_partitioning(vec![message], Some(Arc::new(partitioning)))
        .await?;

    Ok(())
}

// Usage: All these events will be in the same partition, in order,
// because one task awaits each send before the next
async fn process_order(producer: &IggyProducer, order_id: Uuid) -> anyhow::Result<()> {
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
        publish_order_event(producer, &event).await?;
    }
    Ok(())
}
```

### Consumer with Partition Awareness

```rust
use futures::StreamExt;
use iggy::prelude::*;

async fn run_consumer(client: &IggyClient) -> anyhow::Result<()> {
    let mut consumer = client
        .consumer_group("processors", "ecommerce", "order-events")?
        .poll_options(PollOptions {
            // Store processed offsets every 10,000 messages or 5 s
            commit: CommitPolicy::Auto(AutoCommit::default()),
            ..PollOptions::wait(IggyDuration::ONE_SECOND)
        })
        .create_consumer_group_if_not_exists()
        .build();

    // Creates the group if needed and joins it
    consumer.init().await?;

    while let Some(received) = consumer.next().await {
        let received = match received {
            Ok(received) => received,
            Err(error) => {
                eprintln!("Read failed: {error}");
                continue;
            }
        };
        // received.partition_id tells you which partition this came from
        println!(
            "Processing message from partition {}: offset {}",
            received.partition_id, received.message.header.offset
        );

        let event: OrderEvent = serde_json::from_slice(&received.message.payload)?;

        // All events for the same order_id come from the same partition
        // So they arrive in order
        process_order_event(&event).await?;
    }

    consumer.shutdown().await?;
    Ok(())
}
```

`consumer_group(name, stream, topic)` takes the group name first. The
consumer's default commit policy is `Manual`: without `CommitPolicy::Auto`
(or `commit(received.checkpoint())` after processing), nothing is stored and
the next run reads the same messages again.

### Manual Partition Selection (Advanced)

```rust
use iggy::prelude::*;
use std::sync::Arc;

// Sometimes you need explicit control over partition assignment

async fn send_to_specific_partition(
    producer: &IggyProducer,
    partition_id: u32,
    payload: &[u8],
) -> anyhow::Result<()> {
    let message = IggyMessage::builder().payload(payload.to_vec().into()).build()?;

    // Use partition ID directly instead of key-based routing
    let partitioning = Partitioning::partition_id(partition_id);
    producer
        .send_with_partitioning(vec![message], Some(Arc::new(partitioning)))
        .await?;
    Ok(())
}

// Use case: Priority processing
// High-priority orders go to partition 0 (dedicated fast consumer)
// Normal orders distributed across partitions 1-5 (6 partitions in all)

async fn send_order_by_priority(producer: &IggyProducer, order: &Order) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(order)?;
    let message = IggyMessage::builder().payload(payload.into()).build()?;

    let partition_id = if order.priority == Priority::High {
        0
    } else {
        // Hash the order ID over partitions 1-5 only. A messages key would
        // hash over all six partitions, partition 0 included.
        1 + iggy_common::calculate_32(order.id.to_string().as_bytes()) % 5
    };

    let partitioning = Partitioning::partition_id(partition_id);
    producer
        .send_with_partitioning(vec![message], Some(Arc::new(partitioning)))
        .await?;
    Ok(())
}
```

### Partition-Aware Batch Processing

```rust
use iggy::prelude::*;
use std::collections::HashMap;

async fn process_batch_by_partition(messages: Vec<ReceivedMessage>) -> anyhow::Result<()> {
    // Group messages by partition for efficient processing
    let mut by_partition: HashMap<u32, Vec<ReceivedMessage>> = HashMap::new();

    for msg in messages {
        by_partition.entry(msg.partition_id).or_default().push(msg);
    }

    // Process each partition's messages (they're already ordered within partition)
    for (partition_id, partition_messages) in by_partition {
        println!(
            "Processing {} messages from partition {}",
            partition_messages.len(),
            partition_id
        );

        for msg in partition_messages {
            // Messages within a partition are in order
            process_message(&msg).await?;
        }
    }

    Ok(())
}
```

`ReceivedMessage` is what `IggyConsumer` yields: the message
(`msg.message.header.offset`, `msg.message.payload`) plus the
`partition_id` it came from.

---

## Troubleshooting

### Problem: Uneven Partition Distribution

**Symptoms:**
- Some partitions have much more data than others
- Some consumers are idle while others are overloaded

**Diagnosis:**
```bash
# Check partition sizes via Iggy CLI (size and message count per partition)
iggy topic get stream-name topic-name

# Or through this gateway: "partitions" lists each partition's
# messages_count, members and in_sync_members
curl http://localhost:8000/streams/stream-name/topics/topic-name

# Look for skew in message counts per partition
```

On a cluster, a partition whose `in_sync_members` is shorter than its
`members` has a replica behind or down; node `status` in `GET /cluster` is
not a liveness signal on 0.10.

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
   use rand::random;

   // Instead of just user_id for power users
   let salt = random::<u8>() % 10;
   let partition_key = format!("{}-{}", user_id, salt);
   // Spreads a hot user over up to 10 partitions
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
   use iggy::prelude::*;
   use std::time::Duration;

   let consumer = client
       .consumer_group("processors", "ecommerce", "order-events")?
       .poll_options(PollOptions {
           // Messages per read, over all partitions of a fetch (1000 is
           // also the default)
           count: 1000,
           // Store offsets less often: every 10 s instead of every 5 s
           commit: CommitPolicy::Auto(AutoCommit {
               interval: Some(NonZeroIggyDuration::new(Duration::from_secs(10))?),
               ..AutoCommit::default()
           }),
           ..PollOptions::wait(IggyDuration::ONE_SECOND)
       })
       .build();
   ```

   Through this gateway, a poll returns at most `POLL_MAX_COUNT` messages
   (default 100) whatever `count` asks for; raise it for larger reads.

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
    received.partition_id,
    received.message.header.offset
);
```

**Solutions:**

1. **Add partition key** if missing
   ```rust
   use iggy::prelude::*;
   use std::sync::Arc;

   // Before: no key, so each send goes to the next partition
   // producer.send(messages).await?;

   // After: consistent partitioning
   let key = Arc::new(Partitioning::messages_key_str(&order_id)?);
   producer.send_with_partitioning(messages, Some(key)).await?;
   ```

   Through this gateway, add `"partition_key"` to the request body.

2. **Check for multiple producers** using different keys
   ```
   Producer A: partition_key = order.id
   Producer B: partition_key = order.user_id  ← Inconsistent!

   Producer A: messages_key_str("123")
   Producer B: messages_key_u64(123)          ← Same ID, different bytes!

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
└── No  → Round-robin (balanced) is fine

How many partitions? (illustrative; measure your own throughput)
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
- [Iggy Rust SDK 0.12.0](https://docs.rs/iggy/0.12.0/iggy/) - API reference for `Partitioning`, `Partitioner` trait

### Distributed Systems Fundamentals
- [Designing Data-Intensive Applications](https://dataintensive.net/) - Chapter 6: Partitioning (Martin Kleppmann)
- [Jay Kreps: The Log](https://engineering.linkedin.com/distributed-systems/log-what-every-software-engineer-should-know-about-real-time-datas-unifying) - Foundational paper on log-based systems
- [Martin Kleppmann's Blog](https://martin.kleppmann.com/) - Distributed systems deep dives

### Related Technologies
- [Kafka Partitioning](https://kafka.apache.org/documentation/#intro_concepts_and_terms) - Similar concepts (Iggy inspired by Kafka)
- [xxHash](https://github.com/Cyan4973/xxHash) - Hash family of XxHash32, which Iggy uses (seed 0, through the `twox-hash` crate) for partition key routing

### Performance and Benchmarking
- [Iggy Benchmarks](https://benchmarks.iggy.apache.org) - Official performance metrics
- [Transparent Benchmarking Blog Post](https://iggy.apache.org/blogs) - Iggy's approach to benchmarking

---

*Last updated: November 2026 (Iggy server 0.10.0, Rust SDK 0.12.0)*
