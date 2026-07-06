//! Smoke test for the Prometheus metrics exporter (TD-2026-07-05).
//!
//! `init_metrics` installs a process-global recorder, so this lives in its
//! own integration-test binary (own process) where the single install
//! cannot collide with any other test.
//!
//! Run with: `cargo test --test metrics_smoke_test`
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

use iggy_sample::metrics;

/// Reserve an ephemeral loopback port, then release it for the exporter.
///
/// A small TOCTOU window exists between drop and rebind; acceptable for a
/// test, and the same pattern the app integration tests use.
fn ephemeral_addr() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
}

#[tokio::test]
async fn init_metrics_serves_recorded_counter_over_http() {
    let addr = ephemeral_addr();

    metrics::init_metrics(addr).expect("Prometheus exporter must install and bind");

    // Record through the public helpers (counter, histogram, gauge) so a
    // metrics-exporter-prometheus behavior change in any register path is
    // caught here instead of at production startup.
    metrics::record_message_sent("smoke-stream", "smoke-topic", "success");
    metrics::record_send_duration("smoke-stream", "smoke-topic", 0.042);
    metrics::set_connection_status(true);

    // Scrape /metrics, retrying briefly while the listener task comes up.
    // The per-request timeout keeps an accepted-but-unanswered connection
    // from hanging the test past the outer deadline.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("http client");
    let url = format!("http://{addr}/metrics");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let body = loop {
        match client.get(&url).send().await {
            Ok(response) => {
                assert!(
                    response.status().is_success(),
                    "scrape failed: {}",
                    response.status()
                );
                break response.text().await.expect("scrape body");
            }
            Err(e) if tokio::time::Instant::now() < deadline => {
                eprintln!("exporter not up yet ({e}), retrying");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("exporter never came up on {url}: {e}"),
        }
    };

    assert!(
        body.contains(metrics::names::MESSAGES_SENT_TOTAL),
        "counter missing from scrape:\n{body}"
    );
    assert!(
        body.contains(r#"stream="smoke-stream""#),
        "counter labels missing from scrape:\n{body}"
    );
    assert!(
        body.contains(metrics::names::SEND_DURATION_SECONDS),
        "histogram missing from scrape:\n{body}"
    );
    assert!(
        body.contains(metrics::names::CONNECTION_STATUS),
        "gauge missing from scrape:\n{body}"
    );
}
