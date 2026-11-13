# Documentation and Guides

This directory contains comprehensive guides for understanding and working with Apache Iggy and event-driven architecture patterns.

## Available Guides

| Guide | Description |
|-------|-------------|
| [Event-Driven Architecture Guide](guide.md) | Core concepts, streams/topics/partitions, consumer groups, error handling patterns, and production patterns (outbox, saga, idempotency) |
| [Partitioning Guide](partitioning-guide.md) | Deep dive into partition keys, ordering guarantees, and partition selection strategies |
| [Durable Storage Guide](durable-storage-guide.md) | Storage architecture, per-topic durability (`replicated`/`persisted`) and retention options on server 0.10, setting them through the gateway, backup, recovery, and production recommendations |
| [Three-Node Cluster Demo](cluster-demo.md) | Running a three-node Iggy cluster behind the gateway: leader failover under traffic, per-topic replication factors, seed addresses, and what `GET /cluster` does and does not tell you |
| [Connectors Demo](connectors-demo.md) | Iggy's connector runtime with the HTTP source and sink: signed webhooks into a `persisted` topic, events back to the gateway's `/sink/inbound`, what each direction guarantees, the secrets and what is exposed, and issues found in the pinned releases |
| [Structured Concurrency](structured-concurrency.md) | Task lifecycle management, cancellation tokens, and graceful shutdown patterns |
| [Architecture Decision Records](adr/README.md) | Significant design decisions and their reasoning, starting with the Iggy upgrade plan (0.9, then 0.10) |
| [Tech Debt Registry](tech-debt/README.md) | Deferred review findings, each with the trigger that forces its resolution |
| [Session State](SESSION-STATE.md) | Rolling handoff between working sessions, newest block first |

The gateway's own architecture is in [architecture.md](../architecture.md) at the
repository root; `code-reviews/` holds the records of past reviews.

## Quick Navigation

### Getting Started
- New to Iggy? Start with the [Event-Driven Architecture Guide](guide.md)
- Setting up production? See [Durable Storage Guide](durable-storage-guide.md)

### By Topic

**Architecture & Design**
- [Core Concepts](guide.md#core-concepts) - EDA fundamentals, Iggy overview
- [Streams, Topics, and Partitions](guide.md#streams-topics-and-partitions) - Data organization
- [Message Flow Patterns](guide.md#message-flow-patterns) - Pub/sub, work queues, event sourcing

**Data Durability**
- [Durability Configuration](durable-storage-guide.md#durability-configuration) - per-topic durability policies and trade-offs
- [Backup and Archiving](durable-storage-guide.md#backup-and-archiving) - consistent filesystem backups (no built-in archiver)
- [Recovery](durable-storage-guide.md#recovery-and-data-integrity) - Disaster recovery procedures

**Message Ordering**
- [Partition Keys](partitioning-guide.md) - Ensuring message ordering
- [Consumer Groups](guide.md#consumer-groups) - Parallel processing with ordering

**Production Patterns**
- [Error Handling](guide.md#error-handling-strategies) - At-least-once, idempotency, DLQ
- [Outbox Pattern](guide.md#pattern-outbox-for-reliable-publishing) - Reliable event publishing
- [Production Recommendations](durable-storage-guide.md#production-recommendations) - Config templates

**Clustering and Connectors**
- [Three-Node Cluster Demo](cluster-demo.md) - Failover, replication factors, seeds
- [Connectors Demo](connectors-demo.md) - HTTP source and sink, delivery guarantees

**Application Internals**
- [Architecture](../architecture.md) - Components, request flow, SDK integration
- [Structured Concurrency](structured-concurrency.md) - Background task management
- [Graceful Shutdown](structured-concurrency.md) - Clean shutdown procedures

## External Resources

### Official Iggy Documentation
- [Apache Iggy GitHub](https://github.com/apache/iggy)
- [Iggy Documentation](https://iggy.apache.org/docs/)
- [Iggy Architecture](https://iggy.apache.org/docs/introduction/architecture/)
- [Iggy Server Configuration](https://iggy.apache.org/docs/server/configuration)

### Related Reading
- [Event-Driven Architecture Patterns](https://martinfowler.com/articles/201701-event-driven.html) - Martin Fowler
- [Designing Data-Intensive Applications](https://dataintensive.net/) - Martin Kleppmann
- [Enterprise Integration Patterns](https://www.enterpriseintegrationpatterns.com/)
