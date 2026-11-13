# Event-Driven Architecture with Apache Iggy

A comprehensive guide to understanding and implementing event-driven applications using Apache Iggy message streaming.

---

## Table of Contents

1. [Core Concepts](#core-concepts)
2. [Streams, Topics, and Partitions](#streams-topics-and-partitions)
3. [Message Flow Patterns](#message-flow-patterns)
4. [Partition Keys and Ordering](#partition-keys-and-ordering)
5. [Consumer Groups](#consumer-groups)
6. [Message Retention and Expiry](#message-retention-and-expiry)
7. [Error Handling Strategies](#error-handling-strategies)
8. [Production Patterns](#production-patterns)
9. [Code Examples](#code-examples)
10. [Quick Reference](#quick-reference)

---

## Core Concepts

### What is Event-Driven Architecture?

Event-driven architecture (EDA) is a design pattern where the flow of the program is determined by events - significant changes in state that the system needs to react to.

```
Traditional Request/Response:
Client → Request → Server → Response → Client (synchronous, blocking)

Event-Driven:
Producer → Event → Message Broker → Consumer(s) (asynchronous, decoupled)
```

### Key Benefits

| Benefit | Description |
|---------|-------------|
| **Decoupling** | Producers don't need to know about consumers |
| **Scalability** | Add consumers independently to handle load |
| **Resilience** | Events persist; consumers can recover and replay |
| **Flexibility** | Multiple consumers can react to same event differently |

### Apache Iggy Overview

Apache Iggy is a persistent message streaming platform written in Rust, designed for:
- **High throughput**: Millions of messages/second
- **Low latency**: Sub-millisecond tail latencies
- **Durability**: Persistent append-only log with per-topic retention and durability
- **Replication**: Optional clustering (VSR consensus) with a per-topic replication factor
- **Multiple protocols**: TCP, QUIC, HTTP, WebSocket

---

## Streams, Topics, and Partitions

### Hierarchy

```
Iggy Server
└── Stream (logical grouping, like a database)
    └── Topic (event category, like a table)
        └── Partition (parallelism unit, like a shard)
            └── Messages (ordered within partition)
```

### Streams

A **stream** is a top-level container for related topics. Use streams for:
- Multi-tenancy (one stream per tenant)
- Environment separation (dev, staging, prod)
- Domain boundaries (ecommerce, analytics, notifications)

```rust
// Creating a stream (the server assigns its numeric ID)
client.create_stream("ecommerce", &StreamCreateOptions::default()).await?;
```

### Topics

A **topic** is a named channel for a specific category of events. Think of it as:
- A "table" in database terms
- A "queue" in messaging terms
- A "channel" in pub/sub terms

```rust
// Creating a topic with 3 partitions, 7-day retention
client
    .create_topic(
        &Identifier::named("ecommerce")?, // stream, by name or numeric ID
        "orders",                         // topic name
        &TopicCreateOptions {
            partitions_count: Some(3),
            message_expiry: Some(IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?)),
            // Nodes that store each partition; None takes the server's
            // default: every node of a cluster, so 1 on a single node
            replication_factor: None,
            // Options left out take the server's defaults
            ..Default::default()
        },
    )
    .await?;
```

**Topic Design Guidelines:**

| Guideline | Example |
|-----------|---------|
| One topic per event category | `orders`, `users`, `payments` |
| Verb-noun naming | `order-created`, `user-updated` |
| Don't over-partition | Start with 3-10, scale as needed |

### Partitions

**Partitions are NOT for sub-categorization.** They exist for:

1. **Parallelism** - Multiple consumers can read simultaneously
2. **Ordering** - Messages within a partition are strictly ordered

```
Topic: "orders" (3 partitions)
├── Partition 0 → Consumer A (processes ~33% of orders)
├── Partition 1 → Consumer B (processes ~33% of orders)
└── Partition 2 → Consumer C (processes ~33% of orders)
```

**Critical Understanding:**

```
WRONG: Partitions as categories
   Partition 0 = "pending orders"
   Partition 1 = "shipped orders"
   Partition 2 = "cancelled orders"

CORRECT: Partitions as parallel lanes
   Partition 0 = orders hashing to 0 (any status)
   Partition 1 = orders hashing to 1 (any status)
   Partition 2 = orders hashing to 2 (any status)
```

### Choosing Partition Count

| Scenario | Partitions | Reasoning |
|----------|------------|-----------|
| Development/Testing | 1-3 | Simplicity |
| Small production | 3-10 | Moderate parallelism |
| High throughput | 10-50 | High parallelism |
| Massive scale | 50-100+ | Maximum parallelism |

**Rule**: Partitions >= maximum number of parallel consumers you'll need.

---

## Message Flow Patterns

### Pattern 1: Simple Pub/Sub

One producer, multiple independent consumers.

```
Producer → Topic → Consumer A (notifications)
                → Consumer B (analytics)
                → Consumer C (audit log)
```

Each consumer maintains its own offset and processes all messages.

### Pattern 2: Competing Consumers (Work Queue)

Multiple consumers in a group share the workload.

```
Producer → Topic (3 partitions) → Consumer Group "workers"
                                  ├── Worker A (partition 0)
                                  ├── Worker B (partition 1)
                                  └── Worker C (partition 2)
```

Each message is delivered to one consumer in the group. After a rebalance or a
failover, messages that were read but not yet committed can be delivered again,
so processing must be idempotent (see [Idempotency](#idempotency)).

### Pattern 3: Event Sourcing

Events as the source of truth; state derived from event replay.

```
Commands → Event Store (Iggy) → Event Handlers → Read Models
                             ↓
                         Event Replay → Rebuild State
```

### Pattern 4: CQRS (Command Query Responsibility Segregation)

Separate write and read paths.

```
Write Path: Command → Validate → Store Event → Publish to Iggy
Read Path:  Iggy → Consumer → Update Read Database → Query API
```

---

## Partition Keys and Ordering

### The Ordering Problem

```
Without partition keys:
Message 1: "Order 123 created"    → Partition 0
Message 2: "Order 123 paid"       → Partition 2  ← Different partition!
Message 3: "Order 123 shipped"    → Partition 1  ← Different partition!

Consumer might process "shipped" before "created" = BUG
```

### Solution: Partition Keys

A partition key ensures related messages go to the same partition:

```rust
// All events for order-123 go to same partition
let partition_key = format!("order-{}", order_id);
let partitioning = Partitioning::messages_key_str(&partition_key)?;
producer
    .send_with_partitioning(vec![message], Some(Arc::new(partitioning)))
    .await?;
```

Through this gateway, the optional `partition_key` field of a send request does
the same; without it, sends are balanced across partitions.

**How it works:**

```
partition_key   = "order-123"
partition_index = xxh32("order-123") % partitions_count
                = 3483354390 % 3
                = 0

All "order-123" events → Partition 0 → Processed in order
```

A binary client (TCP, QUIC, WebSocket) computes the partition before sending;
over HTTP the server computes it. Adding partitions changes `partitions_count`,
which can move a key to another partition.

### Choosing Partition Keys

| Domain | Partition Key | Reasoning |
|--------|---------------|-----------|
| Orders | `order_id` | All events for an order stay ordered |
| Users | `user_id` | User actions processed in sequence |
| Sessions | `session_id` | Session events stay together |
| Devices | `device_id` | Device telemetry ordered |
| Transactions | `txn_id` | Financial events strictly ordered |

### Code Example

```rust
use std::sync::Arc;

use iggy::prelude::*;
use uuid::Uuid;

// Publishing with partition key
async fn publish_order_event(
    producer: &IggyProducer,
    order_id: Uuid,
    event: &OrderEvent,
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(event)?;
    let message = IggyMessage::builder().payload(payload.into()).build()?;

    // Use order_id as partition key - ensures ordering
    let partitioning = Partitioning::messages_key_str(&order_id.to_string())?;

    producer
        .send_with_partitioning(vec![message], Some(Arc::new(partitioning)))
        .await?;

    Ok(())
}

// Events of order 123, sent one after another from one task, keep this order:
for event in [&created, &paid, &shipped] {
    publish_order_event(&producer, order_123, event).await?;
}
```

The key puts the events in one partition; awaiting each send before the next
keeps their order there. Concurrent sends can reach the partition in any order.

---

## Consumer Groups

### What is a Consumer Group?

A consumer group is a set of consumers that cooperatively consume a topic. The server automatically assigns partitions to consumers in the group.

```
Topic (6 partitions) + Consumer Group "processors" (3 consumers)

Partition 0 ─┐
Partition 1 ─┴─→ Consumer A

Partition 2 ─┐
Partition 3 ─┴─→ Consumer B

Partition 4 ─┐
Partition 5 ─┴─→ Consumer C
```

### Consumer Group Guarantees

| Guarantee | Description |
|-----------|-------------|
| **At-least-once delivery** | Each message goes to one member; a rebalance or failover can deliver messages read but not yet committed again, so consumers must be idempotent |
| **Partition exclusivity** | One consumer owns a partition at a time |
| **Automatic rebalancing** | If a consumer leaves or its session ends, its partitions are reassigned |
| **Offset tracking** | Server stores the group's offset for each partition |

### Creating a Consumer Group

```rust
use futures::StreamExt;
use iggy::prelude::*;

let mut consumer = client
    .consumer_group("order-processors", "ecommerce", "orders")? // group, stream, topic
    .poll_options(PollOptions {
        count: 100,
        // Store the position of processed messages every 5 s or 10,000 messages
        commit: CommitPolicy::Auto(AutoCommit::default()),
        ..PollOptions::wait(IggyDuration::ONE_SECOND)
    })
    .polling_strategy(PollingStrategy::next())
    .build();

// Creates the group if it does not exist (the default) and joins it
consumer.init().await?;

while let Some(received) = consumer.next().await {
    let received = received?;
    process_message(&received.message).await?;
    // The message counts as processed once the loop asks for the next one
}

// Stores the last position and leaves the group
consumer.shutdown().await?;
```

`shutdown()` also counts the message the loop received last as processed. Where
a failed message must come back, commit manually
([At-Least-Once Delivery](#at-least-once-delivery)).

### Scaling with Consumer Groups

```
Initial: 1 consumer, 3 partitions
┌─────────────────────────────────┐
│ Consumer A: P0, P1, P2          │
└─────────────────────────────────┘

Scale up: 3 consumers, 3 partitions (rebalance)
┌───────────┬───────────┬───────────┐
│ Consumer A│ Consumer B│ Consumer C│
│    P0     │    P1     │    P2     │
└───────────┴───────────┴───────────┘

Over-scaled: 4 consumers, 3 partitions (one idle)
┌───────────┬───────────┬───────────┬───────────┐
│ Consumer A│ Consumer B│ Consumer C│ Consumer D│
│    P0     │    P1     │    P2     │   IDLE    │
└───────────┴───────────┴───────────┴───────────┘
```

**Key insight**: You can't have more active consumers than partitions.

---

## Message Retention and Expiry

### Retention Policies

Iggy supports time-based message expiry, set per topic:

```rust
// Topic with 7-day retention
let transient = IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?);

// Topic with 1-year retention (audit logs)
let audit = IggyExpiry::ExpireDuration(IggyDuration::from_str("365days")?);

// Never expire (permanent event store)
let permanent = IggyExpiry::NeverExpire;

// Use server default
let default = IggyExpiry::ServerDefault;
```

A topic can also cap its size (`max_topic_size`). Both delete whole sealed
segments and never touch the active one: expiry deletes a segment once its
newest message has expired, so a message can outlive its expiry. See the
[Durable Storage Guide](durable-storage-guide.md#data-retention-policies).

### Retention Strategy by Use Case

| Use Case | Retention | Reasoning |
|----------|-----------|-----------|
| Transient events | 1-7 days | Processed and done |
| Audit logs | 1-7 years | Compliance requirements |
| Event sourcing | Forever | Events are source of truth |
| Metrics/telemetry | 30-90 days | Trend analysis window |

### Example: Multi-Topic Retention

```rust
let myapp = Identifier::named("myapp")?;

// Transient processing queue - 7 days
let task_queue = TopicCreateOptions {
    partitions_count: Some(3),
    message_expiry: Some(IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?)),
    ..Default::default()
};
client.create_topic(&myapp, "task-queue", &task_queue).await?;

// Audit log - 1 year
let audit_log = TopicCreateOptions {
    partitions_count: Some(1),
    message_expiry: Some(IggyExpiry::ExpireDuration(IggyDuration::from_str("365days")?)),
    ..Default::default()
};
client.create_topic(&myapp, "audit-log", &audit_log).await?;

// Event store - never expire
let event_store = TopicCreateOptions {
    partitions_count: Some(10),
    message_expiry: Some(IggyExpiry::NeverExpire),
    // Acknowledge a write only once it is on stable storage;
    // Replicated, the default, can acknowledge it before it reaches disk
    durability: Durability::Persisted,
    ..Default::default()
};
client.create_topic(&myapp, "event-store", &event_store).await?;
```

---

## Error Handling Strategies

### At-Most-Once Delivery

Commit the offset before processing. May lose messages on failure.

```rust
let mut consumer = client
    .consumer_group("metrics-readers", "stream", "topic")?
    .poll_options(PollOptions {
        // The server stores the offset as part of each read
        commit: CommitPolicy::OnRead,
        ..PollOptions::wait(IggyDuration::ONE_SECOND)
    })
    .build();
consumer.init().await?;

while let Some(received) = consumer.next().await {
    let received = received?;
    // Offset already stored: if processing fails, the message is not delivered again
    if let Err(e) = process_message(&received.message).await {
        log::warn!("Dropped a message: {}", e);
    }
}

consumer.shutdown().await?;
```

On a cluster this is at most once only while the partition's primary stays up:
a failover before the stored offset replicates can deliver the last batch again.

**Use when**: Losing occasional messages is acceptable (metrics, logs).

### At-Least-Once Delivery

Commit offset only after successful processing. May reprocess on failure.

```rust
let mut consumer = client
    .consumer_group("group", "stream", "topic")?
    .poll_options(PollOptions {
        commit: CommitPolicy::Manual, // the default: the application commits
        ..PollOptions::wait(IggyDuration::ONE_SECOND)
    })
    .build();
consumer.init().await?;

while let Some(received) = consumer.next().await {
    let received = received?;
    match process_message(&received.message).await {
        Ok(()) => {
            consumer.commit(received.checkpoint()).await?; // Commit after success
        }
        Err(e) => {
            log::error!("Processing failed: {}, will retry", e);
            // Don't commit, and stop: a commit covers every earlier message of
            // its partition, so committing a later one would skip this one
            break;
        }
    }
}

consumer.shutdown().await?;
```

After `shutdown()` the next member to read the partition, or this consumer once
restarted, starts after the last commit, so the failed message comes back.

**Use when**: Every message must be processed (orders, payments).

### Dead Letter Queue (DLQ)

Move poison messages aside after retry exhaustion.

```rust
const MAX_RETRIES: u32 = 3;

async fn process_with_dlq(
    message: &IggyMessage,
    dlq_producer: &IggyProducer,
) -> anyhow::Result<()> {
    // e.g. from a `retry-count` user header
    let retry_count = get_retry_count(message);

    match process_message(message).await {
        Ok(()) => Ok(()),
        Err(_) if retry_count < MAX_RETRIES => {
            // Requeue with incremented retry count
            requeue_with_retry(message, retry_count + 1).await
        }
        Err(e) => {
            // Max retries exceeded - send to DLQ
            log::error!("Moving to DLQ after {} retries: {}", retry_count, e);
            let dead_letter = IggyMessage::builder()
                .payload(message.payload.clone())
                .build()?;
            dlq_producer.send(vec![dead_letter]).await?;
            Ok(()) // Commit original message
        }
    }
}
```

### Idempotency

Design consumers to handle duplicate messages safely.

**Why idempotency matters:** In at-least-once delivery, the same message may be processed multiple times (network retries, consumer restarts, consumer group rebalances, failovers). Your processing logic must handle this gracefully.

**The Pattern:** Store processed event IDs in a database. Before processing, check if the event was already handled.

```rust
/// Database represents your application's persistent storage.
/// This could be PostgreSQL, MySQL, MongoDB, Redis, or any database.
///
/// The key requirement: it must support atomic transactions so we can
/// record the event ID and apply changes together.
///
/// Example implementations:
/// - PostgreSQL with sqlx: `sqlx::PgPool` (used here)
/// - MongoDB: `mongodb::Client`
/// - Redis: `redis::Client` (for simple cases)
struct Database {
    pool: sqlx::PgPool, // Example: PostgreSQL connection pool
}

impl Database {
    /// Check if we've already processed this event.
    /// This prevents duplicate processing in at-least-once delivery.
    async fn event_exists(&self, event_id: Uuid) -> Result<bool, sqlx::Error> {
        let row = sqlx::query("SELECT 1 FROM processed_events WHERE event_id = $1")
            .bind(event_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.is_some())
    }
}

/// Idempotent event processing: safe to call multiple times with same event.
async fn process_order_idempotently(
    event: &OrderEvent,
    db: &Database,
) -> Result<(), sqlx::Error> {
    // Step 1: Check if already processed using event ID
    // This is the idempotency check - critical for at-least-once delivery
    if db.event_exists(event.id).await? {
        log::info!("Event {} already processed, skipping", event.id);
        return Ok(()); // Safe to return - we already handled this
    }

    // Step 2: Process and record the event ID atomically
    // The transaction ensures: either BOTH happen or NEITHER happens
    // This prevents the bug where we process but don't record (causing reprocess)
    let mut tx = db.pool.begin().await?;

    // Apply the business logic (update order status, send email, etc.)
    apply_order_event(&mut tx, event).await?;

    // Record that we processed this event ID. If another consumer processed
    // the same event since step 1, the primary key fails this insert and the
    // whole transaction rolls back.
    sqlx::query("INSERT INTO processed_events (event_id, event_type) VALUES ($1, $2)")
        .bind(event.id)
        .bind(&event.event_type)
        .execute(&mut *tx)
        .await?;

    tx.commit().await
}

/// Apply the actual business logic for an order event.
/// This is where you update your domain models.
async fn apply_order_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: &OrderEvent,
) -> Result<(), sqlx::Error> {
    match &event.data {
        OrderData::Created { items, total } => {
            // Insert new order into database
            sqlx::query(
                "INSERT INTO orders (id, items, total, status) VALUES ($1, $2, $3, 'pending')",
            )
            .bind(event.order_id)
            .bind(sqlx::types::Json(items))
            .bind(total)
            .execute(&mut **tx)
            .await?;
        }
        OrderData::Paid { amount } => {
            // Update order status to paid
            sqlx::query("UPDATE orders SET status = 'paid', paid_amount = $1 WHERE id = $2")
                .bind(amount)
                .bind(event.order_id)
                .execute(&mut **tx)
                .await?;
        }
        // ... handle other event types
    }
    Ok(())
}
```

**Database Schema for Idempotency:**

```sql
-- Table to track processed events (for idempotency)
CREATE TABLE processed_events (
    event_id UUID PRIMARY KEY,
    processed_at TIMESTAMP DEFAULT NOW(),
    -- Optional: store event type for debugging
    event_type VARCHAR(100)
);

-- The primary key already indexes event_id for the lookup

-- Optional: clean up old records (events older than retention period)
-- Run periodically via cron or scheduled job
DELETE FROM processed_events WHERE processed_at < NOW() - INTERVAL '30 days';
```

---

## Production Patterns

### Pattern: Outbox for Reliable Publishing

**The Problem:** How do you ensure a database change and an event publication happen together? If you do them separately, you risk inconsistency.

```rust
// THE PROBLEM: Two separate operations that can partially fail

async fn create_order_naive(
    db: &Database,
    producer: &IggyProducer,
    order: &Order,
) -> anyhow::Result<()> {
    // Step 1: Save to database
    db.save_order(order).await?;

    // Step 2: Publish event
    let message = order_created_message(order)?;
    producer.send(vec![message]).await?; // What if this fails?

    // If Step 1 succeeds but Step 2 fails:
    // - Order exists in database
    // - But no event was published
    // - Other services never learn about the order
    // - System is now INCONSISTENT

    Ok(())
}
```

**The Solution: Outbox Pattern**

Instead of publishing directly, write the event to an "outbox" table in the SAME database transaction as your business data. A separate process reads the outbox and publishes to the message broker.

```rust
/// The Outbox Pattern: Atomic database + event publishing
///
/// How it works:
/// 1. Business operation + event insert in ONE transaction
/// 2. Separate "publisher" process reads outbox table
/// 3. Publisher sends events to message broker
/// 4. Publisher marks events as published
///
/// Benefits:
/// - Atomicity: If transaction fails, neither data nor event is saved
/// - Reliability: Events are persisted; can retry publishing
/// - Ordering: Events published in order they were created

// Step 1: Write business data AND event in same transaction
async fn create_order_with_outbox(db: &Database, order: &Order) -> Result<(), sqlx::Error> {
    let mut tx = db.pool.begin().await?;

    // Save the order (your business data)
    save_order(&mut tx, order).await?;

    // Insert event into outbox table (same transaction!)
    let event = OrderCreatedEvent {
        order_id: order.id,
        user_id: order.user_id,
        total: order.total,
        created_at: Utc::now(),
    };
    insert_outbox_event(&mut tx, "order.created", &event).await?;

    // Both succeed or both fail - atomic!
    tx.commit().await
}

// Step 2: Separate background process that publishes events
async fn outbox_publisher(db: &Database, producer: &IggyProducer) -> anyhow::Result<()> {
    loop {
        // Fetch unpublished events from outbox table, oldest first
        let events = db.fetch_unpublished_events(100).await?;

        for event in events {
            // The outbox ID doubles as the message ID, so a consumer can
            // recognise a message it has already processed
            let message = IggyMessage::builder()
                .id(event.id.as_u128())
                .payload(event.payload.into())
                .build()?;
            // Keep each order's events in one partition, in order
            let key = Partitioning::messages_key(event.aggregate_id.as_bytes())?;

            // Publish to Iggy
            match producer.send_with_partitioning(vec![message], Some(Arc::new(key))).await {
                Ok(_) => {
                    // Mark as published so we don't send again
                    db.mark_event_published(event.id).await?;
                }
                Err(e) => {
                    // Stop here, so the next iteration retries this event
                    // before any later one
                    log::error!("Failed to publish event {}: {}", event.id, e);
                    break;
                }
            }
        }

        // Small delay to avoid busy-looping
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
```

A crash between the send and `mark_event_published` sends that event again
after a restart, so the outbox publishes at least once: consumers drop
duplicates by message ID or event ID ([Idempotency](#idempotency)).

**Outbox Database Schema:**

```sql
-- Outbox table: stores events to be published
CREATE TABLE outbox (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type VARCHAR(100) NOT NULL,      -- e.g., "order.created"
    payload JSONB NOT NULL,                 -- The event data
    created_at TIMESTAMP DEFAULT NOW(),
    published_at TIMESTAMP NULL,            -- NULL = not yet published
    
    -- Optional: for ordering and debugging
    aggregate_type VARCHAR(100),            -- e.g., "Order"
    aggregate_id UUID                       -- e.g., the order ID
);

-- Index for the publisher query (find unpublished events)
CREATE INDEX idx_outbox_unpublished ON outbox(created_at) 
    WHERE published_at IS NULL;

-- Publisher query
SELECT * FROM outbox 
WHERE published_at IS NULL 
ORDER BY created_at 
LIMIT 100;
```

**Further Reading:**
- [Microservices Patterns: Transactional Outbox](https://microservices.io/patterns/data/transactional-outbox.html)
- [Debezium CDC](https://debezium.io/) - Alternative approach using Change Data Capture

### Pattern: Saga for Distributed Transactions

Coordinate multi-service operations with compensating actions.

```
Order Saga:
1. CreateOrder → OrderCreated
2. ReserveInventory → InventoryReserved (or InventoryFailed → CompensateOrder)
3. ProcessPayment → PaymentProcessed (or PaymentFailed → ReleaseInventory, CompensateOrder)
4. ShipOrder → OrderShipped

Each step publishes events; failures trigger compensating events.
```

### Pattern: Event Versioning

Handle schema evolution gracefully.

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "version")]
enum OrderCreatedEvent {
    #[serde(rename = "1")]
    V1 {
        order_id: Uuid,
        user_id: Uuid,
        total: f64, // Old: floating point
    },
    #[serde(rename = "2")]
    V2 {
        order_id: Uuid,
        user_id: Uuid,
        total: Decimal,   // New: exact decimal
        currency: String, // New field
    },
}

fn process_order_created(event: OrderCreatedEvent) -> anyhow::Result<()> {
    match event {
        OrderCreatedEvent::V1 { order_id, user_id, total } => {
            // Handle legacy format
            let total = Decimal::from_f64_retain(total)
                .ok_or_else(|| anyhow::anyhow!("order {order_id}: total {total} is not finite"))?;
            process_order(order_id, user_id, total, "USD");
        }
        OrderCreatedEvent::V2 { order_id, user_id, total, currency } => {
            process_order(order_id, user_id, total, &currency);
        }
    }
    Ok(())
}
```

---

## Code Examples

### Complete Producer Example

```rust
use std::str::FromStr;
use std::sync::Arc;

use iggy::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
struct OrderEvent {
    id: Uuid,
    order_id: Uuid,
    event_type: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    data: serde_json::Value,
}

async fn run_producer() -> anyhow::Result<()> {
    // Connect to Iggy. A binary client signs in with the credentials in the
    // connection string when it connects.
    let client = IggyClient::from_connection_string("iggy://iggy:iggy@127.0.0.1:8090")?;
    client.connect().await?;

    // Create producer. init() creates the stream and the topic if they do
    // not exist.
    let producer = client
        .producer("ecommerce", "orders")?
        .create_topic_if_not_exists(
            3, // partitions
            IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?),
            MaxTopicSize::ServerDefault,
        )
        // Direct mode, the default: send() returns once the server has
        // answered. `.background(BackgroundConfig)` batches in a background
        // task instead, and then Ok from send() means only that it queued.
        .build();

    producer.init().await?;

    // Send events
    let order_id = Uuid::new_v4();
    let event = OrderEvent {
        id: Uuid::new_v4(),
        order_id,
        event_type: "order.created".to_string(),
        timestamp: chrono::Utc::now(),
        data: serde_json::json!({
            "items": [{"product_id": "123", "quantity": 2}],
            "total": "59.98"
        }),
    };

    let payload = serde_json::to_vec(&event)?;
    let message = IggyMessage::builder().payload(payload.into()).build()?;

    // Use order_id as partition key for ordering
    let partitioning = Partitioning::messages_key_str(&order_id.to_string())?;
    producer
        .send_with_partitioning(vec![message], Some(Arc::new(partitioning)))
        .await?;

    println!("Sent event: {:?}", event.id);

    producer.shutdown().await;
    client.shutdown().await?;
    Ok(())
}
```

### Complete Consumer Example

```rust
use futures::StreamExt;
use iggy::prelude::*;

async fn run_consumer() -> anyhow::Result<()> {
    let client = IggyClient::from_connection_string("iggy://iggy:iggy@127.0.0.1:8090")?;
    client.connect().await?;

    // Create consumer with consumer group (group, stream, topic). init()
    // creates the group if it does not exist and joins it.
    let mut consumer = client
        .consumer_group("order-processors", "ecommerce", "orders")?
        .poll_options(PollOptions {
            count: 100,
            // Manual commit, the default: the loop commits each processed message
            commit: CommitPolicy::Manual,
            ..PollOptions::wait(IggyDuration::ONE_SECOND)
        })
        .polling_strategy(PollingStrategy::next())
        .build();

    consumer.init().await?;

    println!("Consumer started, waiting for messages...");

    while let Some(received) = consumer.next().await {
        let received = received?;
        let event: OrderEvent = serde_json::from_slice(&received.message.payload)?;

        println!(
            "Received: {} - {} (partition {})",
            event.event_type, event.order_id, received.partition_id
        );

        // Process the event, and commit it only once processed
        match process_order_event(&event).await {
            Ok(()) => consumer.commit(received.checkpoint()).await?,
            Err(e) => {
                // Left uncommitted, it is delivered again after a restart or a rebalance
                eprintln!("Processing error: {}", e);
                break;
            }
        }
    }

    // Leave the group before the client goes
    consumer.shutdown().await?;
    client.shutdown().await?;
    Ok(())
}

async fn process_order_event(event: &OrderEvent) -> anyhow::Result<()> {
    match event.event_type.as_str() {
        "order.created" => handle_order_created(event).await,
        "order.paid" => handle_order_paid(event).await,
        "order.shipped" => handle_order_shipped(event).await,
        _ => {
            println!("Unknown event type: {}", event.event_type);
            Ok(())
        }
    }
}
```

---

## Quick Reference

### CLI Commands

```bash
# View messages in a topic (partition 0; partitions are 0-indexed)
iggy -u iggy -p iggy message poll --offset 0 --message-count 100 stream topic 0

# Send a message
iggy -u iggy -p iggy message send --partition-id 0 stream topic "hello world"

# List streams
iggy -u iggy -p iggy stream list

# List topics in a stream
iggy -u iggy -p iggy topic list stream

# Get topic details
iggy -u iggy -p iggy topic get stream topic
```

### HTTP API

```bash
# Health check
curl http://localhost:8000/health

# Create stream
curl -X POST http://localhost:8000/streams \
  -H "Content-Type: application/json" \
  -d '{"name": "my-stream"}'

# Create topic (per-topic options are optional; see the README)
curl -X POST http://localhost:8000/streams/my-stream/topics \
  -H "Content-Type: application/json" \
  -d '{"name": "events", "partitions": 3, "options": {"durability": "persisted"}}'

# Send message: an event, and an optional partition key (without one,
# sends are balanced across partitions)
curl -X POST http://localhost:8000/streams/my-stream/topics/events/messages \
  -H "Content-Type: application/json" \
  -d '{
    "event": {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "event_type": "order.created",
      "timestamp": "2024-01-15T10:30:00Z",
      "payload": {"type": "Generic", "data": {"order_id": "123"}}
    },
    "partition_key": "order-123"
  }'

# Poll messages (partitions are 0-indexed; "order-123" lands in partition 0)
curl "http://localhost:8000/streams/my-stream/topics/events/messages?consumer_id=1&partition_id=0&count=10"
```

### Environment Variables (iggy_sample)

```bash
# Connection: one address, or comma-separated seeds tried in order, e.g.
# iggy://user:pass@node1:8090,node2:8090,node3:8090. The gateway caps the
# SDK's reconnection retries at 3 unless the string sets reconnection_retries.
IGGY_CONNECTION_STRING=iggy://user:pass@host:8090

# Defaults
IGGY_STREAM=sample-stream
IGGY_TOPIC=events
IGGY_PARTITIONS=3
# Default topic options, applied when the gateway creates it (any option of
# POST /streams/{s}/topics, as IGGY_TOPIC_<OPTION>)
IGGY_TOPIC_DURABILITY=persisted

# Rate limiting
RATE_LIMIT_RPS=100
RATE_LIMIT_BURST=50

# Message limits
BATCH_MAX_SIZE=1000
POLL_MAX_COUNT=100
```

### Observability URLs

| Service | URL | Credentials |
|---------|-----|-------------|
| Iggy HTTP API | http://localhost:3000 | iggy/iggy |
| Iggy Web UI | http://localhost:3000/ui | iggy/iggy |
| Prometheus | http://localhost:9090 | - |
| Grafana | http://localhost:3001 | admin/admin |
| Sample App | http://localhost:8000 | - |
| Sample App metrics | http://localhost:9091/metrics | - |

---

## Further Reading

- [Apache Iggy Documentation](https://iggy.apache.org/docs/)
- [Event-Driven Architecture Patterns](https://martinfowler.com/articles/201701-event-driven.html)
- [Designing Data-Intensive Applications](https://dataintensive.net/) - Martin Kleppmann
- [Enterprise Integration Patterns](https://www.enterpriseintegrationpatterns.com/)
