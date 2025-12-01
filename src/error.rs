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
}

/// Error response body for API endpoints.
#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Log the full error details server-side for debugging
        // but only expose sanitized messages to clients
        tracing::error!(error = %self, "Request failed");

        let (status, error_type, message) = match &self {
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
