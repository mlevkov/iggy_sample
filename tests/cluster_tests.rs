//! Three-node cluster tests (ADR-0001 PR 3).
//!
//! Ignored by default: they need the stack from `docker-compose.cluster.yaml`
//! running, and the docker CLI on the host to stop and start its containers.
//! The weekly Extended Tests workflow runs them, then
//! `scripts/cluster-failover-demo.sh`, which covers leader failover:
//!
//! ```bash
//! docker compose -f docker-compose.cluster.yaml up -d --build --wait
//! cargo test --test cluster_tests -- --ignored --test-threads=1
//! ```
//!
//! `CLUSTER_GATEWAY_URL` overrides the gateway address
//! (`http://localhost:8001`). The tests run one at a time because one of them
//! stops a node.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::{Value, json};
use tokio::time::sleep;

fn gateway() -> String {
    std::env::var("CLUSTER_GATEWAY_URL").unwrap_or_else(|_| "http://localhost:8001".to_string())
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("HTTP client")
}

/// Whether the gateway answers `/ready` with 200 (an empty body).
async fn gateway_ready() -> bool {
    client()
        .get(format!("{}/ready", gateway()))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

async fn get_json(path: &str) -> Option<Value> {
    let response = client()
        .get(format!("{}{path}", gateway()))
        .header("X-Request-Timeout", "3000")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

/// Wait until the cluster has settled: a leader is marked, and every
/// partition of the default topic (replication factor "all") has all three
/// nodes in sync. Node `status` cannot tell: server 0.10 reports `healthy`
/// for a node that is down. Returns the cluster, or panics after `budget`.
async fn settled_cluster(budget: Duration) -> Value {
    let deadline = Instant::now() + budget;
    let mut last = (Value::Null, Value::Null);
    while Instant::now() < deadline {
        let cluster = get_json("/cluster").await.unwrap_or_default();
        let topic = get_json("/streams/sample-stream/topics/events")
            .await
            .unwrap_or_default();
        let has_leader = cluster.get("leader").is_some_and(|l| !l.is_null());
        let all_in_sync = topic
            .get("partitions")
            .and_then(Value::as_array)
            .is_some_and(|partitions| {
                !partitions.is_empty()
                    && partitions.iter().all(|partition| {
                        partition
                            .get("in_sync_members")
                            .and_then(Value::as_array)
                            .is_some_and(|members| members.len() == 3)
                    })
            });
        if has_leader && all_in_sync {
            return cluster;
        }
        last = (cluster, topic);
        sleep(Duration::from_millis(500)).await;
    }
    panic!(
        "the cluster did not settle within {budget:?}: cluster {}, topic {}",
        last.0, last.1
    );
}

fn docker(args: &[&str]) {
    let status = Command::new("docker")
        .args(args)
        .stdout(Stdio::null())
        .status()
        .expect("docker CLI");
    assert!(status.success(), "docker {args:?} failed");
}

/// Starts the container again when dropped, so a failing test does not leave
/// a node down for the tests after it.
struct StartOnDrop(&'static str);

impl Drop for StartOnDrop {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["start", self.0])
            .stdout(Stdio::null())
            .status();
    }
}

fn reported(topic: &Value, option: &str) -> (Value, String) {
    let entry = |field: &str| topic.pointer(&format!("/options/{option}/{field}"));
    (
        entry("value").cloned().unwrap_or_default(),
        entry("source")
            .and_then(Value::as_str)
            .unwrap_or("missing")
            .to_string(),
    )
}

fn members(topic: &Value) -> Vec<Vec<u64>> {
    topic
        .get("partitions")
        .and_then(Value::as_array)
        .expect("partitions array")
        .iter()
        .map(|partition| {
            partition
                .get("members")
                .and_then(Value::as_array)
                .expect("members array")
                .iter()
                .map(|id| id.as_u64().expect("replica id"))
                .collect()
        })
        .collect()
}

#[tokio::test]
#[ignore = "needs the docker-compose.cluster.yaml stack"]
async fn test_cluster_reports_three_nodes_and_one_leader() {
    let cluster = settled_cluster(Duration::from_secs(60)).await;

    assert_eq!(
        cluster.get("name"),
        Some(&json!("iggy-sample-cluster")),
        "{cluster}"
    );
    let nodes = cluster
        .get("nodes")
        .and_then(Value::as_array)
        .expect("nodes array");
    let ids: Vec<u64> = nodes
        .iter()
        .map(|node| node.get("replica_id").and_then(Value::as_u64).expect("id"))
        .collect();
    assert_eq!(ids, [0, 1, 2], "{cluster}");
    let leaders = nodes
        .iter()
        .filter(|node| node.get("role") == Some(&json!("leader")))
        .count();
    assert_eq!(leaders, 1, "exactly one metadata leader: {cluster}");
}

#[tokio::test]
#[ignore = "needs the docker-compose.cluster.yaml stack"]
async fn test_replication_factor_places_each_partition_on_that_many_nodes() {
    settled_cluster(Duration::from_secs(60)).await;
    let topic_name = format!("rf-two-{}", std::process::id());

    let response = client()
        .post(format!("{}/streams/sample-stream/topics", gateway()))
        .json(&json!({
            "name": topic_name,
            "partitions": 3,
            "options": {"replication_factor": 2}
        }))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(response.status().as_u16(), 201);

    let topic = get_json(&format!("/streams/sample-stream/topics/{topic_name}"))
        .await
        .expect("created topic");
    assert_eq!(
        reported(&topic, "replication_factor"),
        (json!(2), "explicit".into())
    );
    let placed = members(&topic);
    assert_eq!(placed.len(), 3);
    for partition in &placed {
        assert_eq!(
            partition.len(),
            2,
            "two copies of each partition: {placed:?}"
        );
        assert!(partition.iter().all(|id| *id <= 2), "{placed:?}");
    }

    // The default topic took the server's default_replication_factor, "all".
    let default_topic = get_json("/streams/sample-stream/topics/events")
        .await
        .expect("default topic");
    assert_eq!(
        reported(&default_topic, "replication_factor"),
        (json!(3), "default".into())
    );
    for partition in members(&default_topic) {
        let mut partition = partition;
        partition.sort_unstable();
        assert_eq!(partition, [0, 1, 2]);
    }

    // More copies than nodes: the server refuses it, and the gateway says so.
    let response = client()
        .post(format!("{}/streams/sample-stream/topics", gateway()))
        .json(&json!({
            "name": format!("{topic_name}-four"),
            "options": {"replication_factor": 4}
        }))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(response.status().as_u16(), 400);

    let _ = client()
        .delete(format!(
            "{}/streams/sample-stream/topics/{topic_name}",
            gateway()
        ))
        .send()
        .await;
}

/// The gateway's connection string lists all three nodes. With the first one
/// down, a restarted gateway must start on the next.
#[tokio::test]
#[ignore = "needs the docker-compose.cluster.yaml stack"]
async fn test_gateway_starts_when_the_first_seed_is_down() {
    settled_cluster(Duration::from_secs(60)).await;

    let restart_node = StartOnDrop("iggy-node-0");
    docker(&["stop", "iggy-node-0"]);
    docker(&["restart", "iggy-cluster-app"]);

    let deadline = Instant::now() + Duration::from_secs(90);
    let mut ready = false;
    while Instant::now() < deadline {
        if gateway_ready().await {
            ready = true;
            break;
        }
        sleep(Duration::from_secs(1)).await;
    }
    assert!(ready, "the gateway did not start with its first seed down");

    // And it serves: a send reaches the two remaining nodes.
    let response = client()
        .post(format!("{}/messages", gateway()))
        .json(&json!({
            "event": {
                "id": "550e8400-e29b-41d4-a716-446655440099",
                "event_type": "cluster.seed-test",
                "timestamp": "2026-11-08T00:00:00Z",
                "payload": {"type": "Generic", "data": {}}
            }
        }))
        .send()
        .await
        .expect("Request failed");
    assert!(
        response.status().is_success(),
        "send: {}",
        response.status()
    );

    // Restarted, node 0 catches up and is back in sync on every partition.
    drop(restart_node);
    settled_cluster(Duration::from_secs(90)).await;
}
