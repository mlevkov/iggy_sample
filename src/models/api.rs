use chrono::{DateTime, Utc};
use iggy::prelude::{ClusterMetadata, ClusterNodeRole, ClusterNodeStatus, Partition};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Event, TopicOptionsInput, TopicOptionsView};

/// Request to create a new stream.
#[derive(Debug, Deserialize)]
pub struct CreateStreamRequest {
    /// Stream name (must be unique)
    pub name: String,
}

/// Request to create a new topic within a stream.
///
/// Unknown fields are refused at both levels, so an option sent beside
/// `options` instead of inside it, or misspelt, fails instead of leaving the
/// topic on a server default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTopicRequest {
    /// Topic name (must be unique within the stream)
    pub name: String,
    /// Number of partitions for parallel processing
    #[serde(default = "default_partitions")]
    pub partitions: u32,
    /// Per-topic options, fixed at creation. Omitted ones take the server's
    /// defaults.
    #[serde(default)]
    pub options: TopicOptionsInput,
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
    /// Stored per-topic options, each with its value and who chose it
    pub options: TopicOptionsView,
    /// Each partition and the nodes that hold it. Reported by the single-topic
    /// read only; the listing leaves it out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partitions: Option<Vec<PartitionInfo>>,
}

/// One partition of a topic and the nodes that hold it.
#[derive(Debug, Serialize)]
pub struct PartitionInfo {
    /// Partition ID (0-indexed)
    pub id: u32,
    /// Messages in this partition
    pub messages_count: u64,
    /// Replica IDs of the nodes that store this partition. On a cluster
    /// there are `replication_factor` of them; on a single node, one.
    pub members: Vec<u8>,
    /// The members the answering server reports as caught up with the
    /// partition's log. On a cluster, a member that is down drops out of
    /// this list. Even a healthy single node can report it empty for a
    /// partition, so read placement from `members`.
    pub in_sync_members: Vec<u8>,
}

impl From<&Partition> for PartitionInfo {
    fn from(partition: &Partition) -> Self {
        Self {
            id: partition.id,
            messages_count: partition.messages_count,
            members: partition.members.clone(),
            in_sync_members: partition.in_sync_members.clone(),
        }
    }
}

/// Cluster response: the nodes the connected server reports, with their
/// roles and health.
#[derive(Debug, Serialize)]
pub struct ClusterInfo {
    /// Cluster name; `single-node` when clustering is off
    pub name: String,
    /// Replica ID of the metadata leader, if the answering node marked one
    pub leader: Option<u8>,
    /// Every node in the roster, by replica ID
    pub nodes: Vec<ClusterNodeInfo>,
}

/// One cluster node.
#[derive(Debug, Serialize)]
pub struct ClusterNodeInfo {
    /// The node's replica ID (its `--replica-id`)
    pub replica_id: u8,
    /// The node's roster name
    pub name: String,
    /// The TCP address clients are handed for this node (`ip:port`)
    pub address: String,
    /// `leader` or `follower`: the node's role in metadata consensus.
    /// Partition primaries are placed separately.
    pub role: ClusterNodeRole,
    /// `healthy`, `starting`, `stopping`, `unreachable`, `maintenance` or
    /// `unknown`, as the answering node reports it. Server 0.10 reports
    /// `healthy` even for a node that is down, so this is not a liveness
    /// signal: a topic read's `in_sync_members` shows which replicas are up.
    pub status: ClusterNodeStatus,
}

impl From<ClusterMetadata> for ClusterInfo {
    fn from(metadata: ClusterMetadata) -> Self {
        let mut nodes: Vec<ClusterNodeInfo> = metadata
            .nodes
            .into_iter()
            .map(|node| ClusterNodeInfo {
                replica_id: node.replica_id,
                address: format!("{}:{}", node.ip, node.endpoints.tcp),
                name: node.name,
                role: node.role,
                status: node.status,
            })
            .collect();
        nodes.sort_by_key(|node| node.replica_id);
        let leader = nodes
            .iter()
            .find(|node| node.role == ClusterNodeRole::Leader)
            .map(|node| node.replica_id);
        Self {
            name: metadata.name,
            leader,
            nodes,
        }
    }
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
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
        assert_eq!(request.options, TopicOptionsInput::default());
    }

    #[test]
    fn test_create_topic_request_reads_nested_options() {
        let json = r#"{"name": "orders", "options": {"durability": "persisted", "messages_required_to_save": 1}}"#;
        let request: CreateTopicRequest =
            serde_json::from_str(json).expect("Deserialization should succeed");

        assert_eq!(request.options.durability.as_deref(), Some("persisted"));
        assert_eq!(request.options.messages_required_to_save, Some(1));
    }

    #[test]
    fn test_create_topic_request_refuses_an_option_outside_options() {
        let json = r#"{"name": "orders", "durability": "persisted"}"#;
        let error = serde_json::from_str::<CreateTopicRequest>(json)
            .expect_err("a top-level option must not be ignored");

        assert!(error.to_string().contains("unknown field `durability`"));
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

    fn node(replica_id: u8, role: ClusterNodeRole) -> iggy::prelude::ClusterNode {
        iggy::prelude::ClusterNode {
            replica_id,
            name: format!("iggy-node-{replica_id}"),
            ip: format!("10.29.0.{}", 10 + replica_id),
            endpoints: iggy::prelude::TransportEndpoints::new(8090, 0, 0, 0),
            role,
            status: ClusterNodeStatus::Healthy,
        }
    }

    #[test]
    fn test_cluster_info_orders_nodes_and_names_the_leader() {
        let metadata = ClusterMetadata {
            name: "iggy-cluster".to_string(),
            nodes: vec![
                node(2, ClusterNodeRole::Follower),
                node(0, ClusterNodeRole::Follower),
                node(1, ClusterNodeRole::Leader),
            ],
        };
        let info = ClusterInfo::from(metadata);

        assert_eq!(info.leader, Some(1));
        let ids: Vec<u8> = info.nodes.iter().map(|n| n.replica_id).collect();
        assert_eq!(ids, [0, 1, 2]);
        assert_eq!(info.nodes[1].address, "10.29.0.11:8090");

        let json = serde_json::to_value(&info).expect("Serialization should succeed");
        assert_eq!(json["nodes"][1]["role"], "leader");
        assert_eq!(json["nodes"][1]["status"], "healthy");
    }

    #[test]
    fn test_cluster_info_without_a_marked_leader_reports_null() {
        let metadata = ClusterMetadata {
            name: "iggy-cluster".to_string(),
            nodes: vec![node(0, ClusterNodeRole::Follower)],
        };
        let json = serde_json::to_value(ClusterInfo::from(metadata))
            .expect("Serialization should succeed");
        assert!(json["leader"].is_null());
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
