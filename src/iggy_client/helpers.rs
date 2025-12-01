//! Helper functions for the Iggy client.

use iggy::prelude::Identifier;

use crate::error::AppError;

/// Convert a string to an Identifier, returning an appropriate error on failure.
///
/// Iggy identifiers must be alphanumeric with optional dots, underscores, or hyphens.
/// This function provides clear error messages when validation fails, including the
/// original error from the Iggy SDK for debugging.
pub fn to_identifier(name: &str, resource_type: &str) -> Result<Identifier, AppError> {
    name.try_into().map_err(|e: iggy::prelude::IggyError| {
        // Log the original error for debugging while providing a user-friendly message
        tracing::debug!(
            resource_type,
            name,
            original_error = %e,
            "Identifier conversion failed"
        );
        AppError::BadRequest(format!(
            "Invalid {} name '{}': must be 1-255 characters, alphanumeric with dots, underscores, or hyphens, \
             starting and ending with alphanumeric",
            resource_type, name
        ))
    })
}

/// Generate a random jitter value between 0.0 and 1.0.
///
/// Uses the `rand` crate's thread-local RNG for proper randomness.
/// This prevents predictable backoff patterns that could lead to
/// thundering herd problems when multiple clients reconnect simultaneously.
pub fn rand_jitter() -> f64 {
    use rand::Rng;
    rand::rng().random::<f64>()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_to_identifier_valid_names() {
        assert!(to_identifier("my-stream", "stream").is_ok());
        assert!(to_identifier("my_topic", "topic").is_ok());
        assert!(to_identifier("stream.v2", "stream").is_ok());
        assert!(to_identifier("a", "stream").is_ok());
        assert!(to_identifier("test123", "topic").is_ok());
    }

    #[test]
    fn test_to_identifier_empty_name() {
        let result = to_identifier("", "stream");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid stream name")
        );
    }

    #[test]
    fn test_to_identifier_too_long() {
        let long_name = "a".repeat(300);
        let result = to_identifier(&long_name, "topic");
        assert!(result.is_err());
    }

    #[test]
    fn test_rand_jitter_returns_value_in_range() {
        for _ in 0..100 {
            let jitter = rand_jitter();
            assert!(jitter >= 0.0, "jitter {} should be >= 0.0", jitter);
            assert!(jitter < 1.0, "jitter {} should be < 1.0", jitter);
        }
    }

    #[test]
    fn test_rand_jitter_produces_different_values() {
        let values: Vec<f64> = (0..10).map(|_| rand_jitter()).collect();

        // At least some values should be different (extremely unlikely all same)
        // Use pattern matching to safely access window elements
        let all_same = values.windows(2).all(|w| {
            match (w.first(), w.get(1)) {
                (Some(&a), Some(&b)) => (a - b).abs() < f64::EPSILON,
                _ => true, // Shouldn't happen with windows(2)
            }
        });
        assert!(!all_same, "rand_jitter should produce varying values");
    }
}
