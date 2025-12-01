//! Message sending and polling handlers.
//!
//! # Endpoints
//!
//! - `POST /messages` - Send a single message to default stream/topic
//! - `GET /messages` - Poll messages from default stream/topic
//! - `POST /messages/batch` - Send multiple messages in one request
//! - `POST /streams/{stream}/topics/{topic}/messages` - Send to specific location
//! - `GET /streams/{stream}/topics/{topic}/messages` - Poll from specific location
//!
//! # Configurable Limits
//!
//! - `BATCH_MAX_SIZE` - Maximum messages per batch send (default: 1000)
//! - `POLL_MAX_COUNT` - Maximum messages per poll (default: 100)

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use tracing::instrument;

use crate::error::{AppError, AppResult};
use crate::iggy_client::PollParams;
use crate::models::{Event, PollMessagesResponse, SendMessageRequest, SendMessageResponse};
use crate::state::AppState;
use crate::validation::{
    validate_consumer_id, validate_event_type, validate_partition_id, validate_resource_name,
};

/// Send a single message to the default stream/topic.
///
/// # Request Body
///
/// ```json
/// {
///   "event": {
///     "id": "550e8400-e29b-41d4-a716-446655440000",
///     "event_type": "user.created",
///     "timestamp": "2024-01-15T10:30:00Z",
///     "payload": { "type": "Generic", "data": {} }
///   },
///   "partition_key": "optional-key"
/// }
/// ```
#[instrument(skip(state, payload))]
pub async fn send_message(
    State(state): State<AppState>,
    Json(payload): Json<SendMessageRequest>,
) -> AppResult<(StatusCode, Json<SendMessageResponse>)> {
    // Validate event type before processing
    validate_event_type(&payload.event.event_type)?;

    let response = state
        .producer
        .send(&payload.event, payload.partition_key.as_deref())
        .await?;

    Ok((StatusCode::CREATED, Json(response)))
}

/// Request body for sending a batch of messages.
#[derive(Debug, Deserialize)]
pub struct SendBatchRequest {
    /// List of events to send
    pub events: Vec<Event>,
    /// Optional partition key for all messages in the batch
    #[serde(default)]
    pub partition_key: Option<String>,
}

/// Send multiple messages in a batch.
///
/// Uses true batch sending - all messages are sent in a single network call
/// for optimal performance.
///
/// # Limits
///
/// - Maximum batch size: configured via `BATCH_MAX_SIZE` (default: 1000)
/// - Empty batch: returns 400 Bad Request
///
/// # Request Body
///
/// ```json
/// {
///   "events": [
///     { "id": "...", "event_type": "...", ... },
///     { "id": "...", "event_type": "...", ... }
///   ],
///   "partition_key": "optional-key"
/// }
/// ```
#[instrument(skip(state, payload), fields(batch_size = payload.events.len()))]
pub async fn send_batch(
    State(state): State<AppState>,
    Json(payload): Json<SendBatchRequest>,
) -> AppResult<(StatusCode, Json<Vec<SendMessageResponse>>)> {
    let max_batch_size = state.config.batch_max_size;

    if payload.events.is_empty() {
        return Err(AppError::BadRequest(
            "Events list cannot be empty".to_string(),
        ));
    }

    if payload.events.len() > max_batch_size {
        return Err(AppError::BadRequest(format!(
            "Batch size {} exceeds maximum of {} messages",
            payload.events.len(),
            max_batch_size
        )));
    }

    // Validate all event types before processing
    for (index, event) in payload.events.iter().enumerate() {
        validate_event_type(&event.event_type)
            .map_err(|e| AppError::BadRequest(format!("Event at index {}: {}", index, e)))?;
    }

    let responses = state
        .producer
        .send_batch(&payload.events, payload.partition_key.as_deref())
        .await?;

    Ok((StatusCode::CREATED, Json(responses)))
}

/// Query parameters for polling messages.
#[derive(Debug, Deserialize)]
pub struct PollQuery {
    /// Partition ID to poll from (default: 0, Iggy uses 0-indexed partitions)
    #[serde(default)]
    pub partition_id: u32,
    /// Consumer ID for offset tracking (default: 1)
    #[serde(default = "default_consumer")]
    pub consumer_id: u32,
    /// Starting offset (optional, defaults to next uncommitted)
    pub offset: Option<u64>,
    /// Number of messages to poll (default: 10, capped by POLL_MAX_COUNT)
    #[serde(default = "default_count")]
    pub count: u32,
    /// Whether to auto-commit offset after polling
    #[serde(default)]
    pub auto_commit: bool,
}

fn default_consumer() -> u32 {
    1
}

fn default_count() -> u32 {
    10
}

/// Poll messages from the default stream/topic.
///
/// # Query Parameters
///
/// - `partition_id` - Partition to poll from (default: 1)
/// - `consumer_id` - Consumer ID for offset tracking (default: 1)
/// - `offset` - Starting offset (optional)
/// - `count` - Number of messages to return (default: 10, max: POLL_MAX_COUNT)
/// - `auto_commit` - Auto-commit offset after polling (default: false)
///
/// # Example
///
/// ```bash
/// curl "http://localhost:3000/messages?partition_id=1&count=10&offset=0"
/// ```
#[instrument(skip(state))]
pub async fn poll_messages(
    State(state): State<AppState>,
    Query(query): Query<PollQuery>,
) -> AppResult<Json<PollMessagesResponse>> {
    // Validate poll parameters
    validate_partition_id(query.partition_id)?;
    validate_consumer_id(query.consumer_id)?;

    let max_count = state.config.poll_max_count;
    let count = query.count.min(max_count);

    let params = PollParams::new(query.partition_id, query.consumer_id)
        .with_count(count)
        .with_auto_commit(query.auto_commit);

    let params = match query.offset {
        Some(offset) => params.with_offset(offset),
        None => params,
    };

    let response = state.consumer.poll(params).await?;

    Ok(Json(response))
}

/// Path parameters for stream/topic-specific message operations.
#[derive(Debug, Deserialize)]
pub struct StreamTopicPath {
    /// Stream name
    pub stream: String,
    /// Topic name
    pub topic: String,
}

/// Send a message to a specific stream and topic.
///
/// # Path Parameters
///
/// - `stream` - Target stream name
/// - `topic` - Target topic name
#[instrument(skip(state, payload))]
pub async fn send_message_to(
    State(state): State<AppState>,
    Path(path): Path<StreamTopicPath>,
    Json(payload): Json<SendMessageRequest>,
) -> AppResult<(StatusCode, Json<SendMessageResponse>)> {
    // Validate path parameters before use
    validate_resource_name(&path.stream, "Stream")?;
    validate_resource_name(&path.topic, "Topic")?;
    // Validate event type before processing
    validate_event_type(&payload.event.event_type)?;

    let response = state
        .producer
        .send_to(
            &path.stream,
            &path.topic,
            &payload.event,
            payload.partition_key.as_deref(),
        )
        .await?;

    Ok((StatusCode::CREATED, Json(response)))
}

/// Poll messages from a specific stream and topic.
///
/// # Path Parameters
///
/// - `stream` - Source stream name
/// - `topic` - Source topic name
#[instrument(skip(state))]
pub async fn poll_messages_from(
    State(state): State<AppState>,
    Path(path): Path<StreamTopicPath>,
    Query(query): Query<PollQuery>,
) -> AppResult<Json<PollMessagesResponse>> {
    // Validate path parameters before use
    validate_resource_name(&path.stream, "Stream")?;
    validate_resource_name(&path.topic, "Topic")?;

    // Validate poll parameters
    validate_partition_id(query.partition_id)?;
    validate_consumer_id(query.consumer_id)?;

    let max_count = state.config.poll_max_count;
    let count = query.count.min(max_count);

    let params = PollParams::new(query.partition_id, query.consumer_id)
        .with_count(count)
        .with_auto_commit(query.auto_commit);

    let params = match query.offset {
        Some(offset) => params.with_offset(offset),
        None => params,
    };

    let response = state
        .consumer
        .poll_from(&path.stream, &path.topic, params)
        .await?;

    Ok(Json(response))
}
