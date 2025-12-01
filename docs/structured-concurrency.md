# Structured Concurrency in Rust

A practical guide to managing concurrent tasks with guaranteed cleanup, using real
examples from this codebase.

---

## Table of Contents

1. [What is Structured Concurrency?](#what-is-structured-concurrency)
2. [The Problem: Fire-and-Forget Tasks](#the-problem-fire-and-forget-tasks)
3. [Core Concepts](#core-concepts)
4. [Implementation with Tokio](#implementation-with-tokio)
5. [Patterns and Best Practices](#patterns-and-best-practices)
6. [Common Pitfalls](#common-pitfalls)
7. [Testing Concurrent Code](#testing-concurrent-code)
8. [Further Reading](#further-reading)

---

## What is Structured Concurrency?

Structured concurrency is a programming paradigm where **concurrent operations don't
outlive their parent scope**. Just as structured programming eliminated goto in favor
of blocks with clear entry and exit points, structured concurrency eliminates detached
tasks in favor of task hierarchies with clear lifetimes.

### The Key Principle

> A concurrent operation should not be able to escape the scope in which it was started.

This means:
- Parent tasks wait for child tasks to complete
- Errors in child tasks propagate to parents
- Cancellation flows down the task hierarchy
- Resources are always cleaned up

### Visual Comparison

```text
Unstructured (fire-and-forget):
┌─────────────────────────────────────────────────────────────┐
│ main()                                                      │
│   spawn(task_a)  ─────────────────────────?                 │
│   spawn(task_b)  ─────────────────────────────────────?     │
│   return  // tasks still running!                           │
└─────────────────────────────────────────────────────────────┘

Structured:
┌─────────────────────────────────────────────────────────────┐
│ main()                                                      │
│   ┌─ scope ──────────────────────────────┐                  │
│   │  spawn(task_a)  ─────────────────────│                  │
│   │  spawn(task_b)  ─────────────────────│                  │
│   └──────────────────────────────────────┘                  │
│   // Both tasks complete before continuing                  │
│   return                                                    │
└─────────────────────────────────────────────────────────────┘
```

---

## The Problem: Fire-and-Forget Tasks

### Typical Unstructured Code

```rust
// Problem: What happens to this task when the server shuts down?
tokio::spawn(async move {
    loop {
        refresh_cache().await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
});
```

This pattern causes several issues:

1. **Resource Leaks**: Task may hold connections, file handles, or memory
2. **Incomplete Work**: Task might be mid-operation during shutdown
3. **Orphaned State**: Task updates may be lost
4. **Testing Difficulty**: No way to wait for background work to complete
5. **Silent Failures**: Errors in detached tasks are often lost

### Real-World Symptoms

- Database connections exhausted during shutdown
- Log messages appearing after "shutdown complete"
- Integration tests that pass locally but fail in CI (timing-dependent)
- Memory usage growing over long runs due to leaked tasks

---

## Core Concepts

### 1. Task Tracking

Track all spawned tasks so you can wait for them:

```rust
use tokio_util::task::TaskTracker;

let tracker = TaskTracker::new();

// Spawned tasks are tracked automatically
tracker.spawn(async {
    do_work().await;
});

// Wait for all tasks to complete
tracker.close();  // Prevent new spawns
tracker.wait().await;  // Wait for completion
```

### 2. Cancellation Tokens

Signal tasks to stop gracefully:

```rust
use tokio_util::sync::CancellationToken;

let token = CancellationToken::new();
let child_token = token.clone();

tokio::spawn(async move {
    loop {
        tokio::select! {
            _ = child_token.cancelled() => {
                // Clean up and exit
                break;
            }
            _ = do_work() => {
                // Continue working
            }
        }
    }
});

// Later: signal cancellation
token.cancel();
```

### 3. The Select Pattern with biased

Ensure cancellation is checked first:

```rust
tokio::select! {
    biased;  // Check branches in order (cancellation first)

    _ = cancel_token.cancelled() => {
        // Always checked first
        break;
    }
    result = work_future => {
        handle_result(result);
    }
}
```

Without `biased`, tokio randomly selects which branch to check first, which can
delay cancellation response in high-load scenarios.

---

## Implementation with Tokio

### Complete Example from This Codebase

From `src/state.rs`:

```rust
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub struct AppState {
    // ... other fields ...
    task_tracker: TaskTracker,
    cancellation_token: CancellationToken,
}

impl AppState {
    pub fn new(iggy_client: IggyClientWrapper, config: Config) -> Self {
        let task_tracker = TaskTracker::new();
        let cancellation_token = CancellationToken::new();

        let state = Self {
            iggy_client,
            config,
            task_tracker,
            cancellation_token,
            // ...
        };

        // Spawn tracked background tasks
        state.spawn_stats_refresh_task();
        state.spawn_health_check_task();

        state
    }

    fn spawn_stats_refresh_task(&self) {
        let state = self.clone();
        let ttl = self.config.stats_cache_ttl;
        let cancel = self.cancellation_token.clone();

        self.task_tracker.spawn(async move {
            let mut ticker = interval(ttl);
            ticker.tick().await;  // Skip immediate first tick

            loop {
                tokio::select! {
                    biased;  // Check cancellation first

                    _ = cancel.cancelled() => {
                        debug!("Stats refresh task: cancelled");
                        break;
                    }
                    _ = ticker.tick() => {
                        state.refresh_stats().await;
                    }
                }
            }
            debug!("Stats refresh task: shutdown complete");
        });
    }

    pub async fn shutdown(&self) {
        info!("Initiating graceful shutdown");

        // 1. Signal all tasks to stop
        self.cancellation_token.cancel();

        // 2. Prevent new tasks from being spawned
        self.task_tracker.close();

        // 3. Wait for all tasks to finish
        self.task_tracker.wait().await;

        info!("All background tasks completed");
    }
}
```

### Wiring Up Shutdown in main.rs

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let state = AppState::new(iggy_client, config);
    let app = build_router(state.clone());  // Clone to keep reference

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // After HTTP server stops, clean up background tasks
    state.shutdown().await;

    info!("Shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm_signal() => {}
    }
}
```

---

## Patterns and Best Practices

### Pattern 1: Efficient Wake-Up with Notify

Instead of busy-wait polling, use `tokio::sync::Notify`:

```rust
use tokio::sync::Notify;

struct ConnectionState {
    reconnecting: AtomicBool,
    reconnect_complete: Notify,
}

impl ConnectionState {
    fn stop_reconnecting(&self) {
        self.reconnecting.store(false, Ordering::SeqCst);
        self.reconnect_complete.notify_waiters();
    }

    // IMPORTANT: Register for notification BEFORE checking condition
    // to avoid race conditions
    async fn wait_for_reconnection(&self) {
        let notified = self.reconnect_complete.notified();
        if self.is_reconnecting() {
            notified.await;
        }
    }
}
```

### Pattern 2: Scoped Task Groups

For operations that spawn multiple related tasks:

```rust
async fn process_batch(items: Vec<Item>, token: CancellationToken) {
    let tracker = TaskTracker::new();

    for item in items {
        let token = token.clone();
        tracker.spawn(async move {
            tokio::select! {
                biased;
                _ = token.cancelled() => {}
                _ = process_item(item) => {}
            }
        });
    }

    tracker.close();
    tracker.wait().await;  // All items processed or cancelled
}
```

### Pattern 3: Timeout with Cancellation

Combine timeouts with cancellation for robust operations:

```rust
async fn fetch_with_timeout(
    url: &str,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<Response, Error> {
    tokio::select! {
        biased;

        _ = cancel.cancelled() => {
            Err(Error::Cancelled)
        }
        result = tokio::time::timeout(timeout, fetch(url)) => {
            result.map_err(|_| Error::Timeout)?
        }
    }
}
```

### Pattern 4: Cleanup Guards

Ensure cleanup runs even on panic or early return:

```rust
// Using scopeguard pattern
fn start_reconnecting(&self) -> bool {
    self.reconnecting.swap(true, Ordering::SeqCst)
}

async fn reconnect(&self) -> Result<()> {
    if !self.state.start_reconnecting() {
        // Another task is already reconnecting
        self.state.wait_for_reconnection().await;
        return Ok(());
    }

    // Guard ensures stop_reconnecting() is always called
    let _guard = scopeguard::guard((), |_| {
        self.state.stop_reconnecting();
    });

    // Do reconnection work...
    // Guard runs on success, error, or panic
}
```

---

## Common Pitfalls

### Pitfall 1: Race Condition with Notify

```rust
// WRONG: Race condition
async fn wait_for_reconnection(&self) {
    if self.is_reconnecting() {
        // Gap here! Reconnection could complete between check and await
        self.reconnect_complete.notified().await;
    }
}

// CORRECT: Register before checking
async fn wait_for_reconnection(&self) {
    let notified = self.reconnect_complete.notified();
    if self.is_reconnecting() {
        notified.await;
    }
}
```

### Pitfall 2: Forgetting to Close TaskTracker

```rust
// WRONG: wait() returns immediately if not closed
tracker.wait().await;  // Returns immediately!

// CORRECT: Close first, then wait
tracker.close();
tracker.wait().await;
```

### Pitfall 3: Missing biased in select!

```rust
// WRONG: May delay cancellation under load
tokio::select! {
    _ = cancel.cancelled() => { break; }
    _ = heavy_work() => {}
}

// CORRECT: Always check cancellation first
tokio::select! {
    biased;
    _ = cancel.cancelled() => { break; }
    _ = heavy_work() => {}
}
```

### Pitfall 4: Cloning Entire State

```rust
// INEFFICIENT: Clones entire state including large fields
fn spawn_task(&self) {
    let state = self.clone();  // Clones everything
    self.tracker.spawn(async move {
        state.small_method().await;
    });
}

// BETTER: Clone only what's needed
fn spawn_task(&self) {
    let client = self.iggy_client.clone();
    let cache = self.cache.clone();
    self.tracker.spawn(async move {
        // Use only client and cache
    });
}
```

### Pitfall 5: Blocking in Async Context

```rust
// WRONG: Blocks the async runtime
tokio::select! {
    _ = cancel.cancelled() => {}
    _ = async { std::thread::sleep(Duration::from_secs(1)) } => {}
}

// CORRECT: Use async sleep
tokio::select! {
    _ = cancel.cancelled() => {}
    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
}
```

---

## Testing Concurrent Code

### Test Task Completion

```rust
#[tokio::test]
async fn test_shutdown_completes_all_tasks() {
    let state = AppState::new(client, config);

    // Let background tasks run briefly
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Shutdown should complete without hanging
    tokio::time::timeout(Duration::from_secs(5), state.shutdown())
        .await
        .expect("Shutdown timed out - tasks not responding to cancellation");
}
```

### Test Cancellation Response

```rust
#[tokio::test]
async fn test_task_responds_to_cancellation() {
    let token = CancellationToken::new();
    let tracker = TaskTracker::new();

    let task_token = token.clone();
    tracker.spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = task_token.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
        }
    });

    // Cancel immediately
    token.cancel();
    tracker.close();

    // Should complete quickly, not wait 60 seconds
    tokio::time::timeout(Duration::from_millis(100), tracker.wait())
        .await
        .expect("Task did not respond to cancellation");
}
```

### Test No Resource Leaks

```rust
#[tokio::test]
async fn test_no_leaked_tasks() {
    for _ in 0..100 {
        let state = AppState::new(client.clone(), config.clone());
        state.shutdown().await;
    }
    // If tasks leak, this would eventually exhaust resources
}
```

---

## Further Reading

### Documentation

- [tokio-util TaskTracker](https://docs.rs/tokio-util/latest/tokio_util/task/struct.TaskTracker.html)
- [tokio-util CancellationToken](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html)
- [Tokio select! macro](https://docs.rs/tokio/latest/tokio/macro.select.html)

### Articles and Talks

- [Notes on structured concurrency, or: Go statement considered harmful](https://vorpus.org/blog/notes-on-structured-concurrency-or-go-statement-considered-harmful/) - Nathaniel J. Smith
- [Structured Concurrency](https://250bpm.com/blog:71/) - Martin Sústrik
- [Trio: A friendly Python library for async concurrency](https://github.com/python-trio/trio) - Python's implementation

### Related Patterns

- **Supervision Trees** (Erlang/Elixir): Hierarchical process management with restart strategies
- **Scoped Tasks** (Kotlin): `coroutineScope` ensures all child coroutines complete
- **Task Groups** (Swift): `withTaskGroup` provides similar guarantees in Swift concurrency

---

## Summary

Structured concurrency transforms async programming from managing individual tasks to
managing task hierarchies. The key tools in Rust/Tokio are:

| Tool | Purpose |
|------|---------|
| `TaskTracker` | Track spawned tasks for graceful shutdown |
| `CancellationToken` | Signal tasks to stop |
| `biased` in `select!` | Ensure cancellation is checked first |
| `Notify` | Efficient wake-up without polling |

The pattern in this codebase:

1. **Create** tracker and token at startup
2. **Spawn** tasks through the tracker
3. **Use** `select! { biased; }` with cancellation in task loops
4. **On shutdown**: cancel token → close tracker → wait for tasks

This ensures no orphaned tasks, no resource leaks, and clean shutdowns.
