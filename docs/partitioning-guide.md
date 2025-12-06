# Understanding Partitions in Message Streaming

A deep-dive guide into partitioning strategies, partition keys, and ordering guarantees in event-driven systems.

---

## Table of Contents

1. [What is a Partition?](#what-is-a-partition)
2. [Why Partitions Exist](#why-partitions-exist)
3. [The Ordering Problem](#the-ordering-problem)
4. [Partition Keys Explained](#partition-keys-explained)
5. [Choosing Partition Keys](#choosing-partition-keys)
6. [Common Partitioning Mistakes](#common-partitioning-mistakes)
7. [Partition Count Guidelines](#partition-count-guidelines)
8. [Rebalancing and Consumer Groups](#rebalancing-and-consumer-groups)
9. [Real-World Scenarios](#real-world-scenarios)
10. [Code Examples](#code-examples)
11. [Troubleshooting](#troubleshooting)

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

- [Kafka Partitioning](https://kafka.apache.org/documentation/#intro_concepts_and_terms) - Similar concepts apply
- [Designing Data-Intensive Applications](https://dataintensive.net/) - Chapter 6: Partitioning
- [Martin Kleppmann's Blog](https://martin.kleppmann.com/) - Distributed systems fundamentals
- [Jay Kreps: The Log](https://engineering.linkedin.com/distributed-systems/log-what-every-software-engineer-should-know-about-real-time-datas-unifying) - Foundational paper on log-based systems

---

*Last updated: December 2025*
