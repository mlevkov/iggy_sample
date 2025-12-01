//! Connection state tracking for resilience management.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tokio::sync::Notify;

/// Connection state tracking for resilience management.
///
/// # Memory Ordering
///
/// All atomic operations use `SeqCst` (sequentially consistent) ordering
/// for simplicity and correctness. While `Relaxed` ordering could be used
/// for some counters, the performance difference is negligible for this
/// use case, and `SeqCst` prevents subtle synchronization bugs.
///
/// # Reconnection Coordination
///
/// Uses `tokio::sync::Notify` to efficiently wake waiting tasks when
/// reconnection completes, avoiding busy-wait polling.
pub struct ConnectionState {
    /// Whether the client is currently connected
    connected: AtomicBool,
    /// Number of consecutive failed reconnection attempts
    reconnect_attempts: AtomicU32,
    /// Whether a reconnection is currently in progress
    reconnecting: AtomicBool,
    /// Notification for when reconnection completes (success or failure)
    reconnect_complete: Notify,
}

impl ConnectionState {
    pub fn new() -> Self {
        Self {
            connected: AtomicBool::new(false),
            reconnect_attempts: AtomicU32::new(0),
            reconnecting: AtomicBool::new(false),
            reconnect_complete: Notify::new(),
        }
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::SeqCst);
        if connected {
            self.reconnect_attempts.store(0, Ordering::SeqCst);
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn increment_attempts(&self) -> u32 {
        self.reconnect_attempts.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn start_reconnecting(&self) -> bool {
        // Returns true if we successfully started reconnecting (wasn't already in progress)
        !self.reconnecting.swap(true, Ordering::SeqCst)
    }

    pub fn stop_reconnecting(&self) {
        self.reconnecting.store(false, Ordering::SeqCst);
        // Notify all waiting tasks that reconnection has completed
        self.reconnect_complete.notify_waiters();
    }

    pub fn is_reconnecting(&self) -> bool {
        self.reconnecting.load(Ordering::SeqCst)
    }

    /// Wait for an ongoing reconnection to complete.
    ///
    /// Returns immediately if no reconnection is in progress.
    ///
    /// # Implementation Note
    ///
    /// We register for notification BEFORE checking `is_reconnecting()` to avoid
    /// a race condition: if we checked first and then registered, reconnection
    /// could complete between those two operations, causing us to wait forever.
    pub async fn wait_for_reconnection(&self) {
        // Register for notification FIRST to avoid race condition
        let notified = self.reconnect_complete.notified();
        if self.is_reconnecting() {
            notified.await;
        }
    }

    /// Get the current reconnect attempts count (for testing).
    #[cfg(test)]
    pub fn attempts(&self) -> u32 {
        self.reconnect_attempts.load(Ordering::SeqCst)
    }
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_connection_state_initial() {
        let state = ConnectionState::new();

        assert!(!state.is_connected());
        assert!(!state.is_reconnecting());
        assert_eq!(state.attempts(), 0);
    }

    #[test]
    fn test_connection_state_set_connected() {
        let state = ConnectionState::new();

        state.set_connected(true);
        assert!(state.is_connected());

        state.set_connected(false);
        assert!(!state.is_connected());
    }

    #[test]
    fn test_connection_state_reconnect_attempts_reset_on_connect() {
        let state = ConnectionState::new();

        // Simulate some failed attempts
        state.increment_attempts();
        state.increment_attempts();
        assert_eq!(state.attempts(), 2);

        // Successful connection should reset attempts
        state.set_connected(true);
        assert_eq!(state.attempts(), 0);
    }

    #[test]
    fn test_connection_state_increment_attempts() {
        let state = ConnectionState::new();

        assert_eq!(state.increment_attempts(), 1);
        assert_eq!(state.increment_attempts(), 2);
        assert_eq!(state.increment_attempts(), 3);
        assert_eq!(state.attempts(), 3);
    }

    #[test]
    fn test_connection_state_start_reconnecting() {
        let state = ConnectionState::new();

        // First call should succeed
        assert!(state.start_reconnecting());
        assert!(state.is_reconnecting());

        // Second call should fail (already reconnecting)
        assert!(!state.start_reconnecting());
    }

    #[test]
    fn test_connection_state_stop_reconnecting() {
        let state = ConnectionState::new();

        state.start_reconnecting();
        assert!(state.is_reconnecting());

        state.stop_reconnecting();
        assert!(!state.is_reconnecting());
    }

    #[tokio::test]
    async fn test_connection_state_wait_for_reconnection_not_reconnecting() {
        let state = ConnectionState::new();

        // Should return immediately if not reconnecting
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            state.wait_for_reconnection(),
        )
        .await
        .expect("wait_for_reconnection should return immediately when not reconnecting");
    }

    #[tokio::test]
    async fn test_connection_state_wait_for_reconnection_with_notify() {
        let state = Arc::new(ConnectionState::new());
        state.start_reconnecting();

        let state_clone = state.clone();
        let waiter = tokio::spawn(async move {
            state_clone.wait_for_reconnection().await;
        });

        // Give waiter time to start waiting
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Stop reconnecting - should wake the waiter
        state.stop_reconnecting();

        // Waiter should complete quickly
        tokio::time::timeout(std::time::Duration::from_millis(100), waiter)
            .await
            .expect("waiter timed out")
            .expect("waiter panicked");
    }

    /// Test concurrent access to connection state from multiple tasks.
    ///
    /// This validates that the atomic operations and Notify mechanism
    /// work correctly under concurrent load.
    #[tokio::test]
    async fn test_connection_state_concurrent_access() {
        let state = Arc::new(ConnectionState::new());
        let num_waiters = 10;
        let mut handles = Vec::with_capacity(num_waiters);

        // Start reconnecting
        state.start_reconnecting();

        // Spawn multiple waiters
        for _ in 0..num_waiters {
            let state_clone = state.clone();
            handles.push(tokio::spawn(async move {
                state_clone.wait_for_reconnection().await;
                // After wake-up, should not be reconnecting
                !state_clone.is_reconnecting()
            }));
        }

        // Give waiters time to start
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Simulate successful reconnection
        state.set_connected(true);
        state.stop_reconnecting();

        // All waiters should be notified and complete
        for handle in handles {
            let result = tokio::time::timeout(std::time::Duration::from_millis(500), handle)
                .await
                .expect("waiter timed out")
                .expect("waiter panicked");
            assert!(result, "Waiter should see reconnecting=false after wakeup");
        }
    }

    /// Test rapid state transitions to ensure no race conditions.
    #[tokio::test]
    async fn test_connection_state_rapid_transitions() {
        let state = Arc::new(ConnectionState::new());
        let iterations = 100;

        for _ in 0..iterations {
            // Rapid transitions
            state.start_reconnecting();
            assert!(state.is_reconnecting());

            state.stop_reconnecting();
            assert!(!state.is_reconnecting());

            // set_connected(true) also resets attempts to 0
            state.set_connected(true);
            assert!(state.is_connected());

            // Increment and verify attempts
            state.increment_attempts();
            assert_eq!(state.attempts(), 1);

            // set_connected(true) resets attempts
            state.set_connected(true);
            assert_eq!(state.attempts(), 0);
        }
    }
}
