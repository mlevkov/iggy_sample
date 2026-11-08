//! `/sink/inbound`: the receiver for the connectors demo's HTTP sink
//! (ADR-0001 PR 4). [`crate::services::sink_inbox`] describes the format, the
//! retention bounds and the delivery semantics.
//!
//! The routes exist only when `SINK_INBOUND_TOKEN` is set. `POST` needs that
//! token as a bearer credential, checked by [`require_sink_token`] before the
//! body is read. `GET` is guarded like every other read, by `API_KEY` when
//! one is configured.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::json;
use tracing::{debug, instrument, warn};

use crate::error::{AppError, AppResult};
use crate::metrics;
use crate::models::SinkInboxView;
use crate::services::sink_inbox::parse_batch;
use crate::state::AppState;

/// The content type of the HTTP sink's `ndjson` batch mode.
const NDJSON: &str = "application/x-ndjson";

fn receiver_disabled() -> AppError {
    AppError::NotFound("The sink receiver is disabled (SINK_INBOUND_TOKEN is unset)".to_string())
}

/// Refuse a request without the sink's bearer token, before its body is
/// read. Every refusal is counted and logged, never with the presented
/// value.
pub async fn require_sink_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(inbox) = state.sink_inbox.as_ref() else {
        return receiver_disabled().into_response();
    };
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if !inbox.authorize(authorization) {
        metrics::record_sink_inbound_request("unauthorized");
        warn!("Refused a sink delivery: missing or wrong bearer token");
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Json(json!({
                "error": "unauthorized",
                "message": "Missing or invalid bearer token"
            })),
        )
            .into_response();
    }
    next.run(request).await
}

/// Accept one `ndjson` batch from the HTTP sink.
///
/// Answers `200 {"accepted": <messages>}`; `400` for a batch with a line
/// that is not a sink envelope (nothing from it is kept); `415` for any
/// other content type; `413` past `MAX_REQUEST_BODY_SIZE`. The sink does not
/// retry a 4xx, and it has already stored its offsets, so a refused batch
/// is not delivered again.
#[instrument(skip_all, fields(bytes = body.len()))]
pub async fn receive_sink_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(inbox) = state.sink_inbox.as_ref() else {
        return receiver_disabled().into_response();
    };
    if !is_ndjson(&headers) {
        inbox.reject();
        metrics::record_sink_inbound_request("rejected");
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(json!({
                "error": "unsupported_media_type",
                "message": format!(
                    "Expected Content-Type: {NDJSON}, the HTTP sink's ndjson batch mode"
                )
            })),
        )
            .into_response();
    }
    match parse_batch(&body, Utc::now()) {
        Ok(deliveries) => {
            let accepted = inbox.accept(deliveries);
            metrics::record_sink_inbound_request("accepted");
            metrics::record_sink_inbound_messages(accepted as u64);
            debug!(accepted, "Accepted a sink batch");
            (StatusCode::OK, Json(json!({ "accepted": accepted }))).into_response()
        }
        Err(error) => {
            inbox.reject();
            metrics::record_sink_inbound_request("rejected");
            AppError::BadRequest(format!("Not a sink batch: {error}")).into_response()
        }
    }
}

/// What the HTTP sink has delivered: lifetime counters and the newest
/// deliveries, newest first.
#[instrument(skip_all)]
pub async fn sink_deliveries(State(state): State<AppState>) -> AppResult<Json<SinkInboxView>> {
    let inbox = state.sink_inbox.as_ref().ok_or_else(receiver_disabled)?;
    Ok(Json(inbox.snapshot()))
}

/// Whether the request declares `application/x-ndjson`, parameters such
/// as `charset` allowed.
fn is_ndjson(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case(NDJSON))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn with_content_type(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn test_is_ndjson_accepts_the_media_type_with_parameters_in_any_case() {
        assert!(is_ndjson(&with_content_type("application/x-ndjson")));
        assert!(is_ndjson(&with_content_type(
            "Application/X-NDJSON; charset=utf-8"
        )));
    }

    #[test]
    fn test_is_ndjson_refuses_other_or_missing_types() {
        assert!(!is_ndjson(&with_content_type("application/json")));
        assert!(!is_ndjson(&with_content_type("application/x-ndjsonx")));
        assert!(!is_ndjson(&HeaderMap::new()));
    }
}
