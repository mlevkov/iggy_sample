//! Helper functions for the Iggy client.

use iggy::prelude::{Identifier, IggyError};

use crate::error::AppError;

/// Classify an SDK error into a connection-aware `AppError`.
///
/// Connection-flavored `IggyError` variants map to the dedicated connection
/// variants so `IggyClientWrapper::with_reconnect` can trigger reconnection
/// and record circuit-breaker failures; everything else maps through
/// `fallback` (e.g. `AppError::SendError`). Without this classification the
/// reconnect path could never fire: stringifying every SDK error into an
/// operation error hides the connection failures from `is_connection_error`.
pub fn classify_iggy_error(error: IggyError, fallback: fn(String) -> AppError) -> AppError {
    match error {
        IggyError::Disconnected
        | IggyError::NotConnected
        | IggyError::StaleClient
        | IggyError::ClientShutdown => AppError::Disconnected(error.to_string()),
        // Connection-flavored variants across ALL transports the connection
        // string can select (TCP, QUIC, HTTP, WebSocket) - classifying only
        // the TCP set would leave the reconnect path dead code on the other
        // three. EmptyResponse is in the SDK's own internal reconnect-trigger
        // list. HttpResponseError (a response WITH an error status) is
        // deliberately NOT here: the server answered, that is an application
        // error.
        IggyError::ConnectionClosed
        | IggyError::TcpError
        | IggyError::QuicError
        | IggyError::EmptyResponse
        | IggyError::HttpError(_)
        | IggyError::WebSocketError
        | IggyError::WebSocketConnectionError
        | IggyError::WebSocketCloseError
        | IggyError::WebSocketReceiveError
        | IggyError::WebSocketSendError => AppError::ConnectionReset(error.to_string()),
        IggyError::CannotEstablishConnection => AppError::ConnectionFailed(error.to_string()),
        other => fallback(other.to_string()),
    }
}

/// Convert a resource name to a name-based (string) Identifier.
///
/// Uses `Identifier::named` explicitly rather than `str::try_into`: the
/// `FromStr` conversion reinterprets any all-digit string (e.g. `"42"`) as a
/// NUMERIC server-assigned ID, which would silently target a different
/// resource than the name the caller asked for. All lookups in this service
/// are name-based.
///
/// The SDK only enforces the 1-255 byte length here; the alphanumeric charset
/// invariant is enforced separately by `validation::validate_resource_name`
/// at the HTTP boundary.
pub fn to_identifier(name: &str, resource_type: &str) -> Result<Identifier, AppError> {
    Identifier::named(name).map_err(|e: IggyError| {
        // Log the original error for debugging while providing a user-friendly message
        tracing::debug!(
            resource_type,
            name,
            original_error = %e,
            "Identifier conversion failed"
        );
        AppError::BadRequest(format!(
            "Invalid {} name '{}': must be 1-255 characters",
            resource_type, name
        ))
    })
}

/// Generate a random jitter value between 0.0 and 1.0.
///
/// Uses the `rand` crate's thread-local RNG for proper randomness.
/// This prevents predictable backoff patterns that could lead to
/// thundering herd problems when multiple clients reconnect simultaneously.
pub fn rand_jitter() -> f64 {
    rand::random::<f64>()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_to_identifier_valid_names() {
        assert!(to_identifier("my-stream", "stream").is_ok());
        assert!(to_identifier("my_topic", "topic").is_ok());
        assert!(to_identifier("stream.v2", "stream").is_ok());
        assert!(to_identifier("a", "stream").is_ok());
        assert!(to_identifier("test123", "topic").is_ok());
    }

    #[test]
    fn test_to_identifier_empty_name() {
        let result = to_identifier("", "stream");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid stream name")
        );
    }

    #[test]
    fn test_to_identifier_too_long() {
        let long_name = "a".repeat(300);
        let result = to_identifier(&long_name, "topic");
        assert!(result.is_err());
    }

    #[test]
    fn test_to_identifier_all_digit_name_stays_a_name() {
        // "42" must be a NAMED identifier targeting the stream called "42",
        // not the numeric server-assigned ID 42 (which could be a different
        // resource entirely - dangerous on DELETE).
        let id = to_identifier("42", "stream").expect("all-digit name should be valid");
        let named = Identifier::named("42").expect("named identifier");
        assert_eq!(id, named);

        let numeric = Identifier::numeric(42).expect("numeric identifier");
        assert_ne!(id, numeric);
    }

    #[test]
    fn test_classify_disconnected_variants() {
        for error in [
            IggyError::Disconnected,
            IggyError::NotConnected,
            IggyError::StaleClient,
            IggyError::ClientShutdown,
        ] {
            let classified = classify_iggy_error(error, AppError::SendError);
            assert!(
                matches!(classified, AppError::Disconnected(_)),
                "expected Disconnected, got {:?}",
                classified
            );
        }
    }

    #[test]
    fn test_classify_connection_reset_variants() {
        for error in [
            IggyError::ConnectionClosed,
            IggyError::TcpError,
            IggyError::QuicError,
            IggyError::EmptyResponse,
            IggyError::HttpError("connection refused".to_string()),
            IggyError::WebSocketError,
            IggyError::WebSocketConnectionError,
            IggyError::WebSocketCloseError,
            IggyError::WebSocketReceiveError,
            IggyError::WebSocketSendError,
        ] {
            let classified = classify_iggy_error(error, AppError::SendError);
            assert!(
                matches!(classified, AppError::ConnectionReset(_)),
                "expected ConnectionReset, got {:?}",
                classified
            );
        }
    }

    #[test]
    fn test_classify_http_response_error_is_not_a_connection_error() {
        // The server ANSWERED with an error status - reconnecting would be
        // wrong; it must map through the fallback.
        let classified = classify_iggy_error(
            IggyError::HttpResponseError(500, "boom".to_string()),
            AppError::SendError,
        );
        assert!(matches!(classified, AppError::SendError(_)));
    }

    #[test]
    fn test_classify_cannot_establish_connection() {
        let classified =
            classify_iggy_error(IggyError::CannotEstablishConnection, AppError::SendError);
        assert!(matches!(classified, AppError::ConnectionFailed(_)));
    }

    #[test]
    fn test_classify_non_connection_error_uses_fallback() {
        let classified = classify_iggy_error(
            IggyError::StreamNameAlreadyExists("test".to_string()),
            AppError::StreamError,
        );
        assert!(matches!(classified, AppError::StreamError(_)));

        let classified = classify_iggy_error(IggyError::InvalidMessagesCount, AppError::PollError);
        assert!(matches!(classified, AppError::PollError(_)));
    }

    #[test]
    fn test_rand_jitter_returns_value_in_range() {
        for _ in 0..100 {
            let jitter = rand_jitter();
            assert!(jitter >= 0.0, "jitter {} should be >= 0.0", jitter);
            assert!(jitter < 1.0, "jitter {} should be < 1.0", jitter);
        }
    }

    #[test]
    fn test_rand_jitter_produces_different_values() {
        let values: Vec<f64> = (0..10).map(|_| rand_jitter()).collect();

        // At least some values should be different (extremely unlikely all same)
        // Use pattern matching to safely access window elements
        let all_same = values.windows(2).all(|w| {
            match (w.first(), w.get(1)) {
                (Some(&a), Some(&b)) => (a - b).abs() < f64::EPSILON,
                _ => true, // Shouldn't happen with windows(2)
            }
        });
        assert!(!all_same, "rand_jitter should produce varying values");
    }
}
