//! What `GET /sink/inbound` reports: the deliveries the connectors demo's
//! HTTP sink made to this gateway (ADR-0001 PR 4).

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Map, Value};

/// One message as the HTTP sink delivered it.
///
/// The fields come from the sink's metadata envelope. `stream`, `topic`,
/// `partition_id` and `offset` locate the message in Iggy; `id` is its Iggy
/// message id as the sink formats it (32 lowercase hex digits).
#[derive(Debug, Clone, Serialize)]
pub struct SinkDelivery {
    /// When this gateway accepted the batch carrying the message
    pub received_at: DateTime<Utc>,
    /// Stream the message was consumed from
    pub stream: String,
    /// Topic the message was consumed from
    pub topic: String,
    /// Partition the message was consumed from (0-indexed)
    pub partition_id: u32,
    /// Offset of the message within its partition
    pub offset: u64,
    /// Iggy message id, 32 lowercase hex digits
    pub id: String,
    /// When Iggy stored the message, or `null` if the sink sent a timestamp
    /// that is not a valid instant
    pub timestamp: Option<DateTime<Utc>>,
    /// The message's user headers, when it has any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Map<String, Value>>,
    /// The payload as the sink decoded it: JSON for a `json` stream schema
    pub payload: Value,
    /// Length of the envelope line, counted against the retention budget
    #[serde(skip)]
    pub(crate) envelope_bytes: usize,
}

/// Response of `GET /sink/inbound`.
///
/// The counters cover the gateway's lifetime; `recent` holds only the
/// newest deliveries (see `crate::services::sink_inbox` for the bounds),
/// newest first.
#[derive(Debug, Serialize)]
pub struct SinkInboxView {
    /// Messages accepted since the gateway started
    pub messages_received: u64,
    /// Batches accepted since the gateway started
    pub batches_accepted: u64,
    /// Authenticated batches refused (malformed, wrong content type)
    pub batches_rejected: u64,
    /// Requests refused for a missing or wrong bearer token
    pub unauthorized_requests: u64,
    /// When the last batch was accepted, if any was
    pub last_received_at: Option<DateTime<Utc>>,
    /// Number of deliveries in `recent`
    pub retained: usize,
    /// The newest deliveries, newest first
    pub recent: Vec<SinkDelivery>,
}
