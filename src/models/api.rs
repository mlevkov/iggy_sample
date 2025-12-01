use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Event;

/// Request to create a new stream.
#[derive(Debug, Deserialize)]
pub struct CreateStreamRequest {
    /// Stream name (must be unique)
    pub name: String,
}

/// Request to create a new topic within a stream.
#[derive(Debug, Deserialize)]
pub struct CreateTopicRequest {
    /// Topic name (must be unique within the stream)
    pub name: String,
    /// Number of partitions for parallel processing
    #[serde(default = "default_partitions")]
    pub partitions: u32,
}

fn default_partitions() -> u32 {
    1
}

/// Request to send a message to a topic.
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    /// The event to publish
    pub event: Event,
    /// Optional partition key for consistent routing
    #[serde(default)]
    pub partition_key: Option<String>,
}

/// Response after successfully sending a message.
#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    /// Whether the message was sent successfully
    pub success: bool,
    /// The event ID of the published message
    pub event_id: Uuid,
    /// Stream the message was sent to
    pub stream: String,
    /// Topic the message was sent to
    pub topic: String,
    /// Timestamp of acknowledgment
    pub timestamp: DateTime<Utc>,
}

/// Request to poll messages from a topic.
#[derive(Debug, Deserialize)]
pub struct PollMessagesRequest {
    /// Consumer ID for tracking offsets
    #[serde(default = "default_consumer_id")]
    pub consumer_id: u32,
    /// Partition to poll from (None = all partitions)
    pub partition_id: Option<u32>,
    /// Starting offset (None = from last committed offset)
    pub offset: Option<u64>,
    /// Maximum number of messages to return
    #[serde(default = "default_message_count")]
    pub count: u32,
    /// Whether to auto-commit the offset after polling
    #[serde(default)]
    pub auto_commit: bool,
}

fn default_consumer_id() -> u32 {
    1
}

fn default_message_count() -> u32 {
    10
}

/// Response containing polled messages.
#[derive(Debug, Serialize)]
pub struct PollMessagesResponse {
    /// List of received messages
    pub messages: Vec<ReceivedMessage>,
    /// Number of messages returned
    pub count: usize,
    /// Partition ID the messages came from
    pub partition_id: u32,
    /// Current offset after polling
    pub current_offset: u64,
}

/// A message received from polling.
#[derive(Debug, Serialize)]
pub struct ReceivedMessage {
    /// Message offset within the partition
    pub offset: u64,
    /// Message timestamp
    pub timestamp: DateTime<Utc>,
    /// Message ID
    pub id: u128,
    /// The deserialized event
    pub event: Event,
    /// Raw message size in bytes
    pub size: usize,
}

/// Stream information response.
#[derive(Debug, Serialize)]
pub struct StreamInfo {
    /// Stream ID
    pub id: u32,
    /// Stream name
    pub name: String,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Number of topics in the stream
    pub topics_count: u32,
    /// Total size in bytes
    pub size_bytes: u64,
    /// Total message count across all topics
    pub messages_count: u64,
}

/// Topic information response.
#[derive(Debug, Serialize)]
pub struct TopicInfo {
    /// Topic ID
    pub id: u32,
    /// Topic name
    pub name: String,
    /// Parent stream ID
    pub stream_id: u32,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Number of partitions
    pub partitions_count: u32,
    /// Total size in bytes
    pub size_bytes: u64,
    /// Total message count
    pub messages_count: u64,
}

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Service health status
    pub status: String,
    /// Whether Iggy connection is healthy
    pub iggy_connected: bool,
    /// Service version
    pub version: String,
    /// Current timestamp
    pub timestamp: DateTime<Utc>,
}

/// Statistics response.
///
/// These statistics are retrieved from a background-refreshed cache.
/// The `cache_age_seconds` field indicates how old the data is.
#[derive(Debug, Serialize)]
pub struct StatsResponse {
    /// Number of active streams
    pub streams_count: u32,
    /// Number of active topics
    pub topics_count: u32,
    /// Total messages published
    pub total_messages: u64,
    /// Total data size in bytes
    pub total_size_bytes: u64,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Age of cached statistics in seconds (0 = fresh)
    pub cache_age_seconds: u64,
    /// Whether the cache is considered stale (exceeded TTL)
    pub cache_stale: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_create_stream_request_deserialization() {
        let json = r#"{"name": "my-stream"}"#;
        let request: CreateStreamRequest =
            serde_json::from_str(json).expect("Deserialization should succeed");

        assert_eq!(request.name, "my-stream");
    }

    #[test]
    fn test_create_topic_request_default_partitions() {
        let json = r#"{"name": "my-topic"}"#;
        let request: CreateTopicRequest =
            serde_json::from_str(json).expect("Deserialization should succeed");

        assert_eq!(request.name, "my-topic");
        assert_eq!(request.partitions, 1);
    }

    #[test]
    fn test_poll_messages_request_defaults() {
        let json = r#"{}"#;
        let request: PollMessagesRequest =
            serde_json::from_str(json).expect("Deserialization should succeed");

        assert_eq!(request.consumer_id, 1);
        assert_eq!(request.count, 10);
        assert!(!request.auto_commit);
    }

    #[test]
    fn test_send_message_response_serialization() {
        let response = SendMessageResponse {
            success: true,
            event_id: Uuid::new_v4(),
            stream: "test-stream".to_string(),
            topic: "test-topic".to_string(),
            timestamp: Utc::now(),
        };

        let json = serde_json::to_string(&response).expect("Serialization should succeed");
        assert!(json.contains("\"success\":true"));
    }

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            status: "healthy".to_string(),
            iggy_connected: true,
            version: "0.1.0".to_string(),
            timestamp: Utc::now(),
        };

        let json = serde_json::to_string(&response).expect("Serialization should succeed");
        assert!(json.contains("\"status\":\"healthy\""));
    }
}
