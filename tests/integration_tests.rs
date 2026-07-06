//! Integration tests for the Iggy Sample application using testcontainers.
//!
//! These tests automatically spin up an Iggy container, start the application,
//! and run end-to-end tests. No manual setup required.
//!
//! Run with: `cargo test --test integration_tests`
//!
//! # Reconnection Testing
//!
//! Reconnection logic is tested via unit tests in `src/iggy_client/`.
//! Full integration reconnection testing (container stop/start) is complex
//! and may result in flaky tests. The reconnection implementation uses:
//!
//! - Exponential backoff with proper jitter (rand crate)
//! - Configurable retry limits and delays
//! - Thread-safe state tracking with SeqCst atomics
//!
//! For manual reconnection testing:
//! 1. Start the app with Iggy
//! 2. Stop the Iggy container
//! 3. Observe reconnection attempts in logs
//! 4. Restart Iggy and verify recovery
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::time::Duration;

use reqwest::Client;
use serde_json::json;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::time::sleep;

/// Iggy container configuration
struct IggyContainer {
    tcp_port: u16,
}

impl IggyContainer {
    // Server release paired with the Rust SDK 0.10 line (see Cargo.toml)
    const IMAGE: &'static str = "apache/iggy";
    const TAG: &'static str = "0.8.0";
    const TCP_PORT: u16 = 8090;

    /// Start an Iggy container and return the mapped TCP port
    async fn start() -> (ContainerAsync<GenericImage>, Self) {
        // Note: GenericImage methods (with_exposed_port, with_wait_for) must come before
        // ImageExt methods (with_env_var, with_privileged) due to type transformations
        let container = GenericImage::new(Self::IMAGE, Self::TAG)
            .with_exposed_port(Self::TCP_PORT.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Iggy server is running"))
            // Root credentials (the server generates a random password by default)
            .with_env_var("IGGY_ROOT_USERNAME", "iggy")
            .with_env_var("IGGY_ROOT_PASSWORD", "iggy")
            // Configure server to bind to 0.0.0.0 (accessible from host)
            .with_env_var("IGGY_TCP_ADDRESS", "0.0.0.0:8090")
            .with_env_var("IGGY_HTTP_ADDRESS", "0.0.0.0:3000")
            .with_env_var("IGGY_QUIC_ADDRESS", "0.0.0.0:8080")
            .with_startup_timeout(Duration::from_secs(120))
            .with_privileged(true)
            .start()
            .await
            .expect("Failed to start Iggy container");

        // Give the server a moment to fully initialize all shards
        sleep(Duration::from_secs(3)).await;

        let tcp_port = container
            .get_host_port_ipv4(Self::TCP_PORT)
            .await
            .expect("Failed to get Iggy TCP port");

        (container, Self { tcp_port })
    }

    /// Get the connection string for this container
    fn connection_string(&self) -> String {
        format!("iggy://iggy:iggy@127.0.0.1:{}", self.tcp_port)
    }
}

/// Find an available port for the test server
fn find_available_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("Failed to bind to ephemeral port")
        .local_addr()
        .expect("Failed to get local address")
        .port()
}

/// Test fixture that manages the Iggy container and app server
struct TestFixture {
    _iggy_container: ContainerAsync<GenericImage>,
    base_url: String,
    client: Client,
}

impl TestFixture {
    /// Create a new test fixture with Iggy container and app server
    async fn new() -> Self {
        // Start Iggy container
        let (iggy_container, iggy) = IggyContainer::start().await;

        // Find available port for our app
        let app_port = find_available_port();
        let base_url = format!("http://127.0.0.1:{}", app_port);

        // Start the application server in background
        let connection_string = iggy.connection_string();
        let connection_string_clone = connection_string.clone();

        // Use a channel to communicate server startup status
        let (tx, mut rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

        tokio::spawn(async move {
            match Self::start_server(app_port, &connection_string_clone).await {
                Ok(()) => {}
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        });

        // Wait for server to be ready
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        Self::wait_for_server(&client, &base_url, &mut rx).await;

        Self {
            _iggy_container: iggy_container,
            base_url,
            client,
        }
    }

    /// Start the application server
    async fn start_server(port: u16, iggy_connection_string: &str) -> Result<(), String> {
        use std::time::Duration;

        use iggy_sample::{AppState, Config, IggyClientWrapper, build_router};
        use tokio::net::TcpListener;

        let config = Config {
            // Server configuration
            host: "127.0.0.1".to_string(),
            port,
            // Iggy connection configuration
            iggy_connection_string: iggy_connection_string.to_string(),
            default_stream: "test-stream".to_string(),
            default_topic: "test-events".to_string(),
            topic_partitions: 2,
            // Connection resilience (relaxed for tests)
            max_reconnect_attempts: 3,
            reconnect_base_delay: Duration::from_millis(100),
            reconnect_max_delay: Duration::from_secs(1),
            health_check_interval: Duration::from_secs(30),
            operation_timeout: Duration::from_secs(30),
            // Circuit breaker (default settings for tests)
            circuit_breaker_failure_threshold: 5,
            circuit_breaker_success_threshold: 2,
            circuit_breaker_open_duration: Duration::from_secs(30),
            // Rate limiting (disabled for tests)
            rate_limit_rps: 0,
            rate_limit_burst: 50,
            // Message limits
            batch_max_size: 1000,
            poll_max_count: 100,
            max_request_body_size: 10 * 1024 * 1024, // 10MB
            // Security (disabled for tests)
            api_key: None,
            auth_bypass_paths: vec!["/health".to_string(), "/ready".to_string()],
            cors_allowed_origins: vec!["*".to_string()],
            trusted_proxies: vec![], // Empty = trust all (test mode)
            // Observability
            log_level: "warn".to_string(),
            stats_cache_ttl: Duration::from_secs(5),
            metrics_port: 0, // Disabled for tests
        };

        let iggy_client = IggyClientWrapper::new(config.clone())
            .await
            .map_err(|e| format!("Failed to create Iggy client: {}", e))?;

        iggy_client
            .initialize_defaults()
            .await
            .map_err(|e| format!("Failed to initialize defaults: {}", e))?;

        let state = AppState::new(iggy_client, config);
        let app = build_router(state).map_err(|e| format!("Failed to build router: {}", e))?;

        let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
            .await
            .map_err(|e| format!("Failed to bind server: {}", e))?;

        // Match production: ConnectInfo exposes the peer address for
        // TRUSTED_PROXIES enforcement in the middleware stack.
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .map_err(|e| format!("Server failed: {}", e))?;

        Ok(())
    }

    /// Wait for the server to become ready
    async fn wait_for_server(
        client: &Client,
        base_url: &str,
        error_rx: &mut tokio::sync::oneshot::Receiver<Result<(), String>>,
    ) {
        let health_url = format!("{}/health", base_url);
        let max_attempts = 60;

        for attempt in 1..=max_attempts {
            // Check if server task failed
            match error_rx.try_recv() {
                Ok(Err(e)) => panic!("Server failed to start: {}", e),
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    // The server task dropped its sender without reporting an
                    // error: it panicked or returned early. Surfacing this
                    // immediately beats 30s of polling a dead server followed
                    // by a generic timeout message that hides the real cause.
                    panic!("Server task exited (panic or early return) before becoming ready");
                }
                Ok(Ok(())) | Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }

            match client.get(&health_url).send().await {
                Ok(response) if response.status().is_success() => {
                    return;
                }
                Ok(response) => {
                    if attempt == max_attempts {
                        panic!(
                            "Server returned non-success status after {} attempts: {}",
                            max_attempts,
                            response.status()
                        );
                    }
                }
                Err(_) => {
                    if attempt == max_attempts {
                        panic!("Server failed to respond after {} attempts", max_attempts);
                    }
                }
            }
            sleep(Duration::from_millis(500)).await;
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

// ============================================================================
// Health & Status Tests
// ============================================================================

#[tokio::test]
async fn test_health_endpoint() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .client
        .get(fixture.url("/health"))
        .send()
        .await
        .expect("Health request failed");

    assert!(response.status().is_success());

    let body: serde_json::Value = response.json().await.expect("Failed to parse response");
    assert_eq!(
        body.get("status")
            .and_then(|v| v.as_str())
            .expect("status missing"),
        "healthy"
    );
    assert!(
        body.get("iggy_connected")
            .and_then(|v| v.as_bool())
            .expect("iggy_connected missing")
    );
    assert!(body.get("version").is_some());
    assert!(body.get("timestamp").is_some());
}

#[tokio::test]
async fn test_readiness_endpoint() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .client
        .get(fixture.url("/ready"))
        .send()
        .await
        .expect("Readiness request failed");

    assert!(response.status().is_success());
}

#[tokio::test]
async fn test_stats_endpoint() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .client
        .get(fixture.url("/stats"))
        .send()
        .await
        .expect("Stats request failed");

    assert!(response.status().is_success());

    let body: serde_json::Value = response.json().await.expect("Failed to parse response");
    assert!(body.get("streams_count").is_some());
    assert!(body.get("topics_count").is_some());
    assert!(body.get("uptime_seconds").is_some());
}

// ============================================================================
// Message Tests
// ============================================================================

/// Build a generic test event payload with the given marker message.
fn generic_event(marker: &str, partition_key: Option<&str>) -> serde_json::Value {
    let event = json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "event_type": "test.semantics",
        "timestamp": "2026-07-03T10:30:00Z",
        "payload": {
            "type": "Generic",
            "data": { "message": marker }
        }
    });
    match partition_key {
        Some(key) => json!({ "event": event, "partition_key": key }),
        None => json!({ "event": event }),
    }
}

/// Poll the given queries repeatedly (idempotent offset-0 reads) until their
/// combined count reaches `expected` or a 10s deadline elapses; returns the
/// last per-query counts. Replaces fixed sleeps so slow send-to-poll
/// visibility under CI load retries instead of flaking.
async fn wait_for_total(fixture: &TestFixture, queries: &[String], expected: u64) -> Vec<u64> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let mut counts = Vec::with_capacity(queries.len());
        for q in queries {
            counts.push(poll_count(fixture, q).await);
        }
        if counts.iter().sum::<u64>() >= expected || tokio::time::Instant::now() >= deadline {
            return counts;
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// Poll one partition and return the number of messages.
async fn poll_count(fixture: &TestFixture, query: &str) -> u64 {
    let response = fixture
        .client
        .get(fixture.url(&format!("/messages?{}", query)))
        .send()
        .await
        .expect("Poll request failed");
    assert!(
        response.status().is_success(),
        "poll '{}' failed: {}",
        query,
        response.status()
    );
    let body: serde_json::Value = response.json().await.expect("Failed to parse poll body");
    body.get("count").and_then(|v| v.as_u64()).expect("count")
}

/// Pins the SDK-0.10 semantics of PollingStrategy::next + auto_commit: a poll
/// without an explicit offset resumes from the consumer's committed offset,
/// so a second poll must not re-deliver the first poll's messages.
#[tokio::test]
async fn test_poll_without_offset_advances_committed_offset() {
    let fixture = TestFixture::new().await;

    for i in 0..3 {
        let response = fixture
            .client
            .post(fixture.url("/messages"))
            .json(&generic_event(&format!("offset-semantics-{i}"), None))
            .send()
            .await
            .expect("Send request failed");
        assert!(response.status().is_success());
    }

    // Wait for visibility with idempotent offset-0 reads (consumer 99, no
    // commit) so the committed-offset rounds below are deterministic under
    // CI load.
    let visible = wait_for_total(
        &fixture,
        &[
            "partition_id=0&consumer_id=99&count=10&offset=0".to_string(),
            "partition_id=1&consumer_id=99&count=10&offset=0".to_string(),
        ],
        3,
    )
    .await;
    assert_eq!(
        visible.iter().sum::<u64>(),
        3,
        "messages never became visible"
    );

    // First round: no offset param (PollingStrategy::next) + auto_commit,
    // across both partitions of the 2-partition test topic.
    let first: u64 = poll_count(
        &fixture,
        "partition_id=0&consumer_id=77&count=10&auto_commit=true",
    )
    .await
        + poll_count(
            &fixture,
            "partition_id=1&consumer_id=77&count=10&auto_commit=true",
        )
        .await;
    assert_eq!(
        first, 3,
        "first next-poll round should deliver all messages"
    );

    // Second round: offsets were committed, nothing may be re-delivered.
    let second: u64 = poll_count(
        &fixture,
        "partition_id=0&consumer_id=77&count=10&auto_commit=true",
    )
    .await
        + poll_count(
            &fixture,
            "partition_id=1&consumer_id=77&count=10&auto_commit=true",
        )
        .await;
    assert_eq!(
        second, 0,
        "second next-poll round re-delivered committed messages"
    );
}

/// Pins the SDK-0.10 semantics of Partitioning::messages_key_str: all
/// messages sharing a partition key must land in exactly one partition
/// (per-entity ordering guarantee).
#[tokio::test]
async fn test_same_partition_key_routes_to_single_partition() {
    let fixture = TestFixture::new().await;

    for i in 0..5 {
        let response = fixture
            .client
            .post(fixture.url("/messages"))
            .json(&generic_event(
                &format!("routing-{i}"),
                Some("routing-invariant-key"),
            ))
            .send()
            .await
            .expect("Send request failed");
        assert!(response.status().is_success());
    }
    let counts = wait_for_total(
        &fixture,
        &[
            "partition_id=0&consumer_id=88&count=50&offset=0".to_string(),
            "partition_id=1&consumer_id=88&count=50&offset=0".to_string(),
        ],
        5,
    )
    .await;
    let (p0, p1) = match counts.as_slice() {
        [p0, p1] => (*p0, *p1),
        other => panic!("expected two partition counts, got {other:?}"),
    };

    assert_eq!(p0 + p1, 5, "all keyed messages should be delivered");
    assert!(
        (p0 == 5 && p1 == 0) || (p0 == 0 && p1 == 5),
        "messages with one partition key split across partitions (p0={p0}, p1={p1})"
    );
}

/// Both poll routes must reject count=0 with 400 at the HTTP boundary
/// (the SDK would otherwise fail it with a misleading 500).
#[tokio::test]
async fn test_poll_count_zero_returns_400_on_both_routes() {
    let fixture = TestFixture::new().await;

    for path in [
        "/messages?count=0",
        "/streams/test-stream/topics/test-events/messages?count=0",
    ] {
        let response = fixture
            .client
            .get(fixture.url(path))
            .send()
            .await
            .expect("Request failed");
        assert_eq!(
            response.status().as_u16(),
            400,
            "count=0 on {} should be a client error",
            path
        );
    }
}

/// Pins initialize_defaults idempotence (the ensure_* already-exists arms)
/// and the live health_check() true path against a real server.
#[tokio::test]
async fn test_wrapper_defaults_idempotent_and_health_check_live() {
    use iggy_sample::{Config, IggyClientWrapper};

    let (_container, iggy) = IggyContainer::start().await;

    let config = Config {
        iggy_connection_string: iggy.connection_string(),
        default_stream: "idempotence-stream".to_string(),
        default_topic: "idempotence-events".to_string(),
        topic_partitions: 2,
        operation_timeout: Duration::from_secs(30),
        metrics_port: 0,
        ..Config::default()
    };

    let wrapper = IggyClientWrapper::new(config)
        .await
        .expect("wrapper should connect");

    // First call creates; second call must take the already-exists arms
    // without error (two replicas starting concurrently must not crash-loop).
    wrapper
        .initialize_defaults()
        .await
        .expect("first initialize_defaults");
    wrapper
        .initialize_defaults()
        .await
        .expect("second initialize_defaults (idempotence)");

    // Live health probe: ping succeeds and drives the connected flag.
    assert!(wrapper.health_check().await, "live ping should succeed");
    assert!(wrapper.is_connected());
}

#[tokio::test]
async fn test_send_and_poll_message() {
    let fixture = TestFixture::new().await;

    // Create an event payload
    let event = json!({
        "event": {
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "event_type": "test.event",
            "timestamp": "2024-01-15T10:30:00Z",
            "payload": {
                "type": "Generic",
                "data": {
                    "message": "Hello from integration test",
                    "value": 42
                }
            }
        }
    });

    // Send the message
    let send_response = fixture
        .client
        .post(fixture.url("/messages"))
        .json(&event)
        .send()
        .await
        .expect("Send request failed");

    if !send_response.status().is_success() {
        let status = send_response.status();
        let body = send_response.text().await.unwrap_or_default();
        panic!("Send failed with status: {} - Body: {}", status, body);
    }

    let send_body: serde_json::Value = send_response
        .json()
        .await
        .expect("Failed to parse send response");
    assert!(
        send_body
            .get("success")
            .and_then(|v| v.as_bool())
            .expect("success missing")
    );
    assert!(send_body.get("event_id").is_some());

    // Small delay to ensure message is persisted
    sleep(Duration::from_millis(200)).await;

    // Poll messages
    let poll_response = fixture
        .client
        .get(fixture.url("/messages?partition_id=1&count=10&offset=0"))
        .send()
        .await
        .expect("Poll request failed");

    assert!(poll_response.status().is_success());

    let poll_body: serde_json::Value = poll_response
        .json()
        .await
        .expect("Failed to parse poll response");
    assert!(poll_body.get("messages").is_some());
    assert!(poll_body.get("count").is_some());
}

#[tokio::test]
async fn test_send_user_event() {
    let fixture = TestFixture::new().await;

    let event = json!({
        "event": {
            "id": "550e8400-e29b-41d4-a716-446655440001",
            "event_type": "user.created",
            "timestamp": "2024-01-15T10:30:00Z",
            "payload": {
                "type": "User",
                "data": {
                    "action": "Created",
                    "user_id": "550e8400-e29b-41d4-a716-446655440002",
                    "email": "test@example.com",
                    "name": "Test User"
                }
            }
        }
    });

    let response = fixture
        .client
        .post(fixture.url("/messages"))
        .json(&event)
        .send()
        .await
        .expect("Send request failed");

    assert!(response.status().is_success());

    let body: serde_json::Value = response.json().await.expect("Failed to parse response");
    assert!(
        body.get("success")
            .and_then(|v| v.as_bool())
            .expect("success missing")
    );
}

#[tokio::test]
async fn test_send_batch_messages() {
    let fixture = TestFixture::new().await;

    let batch = json!({
        "events": [
            {
                "id": "550e8400-e29b-41d4-a716-446655440010",
                "event_type": "batch.event.1",
                "timestamp": "2024-01-15T10:30:00Z",
                "payload": {"type": "Generic", "data": {"index": 1}}
            },
            {
                "id": "550e8400-e29b-41d4-a716-446655440011",
                "event_type": "batch.event.2",
                "timestamp": "2024-01-15T10:30:01Z",
                "payload": {"type": "Generic", "data": {"index": 2}}
            },
            {
                "id": "550e8400-e29b-41d4-a716-446655440012",
                "event_type": "batch.event.3",
                "timestamp": "2024-01-15T10:30:02Z",
                "payload": {"type": "Generic", "data": {"index": 3}}
            }
        ]
    });

    let response = fixture
        .client
        .post(fixture.url("/messages/batch"))
        .json(&batch)
        .send()
        .await
        .expect("Batch send request failed");

    assert!(response.status().is_success());

    let body: Vec<serde_json::Value> = response.json().await.expect("Failed to parse response");
    assert_eq!(body.len(), 3);
    assert!(body.iter().all(|r| r["success"] == true));
}

#[tokio::test]
async fn test_send_batch_empty_validation() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .client
        .post(fixture.url("/messages/batch"))
        .json(&json!({"events": []}))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(response.status().as_u16(), 400);

    let body: serde_json::Value = response.json().await.expect("Failed to parse response");
    assert_eq!(
        body.get("error")
            .and_then(|v| v.as_str())
            .expect("error missing"),
        "bad_request"
    );
}

// ============================================================================
// Stream Management Tests
// ============================================================================

#[tokio::test]
async fn test_list_streams() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .client
        .get(fixture.url("/streams"))
        .send()
        .await
        .expect("List streams request failed");

    assert!(response.status().is_success());

    let body: Vec<serde_json::Value> = response.json().await.expect("Failed to parse response");
    // At minimum, the default stream should exist
    assert!(!body.is_empty());
    assert!(body.iter().any(|s| s["name"] == "test-stream"));
}

#[tokio::test]
async fn test_create_and_delete_stream() {
    let fixture = TestFixture::new().await;
    let stream_name = format!("test-stream-{}", uuid::Uuid::new_v4());

    // Create stream
    let create_response = fixture
        .client
        .post(fixture.url("/streams"))
        .json(&json!({"name": stream_name}))
        .send()
        .await
        .expect("Create stream request failed");

    assert!(
        create_response.status().is_success(),
        "Create failed: {}",
        create_response.status()
    );

    // Get stream
    let get_response = fixture
        .client
        .get(fixture.url(&format!("/streams/{}", stream_name)))
        .send()
        .await
        .expect("Get stream request failed");

    assert!(get_response.status().is_success());

    let body: serde_json::Value = get_response.json().await.expect("Failed to parse response");
    assert_eq!(
        body.get("name")
            .and_then(|v| v.as_str())
            .expect("name missing"),
        stream_name
    );

    // Delete stream
    let delete_response = fixture
        .client
        .delete(fixture.url(&format!("/streams/{}", stream_name)))
        .send()
        .await
        .expect("Delete stream request failed");

    assert!(delete_response.status().is_success());

    // Verify deletion
    let verify_response = fixture
        .client
        .get(fixture.url(&format!("/streams/{}", stream_name)))
        .send()
        .await
        .expect("Verify request failed");

    assert_eq!(verify_response.status().as_u16(), 404);
}

#[tokio::test]
async fn test_create_stream_empty_name_validation() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .client
        .post(fixture.url("/streams"))
        .json(&json!({"name": ""}))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(response.status().as_u16(), 400);

    let body: serde_json::Value = response.json().await.expect("Failed to parse response");
    assert_eq!(
        body.get("error")
            .and_then(|v| v.as_str())
            .expect("error missing"),
        "bad_request"
    );
}

// ============================================================================
// Topic Management Tests
// ============================================================================

#[tokio::test]
async fn test_list_topics() {
    let fixture = TestFixture::new().await;

    let response = fixture
        .client
        .get(fixture.url("/streams/test-stream/topics"))
        .send()
        .await
        .expect("List topics request failed");

    assert!(response.status().is_success());

    let body: Vec<serde_json::Value> = response.json().await.expect("Failed to parse response");
    // The default topic should exist
    assert!(!body.is_empty());
    assert!(body.iter().any(|t| t["name"] == "test-events"));
}

#[tokio::test]
async fn test_create_and_delete_topic() {
    let fixture = TestFixture::new().await;
    let topic_name = format!("test-topic-{}", uuid::Uuid::new_v4());

    // Create topic
    let create_response = fixture
        .client
        .post(fixture.url("/streams/test-stream/topics"))
        .json(&json!({"name": topic_name, "partitions": 2}))
        .send()
        .await
        .expect("Create topic request failed");

    assert!(
        create_response.status().is_success(),
        "Create failed: {}",
        create_response.status()
    );

    // Get topic
    let get_response = fixture
        .client
        .get(fixture.url(&format!("/streams/test-stream/topics/{}", topic_name)))
        .send()
        .await
        .expect("Get topic request failed");

    assert!(get_response.status().is_success());

    let body: serde_json::Value = get_response.json().await.expect("Failed to parse response");
    assert_eq!(
        body.get("name")
            .and_then(|v| v.as_str())
            .expect("name missing"),
        topic_name
    );
    assert_eq!(
        body.get("partitions_count")
            .and_then(|v| v.as_u64())
            .expect("partitions_count missing"),
        2
    );

    // Delete topic
    let delete_response = fixture
        .client
        .delete(fixture.url(&format!("/streams/test-stream/topics/{}", topic_name)))
        .send()
        .await
        .expect("Delete topic request failed");

    assert!(delete_response.status().is_success());

    // Verify deletion
    let verify_response = fixture
        .client
        .get(fixture.url(&format!("/streams/test-stream/topics/{}", topic_name)))
        .send()
        .await
        .expect("Verify request failed");

    assert_eq!(verify_response.status().as_u16(), 404);
}

#[tokio::test]
async fn test_create_topic_validation() {
    let fixture = TestFixture::new().await;

    // Empty name
    let response = fixture
        .client
        .post(fixture.url("/streams/test-stream/topics"))
        .json(&json!({"name": "", "partitions": 1}))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(response.status().as_u16(), 400);

    // Zero partitions
    let response = fixture
        .client
        .post(fixture.url("/streams/test-stream/topics"))
        .json(&json!({"name": "valid-name", "partitions": 0}))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(response.status().as_u16(), 400);
}

// ============================================================================
// Message Flow Tests
// ============================================================================

#[tokio::test]
async fn test_message_with_partition_key() {
    let fixture = TestFixture::new().await;

    let event = json!({
        "event": {
            "id": "550e8400-e29b-41d4-a716-446655440020",
            "event_type": "order.created",
            "timestamp": "2024-01-15T10:30:00Z",
            "payload": {
                "type": "Generic",
                "data": {"order_id": "12345"}
            }
        },
        "partition_key": "customer-abc"
    });

    let response = fixture
        .client
        .post(fixture.url("/messages"))
        .json(&event)
        .send()
        .await
        .expect("Send request failed");

    assert!(response.status().is_success());
}

/// X-Request-Timeout propagation (TD-2026-07-04): a valid header must flow
/// through the scoped producer/consumer path and still serve the request; a
/// malformed header is ignored (falls back to the global operation timeout)
/// rather than rejected.
#[tokio::test]
async fn test_request_timeout_header_is_honored_end_to_end() {
    let fixture = TestFixture::new().await;

    // Valid header on a send: bounded by 5s instead of the 30s global.
    let response = fixture
        .client
        .post(fixture.url("/messages"))
        .header("X-Request-Timeout", "5000")
        .json(&generic_event("request-timeout-send", None))
        .send()
        .await
        .expect("Send with timeout header failed");
    assert_eq!(response.status(), 201);

    // Valid-but-minimum header on a poll: 100ms is plenty against a local
    // container, and exercises the scoped consumer path.
    let response = fixture
        .client
        .get(fixture.url("/messages?partition_id=0&count=1&offset=0"))
        .header("X-Request-Timeout", "100")
        .send()
        .await
        .expect("Poll with timeout header failed");
    assert!(response.status().is_success());

    // Malformed header: ignored, request served under the global timeout.
    let response = fixture
        .client
        .post(fixture.url("/messages"))
        .header("X-Request-Timeout", "not-a-number")
        .json(&generic_event("request-timeout-malformed", None))
        .send()
        .await
        .expect("Send with malformed timeout header failed");
    assert_eq!(response.status(), 201);

    // Scoped stream management path (iggy_scoped).
    let response = fixture
        .client
        .get(fixture.url("/streams"))
        .header("X-Request-Timeout", "2000")
        .send()
        .await
        .expect("List streams with timeout header failed");
    assert!(response.status().is_success());
}

#[tokio::test]
async fn test_send_to_specific_stream_topic() {
    let fixture = TestFixture::new().await;

    // Create a new stream and topic
    let stream_name = format!("custom-stream-{}", uuid::Uuid::new_v4());
    let topic_name = "custom-topic";

    fixture
        .client
        .post(fixture.url("/streams"))
        .json(&json!({"name": stream_name}))
        .send()
        .await
        .expect("Create stream failed");

    fixture
        .client
        .post(fixture.url(&format!("/streams/{}/topics", stream_name)))
        .json(&json!({"name": topic_name, "partitions": 1}))
        .send()
        .await
        .expect("Create topic failed");

    // Send message to specific stream/topic
    let event = json!({
        "event": {
            "id": "550e8400-e29b-41d4-a716-446655440030",
            "event_type": "custom.event",
            "timestamp": "2024-01-15T10:30:00Z",
            "payload": {"type": "Generic", "data": {"test": true}}
        }
    });

    let response = fixture
        .client
        .post(fixture.url(&format!(
            "/streams/{}/topics/{}/messages",
            stream_name, topic_name
        )))
        .json(&event)
        .send()
        .await
        .expect("Send request failed");

    assert!(response.status().is_success());

    let body: serde_json::Value = response.json().await.expect("Failed to parse response");
    assert_eq!(
        body.get("stream")
            .and_then(|v| v.as_str())
            .expect("stream missing"),
        stream_name
    );
    assert_eq!(
        body.get("topic")
            .and_then(|v| v.as_str())
            .expect("topic missing"),
        topic_name
    );

    // Cleanup
    fixture
        .client
        .delete(fixture.url(&format!("/streams/{}", stream_name)))
        .send()
        .await
        .ok();
}

// ============================================================================
// End-to-End Message Flow Test
// ============================================================================

/// Complete end-to-end test that verifies:
/// 1. Send a message with specific content
/// 2. Poll the message back
/// 3. Verify the content matches what was sent
#[tokio::test]
async fn test_end_to_end_message_content_verification() {
    let fixture = TestFixture::new().await;

    // Create a unique stream and topic for this test
    let stream_name = format!("e2e-stream-{}", uuid::Uuid::new_v4());
    let topic_name = "e2e-topic";

    // Create stream
    let create_stream = fixture
        .client
        .post(fixture.url("/streams"))
        .json(&json!({"name": stream_name}))
        .send()
        .await
        .expect("Create stream failed");
    assert!(create_stream.status().is_success());

    // Create topic with 1 partition for deterministic polling
    let create_topic = fixture
        .client
        .post(fixture.url(&format!("/streams/{}/topics", stream_name)))
        .json(&json!({"name": topic_name, "partitions": 1}))
        .send()
        .await
        .expect("Create topic failed");
    assert!(create_topic.status().is_success());

    // Unique test data
    let test_uuid = uuid::Uuid::new_v4().to_string();
    let test_message = format!("E2E test message {}", test_uuid);

    // Send message with unique content
    let event = json!({
        "event": {
            "id": test_uuid,
            "event_type": "e2e.test.verification",
            "timestamp": "2024-01-15T10:30:00Z",
            "payload": {
                "type": "Generic",
                "data": {
                    "test_message": test_message,
                    "test_number": 42,
                    "test_flag": true
                }
            }
        }
    });

    let send_response = fixture
        .client
        .post(fixture.url(&format!(
            "/streams/{}/topics/{}/messages",
            stream_name, topic_name
        )))
        .json(&event)
        .send()
        .await
        .expect("Send request failed");

    assert!(
        send_response.status().is_success(),
        "Send failed: {}",
        send_response.status()
    );

    let send_body: serde_json::Value = send_response.json().await.expect("Parse send response");
    assert!(
        send_body
            .get("success")
            .and_then(|v| v.as_bool())
            .expect("success missing")
    );
    assert_eq!(
        send_body
            .get("event_id")
            .and_then(|v| v.as_str())
            .expect("event_id missing"),
        test_uuid
    );

    // Wait for message to be persisted
    sleep(Duration::from_millis(500)).await;

    // Poll message back
    // Testing with partition_id=0 (Iggy appears to use 0-indexed partitions internally)
    let poll_response = fixture
        .client
        .get(fixture.url(&format!(
            "/streams/{}/topics/{}/messages?partition_id=0&count=10&offset=0",
            stream_name, topic_name
        )))
        .send()
        .await
        .expect("Poll request failed");

    assert!(
        poll_response.status().is_success(),
        "Poll failed: {}",
        poll_response.status()
    );

    let poll_body: serde_json::Value = poll_response.json().await.expect("Parse poll response");

    // Verify we got messages
    let messages = poll_body
        .get("messages")
        .and_then(|v| v.as_array())
        .expect("messages array");
    assert!(!messages.is_empty(), "No messages returned");

    // Find our specific message by event ID
    // Note: The ReceivedMessage structure has the event nested, so check event.id
    let our_message = messages
        .iter()
        .find(|m| {
            m.get("event")
                .and_then(|e| e.get("id"))
                .and_then(|id| id.as_str())
                == Some(test_uuid.as_str())
        })
        .expect("Our message not found in poll results");

    // Verify content matches what we sent
    // The ReceivedMessage structure has an 'event' field containing the original Event
    let event = our_message.get("event").expect("event missing");
    assert_eq!(
        event
            .get("event_type")
            .and_then(|v| v.as_str())
            .expect("event_type missing"),
        "e2e.test.verification"
    );
    let payload_data = event
        .get("payload")
        .and_then(|p| p.get("data"))
        .expect("payload.data missing");
    assert_eq!(
        payload_data
            .get("test_message")
            .and_then(|v| v.as_str())
            .expect("test_message missing"),
        test_message
    );
    assert_eq!(
        payload_data
            .get("test_number")
            .and_then(|v| v.as_u64())
            .expect("test_number missing"),
        42
    );
    assert!(
        payload_data
            .get("test_flag")
            .and_then(|v| v.as_bool())
            .expect("test_flag missing")
    );

    // Cleanup
    fixture
        .client
        .delete(fixture.url(&format!("/streams/{}", stream_name)))
        .send()
        .await
        .ok();
}

// ============================================================================
// Security Boundary Tests
// ============================================================================

/// Test fixture with security features enabled (API key auth + rate limiting)
struct SecureTestFixture {
    _iggy_container: ContainerAsync<GenericImage>,
    base_url: String,
    client: Client,
    api_key: String,
}

impl SecureTestFixture {
    const TEST_API_KEY: &'static str = "test-secret-api-key-12345";

    async fn new() -> Self {
        // Start Iggy container
        let (iggy_container, iggy) = IggyContainer::start().await;

        // Find available port for our app
        let app_port = find_available_port();
        let base_url = format!("http://127.0.0.1:{}", app_port);

        let connection_string = iggy.connection_string();
        let api_key = Self::TEST_API_KEY.to_string();

        // Use a channel to communicate server startup status
        let (tx, mut rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let connection_string_clone = connection_string.clone();
        let api_key_clone = api_key.clone();

        tokio::spawn(async move {
            match Self::start_secure_server(app_port, &connection_string_clone, &api_key_clone)
                .await
            {
                Ok(()) => {}
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        });

        // Wait for server to be ready
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        // Wait for server - health is bypassed, so we can use it
        Self::wait_for_server(&client, &base_url, &mut rx).await;

        Self {
            _iggy_container: iggy_container,
            base_url,
            client,
            api_key,
        }
    }

    async fn start_secure_server(
        port: u16,
        iggy_connection_string: &str,
        api_key: &str,
    ) -> Result<(), String> {
        use iggy_sample::{AppState, Config, IggyClientWrapper, build_router};
        use tokio::net::TcpListener;

        let config = Config {
            host: "127.0.0.1".to_string(),
            port,
            iggy_connection_string: iggy_connection_string.to_string(),
            default_stream: "secure-test-stream".to_string(),
            default_topic: "secure-test-events".to_string(),
            topic_partitions: 2,
            max_reconnect_attempts: 3,
            reconnect_base_delay: Duration::from_millis(100),
            reconnect_max_delay: Duration::from_secs(1),
            health_check_interval: Duration::from_secs(30),
            operation_timeout: Duration::from_secs(30),
            // Circuit breaker (default settings for tests)
            circuit_breaker_failure_threshold: 5,
            circuit_breaker_success_threshold: 2,
            circuit_breaker_open_duration: Duration::from_secs(30),
            // Rate limiting enabled - 5 RPS with burst of 2 for testing
            rate_limit_rps: 5,
            rate_limit_burst: 2,
            batch_max_size: 1000,
            poll_max_count: 100,
            max_request_body_size: 10 * 1024 * 1024,
            // API key authentication enabled
            api_key: Some(api_key.to_string()),
            auth_bypass_paths: vec!["/health".to_string(), "/ready".to_string()],
            cors_allowed_origins: vec!["*".to_string()],
            // Trusted-proxy enforcement ON: the test client's peer address is
            // 127.0.0.1 (untrusted), so spoofed forwarded headers must be
            // ignored - this makes the enforcement path itself wire-tested.
            trusted_proxies: vec!["10.0.0.0/8".to_string()],
            log_level: "warn".to_string(),
            stats_cache_ttl: Duration::from_secs(5),
            metrics_port: 0, // Disabled for tests
        };

        let iggy_client = IggyClientWrapper::new(config.clone())
            .await
            .map_err(|e| format!("Failed to create Iggy client: {}", e))?;

        iggy_client
            .initialize_defaults()
            .await
            .map_err(|e| format!("Failed to initialize defaults: {}", e))?;

        let state = AppState::new(iggy_client, config);
        let app = build_router(state).map_err(|e| format!("Failed to build router: {}", e))?;

        let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
            .await
            .map_err(|e| format!("Failed to bind server: {}", e))?;

        // Match production: ConnectInfo exposes the peer address for
        // TRUSTED_PROXIES enforcement in the middleware stack.
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .map_err(|e| format!("Server failed: {}", e))?;

        Ok(())
    }

    async fn wait_for_server(
        client: &Client,
        base_url: &str,
        error_rx: &mut tokio::sync::oneshot::Receiver<Result<(), String>>,
    ) {
        let health_url = format!("{}/health", base_url);
        let max_attempts = 60;

        for attempt in 1..=max_attempts {
            // Fail fast if the server task died instead of polling a corpse
            // for 30s and reporting a generic timeout (mirrors the standard
            // fixture's handling).
            match error_rx.try_recv() {
                Ok(Err(e)) => panic!("Server failed to start: {}", e),
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("Server task exited (panic or early return) before becoming ready");
                }
                Ok(Ok(())) | Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }

            match client.get(&health_url).send().await {
                Ok(response) if response.status().is_success() => {
                    return;
                }
                Ok(_) | Err(_) => {
                    if attempt == max_attempts {
                        panic!("Server failed to respond after {} attempts", max_attempts);
                    }
                }
            }
            sleep(Duration::from_millis(500)).await;
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

// ============================================================================
// API Key Authentication Tests
// ============================================================================

#[tokio::test]
async fn test_auth_required_without_api_key() {
    let fixture = SecureTestFixture::new().await;

    // Request without API key should return 401
    let response = fixture
        .client
        .get(fixture.url("/stats"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(
        response.status().as_u16(),
        401,
        "Expected 401 Unauthorized without API key"
    );

    let body: serde_json::Value = response.json().await.expect("Failed to parse response");
    assert_eq!(
        body.get("error")
            .and_then(|v| v.as_str())
            .expect("error missing"),
        "unauthorized"
    );
}

#[tokio::test]
async fn test_auth_success_with_header() {
    let fixture = SecureTestFixture::new().await;

    // Request with valid API key in header should succeed
    let response = fixture
        .client
        .get(fixture.url("/stats"))
        .header("x-api-key", &fixture.api_key)
        .send()
        .await
        .expect("Request failed");

    assert!(
        response.status().is_success(),
        "Expected success with valid API key, got {}",
        response.status()
    );
}

#[tokio::test]
async fn test_auth_success_with_query_param() {
    let fixture = SecureTestFixture::new().await;

    // Request with valid API key in query param should succeed (deprecated but supported)
    let response = fixture
        .client
        .get(fixture.url(&format!("/stats?api_key={}", fixture.api_key)))
        .send()
        .await
        .expect("Request failed");

    assert!(
        response.status().is_success(),
        "Expected success with valid API key in query, got {}",
        response.status()
    );
}

#[tokio::test]
async fn test_auth_invalid_api_key() {
    let fixture = SecureTestFixture::new().await;

    // Request with invalid API key should return 401
    let response = fixture
        .client
        .get(fixture.url("/stats"))
        .header("x-api-key", "wrong-api-key")
        .send()
        .await
        .expect("Request failed");

    assert_eq!(
        response.status().as_u16(),
        401,
        "Expected 401 Unauthorized with wrong API key"
    );
}

// Note: valid-key-never-throttled and failure-budget-throttling semantics are
// covered by deterministic unit tests in src/middleware/auth.rs (the secure
// fixture's general rate limiter (5 RPS) would dominate a wire-level test).

/// Wire-level proof that TRUSTED_PROXIES enforcement flows through the real
/// axum serve path (ConnectInfo included): an untrusted peer (127.0.0.1)
/// rotating spoofed X-Forwarded-For values must still hit the per-IP rate
/// limit, because every request is keyed on the actual peer address. A
/// regression that trusts headers again gives each spoofed value a fresh
/// bucket and this test fails.
#[tokio::test]
async fn test_spoofed_xff_rotation_cannot_bypass_rate_limit() {
    let fixture = SecureTestFixture::new().await;

    // Quota is 5 RPS + burst 2; 30 rapid requests with rotating spoofed
    // XFF values must trip the limiter if they share one bucket.
    let mut saw_429 = false;
    for i in 0..30 {
        let response = fixture
            .client
            .get(fixture.url("/stats"))
            .header("x-api-key", &fixture.api_key)
            .header("x-forwarded-for", format!("203.0.113.{}", i + 1))
            .send()
            .await
            .expect("Request failed");

        if response.status().as_u16() == 429 {
            saw_429 = true;
            break;
        }
    }

    assert!(
        saw_429,
        "rotating spoofed X-Forwarded-For values bypassed the per-IP rate \
         limit despite TRUSTED_PROXIES enforcement"
    );
}

#[tokio::test]
async fn test_health_bypasses_auth() {
    let fixture = SecureTestFixture::new().await;

    // Health endpoint should work without API key
    let response = fixture
        .client
        .get(fixture.url("/health"))
        .send()
        .await
        .expect("Request failed");

    assert!(
        response.status().is_success(),
        "Health endpoint should bypass auth"
    );
}

#[tokio::test]
async fn test_ready_bypasses_auth() {
    let fixture = SecureTestFixture::new().await;

    // Ready endpoint should work without API key
    let response = fixture
        .client
        .get(fixture.url("/ready"))
        .send()
        .await
        .expect("Request failed");

    assert!(
        response.status().is_success(),
        "Ready endpoint should bypass auth"
    );
}

// ============================================================================
// Rate Limiting Tests
// ============================================================================

#[tokio::test]
async fn test_rate_limit_returns_429() {
    let fixture = SecureTestFixture::new().await;

    // Send requests rapidly to trigger rate limit
    // The fixture is configured with 5 RPS + burst of 2 = 7 quick requests allowed
    let mut hit_rate_limit = false;

    for i in 0..20 {
        let response = fixture
            .client
            .get(fixture.url("/stats"))
            .header("x-api-key", &fixture.api_key)
            .send()
            .await
            .expect("Request failed");

        if response.status().as_u16() == 429 {
            hit_rate_limit = true;

            // Verify rate limit headers are present
            assert!(
                response.headers().contains_key("retry-after"),
                "Rate limited response should include Retry-After header"
            );
            assert!(
                response.headers().contains_key("x-ratelimit-limit"),
                "Rate limited response should include X-RateLimit-Limit header"
            );
            assert!(
                response.headers().contains_key("x-ratelimit-remaining"),
                "Rate limited response should include X-RateLimit-Remaining header"
            );
            break;
        }

        // Don't sleep - we want to exhaust the rate limit
        if i > 0 && i % 5 == 0 {
            // Brief pause to let some tokens replenish, but not all
            sleep(Duration::from_millis(50)).await;
        }
    }

    assert!(
        hit_rate_limit,
        "Should have hit rate limit after rapid requests"
    );
}

#[tokio::test]
async fn test_rate_limit_recovery() {
    let fixture = SecureTestFixture::new().await;

    // First, exhaust the rate limit
    for _ in 0..15 {
        let _ = fixture
            .client
            .get(fixture.url("/stats"))
            .header("x-api-key", &fixture.api_key)
            .send()
            .await;
    }

    // Wait for rate limit to reset (at 5 RPS, wait ~1 second for tokens to replenish)
    sleep(Duration::from_millis(1200)).await;

    // Should be able to make requests again
    let response = fixture
        .client
        .get(fixture.url("/stats"))
        .header("x-api-key", &fixture.api_key)
        .send()
        .await
        .expect("Request failed");

    assert!(
        response.status().is_success(),
        "Should succeed after rate limit window resets, got {}",
        response.status()
    );
}
