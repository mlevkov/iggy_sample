//! Scope guard for cleanup on drop.
//!
//! This is a minimal implementation to avoid adding the `scopeguard` crate
//! as a dependency for a single use case. The `Drop`-based guard is
//! load-bearing in `reconnect()`: it releases the reconnection-in-progress
//! flag on every exit path, including early returns and future cancellation.

/// A guard that executes a closure when dropped.
pub struct ScopeGuard<F: FnOnce()> {
    callback: Option<F>,
}

impl<F: FnOnce()> Drop for ScopeGuard<F> {
    fn drop(&mut self) {
        if let Some(callback) = self.callback.take() {
            callback();
        }
    }
}

/// Create a scope guard that will execute `callback` when dropped.
///
/// # Example
///
/// ```rust,ignore
/// let flag = Arc::new(AtomicBool::new(true));
/// let flag_clone = flag.clone();
/// let _guard = guard(move || {
///     flag_clone.store(false, Ordering::SeqCst);
/// });
/// // When _guard goes out of scope, the flag will be set to false
/// ```
pub fn guard<F: FnOnce()>(callback: F) -> ScopeGuard<F> {
    ScopeGuard {
        callback: Some(callback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_scopeguard_executes_on_drop() {
        let executed = Arc::new(AtomicBool::new(false));
        let executed_clone = executed.clone();

        {
            let _guard = guard(move || {
                executed_clone.store(true, Ordering::SeqCst);
            });
        }

        assert!(
            executed.load(Ordering::SeqCst),
            "callback should have executed"
        );
    }

    #[test]
    fn test_scopeguard_executes_on_early_return() {
        fn early_return(flag: Arc<AtomicBool>) -> u32 {
            let _guard = guard(move || {
                flag.store(true, Ordering::SeqCst);
            });
            42 // guard drops here
        }

        let executed = Arc::new(AtomicBool::new(false));
        assert_eq!(early_return(executed.clone()), 42);
        assert!(executed.load(Ordering::SeqCst));
    }
}
