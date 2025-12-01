//! Parameter types for Iggy client operations.

/// Default number of messages to poll when not specified.
pub const DEFAULT_POLL_COUNT: u32 = 10;

/// Parameters for polling messages from a topic.
///
/// This struct groups the many parameters needed for polling, making the API
/// cleaner and easier to extend without breaking changes.
///
/// # Example
///
/// ```rust,ignore
/// let params = PollParams::new(1, 1) // partition_id, consumer_id
///     .with_offset(100)
///     .with_count(50)
///     .with_auto_commit(true);
///
/// client.poll_messages("stream", "topic", params).await?;
/// ```
#[derive(Debug, Clone)]
pub struct PollParams {
    /// Partition to poll from
    pub partition_id: u32,
    /// Consumer identifier for offset tracking
    pub consumer_id: u32,
    /// Starting offset (None = from last committed)
    pub offset: Option<u64>,
    /// Maximum messages to return
    pub count: u32,
    /// Whether to auto-commit offset after polling
    pub auto_commit: bool,
}

impl PollParams {
    /// Create new poll parameters with required fields.
    ///
    /// Defaults:
    /// - offset: None (use last committed)
    /// - count: DEFAULT_POLL_COUNT (10)
    /// - auto_commit: false
    pub fn new(partition_id: u32, consumer_id: u32) -> Self {
        Self {
            partition_id,
            consumer_id,
            offset: None,
            count: DEFAULT_POLL_COUNT,
            auto_commit: false,
        }
    }

    /// Set the starting offset.
    pub fn with_offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Set the maximum message count.
    pub fn with_count(mut self, count: u32) -> Self {
        self.count = count;
        self
    }

    /// Set auto-commit behavior.
    pub fn with_auto_commit(mut self, auto_commit: bool) -> Self {
        self.auto_commit = auto_commit;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poll_params_new_with_defaults() {
        let params = PollParams::new(1, 2);

        assert_eq!(params.partition_id, 1);
        assert_eq!(params.consumer_id, 2);
        assert_eq!(params.offset, None);
        assert_eq!(params.count, 10);
        assert!(!params.auto_commit);
    }

    #[test]
    fn test_poll_params_builder_chain() {
        let params = PollParams::new(1, 1)
            .with_offset(100)
            .with_count(50)
            .with_auto_commit(true);

        assert_eq!(params.partition_id, 1);
        assert_eq!(params.consumer_id, 1);
        assert_eq!(params.offset, Some(100));
        assert_eq!(params.count, 50);
        assert!(params.auto_commit);
    }

    #[test]
    fn test_poll_params_partial_builder() {
        let params = PollParams::new(3, 5).with_count(25);

        assert_eq!(params.partition_id, 3);
        assert_eq!(params.consumer_id, 5);
        assert_eq!(params.offset, None);
        assert_eq!(params.count, 25);
        assert!(!params.auto_commit);
    }
}
