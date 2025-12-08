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
- **Durability**: Persistent append-only log with configurable retention
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
// Creating a stream
client.create_stream("ecommerce", Some(1)).await?;
```

### Topics

A **topic** is a named channel for a specific category of events. Think of it as:
- A "table" in database terms
- A "queue" in messaging terms
- A "channel" in pub/sub terms

```rust
// Creating a topic with 3 partitions, 7-day retention
client.create_topic(
    "ecommerce",           // stream
    "orders",              // topic name
    3,                     // partitions
    None,                  // compression (None = default)
    None,                  // replication (future)
    Some(IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?)),
    None,                  // max topic size
).await?;
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
❌ WRONG: Partitions as categories
   Partition 0 = "pending orders"
   Partition 1 = "shipped orders"
   Partition 2 = "cancelled orders"

✅ CORRECT: Partitions as parallel lanes
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

Each message is processed by exactly one consumer in the group.

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
producer.send_with_key(partition_key, message).await?;
```

**How it works:**

```
partition_key = "order-123"
partition_index = hash("order-123") % num_partitions
                = 2847623847 % 3
                = 1

All "order-123" events → Partition 1 → Processed in order
```

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
use iggy::prelude::*;

// Publishing with partition key
async fn publish_order_event(
    producer: &IggyProducer,
    order_id: Uuid,
    event: OrderEvent,
) -> Result<(), Error> {
    let message = serde_json::to_vec(&event)?;
    
    // Use order_id as partition key - ensures ordering
    let partition_key = order_id.to_string();
    
    producer
        .send_with_key(partition_key, message)
        .await?;
    
    Ok(())
}

// All these events for order-123 will be processed in order:
publish_order_event(&producer, order_123, OrderEvent::Created { .. }).await?;
publish_order_event(&producer, order_123, OrderEvent::Paid { .. }).await?;
publish_order_event(&producer, order_123, OrderEvent::Shipped { .. }).await?;
```

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
| **Each message processed once** | Within a group, no duplicates |
| **Partition exclusivity** | One consumer owns a partition at a time |
| **Automatic rebalancing** | If consumer dies, partitions reassigned |
| **Offset tracking** | Server tracks progress per consumer |

### Creating a Consumer Group

```rust
use iggy::prelude::*;

let mut consumer = client
    .consumer_group("ecommerce", "orders", "order-processors")?
    .auto_commit(AutoCommit::IntervalOrWhen(
        IggyDuration::from_str("5s")?,
        AutoCommitWhen::ConsumingAllMessages,
    ))
    .create_consumer_group_if_not_exists()
    .auto_join_consumer_group()
    .polling_strategy(PollingStrategy::next())
    .batch_size(100)
    .build();

consumer.init().await?;

while let Some(message) = consumer.next().await {
    process_message(message)?;
    // Offset auto-committed every 5s or when batch complete
}
```

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

Iggy supports time-based message expiry:

```rust
// Topic with 7-day retention
IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?)

// Topic with 1-year retention (audit logs)
IggyExpiry::ExpireDuration(IggyDuration::from_str("365days")?)

// Never expire (permanent event store)
IggyExpiry::NeverExpire

// Use server default
IggyExpiry::ServerDefault
```

### Retention Strategy by Use Case

| Use Case | Retention | Reasoning |
|----------|-----------|-----------|
| Transient events | 1-7 days | Processed and done |
| Audit logs | 1-7 years | Compliance requirements |
| Event sourcing | Forever | Events are source of truth |
| Metrics/telemetry | 30-90 days | Trend analysis window |

### Example: Multi-Topic Retention

```rust
// Transient processing queue - 7 days
client.create_topic(
    "myapp", "task-queue", 3, None, None,
    Some(IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?)),
    None,
).await?;

// Audit log - 1 year
client.create_topic(
    "myapp", "audit-log", 1, None, None,
    Some(IggyExpiry::ExpireDuration(IggyDuration::from_str("365days")?)),
    None,
).await?;

// Event store - never expire
client.create_topic(
    "myapp", "event-store", 10, None, None,
    Some(IggyExpiry::NeverExpire),
    None,
).await?;
```

---

## Error Handling Strategies

### At-Most-Once Delivery

Process before committing offset. May lose messages on failure.

```rust
while let Some(message) = consumer.next().await {
    // Offset committed immediately (auto-commit)
    // If process_message fails, message is lost
    process_message(message)?;
}
```

**Use when**: Losing occasional messages is acceptable (metrics, logs).

### At-Least-Once Delivery

Commit offset only after successful processing. May reprocess on failure.

```rust
let mut consumer = client
    .consumer_group("stream", "topic", "group")?
    .auto_commit(AutoCommit::Disabled)  // Manual commit
    .build();

while let Some(message) = consumer.next().await {
    match process_message(&message) {
        Ok(_) => {
            consumer.commit_offset().await?;  // Commit after success
        }
        Err(e) => {
            log::error!("Processing failed: {}, will retry", e);
            // Don't commit - message will be redelivered
        }
    }
}
```

**Use when**: Every message must be processed (orders, payments).

### Dead Letter Queue (DLQ)

Move poison messages aside after retry exhaustion.

```rust
const MAX_RETRIES: u32 = 3;

async fn process_with_dlq(
    message: &Message,
    dlq_producer: &IggyProducer,
) -> Result<(), Error> {
    let retry_count = get_retry_count(message);
    
    match process_message(message).await {
        Ok(_) => Ok(()),
        Err(e) if retry_count < MAX_RETRIES => {
            // Requeue with incremented retry count
            requeue_with_retry(message, retry_count + 1).await
        }
        Err(e) => {
            // Max retries exceeded - send to DLQ
            log::error!("Moving to DLQ after {} retries: {}", retry_count, e);
            dlq_producer.send(message.to_dlq_format()).await?;
            Ok(())  // Commit original message
        }
    }
}
```

### Idempotency

Design consumers to handle duplicate messages safely.

**Why idempotency matters:** In at-least-once delivery, the same message may be processed multiple times (network retries, consumer restarts). Your processing logic must handle this gracefully.

**The Pattern:** Store processed event IDs in a database. Before processing, check if the event was already handled.

```rust
/// Database represents your application's persistent storage.
/// This could be PostgreSQL, MySQL, MongoDB, Redis, or any database.
/// 
/// The key requirement: it must support atomic transactions so we can
/// record the event ID and apply changes together.
/// 
/// Example implementations:
/// - PostgreSQL with sqlx: `sqlx::PgPool`
/// - MongoDB: `mongodb::Client`
/// - Redis: `redis::Client` (for simple cases)
struct Database {
    pool: sqlx::PgPool,  // Example: PostgreSQL connection pool
}

impl Database {
    /// Check if we've already processed this event.
    /// This prevents duplicate processing in at-least-once delivery.
    async fn event_exists(&self, event_id: Uuid) -> Result<bool, Error> {
        let result = sqlx::query!(
            "SELECT 1 FROM processed_events WHERE event_id = $1",
            event_id
        )
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(result.is_some())
    }
    
    /// Execute multiple operations atomically.
    /// Either all succeed or all fail - no partial updates.
    async fn transaction<F, T>(&self, f: F) -> Result<T, Error>
    where
        F: FnOnce(&mut Transaction) -> Future<Output = Result<T, Error>>,
    {
        let mut tx = self.pool.begin().await?;
        let result = f(&mut tx).await?;
        tx.commit().await?;
        Ok(result)
    }
}

/// Idempotent event processing: safe to call multiple times with same event.
async fn process_order_idempotently(
    event: &OrderEvent,
    db: &Database,
) -> Result<(), Error> {
    // Step 1: Check if already processed using event ID
    // This is the idempotency check - critical for at-least-once delivery
    if db.event_exists(event.id).await? {
        log::info!("Event {} already processed, skipping", event.id);
        return Ok(());  // Safe to return - we already handled this
    }
    
    // Step 2: Process and record the event ID atomically
    // The transaction ensures: either BOTH happen or NEITHER happens
    // This prevents the bug where we process but don't record (causing reprocess)
    db.transaction(|tx| async {
        // Apply the business logic (update order status, send email, etc.)
        apply_order_event(tx, event).await?;
        
        // Record that we processed this event ID
        tx.record_event_id(event.id).await?;
        
        Ok(())
    }).await
}

/// Apply the actual business logic for an order event.
/// This is where you update your domain models.
async fn apply_order_event(tx: &mut Transaction, event: &OrderEvent) -> Result<(), Error> {
    match &event.data {
        OrderData::Created { items, total } => {
            // Insert new order into database
            sqlx::query!(
                "INSERT INTO orders (id, items, total, status) VALUES ($1, $2, $3, 'pending')",
                event.order_id, items, total
            ).execute(tx).await?;
        }
        OrderData::Paid { amount } => {
            // Update order status to paid
            sqlx::query!(
                "UPDATE orders SET status = 'paid', paid_amount = $1 WHERE id = $2",
                amount, event.order_id
            ).execute(tx).await?;
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

-- Index for fast lookups
CREATE INDEX idx_processed_events_id ON processed_events(event_id);

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

async fn create_order_naive(order: &Order, producer: &IggyProducer) -> Result<(), Error> {
    // Step 1: Save to database
    db.save_order(&order).await?;
    
    // Step 2: Publish event
    producer.send(order_created_event).await?;  // What if this fails?
    
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
async fn create_order_with_outbox(
    db: &Database,
    order: &Order,
) -> Result<(), Error> {
    db.transaction(|tx| async {
        // Save the order (your business data)
        tx.save_order(&order).await?;
        
        // Insert event into outbox table (same transaction!)
        let event = OrderCreatedEvent {
            order_id: order.id,
            user_id: order.user_id,
            total: order.total,
            created_at: Utc::now(),
        };
        tx.insert_outbox_event("order.created", &event).await?;
        
        // Both succeed or both fail - atomic!
        Ok(())
    }).await
}

// Step 2: Separate background process that publishes events
async fn outbox_publisher(db: &Database, producer: &IggyProducer) {
    loop {
        // Fetch unpublished events from outbox table
        let events = db.fetch_unpublished_events(100).await?;
        
        for event in events {
            // Publish to Iggy
            match producer.send(event.payload.clone()).await {
                Ok(_) => {
                    // Mark as published so we don't send again
                    db.mark_event_published(event.id).await?;
                }
                Err(e) => {
                    // Log error, will retry on next iteration
                    log::error!("Failed to publish event {}: {}", event.id, e);
                }
            }
        }
        
        // Small delay to avoid busy-looping
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
```

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
        total: f64,  // Old: floating point
    },
    #[serde(rename = "2")]
    V2 {
        order_id: Uuid,
        user_id: Uuid,
        total: Decimal,  // New: exact decimal
        currency: String,  // New field
    },
}

fn process_order_created(event: OrderCreatedEvent) {
    match event {
        OrderCreatedEvent::V1 { order_id, user_id, total } => {
            // Handle legacy format
            let total = Decimal::from_f64_retain(total).unwrap();
            process_order(order_id, user_id, total, "USD")
        }
        OrderCreatedEvent::V2 { order_id, user_id, total, currency } => {
            process_order(order_id, user_id, total, &currency)
        }
    }
}
```

---

## Code Examples

### Complete Producer Example

```rust
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

async fn run_producer() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to Iggy
    let client = IggyClient::builder()
        .with_tcp()
        .with_server_address("127.0.0.1:8090")
        .build()?;
    
    client.connect().await?;
    client.login_user("iggy", "iggy").await?;
    
    // Ensure stream and topic exist
    let _ = client.create_stream("ecommerce", Some(1)).await;
    let _ = client.create_topic(
        "ecommerce",
        "orders",
        3,  // partitions
        None,
        None,
        Some(IggyExpiry::ExpireDuration(IggyDuration::from_str("7days")?)),
        None,
    ).await;
    
    // Create producer
    let producer = client
        .producer("ecommerce", "orders")?
        .batch_size(100)
        .send_interval(IggyDuration::from_str("100ms")?)
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
    
    // Use order_id as partition key for ordering
    producer
        .send_with_key(order_id.to_string(), payload)
        .await?;
    
    println!("Sent event: {:?}", event.id);
    
    Ok(())
}
```

### Complete Consumer Example

```rust
use iggy::prelude::*;

async fn run_consumer() -> Result<(), Box<dyn std::error::Error>> {
    let client = IggyClient::builder()
        .with_tcp()
        .with_server_address("127.0.0.1:8090")
        .build()?;
    
    client.connect().await?;
    client.login_user("iggy", "iggy").await?;
    
    // Create consumer with consumer group
    let mut consumer = client
        .consumer_group("ecommerce", "orders", "order-processors")?
        .auto_commit(AutoCommit::IntervalOrWhen(
            IggyDuration::from_str("5s")?,
            AutoCommitWhen::ConsumingAllMessages,
        ))
        .create_consumer_group_if_not_exists()
        .auto_join_consumer_group()
        .polling_strategy(PollingStrategy::next())
        .poll_interval(IggyDuration::from_str("100ms")?)
        .batch_size(100)
        .build();
    
    consumer.init().await?;
    
    println!("Consumer started, waiting for messages...");
    
    while let Some(message) = consumer.next().await {
        let event: OrderEvent = serde_json::from_slice(&message.payload)?;
        
        println!(
            "Received: {} - {} (partition {})",
            event.event_type,
            event.order_id,
            message.partition_id
        );
        
        // Process the event
        match process_order_event(&event).await {
            Ok(_) => println!("Processed successfully"),
            Err(e) => eprintln!("Processing error: {}", e),
        }
    }
    
    Ok(())
}

async fn process_order_event(event: &OrderEvent) -> Result<(), Box<dyn std::error::Error>> {
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
# View messages in a topic
iggy -u iggy -p iggy message poll --offset 0 --message-count 100 stream topic 1

# Send a message
iggy -u iggy -p iggy message send --partition-id 1 stream topic "hello world"

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
curl http://localhost:3000/

# Create stream
curl -X POST http://localhost:3000/streams \
  -H "Content-Type: application/json" \
  -d '{"stream_id": 1, "name": "my-stream"}'

# Create topic
curl -X POST http://localhost:3000/streams/my-stream/topics \
  -H "Content-Type: application/json" \
  -d '{"topic_id": 1, "name": "events", "partitions_count": 3}'

# Send message
curl -X POST http://localhost:3000/streams/my-stream/topics/events/messages \
  -H "Content-Type: application/json" \
  -d '{"partitioning": {"kind": "partition_id", "value": 1}, "messages": [{"payload": "aGVsbG8="}]}'

# Poll messages
curl "http://localhost:3000/streams/my-stream/topics/events/messages?consumer_id=1&partition_id=1&count=10"
```

### Environment Variables (iggy_sample)

```bash
# Connection
IGGY_CONNECTION_STRING=iggy://user:pass@host:8090

# Defaults
IGGY_STREAM=sample-stream
IGGY_TOPIC=events
IGGY_PARTITIONS=3

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
| Iggy Web UI | http://localhost:3050 | iggy/iggy |
| Prometheus | http://localhost:9090 | - |
| Grafana | http://localhost:3001 | admin/admin |
| Sample App | http://localhost:8000 | - |

---

## Further Reading

- [Apache Iggy Documentation](https://iggy.apache.org/docs/)
- [Event-Driven Architecture Patterns](https://martinfowler.com/articles/201701-event-driven.html)
- [Designing Data-Intensive Applications](https://dataintensive.net/) - Martin Kleppmann
- [Enterprise Integration Patterns](https://www.enterpriseintegrationpatterns.com/)

---

*Last updated: December 2025*
