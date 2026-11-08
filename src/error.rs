use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

/// Application-wide error types with appropriate HTTP status codes.
///
/// # Connection Errors
///
/// Connection-related errors are split into specific variants to enable
/// proper pattern matching for reconnection logic:
///
/// - `ConnectionFailed` - Initial connection or reconnection failed
/// - `Disconnected` - Lost connection during operation (triggers reconnection)
/// - `ConnectionReset` - Connection was reset by peer (triggers reconnection)
///
/// # Request-outcome errors (Iggy 0.9 and later)
///
/// The server and SDK report three request outcomes that are not connection
/// failures, and the SDK has already retried each one before it surfaces.
/// None of them triggers reconnection, a retry, or a circuit-breaker failure:
///
/// - `NotAccepted` - never admitted, nothing applied; the client may retry
/// - `OutcomeUnknown` - may or may not have been applied; a blind resend
///   could duplicate it
/// - `AlreadyApplied` - applied, but the reply is gone; answered `202`
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Failed to connect to Iggy server: {0}")]
    ConnectionFailed(String),

    #[error("Disconnected from Iggy server: {0}")]
    Disconnected(String),

    #[error("Connection reset: {0}")]
    ConnectionReset(String),

    #[error("Stream operation failed: {0}")]
    StreamError(String),

    #[error("Topic operation failed: {0}")]
    TopicError(String),

    #[error("Failed to send message: {0}")]
    SendError(String),

    #[error("Failed to poll messages: {0}")]
    PollError(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Invalid request: {0}")]
    BadRequest(String),

    #[error("Internal server error: {0}")]
    Internal(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Operation timed out: {0}")]
    OperationTimeout(String),

    #[error("Circuit breaker open: {0}")]
    CircuitOpen(String),

    /// The broker refused the request before admitting it (Iggy
    /// `TransientNotAccepted`, a lane refusal, or `LifecycleBusy`): nothing
    /// was applied, so retrying is safe.
    #[error("Request not accepted by the broker: {0}")]
    NotAccepted(String),

    /// The broker could not confirm whether the request was applied (Iggy
    /// `TransientNotCommitted`, `OperationUncertain`, `FetchProgressUncertain`
    /// or `RequestTooOld`, surfaced after the SDK's own replays). Resending
    /// could duplicate it.
    #[error("Request outcome unknown: {0}")]
    OutcomeUnknown(String),

    /// The broker applied the request but no longer holds its reply (Iggy
    /// `RequestAlreadyApplied`). Answered `202 Accepted` so clients do not
    /// resend.
    #[error("Request already applied: {0}")]
    AlreadyApplied(String),

    /// The broker speaks a binary protocol version this build cannot use
    /// (Iggy `IncompatibleProtocolVersion`). Retrying cannot help; the server
    /// and the SDK have to be upgraded together.
    #[error("Incompatible Iggy server: {0}")]
    IncompatibleServer(String),
}

/// Error response body for API endpoints.
#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

/// Body of the `202 Accepted` answer to [`AppError::AlreadyApplied`].
#[derive(Serialize)]
struct AlreadyAppliedResponse {
    status: &'static str,
    message: &'static str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Log the full error details server-side for debugging
        // but only expose sanitized messages to clients
        if matches!(self, AppError::AlreadyApplied(_)) {
            tracing::warn!(error = %self, "Request applied but its reply was unavailable");
        } else {
            tracing::error!(error = %self, "Request failed");
        }

        let (status, error_type, message) = match &self {
            // Not a failure: the write happened, only its reply was lost.
            // Answer 2xx so clients treat it as done and do not resend.
            AppError::AlreadyApplied(_) => {
                return (
                    StatusCode::ACCEPTED,
                    axum::Json(AlreadyAppliedResponse {
                        status: "applied",
                        message: "The request was applied, but the message broker no longer \
                                  holds its reply. Do not resend it.",
                    }),
                )
                    .into_response();
            }

            // Service availability errors - don't leak connection details
            // All connection-related errors return 503 to signal temporary unavailability
            AppError::ConnectionFailed(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "connection_failed",
                "Message broker is temporarily unavailable. Please try again later.",
            ),
            AppError::Disconnected(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "disconnected",
                "Connection to message broker was lost. Please try again.",
            ),
            AppError::ConnectionReset(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "connection_reset",
                "Connection to message broker was reset. Please try again.",
            ),

            // Internal errors - never expose internal details to clients
            AppError::StreamError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "stream_error",
                "Stream operation failed. Please contact support if the issue persists.",
            ),
            AppError::TopicError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "topic_error",
                "Topic operation failed. Please contact support if the issue persists.",
            ),
            AppError::SendError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "send_error",
                "Failed to send message. Please try again.",
            ),
            AppError::PollError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "poll_error",
                "Failed to retrieve messages. Please try again.",
            ),
            AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An internal error occurred. Please contact support if the issue persists.",
            ),
            AppError::ConfigError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "config_error",
                "Service configuration error. Please contact support.",
            ),

            // Timeout errors - client can retry
            AppError::OperationTimeout(_) => (
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                "Operation timed out. Please try again.",
            ),

            // Circuit breaker open - service is protecting itself from cascading failures
            AppError::CircuitOpen(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "circuit_open",
                "Service is temporarily unavailable due to recent failures. Please retry later.",
            ),

            // Request-outcome errors (see the enum docs)
            AppError::NotAccepted(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "not_accepted",
                "The message broker did not accept the request, and nothing was applied. \
                 It is safe to retry.",
            ),
            AppError::OutcomeUnknown(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "outcome_unknown",
                "The message broker could not confirm whether the request was applied. \
                 Resending it may apply it twice.",
            ),
            AppError::IncompatibleServer(_) => (
                StatusCode::BAD_GATEWAY,
                "incompatible_broker",
                "The message broker runs a protocol version this service cannot use. \
                 Please contact support.",
            ),

            // Client errors - safe to show the message as it's user-facing
            AppError::SerializationError(e) => {
                // Serde errors can be helpful for clients debugging their payload
                // but sanitize to avoid leaking internal type names
                let sanitized = sanitize_serde_error(e);
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(ErrorResponse {
                        error: "serialization_error".to_string(),
                        message: sanitized,
                        details: None,
                    }),
                )
                    .into_response();
            }
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, "not_found", msg.as_str()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg.as_str()),
        };

        let body = ErrorResponse {
            error: error_type.to_string(),
            message: message.to_string(),
            details: None, // Never expose internal details to clients
        };

        (status, axum::Json(body)).into_response()
    }
}

/// Sanitize serde error messages to avoid leaking internal type information.
///
/// Serde errors can contain internal struct/field names which shouldn't be
/// exposed to external clients. This function extracts the useful parts.
fn sanitize_serde_error(e: &serde_json::Error) -> String {
    let msg = e.to_string();

    // Common patterns to simplify for users
    if msg.contains("missing field")
        && let Some(start) = msg.find('`')
        && let Some(end) = msg[start + 1..].find('`')
    {
        let field = &msg[start + 1..start + 1 + end];
        return format!("Missing required field: {field}");
    }

    if msg.contains("unknown field")
        && let Some(start) = msg.find('`')
        && let Some(end) = msg[start + 1..].find('`')
    {
        let field = &msg[start + 1..start + 1 + end];
        return format!("Unknown field: {field}");
    }

    if msg.contains("invalid type") {
        return "Invalid data type in request body".to_string();
    }

    if msg.contains("EOF while parsing") || msg.contains("expected") {
        return "Malformed JSON in request body".to_string();
    }

    // Generic fallback that doesn't leak internal details
    "Invalid request format".to_string()
}

/// Convenience type alias for Results with AppError.
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    async fn status_and_body(error: AppError) -> (StatusCode, serde_json::Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// String field `key` of a JSON body; panics (failing the test) if absent.
    fn field<'a>(body: &'a serde_json::Value, key: &str) -> &'a str {
        body.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("missing string field `{key}` in {body}"))
    }

    #[tokio::test]
    async fn test_not_accepted_is_a_retryable_503() {
        let (status, body) = status_and_body(AppError::NotAccepted("x".into())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(field(&body, "error"), "not_accepted");
        assert!(field(&body, "message").contains("safe to retry"));
    }

    #[tokio::test]
    async fn test_outcome_unknown_is_a_503_that_warns_against_resending() {
        let (status, body) = status_and_body(AppError::OutcomeUnknown("x".into())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(field(&body, "error"), "outcome_unknown");
        assert!(field(&body, "message").contains("twice"));
    }

    #[tokio::test]
    async fn test_already_applied_is_a_202_without_an_error_field() {
        let (status, body) = status_and_body(AppError::AlreadyApplied("x".into())).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(field(&body, "status"), "applied");
        assert!(body.get("error").is_none(), "a 2xx must not carry an error");
        assert!(field(&body, "message").contains("Do not resend"));
    }

    #[tokio::test]
    async fn test_incompatible_server_is_a_502_without_internal_details() {
        let (status, body) = status_and_body(AppError::IncompatibleServer(
            "client 0.11 server [0.9, 0.9]".into(),
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(field(&body, "error"), "incompatible_broker");
        assert!(
            !body.to_string().contains("0.11"),
            "internal detail leaked to the client"
        );
    }
}
