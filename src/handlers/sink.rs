//! `/sink/inbound`: the receiver for the connectors demo's HTTP sink
//! (ADR-0001 PR 4). [`crate::services::sink_inbox`] describes the format, the
//! retention bounds and the delivery semantics.
//!
//! The routes exist only when `SINK_INBOUND_TOKEN` is set. `POST` needs that
//! token as a bearer credential ([`require_sink_token`]), then the `ndjson`
//! content type ([`require_ndjson`]), both checked before the body is read.
//! `GET` is guarded like every other read, by `API_KEY` when one is
//! configured.

use std::sync::Arc;

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
use crate::services::SinkInbox;
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

/// Refuse a delivery that is not an `ndjson` batch (`415`), before its body
/// is read: the sink's `json_array` and `individual` modes, and anything
/// else. It runs after [`require_sink_token`], so the refusal is counted as
/// an authenticated batch rejected, and a caller without the token learns
/// nothing about content types.
pub async fn require_ndjson(
    State(inbox): State<Arc<SinkInbox>>,
    request: Request,
    next: Next,
) -> Response {
    if !is_ndjson(request.headers()) {
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
    next.run(request).await
}

/// Accept one `ndjson` batch from the HTTP sink. The route reaches it only
/// through [`require_sink_token`] and [`require_ndjson`].
///
/// Answers `200 {"accepted": <messages>}`; `400` for a batch with a line
/// that is not a sink envelope (nothing from it is kept); `413` past
/// `MAX_REQUEST_BODY_SIZE`. The sink does not retry a 4xx, and it has
/// already stored its offsets, so a refused batch is not delivered again.
#[instrument(skip_all, fields(bytes = body.len()))]
pub async fn receive_sink_batch(State(state): State<AppState>, body: Bytes) -> Response {
    let Some(inbox) = state.sink_inbox.as_ref() else {
        return receiver_disabled().into_response();
    };
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
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::Router;
    use axum::body::Body;
    use axum::http::HeaderValue;
    use axum::middleware;
    use axum::routing::post;
    use tower::ServiceExt;

    use crate::services::SinkToken;

    /// A route guarded by [`require_ndjson`] whose handler records that it
    /// ran. The handler is what reads a body, so a request it never reaches
    /// was refused with its body unread.
    fn guarded_route(inbox: &Arc<SinkInbox>, reached: &Arc<AtomicBool>) -> Router {
        let reached: Arc<AtomicBool> = Arc::clone(reached);
        Router::new()
            .route(
                "/",
                post(move |_body: Bytes| async move {
                    reached.store(true, Ordering::SeqCst);
                    StatusCode::OK
                }),
            )
            .layer(middleware::from_fn_with_state(
                Arc::clone(inbox),
                require_ndjson,
            ))
    }

    fn test_inbox() -> Arc<SinkInbox> {
        Arc::new(SinkInbox::new(SinkToken::new(&"t".repeat(32)).unwrap()))
    }

    fn request(content_type: Option<&'static str>) -> Request {
        let mut builder = Request::post("/");
        if let Some(value) = content_type {
            builder = builder.header(header::CONTENT_TYPE, value);
        }
        builder.body(Body::from("{}\n")).unwrap()
    }

    #[tokio::test]
    async fn test_require_ndjson_refuses_other_types_before_the_handler_reads_the_body() {
        let inbox: Arc<SinkInbox> = test_inbox();
        let reached = Arc::new(AtomicBool::new(false));
        for content_type in [Some("application/json"), Some("text/plain"), None] {
            let response = guarded_route(&inbox, &reached)
                .oneshot(request(content_type))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
        assert!(!reached.load(Ordering::SeqCst), "the handler ran");
        assert_eq!(inbox.snapshot().batches_rejected, 3);
    }

    #[tokio::test]
    async fn test_require_ndjson_passes_an_ndjson_batch_to_the_handler() {
        let inbox: Arc<SinkInbox> = test_inbox();
        let reached = Arc::new(AtomicBool::new(false));
        let response = guarded_route(&inbox, &reached)
            .oneshot(request(Some("application/x-ndjson; charset=utf-8")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(reached.load(Ordering::SeqCst));
        assert_eq!(inbox.snapshot().batches_rejected, 0);
    }

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
