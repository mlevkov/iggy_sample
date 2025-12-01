//! Scope guard for cleanup on drop.
//!
//! This is a minimal implementation to avoid adding the `scopeguard` crate
//! as a dependency for a single use case.

/// A guard that executes a closure when dropped.
pub struct ScopeGuard<T, F: FnOnce(T)> {
    value: Option<T>,
    dropper: Option<F>,
}

impl<T, F: FnOnce(T)> Drop for ScopeGuard<T, F> {
    fn drop(&mut self) {
        if let (Some(value), Some(dropper)) = (self.value.take(), self.dropper.take()) {
            dropper(value);
        }
    }
}

/// Create a scope guard that will execute `dropper` with `value` when dropped.
///
/// # Example
///
/// ```rust,ignore
/// let flag = Arc::new(AtomicBool::new(true));
/// let flag_clone = flag.clone();
/// let _guard = guard((), move |_| {
///     flag_clone.store(false, Ordering::SeqCst);
/// });
/// // When _guard goes out of scope, the flag will be set to false
/// ```
pub fn guard<T, F: FnOnce(T)>(value: T, dropper: F) -> ScopeGuard<T, F> {
    ScopeGuard {
        value: Some(value),
        dropper: Some(dropper),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn test_scopeguard_executes_on_drop() {
        let executed = Arc::new(AtomicBool::new(false));
        let executed_clone = executed.clone();

        {
            let _guard = guard((), move |_| {
                executed_clone.store(true, Ordering::SeqCst);
            });
        }

        assert!(
            executed.load(Ordering::SeqCst),
            "dropper should have executed"
        );
    }

    #[test]
    fn test_scopeguard_passes_value_to_dropper() {
        let received_value = Arc::new(AtomicUsize::new(0));
        let received_clone = received_value.clone();

        {
            let _guard = guard(42usize, move |v| {
                received_clone.store(v, Ordering::SeqCst);
            });
        }

        assert_eq!(received_value.load(Ordering::SeqCst), 42);
    }
}
