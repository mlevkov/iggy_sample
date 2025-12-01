//! Consumer service for polling messages from Iggy streams.
//!
//! This service handles message consumption with:
//! - Automatic message parsing and deserialization
//! - Offset tracking per consumer
//! - Message statistics
//!
//! # Consumer IDs
//!
//! Each consumer ID maintains its own offset position. Use consistent IDs
//! across application restarts to resume from the last committed position.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use iggy::prelude::IggyMessage;
use tracing::{debug, instrument, warn};

use crate::error::AppResult;
use crate::iggy_client::{IggyClientWrapper, PollParams};
use crate::models::{Event, PollMessagesResponse, ReceivedMessage};

/// Service for consuming messages from Iggy streams.
///
/// Thread-safe and clonable for use across async tasks.
///
/// # Counter Memory Ordering
///
/// The `messages_consumed` counter uses `Ordering::Relaxed` because:
/// - It's a monotonically increasing counter used only for metrics
/// - Eventual consistency is acceptable (exact real-time count not required)
/// - No other operations depend on this counter's value for correctness
/// - `Relaxed` has minimal overhead on all architectures
#[derive(Clone)]
pub struct ConsumerService {
    client: IggyClientWrapper,
    /// Total messages consumed (monotonic counter, eventually consistent).
    messages_consumed: Arc<AtomicU64>,
}

impl ConsumerService {
    /// Create a new consumer service.
    pub fn new(client: IggyClientWrapper) -> Self {
        Self {
            client,
            messages_consumed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Poll messages from the default stream and topic.
    ///
    /// # Arguments
    ///
    /// * `params` - Polling parameters (partition, consumer, offset, count, auto_commit)
    #[instrument(skip(self, params), fields(partition_id = params.partition_id, consumer_id = params.consumer_id))]
    pub async fn poll(&self, params: PollParams) -> AppResult<PollMessagesResponse> {
        let partition_id = params.partition_id;
        let polled = self.client.poll_messages_default(params).await?;

        let messages = self.parse_messages(&polled.messages);
        let message_count = messages.len();

        self.messages_consumed
            .fetch_add(message_count as u64, Ordering::Relaxed);

        Ok(PollMessagesResponse {
            messages,
            count: message_count,
            partition_id,
            current_offset: polled.current_offset,
        })
    }

    /// Poll messages from a specific stream and topic.
    #[instrument(skip(self, params), fields(partition_id = params.partition_id, consumer_id = params.consumer_id))]
    pub async fn poll_from(
        &self,
        stream: &str,
        topic: &str,
        params: PollParams,
    ) -> AppResult<PollMessagesResponse> {
        let partition_id = params.partition_id;
        let polled = self.client.poll_messages(stream, topic, params).await?;

        let messages = self.parse_messages(&polled.messages);
        let message_count = messages.len();

        self.messages_consumed
            .fetch_add(message_count as u64, Ordering::Relaxed);

        Ok(PollMessagesResponse {
            messages,
            count: message_count,
            partition_id,
            current_offset: polled.current_offset,
        })
    }

    /// Parse raw Iggy messages into our Event format.
    ///
    /// # Message Parsing
    ///
    /// - Successfully parsed messages are returned in the result
    /// - Failed parsing is logged and the message is skipped
    /// - Invalid timestamps are logged and fall back to current time
    fn parse_messages(&self, messages: &[IggyMessage]) -> Vec<ReceivedMessage> {
        let mut parsed = Vec::with_capacity(messages.len());

        for msg in messages {
            match serde_json::from_slice::<Event>(&msg.payload) {
                Ok(event) => {
                    // Convert timestamp with proper error handling
                    let timestamp =
                        self.parse_timestamp(msg.header.timestamp as i64, msg.header.offset);

                    parsed.push(ReceivedMessage {
                        offset: msg.header.offset,
                        timestamp,
                        id: msg.header.id,
                        event,
                        size: msg.payload.len(),
                    });
                }
                Err(e) => {
                    warn!(
                        offset = msg.header.offset,
                        message_id = msg.header.id,
                        payload_size = msg.payload.len(),
                        error = %e,
                        "Failed to parse message as Event, skipping"
                    );
                }
            }
        }

        debug!(
            total = messages.len(),
            parsed = parsed.len(),
            "Message parsing complete"
        );
        parsed
    }

    /// Parse a microsecond timestamp to DateTime, logging invalid values.
    ///
    /// # Invalid Timestamps
    ///
    /// If the timestamp cannot be converted (e.g., overflow, invalid value),
    /// a warning is logged with the message offset for debugging, and the
    /// current time is returned as a fallback.
    fn parse_timestamp(&self, timestamp_micros: i64, message_offset: u64) -> DateTime<Utc> {
        match DateTime::from_timestamp_micros(timestamp_micros) {
            Some(dt) => dt,
            None => {
                // Log the invalid timestamp for debugging
                warn!(
                    message_offset,
                    timestamp_micros, "Invalid message timestamp, using current time as fallback"
                );
                Utc::now()
            }
        }
    }

    /// Get the total number of messages consumed.
    pub fn messages_consumed(&self) -> u64 {
        self.messages_consumed.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consumer_messages_counter() {
        let counter = AtomicU64::new(0);
        counter.fetch_add(5, Ordering::Relaxed);
        counter.fetch_add(3, Ordering::Relaxed);
        assert_eq!(counter.load(Ordering::Relaxed), 8);
    }
}
