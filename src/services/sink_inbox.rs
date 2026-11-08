//! Receiver for the connectors demo's HTTP sink (ADR-0001 PR 4).
//!
//! Upstream's HTTP sink (`iggy_connector_http_sink`) consumes a topic and,
//! in `ndjson` batch mode, POSTs each polled batch as newline-delimited JSON:
//! one envelope per message,
//! `{"metadata": {"iggy_id": .., "iggy_offset": .., ..}, "payload": ..}`.
//! `POST /sink/inbound` accepts those batches and keeps the newest
//! deliveries in memory for `GET /sink/inbound`, so the outbound loop
//! (gateway -> Iggy -> sink -> gateway) closes inside this repository.
//!
//! # Delivery semantics
//!
//! The sink stores its consumer offsets when it polls, before it delivers,
//! so a batch this receiver refuses is not delivered again: at-most-once.
//! It does retry a 429, a 5xx or a network error, so a batch whose answer
//! was lost arrives twice. Nothing here deduplicates. Stream, topic,
//! partition and offset (or the Iggy message id) identify a message.
//!
//! # Authentication
//!
//! The sink sends `Authorization: Bearer <token>` from its configuration's
//! headers table, and [`SinkInbox::authorize`] compares it in constant time
//! with `SINK_INBOUND_TOKEN`. Without that setting the routes do not exist.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value};
use subtle::ConstantTimeEq;

use crate::models::{SinkDelivery, SinkInboxView};

/// Most deliveries `GET /sink/inbound` keeps.
pub const RETAINED_DELIVERIES: usize = 100;

/// Most envelope bytes kept across retained deliveries, counted as the JSON
/// arrived. Parsed values take more memory than their text, so this bounds
/// the input, not the heap. The newest delivery is kept even when it alone
/// is larger.
pub const RETAINED_BYTES: usize = 1024 * 1024;

/// Shortest accepted `SINK_INBOUND_TOKEN`, in bytes.
pub const MIN_TOKEN_LEN: usize = 32;

/// Longest accepted `SINK_INBOUND_TOKEN`, in bytes.
pub const MAX_TOKEN_LEN: usize = 1024;

/// The shared secret the HTTP sink presents as a bearer token.
///
/// Its `Debug` output never shows the value.
#[derive(Clone)]
pub struct SinkToken(String);

impl SinkToken {
    /// Accept a token of [`MIN_TOKEN_LEN`] to [`MAX_TOKEN_LEN`] printable
    /// ASCII characters without spaces, so it fits an HTTP header unchanged.
    ///
    /// # Errors
    ///
    /// Returns the rule the value breaks. The message never repeats the
    /// value.
    pub fn new(value: &str) -> Result<Self, String> {
        if value.len() < MIN_TOKEN_LEN {
            return Err(format!(
                "must be at least {MIN_TOKEN_LEN} characters (got {}); generate one with \
                 `openssl rand -hex 32`",
                value.len()
            ));
        }
        if value.len() > MAX_TOKEN_LEN {
            return Err(format!("must be at most {MAX_TOKEN_LEN} characters"));
        }
        if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("must be printable ASCII without spaces".to_string());
        }
        Ok(Self(value.to_string()))
    }

    /// Whether `presented` equals the token, compared in constant time for
    /// equal lengths.
    fn matches(&self, presented: &[u8]) -> bool {
        self.0.as_bytes().ct_eq(presented).into()
    }
}

impl fmt::Debug for SinkToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SinkToken(<redacted>)")
    }
}

/// Why a batch was refused. Shown to the (authenticated) sender.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BatchError {
    /// The body held no envelope.
    #[error("the batch is empty")]
    Empty,
    /// A line is not a sink envelope.
    #[error("line {line}: {reason}")]
    Line {
        /// 1-based line number
        line: usize,
        /// What is wrong with it, without echoing its content
        reason: String,
    },
}

/// The sink's metadata envelope. Fields the sink adds optionally
/// (`iggy_checksum`, `iggy_origin_timestamp`) are ignored.
#[derive(Deserialize)]
struct Envelope {
    metadata: EnvelopeMetadata,
    payload: Value,
}

#[derive(Deserialize)]
struct EnvelopeMetadata {
    iggy_id: String,
    iggy_offset: u64,
    iggy_timestamp: u64,
    iggy_stream: String,
    iggy_topic: String,
    iggy_partition_id: u32,
    #[serde(default)]
    iggy_headers: Option<Map<String, Value>>,
}

/// Parse an `ndjson` batch from the HTTP sink. Blank lines are skipped; one
/// malformed line refuses the whole batch, so a batch is kept whole or not
/// at all.
///
/// # Errors
///
/// [`BatchError::Empty`] for a body without envelopes, [`BatchError::Line`]
/// naming the first line that is not one.
pub fn parse_batch(
    body: &[u8],
    received_at: DateTime<Utc>,
) -> Result<Vec<SinkDelivery>, BatchError> {
    let mut deliveries = Vec::new();
    for (index, raw_line) in body.split(|byte| *byte == b'\n').enumerate() {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let line_number = index + 1;
        let envelope: Envelope =
            serde_json::from_slice(line).map_err(|error| BatchError::Line {
                line: line_number,
                reason: describe_json_error(&error),
            })?;
        deliveries.push(into_delivery(
            envelope,
            line.len(),
            received_at,
            line_number,
        )?);
    }
    if deliveries.is_empty() {
        return Err(BatchError::Empty);
    }
    Ok(deliveries)
}

fn into_delivery(
    envelope: Envelope,
    envelope_bytes: usize,
    received_at: DateTime<Utc>,
    line: usize,
) -> Result<SinkDelivery, BatchError> {
    let metadata = envelope.metadata;
    if metadata.iggy_id.len() != 32
        || !metadata
            .iggy_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(BatchError::Line {
            line,
            reason: "metadata.iggy_id is not 32 lowercase hex digits".to_string(),
        });
    }
    let timestamp = i64::try_from(metadata.iggy_timestamp)
        .ok()
        .and_then(DateTime::from_timestamp_micros);
    Ok(SinkDelivery {
        received_at,
        stream: metadata.iggy_stream,
        topic: metadata.iggy_topic,
        partition_id: metadata.iggy_partition_id,
        offset: metadata.iggy_offset,
        id: metadata.iggy_id,
        timestamp,
        headers: metadata.iggy_headers,
        payload: envelope.payload,
        envelope_bytes,
    })
}

/// Describe a parse failure by kind. serde's own message can quote the
/// offending value and name internal types, so only a missing field is
/// named, since that is the sink's documented wire format.
fn describe_json_error(error: &serde_json::Error) -> String {
    use serde_json::error::Category;
    match error.classify() {
        Category::Syntax | Category::Eof | Category::Io => "malformed JSON".to_string(),
        Category::Data => {
            let message = error.to_string();
            match message
                .strip_prefix("missing field `")
                .and_then(|rest| rest.split_once('`'))
            {
                Some((field, _)) => format!("not a sink envelope: missing field `{field}`"),
                None => "not a sink envelope".to_string(),
            }
        }
    }
}

/// Counters and the newest deliveries, behind one lock.
#[derive(Default)]
struct InboxState {
    recent: VecDeque<SinkDelivery>,
    retained_bytes: usize,
    messages_received: u64,
    batches_accepted: u64,
    batches_rejected: u64,
    unauthorized_requests: u64,
    last_received_at: Option<DateTime<Utc>>,
}

/// What the HTTP sink delivered: lifetime counters and the newest
/// deliveries, bounded by [`RETAINED_DELIVERIES`] and [`RETAINED_BYTES`].
#[derive(Debug)]
pub struct SinkInbox {
    token: SinkToken,
    state: Mutex<InboxState>,
}

impl fmt::Debug for InboxState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InboxState")
            .field("retained", &self.recent.len())
            .field("retained_bytes", &self.retained_bytes)
            .field("messages_received", &self.messages_received)
            .finish_non_exhaustive()
    }
}

impl SinkInbox {
    /// An empty inbox that accepts requests bearing `token`.
    pub fn new(token: SinkToken) -> Self {
        Self {
            token,
            state: Mutex::new(InboxState::default()),
        }
    }

    /// The counters only ever grow and the deliveries are copies, so a
    /// panic while the lock was held leaves nothing half-updated that
    /// matters here: a poisoned lock is used as is.
    fn state(&self) -> std::sync::MutexGuard<'_, InboxState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether an `Authorization` header value carries the token as a
    /// bearer credential. A refusal is counted.
    pub fn authorize(&self, authorization: Option<&str>) -> bool {
        let authorized = authorization
            .and_then(bearer_credential)
            .is_some_and(|credential| self.token.matches(credential.as_bytes()));
        if !authorized {
            self.state().unauthorized_requests += 1;
        }
        authorized
    }

    /// Keep an accepted batch, evicting the oldest deliveries past the
    /// bounds. Returns how many messages the batch held.
    pub fn accept(&self, deliveries: Vec<SinkDelivery>) -> usize {
        let count = deliveries.len();
        let mut state = self.state();
        state.batches_accepted += 1;
        state.messages_received += count as u64;
        if let Some(last) = deliveries.last() {
            state.last_received_at = Some(last.received_at);
        }
        for delivery in deliveries {
            state.retained_bytes += delivery.envelope_bytes;
            state.recent.push_back(delivery);
        }
        while state.recent.len() > 1
            && (state.recent.len() > RETAINED_DELIVERIES || state.retained_bytes > RETAINED_BYTES)
        {
            if let Some(evicted) = state.recent.pop_front() {
                state.retained_bytes -= evicted.envelope_bytes;
            }
        }
        count
    }

    /// Count an authenticated batch that was refused.
    pub fn reject(&self) {
        self.state().batches_rejected += 1;
    }

    /// The counters and retained deliveries, newest first.
    pub fn snapshot(&self) -> SinkInboxView {
        let state = self.state();
        SinkInboxView {
            messages_received: state.messages_received,
            batches_accepted: state.batches_accepted,
            batches_rejected: state.batches_rejected,
            unauthorized_requests: state.unauthorized_requests,
            last_received_at: state.last_received_at,
            retained: state.recent.len(),
            recent: state.recent.iter().rev().cloned().collect(),
        }
    }
}

/// The credential of a `Bearer` authorization (scheme case-insensitive).
fn bearer_credential(authorization: &str) -> Option<&str> {
    let (scheme, credential) = authorization.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let credential = credential.trim_start_matches(' ');
    (!credential.is_empty()).then_some(credential)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn envelope(offset: u64, payload: Value) -> String {
        json!({
            "metadata": {
                "iggy_id": format!("{:032x}", offset + 1),
                "iggy_offset": offset,
                "iggy_timestamp": 1_762_560_000_000_000_u64,
                "iggy_stream": "sample-stream",
                "iggy_topic": "events",
                "iggy_partition_id": 1,
                "iggy_checksum": 42
            },
            "payload": payload
        })
        .to_string()
    }

    fn inbox() -> SinkInbox {
        SinkInbox::new(SinkToken::new(TOKEN).unwrap())
    }

    #[test]
    fn test_token_rejects_short_long_and_unprintable_values_without_echoing_them() {
        let short = SinkToken::new("short-secret").unwrap_err();
        assert!(short.contains("at least 32"), "{short}");
        assert!(!short.contains("short-secret"));
        assert!(SinkToken::new(&"a".repeat(MAX_TOKEN_LEN + 1)).is_err());
        let spaced = SinkToken::new(&format!("{TOKEN} with spaces")).unwrap_err();
        assert!(!spaced.contains(TOKEN));
        assert!(SinkToken::new(&format!("{TOKEN}\u{e9}")).is_err());
        assert!(SinkToken::new(TOKEN).is_ok());
    }

    #[test]
    fn test_token_debug_never_shows_the_value() {
        let token = SinkToken::new(TOKEN).unwrap();
        assert_eq!(format!("{token:?}"), "SinkToken(<redacted>)");
        assert!(!format!("{:?}", inbox()).contains(TOKEN));
    }

    #[test]
    fn test_authorize_accepts_only_the_bearer_token_and_counts_refusals() {
        let inbox = inbox();
        assert!(inbox.authorize(Some(&format!("Bearer {TOKEN}"))));
        assert!(inbox.authorize(Some(&format!("bearer {TOKEN}"))));
        assert!(!inbox.authorize(None));
        assert!(!inbox.authorize(Some(TOKEN)));
        assert!(!inbox.authorize(Some(&format!("Basic {TOKEN}"))));
        assert!(!inbox.authorize(Some("Bearer ")));
        assert!(!inbox.authorize(Some(&format!("Bearer {TOKEN}x"))));
        assert!(!inbox.authorize(Some(&format!("Bearer {}", &TOKEN[1..]))));
        assert_eq!(inbox.snapshot().unauthorized_requests, 6);
    }

    #[test]
    fn test_parse_batch_reads_each_envelope_and_skips_blank_lines() {
        let body = format!(
            "{}\r\n\n{}\n",
            envelope(7, json!({"id": "a"})),
            envelope(8, json!("text"))
        );
        let received_at = Utc::now();
        let deliveries = parse_batch(body.as_bytes(), received_at).unwrap();

        assert_eq!(deliveries.len(), 2);
        let first = &deliveries[0];
        assert_eq!(first.stream, "sample-stream");
        assert_eq!(first.topic, "events");
        assert_eq!(first.partition_id, 1);
        assert_eq!(first.offset, 7);
        assert_eq!(first.id, format!("{:032x}", 8));
        assert_eq!(first.payload, json!({"id": "a"}));
        assert_eq!(first.received_at, received_at);
        assert_eq!(
            first.timestamp.map(|t| t.timestamp_micros()),
            Some(1_762_560_000_000_000)
        );
        assert!(first.headers.is_none());
        assert_eq!(deliveries[1].payload, json!("text"));
    }

    #[test]
    fn test_parse_batch_refuses_the_whole_batch_and_names_the_bad_line() {
        let body = format!(
            "{}\n{{\"metadata\": {{}}, \"payload\": 1}}\n",
            envelope(1, json!(1))
        );
        let error = parse_batch(body.as_bytes(), Utc::now()).unwrap_err();
        assert_eq!(
            error,
            BatchError::Line {
                line: 2,
                reason: "not a sink envelope: missing field `iggy_id`".to_string()
            }
        );

        let error = parse_batch(b"{\"metadata\": ", Utc::now()).unwrap_err();
        assert_eq!(error.to_string(), "line 1: malformed JSON");
    }

    #[test]
    fn test_parse_batch_does_not_echo_the_offending_value() {
        let body = envelope(1, json!(1)).replace("\"iggy_offset\":1", "\"iggy_offset\":\"secret\"");
        let error = parse_batch(body.as_bytes(), Utc::now()).unwrap_err();
        assert_eq!(error.to_string(), "line 1: not a sink envelope");
    }

    #[test]
    fn test_parse_batch_refuses_an_empty_body_and_a_malformed_id() {
        assert_eq!(
            parse_batch(b"\n \n", Utc::now()).unwrap_err(),
            BatchError::Empty
        );
        let body = envelope(1, json!(1)).replace(&format!("{:032x}", 2), "not-hex");
        let error = parse_batch(body.as_bytes(), Utc::now()).unwrap_err();
        assert!(error.to_string().contains("iggy_id"), "{error}");
    }

    #[test]
    fn test_parse_batch_keeps_headers_and_tolerates_an_out_of_range_timestamp() {
        let body = json!({
            "metadata": {
                "iggy_id": format!("{:032x}", 1),
                "iggy_offset": 0,
                "iggy_timestamp": u64::MAX,
                "iggy_stream": "s",
                "iggy_topic": "t",
                "iggy_partition_id": 0,
                "iggy_headers": {"x-trace": "abc"}
            },
            "payload": {}
        })
        .to_string();
        let deliveries = parse_batch(body.as_bytes(), Utc::now()).unwrap();
        assert_eq!(deliveries[0].timestamp, None);
        assert_eq!(
            deliveries[0]
                .headers
                .as_ref()
                .and_then(|h| h.get("x-trace")),
            Some(&json!("abc"))
        );
    }

    #[test]
    fn test_accept_counts_and_reports_newest_first() {
        let inbox = inbox();
        let body = format!("{}\n{}", envelope(1, json!(1)), envelope(2, json!(2)));
        let accepted = inbox.accept(parse_batch(body.as_bytes(), Utc::now()).unwrap());
        inbox.reject();

        assert_eq!(accepted, 2);
        let view = inbox.snapshot();
        assert_eq!(view.messages_received, 2);
        assert_eq!(view.batches_accepted, 1);
        assert_eq!(view.batches_rejected, 1);
        assert_eq!(view.retained, 2);
        assert_eq!(view.recent[0].offset, 2);
        assert_eq!(view.recent[1].offset, 1);
        assert!(view.last_received_at.is_some());
    }

    #[test]
    fn test_accept_evicts_the_oldest_past_the_count_bound() {
        let inbox = inbox();
        for offset in 0..(RETAINED_DELIVERIES as u64 + 5) {
            let body = envelope(offset, json!(offset));
            inbox.accept(parse_batch(body.as_bytes(), Utc::now()).unwrap());
        }
        let view = inbox.snapshot();
        assert_eq!(view.retained, RETAINED_DELIVERIES);
        assert_eq!(view.messages_received, RETAINED_DELIVERIES as u64 + 5);
        assert_eq!(view.recent[0].offset, RETAINED_DELIVERIES as u64 + 4);
        assert_eq!(view.recent[RETAINED_DELIVERIES - 1].offset, 5);
    }

    #[test]
    fn test_accept_evicts_past_the_byte_bound_but_keeps_the_newest() {
        let inbox = inbox();
        let large = "x".repeat(RETAINED_BYTES / 3);
        for offset in 0..4 {
            let body = envelope(offset, json!(large));
            inbox.accept(parse_batch(body.as_bytes(), Utc::now()).unwrap());
        }
        let view = inbox.snapshot();
        assert_eq!(view.retained, 2, "three large deliveries exceed 1 MiB");
        assert_eq!(view.recent[0].offset, 3);

        let huge = "y".repeat(RETAINED_BYTES + 1);
        let body = envelope(9, json!(huge));
        inbox.accept(parse_batch(body.as_bytes(), Utc::now()).unwrap());
        let view = inbox.snapshot();
        assert_eq!(view.retained, 1, "the newest delivery is kept alone");
        assert_eq!(view.recent[0].offset, 9);
    }
}
