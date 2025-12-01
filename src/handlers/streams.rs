use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use tracing::instrument;

use super::util::parse_timestamp_with_context;
use crate::error::AppResult;
use crate::models::{CreateStreamRequest, StreamInfo};
use crate::state::AppState;
use crate::validation::validate_resource_name;

/// List all streams.
#[instrument(skip(state))]
pub async fn list_streams(State(state): State<AppState>) -> AppResult<Json<Vec<StreamInfo>>> {
    let streams = state.iggy_client.list_streams().await?;

    let stream_infos: Vec<StreamInfo> = streams
        .into_iter()
        .map(|s| {
            let created_at =
                parse_timestamp_with_context(s.created_at.as_micros() as i64, "stream", &s.name);
            StreamInfo {
                id: s.id,
                name: s.name,
                created_at,
                topics_count: s.topics_count,
                size_bytes: s.size.as_bytes_u64(),
                messages_count: s.messages_count,
            }
        })
        .collect();

    Ok(Json(stream_infos))
}

/// Get a specific stream by name.
#[instrument(skip(state))]
pub async fn get_stream(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Json<StreamInfo>> {
    // Validate path parameter before use
    validate_resource_name(&name, "Stream")?;

    let stream = state.iggy_client.get_stream(&name).await?;

    let created_at =
        parse_timestamp_with_context(stream.created_at.as_micros() as i64, "stream", &stream.name);

    Ok(Json(StreamInfo {
        id: stream.id,
        name: stream.name,
        created_at,
        topics_count: stream.topics_count,
        size_bytes: stream.size.as_bytes_u64(),
        messages_count: stream.messages_count,
    }))
}

/// Create a new stream.
#[instrument(skip(state))]
pub async fn create_stream(
    State(state): State<AppState>,
    Json(payload): Json<CreateStreamRequest>,
) -> AppResult<StatusCode> {
    validate_resource_name(&payload.name, "Stream")?;

    state.iggy_client.create_stream(&payload.name).await?;

    Ok(StatusCode::CREATED)
}

/// Delete a stream by name.
#[instrument(skip(state))]
pub async fn delete_stream(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<StatusCode> {
    // Validate path parameter before use
    validate_resource_name(&name, "Stream")?;

    state.iggy_client.delete_stream(&name).await?;

    Ok(StatusCode::NO_CONTENT)
}
