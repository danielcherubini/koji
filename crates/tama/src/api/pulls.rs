//! Pulls Center API endpoints.
//!
//! Provides REST endpoints to query the pull queue (active + history),
//! cancel items, and stream real-time events via SSE.
use crate::api::error::error_body;
use tama_core::proxy::ProxyState;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{
    sse::{Event, KeepAlive},
    Json, Sse,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── DTO types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullQueueItemDto {
    pub job_id: String,
    pub repo_id: String,
    pub filename: String,
    pub display_name: Option<String>,
    pub status: String,
    pub bytes_pulled: i64,
    pub total_bytes: Option<i64>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub queued_at: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullsActiveResponse {
    pub items: Vec<PullQueueItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullsHistoryResponse {
    pub items: Vec<PullQueueItemDto>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullCancelResponse {
    pub ok: bool,
    pub message: Option<String>,
}

/// Convert a `PullQueueItem` to a `PullQueueItemDto`.
/// Note: progress_percent is computed client-side from bytes_pulled
/// and total_bytes, so it's not included in the API response.
/// Format a timestamp the same shape the API always emitted: `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn format_ts(dt: &sqlx::types::time::OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.millisecond(),
    )
}

fn item_to_dto(item: &tama_core::db::queries::PullQueueItem) -> PullQueueItemDto {
    PullQueueItemDto {
        job_id: item.job_id.clone(),
        repo_id: item.repo_id.clone(),
        filename: item.filename.clone(),
        display_name: item.display_name.clone(),
        status: item.status.clone(),
        bytes_pulled: item.bytes_pulled,
        total_bytes: item.total_bytes,
        error_message: item.error_message.clone(),
        started_at: item.started_at.as_ref().map(format_ts),
        completed_at: item.completed_at.as_ref().map(format_ts),
        queued_at: format_ts(&item.queued_at),
        kind: item.kind.clone(),
    }
}

// ── Query params for history endpoint ────────────────────────────────────────

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default = "default_offset")]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

fn default_offset() -> i64 {
    0
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// GET /tama/v1/pulls/active
pub async fn get_active_pulls(
    State(state): State<Arc<ProxyState>>,
) -> Result<Json<PullsActiveResponse>, (StatusCode, Json<serde_json::Value>)> {
    let svc = state.pull_queue().as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_body(
                "Pull queue not configured",
                Some("ServiceUnavailableError"),
            )),
        )
    })?;

    let items = svc.get_active_items_dto().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_body(e.to_string(), None)),
        )
    })?;

    let dto_items: Vec<PullQueueItemDto> = items.iter().map(item_to_dto).collect();

    Ok(Json(PullsActiveResponse { items: dto_items }))
}

/// GET /tama/v1/pulls/history?limit=50&offset=0
pub async fn get_pull_history(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> Result<Json<PullsHistoryResponse>, (StatusCode, Json<serde_json::Value>)> {
    let svc = state.pull_queue().as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_body(
                "Pull queue not configured",
                Some("ServiceUnavailableError"),
            )),
        )
    })?;

    let items = svc
        .get_history_items_dto(query.limit, query.offset)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_body(e.to_string(), None)),
            )
        })?;

    let total = svc.count_history_items().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_body(e.to_string(), None)),
        )
    })?;

    let dto_items: Vec<PullQueueItemDto> = items.iter().map(item_to_dto).collect();

    Ok(Json(PullsHistoryResponse {
        items: dto_items,
        total,
    }))
}

/// POST /tama/v1/pulls/:job_id/cancel
pub async fn cancel_pull(
    State(state): State<Arc<ProxyState>>,
    Path(job_id): axum::extract::Path<String>,
) -> Json<PullCancelResponse> {
    let svc = match &state.pull_queue() {
        Some(svc) => svc,
        None => {
            return Json(PullCancelResponse {
                ok: false,
                message: Some("Pull queue not configured".to_string()),
            })
        }
    };

    match svc.cancel(&job_id).await {
        Ok(()) => {
            // Best-effort: hand the cancel to the host running the pull
            // (plan-191 follow-up B: `CancelJob` RPC; the relay converges
            // the job state when the terminal `cancelled` event arrives).
            {
                if let Some(tamad_job_id) = state.pull_job_tamad_job_id(&job_id).await {
                    state.cancel_pull_host_job(&tamad_job_id).await;
                }
            }
            Json(PullCancelResponse {
                ok: true,
                message: None,
            })
        }
        Err(e) => Json(PullCancelResponse {
            ok: false,
            message: Some(e.to_string()),
        }),
    }
}

/// GET /tama/v1/pulls/events — SSE stream of pull lifecycle events.
pub async fn pull_events_sse(
    State(state): State<Arc<ProxyState>>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
    let svc = state
        .pull_queue()
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let rx = svc.subscribe_events();
    let stream = crate::api::sse::broadcast_to_sse(
        rx,
        tama_core::proxy::pull_queue::PullEvent::to_sse_event,
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
