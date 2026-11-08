//! Connectors demo tests (ADR-0001 PR 4).
//!
//! Ignored by default: they need the stack from `docker-compose.yaml` plus
//! `docker-compose.connectors.yaml` running, and the docker CLI on the host
//! to read the webhook secrets connectors-init generated. The weekly
//! Extended Tests workflow runs them, then `scripts/connectors-demo.sh`:
//!
//! ```bash
//! docker compose -f docker-compose.yaml -f docker-compose.connectors.yaml up -d --build --wait
//! cargo test --test connectors_tests -- --ignored --test-threads=1
//! ```
//!
//! `GATEWAY_URL` (`http://localhost:8000`), `WEBHOOK_URL`
//! (`http://localhost:8085`) and `PROMETHEUS_URL` (`http://localhost:9090`)
//! override the addresses. The tests read offsets from the shared webhook
//! topic, so they run one at a time.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::future::Future;
use std::process::Command;
use std::time::{Duration, Instant};

use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use tokio::time::sleep;

const CONNECTORS_CONTAINER: &str = "iggy-connectors";
const WEBHOOK_TOPIC: &str = "/streams/sample-stream/topics/webhooks";

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn gateway() -> String {
    env_or("GATEWAY_URL", "http://localhost:8000")
}

fn webhook_base() -> String {
    env_or("WEBHOOK_URL", "http://localhost:8085")
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP client")
}

/// A secret connectors-init generated, read inside the connectors container.
fn secret(name: &str) -> String {
    let output = Command::new("docker")
        .args([
            "exec",
            CONNECTORS_CONTAINER,
            "cat",
            &format!("/run/secrets/connectors/{name}"),
        ])
        .output()
        .expect("docker CLI");
    assert!(
        output.status.success(),
        "reading {name} from {CONNECTORS_CONTAINER} failed: is the connectors overlay running?"
    );
    String::from_utf8(output.stdout)
        .expect("UTF-8")
        .trim()
        .to_string()
}

/// HMAC-SHA256 of `body` under `key`, hex, as the source's endpoint expects.
fn sign(key: &str, body: &str) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    hex::encode(ring::hmac::sign(&key, body.as_bytes()).as_ref())
}

fn event(id: &str, event_type: &str) -> Value {
    json!({
        "id": id,
        "event_type": event_type,
        "timestamp": "2026-11-08T00:00:00Z",
        "payload": {"type": "Generic", "data": {"source": "connectors-tests"}}
    })
}

fn unique_id(tag: u64) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    // A UUID-shaped id, unique per run and call.
    let hex = format!("{:032x}", nanos ^ (u128::from(tag) << 96));
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

async fn get_json(path: &str) -> Option<Value> {
    let response = client()
        .get(format!("{}{path}", gateway()))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

/// Poll `check` every half second until it yields a value, or panic after
/// `budget` with `what`.
async fn wait_for<T, F, Fut>(budget: Duration, what: &str, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(value) = check().await {
            return value;
        }
        sleep(Duration::from_millis(500)).await;
    }
    panic!("{what} within {budget:?}");
}

/// POST a webhook to the endpoint, waiting out the 404s a fresh source
/// answers until its poll task publishes its routes (a 404 stores nothing).
async fn post_webhook(path: &str, body: &str, signature: Option<&str>) -> (StatusCode, String) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut request = client()
            .post(format!("{}{path}", webhook_base()))
            .header("content-type", "application/json")
            .header("x-webhook-delivery", "connectors-tests")
            .body(body.to_string());
        if let Some(signature) = signature {
            request = request.header("x-signature-256", format!("sha256={signature}"));
        }
        let response = request.send().await.expect("webhook request");
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status != StatusCode::NOT_FOUND || Instant::now() >= deadline {
            return (status, text);
        }
        sleep(Duration::from_millis(500)).await;
    }
}

/// Messages in the webhook topic's only partition, or 0 before it exists.
async fn webhook_topic_messages() -> u64 {
    get_json(WEBHOOK_TOPIC)
        .await
        .and_then(|topic| topic.pointer("/partitions/0/messages_count")?.as_u64())
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "needs the docker-compose.connectors.yaml stack"]
async fn test_signed_webhook_is_stored_and_readable_through_the_gateway() {
    let endpoint = format!("/e/{}", secret("webhook_endpoint_id"));
    let hmac_secret = secret("webhook_hmac_secret");
    let start_offset = webhook_topic_messages().await;
    let id = unique_id(1);
    let body = event(&id, "webhook.received").to_string();

    let (status, answer) = post_webhook(&endpoint, &body, Some(&sign(&hmac_secret, &body))).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        serde_json::from_str::<Value>(&answer).ok(),
        Some(json!({"status": "queued"}))
    );

    let message = wait_for(
        Duration::from_secs(30),
        "the webhook was not readable through the gateway",
        || async {
            let poll = get_json(&format!(
                "{WEBHOOK_TOPIC}/messages?partition_id=0&offset={start_offset}&count=100"
            ))
            .await?;
            poll.get("messages")?
                .as_array()?
                .iter()
                .find(|message| message.pointer("/event/id") == Some(&json!(id)))
                .cloned()
        },
    )
    .await;
    assert_eq!(
        message.pointer("/event/event_type"),
        Some(&json!("webhook.received"))
    );

    // The runtime created the topic durable: the HTTP source writes to
    // nothing else.
    let topic = get_json(WEBHOOK_TOPIC).await.expect("webhook topic");
    assert_eq!(
        topic.pointer("/options/durability/value"),
        Some(&json!("persisted")),
        "{topic}"
    );
}

#[tokio::test]
#[ignore = "needs the docker-compose.connectors.yaml stack"]
async fn test_webhook_with_a_bad_signature_or_unknown_endpoint_is_refused() {
    let endpoint = format!("/e/{}", secret("webhook_endpoint_id"));
    let hmac_secret = secret("webhook_hmac_secret");
    let body = event(&unique_id(2), "webhook.forged").to_string();

    // Signed with another key, signed over another body, unsigned.
    let (status, _) = post_webhook(&endpoint, &body, Some(&sign("not-the-secret", &body))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post_webhook(
        &endpoint,
        &body,
        Some(&sign(&hmac_secret, &format!("{body} "))),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post_webhook(&endpoint, &body, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A path that is not the secret id answers 404, like a revoked one.
    let response = client()
        .post(format!("{}/e/{}", webhook_base(), "0".repeat(32)))
        .body(body.clone())
        .send()
        .await
        .expect("webhook request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The webhook topic takes any body; the gateway's poll API reads Events,
/// and counts what it leaves out.
#[tokio::test]
#[ignore = "needs the docker-compose.connectors.yaml stack"]
async fn test_a_webhook_that_is_not_an_event_is_counted_as_skipped() {
    let endpoint = format!("/e/{}", secret("webhook_endpoint_id"));
    let hmac_secret = secret("webhook_hmac_secret");
    let start_offset = webhook_topic_messages().await;
    let body = json!({"zen": "Design for failure.", "hook_id": 7}).to_string();

    let (status, answer) = post_webhook(&endpoint, &body, Some(&sign(&hmac_secret, &body))).await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    wait_for(
        Duration::from_secs(30),
        "the webhook did not reach the topic",
        || async { (webhook_topic_messages().await > start_offset).then_some(()) },
    )
    .await;
    let poll = get_json(&format!(
        "{WEBHOOK_TOPIC}/messages?partition_id=0&offset={start_offset}&count=1"
    ))
    .await
    .expect("poll");
    assert_eq!(poll.get("count"), Some(&json!(0)), "{poll}");
    assert_eq!(poll.get("skipped"), Some(&json!(1)), "{poll}");
}

/// Three events, which the gateway spreads over the topic's three
/// partitions: with SDK 0.12.0, a consumer group that starts on a topic whose
/// partitions are all empty holds the first message until a second partition
/// receives one (docs/connectors-demo.md, Known issues).
#[tokio::test]
#[ignore = "needs the docker-compose.connectors.yaml stack"]
async fn test_events_sent_through_the_gateway_come_back_through_the_sink() {
    let ids: Vec<String> = (10..13).map(unique_id).collect();
    for id in &ids {
        let response = client()
            .post(format!("{}/messages", gateway()))
            .json(&json!({"event": event(id, "sink.roundtrip")}))
            .send()
            .await
            .expect("send");
        assert!(response.status().is_success(), "{}", response.status());
    }

    let deliveries = wait_for(
        Duration::from_secs(30),
        "the HTTP sink did not deliver all three events back to the gateway",
        || async {
            let inbox = get_json("/sink/inbound").await?;
            let recent = inbox.get("recent")?.as_array()?;
            let found: Vec<Value> = ids
                .iter()
                .filter_map(|id| {
                    recent
                        .iter()
                        .find(|delivery| delivery.pointer("/payload/id") == Some(&json!(id)))
                        .cloned()
                })
                .collect();
            (found.len() == ids.len()).then_some(found)
        },
    )
    .await;

    for delivery in &deliveries {
        assert_eq!(delivery.get("stream"), Some(&json!("sample-stream")));
        assert_eq!(delivery.get("topic"), Some(&json!("events")));
        assert_eq!(
            delivery.pointer("/payload/event_type"),
            Some(&json!("sink.roundtrip"))
        );
        let iggy_id = delivery.get("id").and_then(Value::as_str).unwrap_or("");
        assert_eq!(iggy_id.len(), 32, "{delivery}");
    }
}

#[tokio::test]
#[ignore = "needs the docker-compose.connectors.yaml stack"]
async fn test_sink_receiver_refuses_a_request_without_the_token() {
    let before = get_json("/sink/inbound").await.expect("sink inbox");
    let response = client()
        .post(format!("{}/sink/inbound", gateway()))
        .header("content-type", "application/x-ndjson")
        .body("{}\n")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let after = get_json("/sink/inbound").await.expect("sink inbox");
    let count = |inbox: &Value| inbox.get("unauthorized_requests").and_then(Value::as_u64);
    assert_eq!(count(&after), count(&before).map(|n| n + 1));
}

/// Prometheus reaches the runtime's /metrics with the generated api-key and
/// the source's admin metrics on the compose network.
#[tokio::test]
#[ignore = "needs the docker-compose.connectors.yaml stack"]
async fn test_prometheus_scrapes_the_runtime_and_the_webhook_listener() {
    let prometheus = env_or("PROMETHEUS_URL", "http://localhost:9090");
    for job in ["iggy-connectors", "iggy-connectors-webhooks"] {
        wait_for(
            Duration::from_secs(90),
            &format!("Prometheus did not report job {job} up"),
            || async {
                let response = client()
                    .get(format!("{prometheus}/api/v1/query"))
                    .query(&[("query", format!("up{{job=\"{job}\"}}"))])
                    .send()
                    .await
                    .ok()?;
                let body: Value = response.json().await.ok()?;
                let value = body.pointer("/data/result/0/value/1")?.as_str()?;
                (value == "1").then_some(())
            },
        )
        .await;
    }
}
