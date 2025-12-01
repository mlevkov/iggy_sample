use crate::error::{AppError, AppResult};

// =============================================================================
// Validation Constants
// =============================================================================

/// Maximum length for resource names (streams, topics).
///
/// This matches Iggy's internal limit for identifier length.
pub const MAX_NAME_LENGTH: usize = 255;

/// Maximum length for event type strings.
///
/// Chosen to be reasonably generous while preventing abuse.
/// Format recommendation: `domain.action` (e.g., `user.created`)
pub const MAX_EVENT_TYPE_LENGTH: usize = 256;

/// Minimum length for event type strings.
pub const MIN_EVENT_TYPE_LENGTH: usize = 1;

/// Maximum number of partitions per topic.
///
/// This is a reasonable limit to prevent misconfiguration.
/// Higher values are possible but likely indicate an error.
pub const MAX_PARTITIONS: u32 = 1000;

/// Minimum number of partitions per topic.
pub const MIN_PARTITIONS: u32 = 1;

/// Validate a resource name (stream or topic).
///
/// Rules:
/// - Must be between 1 and 255 characters
/// - Must start with an alphanumeric character
/// - Can contain alphanumeric characters, dots, underscores, and hyphens
/// - Cannot contain consecutive dots, underscores, or hyphens
/// - Cannot end with a dot, underscore, or hyphen
pub fn validate_resource_name(name: &str, resource_type: &str) -> AppResult<()> {
    // Check length
    if name.is_empty() {
        return Err(AppError::BadRequest(format!(
            "{resource_type} name cannot be empty"
        )));
    }

    if name.len() > MAX_NAME_LENGTH {
        return Err(AppError::BadRequest(format!(
            "{resource_type} name cannot exceed {MAX_NAME_LENGTH} characters"
        )));
    }

    // Check for valid characters and patterns
    let chars: Vec<char> = name.chars().collect();

    // Must start with alphanumeric (using .first() for defensive coding)
    if !chars.first().is_some_and(|c| c.is_ascii_alphanumeric()) {
        return Err(AppError::BadRequest(format!(
            "{resource_type} name must start with an alphanumeric character"
        )));
    }

    // Must end with alphanumeric
    if !chars.last().is_some_and(|c| c.is_ascii_alphanumeric()) {
        return Err(AppError::BadRequest(format!(
            "{} name must end with an alphanumeric character",
            resource_type
        )));
    }

    // Check each character and patterns
    let mut prev_special = false;
    for (i, &c) in chars.iter().enumerate() {
        let is_special = c == '.' || c == '_' || c == '-';

        if !c.is_ascii_alphanumeric() && !is_special {
            return Err(AppError::BadRequest(format!(
                "{resource_type} name contains invalid character '{c}' at position {i}. \
                 Only alphanumeric characters, dots, underscores, and hyphens are allowed"
            )));
        }

        // Check for consecutive special characters
        if is_special && prev_special {
            return Err(AppError::BadRequest(format!(
                "{resource_type} name cannot contain consecutive special characters at position {i}"
            )));
        }

        prev_special = is_special;
    }

    Ok(())
}

/// Validate partition count for a topic.
pub fn validate_partition_count(partitions: u32, resource_type: &str) -> AppResult<()> {
    if partitions < MIN_PARTITIONS {
        return Err(AppError::BadRequest(format!(
            "{resource_type} must have at least {MIN_PARTITIONS} partition"
        )));
    }

    if partitions > MAX_PARTITIONS {
        return Err(AppError::BadRequest(format!(
            "{resource_type} cannot have more than {MAX_PARTITIONS} partitions"
        )));
    }

    Ok(())
}

/// Validate an event type string.
///
/// Rules:
/// - Must be between 1 and 256 characters
/// - Must only contain printable ASCII characters (no control characters)
/// - Recommended format: `domain.action` (e.g., `user.created`, `order.shipped`)
pub fn validate_event_type(event_type: &str) -> AppResult<()> {
    // Check length
    if event_type.len() < MIN_EVENT_TYPE_LENGTH {
        return Err(AppError::BadRequest(
            "Event type cannot be empty".to_string(),
        ));
    }

    if event_type.len() > MAX_EVENT_TYPE_LENGTH {
        return Err(AppError::BadRequest(format!(
            "Event type cannot exceed {} characters (got {})",
            MAX_EVENT_TYPE_LENGTH,
            event_type.len()
        )));
    }

    // Check for control characters (non-printable ASCII)
    if let Some(pos) = event_type.chars().position(|c| c.is_control()) {
        return Err(AppError::BadRequest(format!(
            "Event type contains invalid control character at position {pos}"
        )));
    }

    Ok(())
}

/// Validate a partition ID for polling.
///
/// Iggy partitions are 0-indexed internally, so partition_id=0 is valid
/// and refers to the first partition. This validation is a no-op but exists
/// for consistency with other validation functions and potential future
/// constraints (e.g., maximum partition ID).
pub fn validate_partition_id(_partition_id: u32) -> AppResult<()> {
    // Iggy uses 0-indexed partitions, so all u32 values are technically valid.
    // The Iggy server will return an error if the partition doesn't exist.
    Ok(())
}

/// Validate a consumer ID for polling.
///
/// Consumer IDs must be positive (at least 1).
/// A consumer_id of 0 is invalid.
pub fn validate_consumer_id(consumer_id: u32) -> AppResult<()> {
    if consumer_id == 0 {
        return Err(AppError::BadRequest(
            "Consumer ID must be at least 1".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_names() {
        assert!(validate_resource_name("my-stream", "Stream").is_ok());
        assert!(validate_resource_name("my_stream", "Stream").is_ok());
        assert!(validate_resource_name("my.stream", "Stream").is_ok());
        assert!(validate_resource_name("stream123", "Stream").is_ok());
        assert!(validate_resource_name("a", "Stream").is_ok());
        assert!(validate_resource_name("test-stream_v2.0", "Stream").is_ok());
    }

    #[test]
    fn test_empty_name() {
        let result = validate_resource_name("", "Stream");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_name_too_long() {
        let long_name = "a".repeat(256);
        let result = validate_resource_name(&long_name, "Stream");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot exceed"));
    }

    #[test]
    fn test_invalid_start_character() {
        let result = validate_resource_name("-stream", "Stream");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must start with an alphanumeric")
        );
    }

    #[test]
    fn test_invalid_end_character() {
        let result = validate_resource_name("stream-", "Stream");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must end with an alphanumeric")
        );
    }

    #[test]
    fn test_invalid_characters() {
        let result = validate_resource_name("stream@name", "Stream");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid character")
        );
    }

    #[test]
    fn test_consecutive_special_characters() {
        let result = validate_resource_name("stream--name", "Stream");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("consecutive special")
        );
    }

    #[test]
    fn test_valid_partition_counts() {
        assert!(validate_partition_count(1, "Topic").is_ok());
        assert!(validate_partition_count(100, "Topic").is_ok());
        assert!(validate_partition_count(1000, "Topic").is_ok());
    }

    #[test]
    fn test_invalid_partition_count_zero() {
        let result = validate_partition_count(0, "Topic");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least 1"));
    }

    #[test]
    fn test_invalid_partition_count_too_high() {
        let result = validate_partition_count(1001, "Topic");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("more than 1000"));
    }

    #[test]
    fn test_valid_event_types() {
        assert!(validate_event_type("user.created").is_ok());
        assert!(validate_event_type("order.shipped").is_ok());
        assert!(validate_event_type("a").is_ok());
        assert!(validate_event_type("UPPERCASE_EVENT").is_ok());
        assert!(validate_event_type("event-with-dashes").is_ok());
        assert!(validate_event_type("event_with_underscores").is_ok());
    }

    #[test]
    fn test_empty_event_type() {
        let result = validate_event_type("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_event_type_too_long() {
        let long_type = "a".repeat(257);
        let result = validate_event_type(&long_type);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot exceed"));
    }

    #[test]
    fn test_event_type_control_characters() {
        let result = validate_event_type("event\nwith\nnewlines");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("control character")
        );
    }

    #[test]
    fn test_event_type_with_tab() {
        let result = validate_event_type("event\twith\ttabs");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("control character")
        );
    }

    #[test]
    fn test_valid_partition_ids() {
        // Iggy uses 0-indexed partitions, so all values are valid
        assert!(validate_partition_id(0).is_ok()); // First partition
        assert!(validate_partition_id(1).is_ok());
        assert!(validate_partition_id(10).is_ok());
        assert!(validate_partition_id(1000).is_ok());
        assert!(validate_partition_id(u32::MAX).is_ok());
    }

    #[test]
    fn test_valid_consumer_ids() {
        assert!(validate_consumer_id(1).is_ok());
        assert!(validate_consumer_id(100).is_ok());
        assert!(validate_consumer_id(u32::MAX).is_ok());
    }

    #[test]
    fn test_invalid_consumer_id_zero() {
        let result = validate_consumer_id(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least 1"));
    }
}
