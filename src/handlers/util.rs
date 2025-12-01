//! Shared utilities for handlers.

use chrono::{DateTime, Utc};
use tracing::warn;

/// Parse a timestamp from microseconds with proper logging for invalid values.
///
/// If the timestamp cannot be converted (e.g., overflow, invalid value),
/// a warning is logged with the entity context for debugging, and the
/// current time is returned as a fallback.
///
/// # Arguments
///
/// * `timestamp_micros` - Timestamp in microseconds since Unix epoch
/// * `entity_type` - Type of entity (e.g., "stream", "topic") for logging
/// * `entity_name` - Name of the entity for logging
///
/// # Returns
///
/// The parsed `DateTime<Utc>`, or current time if parsing fails.
pub fn parse_timestamp_with_context(
    timestamp_micros: i64,
    entity_type: &str,
    entity_name: &str,
) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(timestamp_micros).unwrap_or_else(|| {
        warn!(
            entity_type,
            entity_name, timestamp_micros, "Invalid timestamp, using current time as fallback"
        );
        Utc::now()
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_timestamp() {
        // Known timestamp: 2024-01-15T10:30:00Z = 1705315800 seconds
        let micros = 1705315800_i64 * 1_000_000;
        let result = parse_timestamp_with_context(micros, "test", "entity");

        assert_eq!(result.timestamp_micros(), micros);
    }

    #[test]
    fn test_parse_zero_timestamp() {
        let result = parse_timestamp_with_context(0, "test", "entity");
        // Zero is valid (Unix epoch)
        assert_eq!(result.timestamp(), 0);
    }

    #[test]
    fn test_parse_invalid_timestamp_uses_fallback() {
        // i64::MAX microseconds would overflow DateTime
        let result = parse_timestamp_with_context(i64::MAX, "test", "invalid_entity");

        // Should return a recent time (within last minute)
        let now = Utc::now();
        let diff = (now - result).num_seconds().abs();
        assert!(diff < 60, "Fallback should be close to current time");
    }
}
