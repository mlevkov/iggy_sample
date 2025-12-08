# Documentation and Guides

This directory contains comprehensive guides for understanding and working with Apache Iggy and event-driven architecture patterns.

## Available Guides

| Guide | Description |
|-------|-------------|
| [Event-Driven Architecture Guide](guide.md) | Core concepts, streams/topics/partitions, consumer groups, error handling patterns, and production patterns (outbox, saga, idempotency) |
| [Partitioning Guide](partitioning-guide.md) | Deep dive into partition keys, ordering guarantees, and partition selection strategies |
| [Durable Storage Guide](durable-storage-guide.md) | Storage architecture, fsync configuration, backup/archiving to S3, recovery procedures, and production recommendations |
| [Structured Concurrency](structured-concurrency.md) | Task lifecycle management, cancellation tokens, and graceful shutdown patterns |

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
- [Durability Configuration](durable-storage-guide.md#durability-configuration) - fsync settings and trade-offs
- [Backup and Archiving](durable-storage-guide.md#backup-and-archiving) - S3 and disk archival
- [Recovery](durable-storage-guide.md#recovery-and-data-integrity) - Disaster recovery procedures

**Message Ordering**
- [Partition Keys](partitioning-guide.md) - Ensuring message ordering
- [Consumer Groups](guide.md#consumer-groups) - Parallel processing with ordering

**Production Patterns**
- [Error Handling](guide.md#error-handling-strategies) - At-least-once, idempotency, DLQ
- [Outbox Pattern](guide.md#pattern-outbox-for-reliable-publishing) - Reliable event publishing
- [Production Recommendations](durable-storage-guide.md#production-recommendations) - Config templates

**Application Internals**
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

---

*Last updated: December 2025*
