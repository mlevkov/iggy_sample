use axum::Json;
use axum::extract::State;
use tracing::instrument;

use crate::error::AppResult;
use crate::middleware::RequestTimeout;
use crate::models::ClusterInfo;
use crate::state::AppState;

/// The cluster's nodes, roles and health, from the connected server's cluster
/// metadata.
///
/// The role is the node's role in metadata consensus; a partition's primary
/// is placed separately, and a topic read reports which nodes hold each
/// partition. `leader` is `null` when the answering node marked none, which
/// it does briefly during an election.
///
/// Node `status` is reported as the server gives it, and server 0.10 gives
/// `healthy` even for a node that is down. Which replicas are actually up
/// shows in each partition's `in_sync_members` on a topic read.
#[instrument(skip(state, timeout))]
pub async fn cluster(
    State(state): State<AppState>,
    timeout: Option<RequestTimeout>,
) -> AppResult<Json<ClusterInfo>> {
    let metadata = state.iggy_scoped(timeout).cluster_metadata().await?;
    Ok(Json(ClusterInfo::from(metadata)))
}
