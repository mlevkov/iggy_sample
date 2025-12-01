use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Base event structure with common metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Unique event identifier
    pub id: Uuid,
    /// Event type discriminator
    pub event_type: String,
    /// ISO 8601 timestamp of event creation
    pub timestamp: DateTime<Utc>,
    /// Event payload (type-specific data)
    pub payload: EventPayload,
    /// Optional correlation ID for tracing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<Uuid>,
    /// Optional source system identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Event {
    /// Create a new event with the given type and payload.
    pub fn new(event_type: impl Into<String>, payload: EventPayload) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type: event_type.into(),
            timestamp: Utc::now(),
            payload,
            correlation_id: None,
            source: None,
        }
    }

    /// Set correlation ID for distributed tracing.
    pub fn with_correlation_id(mut self, correlation_id: Uuid) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }

    /// Set source system identifier.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Type-safe event payloads using enum variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum EventPayload {
    /// User-related events
    User(UserEvent),
    /// Order-related events
    Order(OrderEvent),
    /// Generic JSON payload for flexibility
    Generic(serde_json::Value),
}

/// User domain events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum UserEvent {
    Created {
        user_id: Uuid,
        email: String,
        name: String,
    },
    Updated {
        user_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Deleted {
        user_id: Uuid,
    },
    LoggedIn {
        user_id: Uuid,
        ip_address: String,
    },
}

/// Order domain events.
///
/// # Monetary Values
///
/// This module uses `rust_decimal::Decimal` for monetary amounts (`total_amount`,
/// `unit_price`) to ensure exact decimal arithmetic without floating-point
/// precision issues. This is the recommended approach for financial calculations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum OrderEvent {
    Created {
        order_id: Uuid,
        user_id: Uuid,
        items: Vec<OrderItem>,
        /// Total order amount using exact decimal representation.
        total_amount: Decimal,
    },
    Updated {
        order_id: Uuid,
        status: OrderStatus,
    },
    Cancelled {
        order_id: Uuid,
        reason: String,
    },
    Shipped {
        order_id: Uuid,
        tracking_number: String,
        carrier: String,
    },
}

/// Order line item.
///
/// Uses `Decimal` for exact monetary representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderItem {
    pub product_id: Uuid,
    pub quantity: u32,
    /// Unit price using exact decimal representation.
    pub unit_price: Decimal,
}

/// Order status enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Pending,
    Confirmed,
    Processing,
    Shipped,
    Delivered,
    Cancelled,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_event_creation() {
        let user_event = UserEvent::Created {
            user_id: Uuid::new_v4(),
            email: "test@example.com".to_string(),
            name: "Test User".to_string(),
        };

        let event = Event::new("user.created", EventPayload::User(user_event));

        assert_eq!(event.event_type, "user.created");
        assert!(event.correlation_id.is_none());
    }

    #[test]
    fn test_event_with_correlation_id() {
        let correlation_id = Uuid::new_v4();
        let event = Event::new("test", EventPayload::Generic(serde_json::json!({})))
            .with_correlation_id(correlation_id);

        assert_eq!(event.correlation_id, Some(correlation_id));
    }

    #[test]
    fn test_event_serialization_roundtrip() {
        let order_event = OrderEvent::Created {
            order_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            items: vec![OrderItem {
                product_id: Uuid::new_v4(),
                quantity: 2,
                unit_price: Decimal::from_str("29.99").unwrap(),
            }],
            total_amount: Decimal::from_str("59.98").unwrap(),
        };

        let event = Event::new("order.created", EventPayload::Order(order_event));
        let json = serde_json::to_string(&event).expect("Serialization should succeed");
        let parsed: Event = serde_json::from_str(&json).expect("Deserialization should succeed");

        assert_eq!(parsed.id, event.id);
        assert_eq!(parsed.event_type, event.event_type);
    }

    #[test]
    fn test_order_status_serialization() {
        let status = OrderStatus::Processing;
        let json = serde_json::to_string(&status).expect("Serialization should succeed");
        assert_eq!(json, "\"processing\"");
    }
}
