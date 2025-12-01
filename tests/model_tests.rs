//! Unit tests for domain models.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

// We need to import from the main crate
// Note: These tests can be run with: cargo test --test model_tests

/// Event model module containing types for testing
mod event_tests {
    use super::*;
    use iggy_sample::models::{Event, EventPayload, OrderEvent, OrderItem, OrderStatus, UserEvent};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_event_creation_with_user_payload() {
        let user_id = Uuid::new_v4();
        let user_event = UserEvent::Created {
            user_id,
            email: "test@example.com".to_string(),
            name: "Test User".to_string(),
        };

        let event = Event::new("user.created", EventPayload::User(user_event));

        assert_eq!(event.event_type, "user.created");
        assert!(event.correlation_id.is_none());
        assert!(event.source.is_none());
    }

    #[test]
    fn test_event_with_correlation_id() {
        let correlation_id = Uuid::new_v4();
        let event = Event::new("test", EventPayload::Generic(json!({})))
            .with_correlation_id(correlation_id);

        assert_eq!(event.correlation_id, Some(correlation_id));
    }

    #[test]
    fn test_event_with_source() {
        let event =
            Event::new("test", EventPayload::Generic(json!({}))).with_source("order-service");

        assert_eq!(event.source, Some("order-service".to_string()));
    }

    #[test]
    fn test_event_chaining() {
        let correlation_id = Uuid::new_v4();
        let event = Event::new("test", EventPayload::Generic(json!({})))
            .with_correlation_id(correlation_id)
            .with_source("test-service");

        assert_eq!(event.correlation_id, Some(correlation_id));
        assert_eq!(event.source, Some("test-service".to_string()));
    }

    #[test]
    fn test_user_created_event_serialization() {
        let user_event = UserEvent::Created {
            user_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            email: "user@example.com".to_string(),
            name: "John Doe".to_string(),
        };

        let json = serde_json::to_string(&user_event).expect("Serialization failed");
        assert!(json.contains("\"action\":\"Created\""));
        assert!(json.contains("user@example.com"));
    }

    #[test]
    fn test_user_event_deserialization() {
        let json = r#"{
            "action": "LoggedIn",
            "user_id": "550e8400-e29b-41d4-a716-446655440000",
            "ip_address": "192.168.1.1"
        }"#;

        let user_event: UserEvent = serde_json::from_str(json).expect("Deserialization failed");

        match user_event {
            UserEvent::LoggedIn {
                user_id,
                ip_address,
            } => {
                assert_eq!(ip_address, "192.168.1.1");
                assert_eq!(
                    user_id,
                    Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
                );
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_order_created_event() {
        let order_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        let order_event = OrderEvent::Created {
            order_id,
            user_id,
            items: vec![
                OrderItem {
                    product_id: Uuid::new_v4(),
                    quantity: 2,
                    unit_price: Decimal::from_str("29.99").unwrap(),
                },
                OrderItem {
                    product_id: Uuid::new_v4(),
                    quantity: 1,
                    unit_price: Decimal::from_str("49.99").unwrap(),
                },
            ],
            total_amount: Decimal::from_str("109.97").unwrap(),
        };

        let event = Event::new("order.created", EventPayload::Order(order_event));

        let json = serde_json::to_string(&event).expect("Serialization failed");
        let parsed: Event = serde_json::from_str(&json).expect("Deserialization failed");

        assert_eq!(parsed.event_type, "order.created");
    }

    #[test]
    fn test_order_status_serialization() {
        let statuses = vec![
            (OrderStatus::Pending, "\"pending\""),
            (OrderStatus::Confirmed, "\"confirmed\""),
            (OrderStatus::Processing, "\"processing\""),
            (OrderStatus::Shipped, "\"shipped\""),
            (OrderStatus::Delivered, "\"delivered\""),
            (OrderStatus::Cancelled, "\"cancelled\""),
        ];

        for (status, expected) in statuses {
            let json = serde_json::to_string(&status).expect("Serialization failed");
            assert_eq!(json, expected);
        }
    }

    #[test]
    fn test_order_status_deserialization() {
        let statuses = vec![
            ("\"pending\"", OrderStatus::Pending),
            ("\"shipped\"", OrderStatus::Shipped),
        ];

        for (json, expected) in statuses {
            let status: OrderStatus = serde_json::from_str(json).expect("Deserialization failed");
            assert_eq!(status, expected);
        }
    }

    #[test]
    fn test_generic_payload() {
        let payload = json!({
            "custom_field": "value",
            "nested": {
                "data": [1, 2, 3]
            }
        });

        let event = Event::new("custom.event", EventPayload::Generic(payload));
        let json = serde_json::to_string(&event).expect("Serialization failed");
        let parsed: Event = serde_json::from_str(&json).expect("Deserialization failed");

        match parsed.payload {
            EventPayload::Generic(data) => {
                assert_eq!(
                    data.get("custom_field")
                        .and_then(|v| v.as_str())
                        .expect("custom_field missing"),
                    "value"
                );
                assert_eq!(
                    data.get("nested")
                        .and_then(|v| v.get("data"))
                        .expect("nested.data missing"),
                    &json!([1, 2, 3])
                );
            }
            _ => panic!("Wrong payload type"),
        }
    }

    #[test]
    fn test_event_full_roundtrip() {
        let correlation_id = Uuid::new_v4();
        let user_event = UserEvent::Updated {
            user_id: Uuid::new_v4(),
            email: Some("new@example.com".to_string()),
            name: None,
        };

        let original = Event::new("user.updated", EventPayload::User(user_event))
            .with_correlation_id(correlation_id)
            .with_source("user-service");

        let json = serde_json::to_string(&original).expect("Serialization failed");
        let parsed: Event = serde_json::from_str(&json).expect("Deserialization failed");

        assert_eq!(parsed.id, original.id);
        assert_eq!(parsed.event_type, original.event_type);
        assert_eq!(parsed.correlation_id, original.correlation_id);
        assert_eq!(parsed.source, original.source);
    }
}

/// API model tests
mod api_tests {
    use super::*;
    use iggy_sample::models::{
        CreateStreamRequest, CreateTopicRequest, HealthResponse, PollMessagesRequest, StatsResponse,
    };

    #[test]
    fn test_create_stream_request() {
        let json = r#"{"name": "my-stream"}"#;
        let request: CreateStreamRequest =
            serde_json::from_str(json).expect("Deserialization failed");
        assert_eq!(request.name, "my-stream");
    }

    #[test]
    fn test_create_topic_request_with_defaults() {
        let json = r#"{"name": "my-topic"}"#;
        let request: CreateTopicRequest =
            serde_json::from_str(json).expect("Deserialization failed");
        assert_eq!(request.name, "my-topic");
        assert_eq!(request.partitions, 1);
    }

    #[test]
    fn test_create_topic_request_with_partitions() {
        let json = r#"{"name": "my-topic", "partitions": 5}"#;
        let request: CreateTopicRequest =
            serde_json::from_str(json).expect("Deserialization failed");
        assert_eq!(request.name, "my-topic");
        assert_eq!(request.partitions, 5);
    }

    #[test]
    fn test_poll_messages_request_defaults() {
        let json = r#"{}"#;
        let request: PollMessagesRequest =
            serde_json::from_str(json).expect("Deserialization failed");
        assert_eq!(request.consumer_id, 1);
        assert_eq!(request.count, 10);
        assert!(!request.auto_commit);
        assert!(request.partition_id.is_none());
        assert!(request.offset.is_none());
    }

    #[test]
    fn test_poll_messages_request_custom() {
        let json = r#"{
            "consumer_id": 5,
            "partition_id": 2,
            "offset": 100,
            "count": 50,
            "auto_commit": true
        }"#;
        let request: PollMessagesRequest =
            serde_json::from_str(json).expect("Deserialization failed");
        assert_eq!(request.consumer_id, 5);
        assert_eq!(request.partition_id, Some(2));
        assert_eq!(request.offset, Some(100));
        assert_eq!(request.count, 50);
        assert!(request.auto_commit);
    }

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            status: "healthy".to_string(),
            iggy_connected: true,
            version: "0.1.0".to_string(),
            timestamp: Utc::now(),
        };

        let json = serde_json::to_string(&response).expect("Serialization failed");
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(json.contains("\"iggy_connected\":true"));
        assert!(json.contains("\"version\":\"0.1.0\""));
    }

    #[test]
    fn test_stats_response_serialization() {
        let response = StatsResponse {
            streams_count: 3,
            topics_count: 10,
            total_messages: 1000,
            total_size_bytes: 1024 * 1024,
            uptime_seconds: 3600,
            cache_age_seconds: 2,
            cache_stale: false,
        };

        let json = serde_json::to_string(&response).expect("Serialization failed");
        assert!(json.contains("\"streams_count\":3"));
        assert!(json.contains("\"topics_count\":10"));
        assert!(json.contains("\"total_messages\":1000"));
        assert!(json.contains("\"cache_age_seconds\":2"));
        assert!(json.contains("\"cache_stale\":false"));
    }
}

/// Config module tests
mod config_tests {
    use std::env;

    #[test]
    fn test_server_addr_format() {
        // This test verifies the server address formatting logic
        let host = "127.0.0.1";
        let port = 8080u16;
        let addr = format!("{}:{}", host, port);
        assert_eq!(addr, "127.0.0.1:8080");
    }

    #[test]
    fn test_default_values() {
        // Test default value logic
        let default_stream =
            env::var("IGGY_STREAM").unwrap_or_else(|_| "sample-stream".to_string());

        // When env var is not set, should use default
        if env::var("IGGY_STREAM").is_err() {
            assert_eq!(default_stream, "sample-stream");
        }
    }
}
