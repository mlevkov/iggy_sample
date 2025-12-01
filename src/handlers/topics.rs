use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use tracing::instrument;

use super::util::parse_timestamp_with_context;
use crate::error::AppResult;
use crate::models::{CreateTopicRequest, TopicInfo};
use crate::state::AppState;
use crate::validation::{validate_partition_count, validate_resource_name};

/// Path parameters for topic operations.
#[derive(Debug, Deserialize)]
pub struct StreamPath {
    pub stream: String,
}

/// Path parameters for specific topic operations.
#[derive(Debug, Deserialize)]
pub struct TopicPath {
    pub stream: String,
    pub topic: String,
}

/// List all topics in a stream.
#[instrument(skip(state))]
pub async fn list_topics(
    State(state): State<AppState>,
    Path(path): Path<StreamPath>,
) -> AppResult<Json<Vec<TopicInfo>>> {
    // Validate path parameter before use
    validate_resource_name(&path.stream, "Stream")?;

    let topics = state.iggy_client.list_topics(&path.stream).await?;

    let stream_details = state.iggy_client.get_stream(&path.stream).await?;
    let topic_infos: Vec<TopicInfo> = topics
        .into_iter()
        .map(|t| {
            let created_at =
                parse_timestamp_with_context(t.created_at.as_micros() as i64, "topic", &t.name);
            TopicInfo {
                id: t.id,
                name: t.name,
                stream_id: stream_details.id,
                created_at,
                partitions_count: t.partitions_count,
                size_bytes: t.size.as_bytes_u64(),
                messages_count: t.messages_count,
            }
        })
        .collect();

    Ok(Json(topic_infos))
}

/// Get a specific topic by name.
#[instrument(skip(state))]
pub async fn get_topic(
    State(state): State<AppState>,
    Path(path): Path<TopicPath>,
) -> AppResult<Json<TopicInfo>> {
    // Validate path parameters before use
    validate_resource_name(&path.stream, "Stream")?;
    validate_resource_name(&path.topic, "Topic")?;

    let topic = state
        .iggy_client
        .get_topic(&path.stream, &path.topic)
        .await?;

    let stream_details = state.iggy_client.get_stream(&path.stream).await?;
    let created_at =
        parse_timestamp_with_context(topic.created_at.as_micros() as i64, "topic", &topic.name);

    Ok(Json(TopicInfo {
        id: topic.id,
        name: topic.name,
        stream_id: stream_details.id,
        created_at,
        partitions_count: topic.partitions_count,
        size_bytes: topic.size.as_bytes_u64(),
        messages_count: topic.messages_count,
    }))
}

/// Create a new topic in a stream.
#[instrument(skip(state))]
pub async fn create_topic(
    State(state): State<AppState>,
    Path(path): Path<StreamPath>,
    Json(payload): Json<CreateTopicRequest>,
) -> AppResult<StatusCode> {
    // Validate path parameter before use
    validate_resource_name(&path.stream, "Stream")?;
    // Validate request body
    validate_resource_name(&payload.name, "Topic")?;
    validate_partition_count(payload.partitions, "Topic")?;

    state
        .iggy_client
        .create_topic(&path.stream, &payload.name, payload.partitions)
        .await?;

    Ok(StatusCode::CREATED)
}

/// Delete a topic from a stream.
#[instrument(skip(state))]
pub async fn delete_topic(
    State(state): State<AppState>,
    Path(path): Path<TopicPath>,
) -> AppResult<StatusCode> {
    // Validate path parameters before use
    validate_resource_name(&path.stream, "Stream")?;
    validate_resource_name(&path.topic, "Topic")?;

    state
        .iggy_client
        .delete_topic(&path.stream, &path.topic)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
