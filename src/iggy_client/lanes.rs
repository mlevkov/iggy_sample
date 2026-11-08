//! Freeing SDK lanes that an unknown outcome blocks (SDK 0.12).
//!
//! SDK 0.12 admits one mutation at a time per *lane*: one lane for metadata
//! (stream and topic creates and deletes) and one per partition (sends), plus
//! a progress lane per partition for auto-commit and `Next` polls. A mutation
//! whose outcome the SDK cannot prove stays *retained* in its lane, and every
//! later call on that lane fails at once with `LaneBlocked` or
//! `PartitionLaneBlocked` until the application resolves or abandons it. The
//! SDK's `IggyProducer` does that itself; the trait-level API this gateway
//! uses leaves it to the caller.
//!
//! Two paths retain an operation:
//!
//! - The call returns `OperationUncertain(op)` (a mutation) or
//!   `FetchProgressUncertain(op)` (an auto-commit poll).
//! - The call is cancelled after its request left the client, which is what
//!   the wrapper's per-request timeout does. Nothing names `op` then: the
//!   next call on that lane is the first to see it, as `LaneBlocked(op)`.
//!
//! Left alone, either path would block every later send to that partition,
//! or every create and delete, for the life of the client.
//!
//! The gateway has no use for a retained operation. Its HTTP caller already
//! has an answer (503 outcome unknown, or a timeout), and this service never
//! resends a request whose outcome is unknown. So [`settle_lanes`] abandons
//! every operation it sees named. Abandoning frees the lane and leaves that
//! operation's outcome unknown, which is what its caller was told. A lane
//! refusal takes no request id, so the refused call was never sent: after
//! abandoning the blocker, the call runs once more as its first real attempt.

use std::future::Future;

use iggy::prelude::{
    Client, Identifier, IggyClient, IggyError, IggyMessage, MessageClient, Partitioning,
    SendMessagesResponse,
};
use tracing::debug;

/// [`settle_lanes`] against `client`: run `call`, abandoning on `client`.
///
/// Every SDK call that takes a lane goes through this or [`send_settled`]:
/// sends, creates, deletes and polls. Reads take no lane and do not need it.
pub(super) async fn settled<T, C, Fut>(client: &IggyClient, call: C) -> Result<T, IggyError>
where
    C: FnMut() -> Fut,
    Fut: Future<Output = Result<T, IggyError>>,
{
    settle_lanes(call, |op| abandon(client, op)).await
}

/// Send `messages`, settling lanes as [`settled`] does.
///
/// Written out rather than through [`settle_lanes`]: the SDK takes the batch
/// as `&mut`, and a `FnMut` closure cannot hand out a fresh `&mut` borrow to
/// each future it returns. Sending the same batch twice is safe here: the
/// lane refused the first attempt before anything was sent, and the SDK
/// changes a batch only to encrypt it, with a client-side encryptor that a
/// connection-string client never has.
pub(super) async fn send_settled(
    client: &IggyClient,
    stream: &Identifier,
    topic: &Identifier,
    partitioning: &Partitioning,
    messages: &mut [IggyMessage],
) -> Result<SendMessagesResponse, IggyError> {
    let mut result = client
        .send_messages(stream, topic, partitioning, messages)
        .await;
    if let Some(op) = lane_blocker(&result) {
        abandon(client, op).await;
        result = client
            .send_messages(stream, topic, partitioning, messages)
            .await;
    }
    if let Err(error) = &result
        && let Some(op) = retained_operation(error)
    {
        abandon(client, op).await;
    }
    result
}

/// Abandon `op` on `client`. An operation that is already gone (a concurrent
/// request abandoned it first) is not an error here.
async fn abandon(client: &IggyClient, op: u64) {
    debug!(op, "Abandoning a retained operation");
    if let Err(error) = client.abandon_operation(op).await {
        debug!(op, %error, "Retained operation was already released");
    }
}

/// Run one SDK call, abandoning any operation that blocks or would block its lane.
///
/// - A lane refusal (`LaneBlocked`, `PartitionLaneBlocked`) abandons the
///   blocking operation and runs `call` once more. A second refusal is
///   returned, after abandoning its blocker too, rather than looping.
/// - An unknown outcome (`OperationUncertain`, `FetchProgressUncertain`) is
///   returned unchanged after abandoning its operation, so later calls on the
///   lane are admitted.
/// - Every other result passes through untouched.
///
/// `abandon` must not fail the call: an operation that is already gone (a
/// concurrent request abandoned it first) is not an error here.
///
/// The closures are plain `FnMut`/`Fn` returning futures rather than async
/// closures: axum needs handler futures to be `Send`, and rustc cannot yet
/// prove that for the higher-ranked futures an `AsyncFnMut` bound produces.
pub(super) async fn settle_lanes<T, C, Fut, A, AFut>(
    mut call: C,
    abandon: A,
) -> Result<T, IggyError>
where
    C: FnMut() -> Fut,
    Fut: Future<Output = Result<T, IggyError>>,
    A: Fn(u64) -> AFut,
    AFut: Future<Output = ()>,
{
    let mut result = call().await;
    if let Some(op) = lane_blocker(&result) {
        abandon(op).await;
        result = call().await;
    }
    if let Err(error) = &result
        && let Some(op) = retained_operation(error)
    {
        abandon(op).await;
    }
    result
}

/// The earlier operation that refused this call its lane, if any. A refusal
/// takes no request id, so the refused call was never sent.
fn lane_blocker<T>(result: &Result<T, IggyError>) -> Option<u64> {
    match result {
        Err(IggyError::LaneBlocked(op) | IggyError::PartitionLaneBlocked(_, op)) => Some(*op),
        _ => None,
    }
}

/// The operation an error says is retained in its lane, if any.
fn retained_operation(error: &IggyError) -> Option<u64> {
    match error {
        IggyError::OperationUncertain(op)
        | IggyError::FetchProgressUncertain(op)
        | IggyError::LaneBlocked(op)
        | IggyError::PartitionLaneBlocked(_, op) => Some(*op),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;

    /// Runs `settle_lanes` over scripted call results and returns the final
    /// result, how many times the call ran, and which operations were abandoned.
    async fn run(script: Vec<Result<u32, IggyError>>) -> (Result<u32, IggyError>, usize, Vec<u64>) {
        let script = RefCell::new(VecDeque::from(script));
        let calls = RefCell::new(0_usize);
        let abandoned = RefCell::new(Vec::new());
        let result = settle_lanes(
            || {
                *calls.borrow_mut() += 1;
                let next = script
                    .borrow_mut()
                    .pop_front()
                    .expect("call ran more often than scripted");
                async move { next }
            },
            |op| {
                abandoned.borrow_mut().push(op);
                async {}
            },
        )
        .await;
        (result, calls.into_inner(), abandoned.into_inner())
    }

    #[tokio::test]
    async fn test_settle_lanes_passes_success_through_without_abandoning() {
        let (result, calls, abandoned) = run(vec![Ok(7)]).await;
        assert_eq!(result, Ok(7));
        assert_eq!(calls, 1);
        assert!(abandoned.is_empty());
    }

    #[tokio::test]
    async fn test_settle_lanes_passes_other_errors_through_without_abandoning() {
        let (result, calls, abandoned) = run(vec![Err(IggyError::TransientNotAccepted)]).await;
        assert_eq!(result, Err(IggyError::TransientNotAccepted));
        assert_eq!(calls, 1);
        assert!(abandoned.is_empty());
    }

    #[tokio::test]
    async fn test_settle_lanes_abandons_an_uncertain_mutation_without_resending() {
        let (result, calls, abandoned) = run(vec![Err(IggyError::OperationUncertain(9))]).await;
        assert_eq!(result, Err(IggyError::OperationUncertain(9)));
        assert_eq!(calls, 1, "an unknown outcome must never be resent");
        assert_eq!(abandoned, vec![9]);
    }

    #[tokio::test]
    async fn test_settle_lanes_abandons_an_uncertain_poll_without_repolling() {
        let (result, calls, abandoned) = run(vec![Err(IggyError::FetchProgressUncertain(4))]).await;
        assert_eq!(result, Err(IggyError::FetchProgressUncertain(4)));
        assert_eq!(calls, 1);
        assert_eq!(abandoned, vec![4]);
    }

    #[tokio::test]
    async fn test_settle_lanes_frees_a_blocked_lane_and_sends_once_more() {
        let (result, calls, abandoned) = run(vec![Err(IggyError::LaneBlocked(3)), Ok(1)]).await;
        assert_eq!(result, Ok(1));
        assert_eq!(calls, 2);
        assert_eq!(abandoned, vec![3]);
    }

    #[tokio::test]
    async fn test_settle_lanes_frees_a_blocked_partition_progress_lane() {
        let (result, calls, abandoned) =
            run(vec![Err(IggyError::PartitionLaneBlocked(0, 5)), Ok(2)]).await;
        assert_eq!(result, Ok(2));
        assert_eq!(calls, 2);
        assert_eq!(abandoned, vec![5]);
    }

    #[tokio::test]
    async fn test_settle_lanes_abandons_an_uncertain_outcome_of_the_second_attempt() {
        let (result, calls, abandoned) = run(vec![
            Err(IggyError::LaneBlocked(3)),
            Err(IggyError::OperationUncertain(8)),
        ])
        .await;
        assert_eq!(result, Err(IggyError::OperationUncertain(8)));
        assert_eq!(calls, 2);
        assert_eq!(abandoned, vec![3, 8]);
    }

    #[tokio::test]
    async fn test_settle_lanes_returns_a_second_refusal_instead_of_looping() {
        let (result, calls, abandoned) = run(vec![
            Err(IggyError::LaneBlocked(3)),
            Err(IggyError::LaneBlocked(4)),
        ])
        .await;
        assert_eq!(result, Err(IggyError::LaneBlocked(4)));
        assert_eq!(calls, 2, "a lane refusal is retried once, never in a loop");
        assert_eq!(abandoned, vec![3, 4]);
    }
}
