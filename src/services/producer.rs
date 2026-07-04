use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use tracing::{info, instrument};

use crate::error::AppResult;
use crate::iggy_client::IggyClientWrapper;
use crate::models::{Event, EventPayload, SendMessageResponse};

/// Service for producing messages to Iggy streams.
///
/// # Counter Memory Ordering
///
/// The `messages_sent` counter uses `Ordering::Relaxed` because:
/// - It's a monotonically increasing counter used only for metrics
/// - Eventual consistency is acceptable (exact real-time count not required)
/// - No other operations depend on this counter's value for correctness
/// - `Relaxed` has minimal overhead on all architectures
///
/// This differs from `ConnectionState` which uses `SeqCst` because connection
/// state affects control flow and must be immediately visible across threads.
#[derive(Clone)]
pub struct ProducerService {
    client: IggyClientWrapper,
    /// Total messages sent (monotonic counter, eventually consistent).
    messages_sent: Arc<AtomicU64>,
}

impl ProducerService {
    /// Create a new producer service.
    pub fn new(client: IggyClientWrapper) -> Self {
        Self {
            client,
            messages_sent: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Send an event to the default stream and topic.
    #[instrument(skip(self, event), fields(event_id = %event.id))]
    pub async fn send(
        &self,
        event: &Event,
        partition_key: Option<&str>,
    ) -> AppResult<SendMessageResponse> {
        let stream = self.client.default_stream().to_string();
        let topic = self.client.default_topic().to_string();
        self.send_to(&stream, &topic, event, partition_key).await
    }

    /// Send an event to a specific stream and topic.
    #[instrument(skip(self, event), fields(event_id = %event.id))]
    pub async fn send_to(
        &self,
        stream: &str,
        topic: &str,
        event: &Event,
        partition_key: Option<&str>,
    ) -> AppResult<SendMessageResponse> {
        let start = std::time::Instant::now();
        let result = self
            .client
            .send_event(stream, topic, event, partition_key)
            .await;
        crate::metrics::record_send_duration(stream, topic, start.elapsed().as_secs_f64());
        if result.is_err() {
            crate::metrics::record_message_sent(stream, topic, "failure");
        }
        result?;

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        crate::metrics::record_message_sent(stream, topic, "success");

        Ok(SendMessageResponse {
            success: true,
            event_id: event.id,
            stream: stream.to_string(),
            topic: topic.to_string(),
            timestamp: Utc::now(),
        })
    }

    /// Send multiple events in a batch using a single network call.
    /// This is significantly more efficient than individual sends.
    #[instrument(skip(self, events), fields(batch_size = events.len()))]
    pub async fn send_batch(
        &self,
        events: &[Event],
        partition_key: Option<&str>,
    ) -> AppResult<Vec<SendMessageResponse>> {
        let stream = self.client.default_stream().to_string();
        let topic = self.client.default_topic().to_string();
        self.send_batch_to(&stream, &topic, events, partition_key)
            .await
    }

    /// Send multiple events in a batch to a specific stream and topic.
    #[instrument(skip(self, events), fields(batch_size = events.len()))]
    pub async fn send_batch_to(
        &self,
        stream: &str,
        topic: &str,
        events: &[Event],
        partition_key: Option<&str>,
    ) -> AppResult<Vec<SendMessageResponse>> {
        let start = std::time::Instant::now();
        let result = self
            .client
            .send_events_batch(stream, topic, events, partition_key)
            .await;
        crate::metrics::record_send_duration(stream, topic, start.elapsed().as_secs_f64());
        if result.is_err() {
            crate::metrics::record_messages_sent_batch(
                stream,
                topic,
                "failure",
                events.len() as u64,
            );
        }
        result?;

        self.messages_sent
            .fetch_add(events.len() as u64, Ordering::Relaxed);
        crate::metrics::record_messages_sent_batch(stream, topic, "success", events.len() as u64);

        let timestamp = Utc::now();
        // Allocate stream/topic once outside the loop to avoid per-event allocation
        let stream_owned = stream.to_string();
        let topic_owned = topic.to_string();

        let responses = events
            .iter()
            .map(|event| SendMessageResponse {
                success: true,
                event_id: event.id,
                stream: stream_owned.clone(),
                topic: topic_owned.clone(),
                timestamp,
            })
            .collect();

        info!(
            "Sent batch of {} events to {}/{} in a single network call",
            events.len(),
            stream,
            topic
        );
        Ok(responses)
    }

    /// Create and send a generic event with a JSON payload.
    #[instrument(skip(self, payload))]
    pub async fn send_generic(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        partition_key: Option<&str>,
    ) -> AppResult<SendMessageResponse> {
        let event = Event::new(event_type, EventPayload::Generic(payload));
        self.send(&event, partition_key).await
    }

    /// Get the total number of messages sent.
    pub fn messages_sent(&self) -> u64 {
        self.messages_sent.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_producer_messages_counter() {
        // This is a unit test for the counter logic only
        let counter = AtomicU64::new(0);
        counter.fetch_add(1, Ordering::Relaxed);
        counter.fetch_add(1, Ordering::Relaxed);
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }
}
