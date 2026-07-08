//! Downloads Center API endpoints.
//!
//! Provides REST endpoints to query the download queue (active + history),
//! cancel items, and stream real-time events via SSE.
use crate::api::error::error_body;
use tama_core::proxy::ProxyState;

use async_stream::stream;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{
    sse::{Event, KeepAlive},
    Json, Sse,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;

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
pub struct DownloadsActiveResponse {
    pub items: Vec<PullQueueItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadsHistoryResponse {
    pub items: Vec<PullQueueItemDto>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadCancelResponse {
    pub ok: bool,
    pub message: Option<String>,
}

/// Convert a `PullQueueDto` to a `PullQueueItemDto`.
/// Note: progress_percent is computed client-side from bytes_pulled
/// and total_bytes, so it's not included in the API response.
fn item_to_dto(item: &tama_core::db::repository::PullQueueDto) -> PullQueueItemDto {
    PullQueueItemDto {
        job_id: item.job_id.clone(),
        repo_id: item.repo_id.clone(),
        filename: item.filename.clone(),
        display_name: item.display_name.clone(),
        status: item.status.clone(),
        bytes_pulled: item.bytes_pulled,
        total_bytes: item.total_bytes,
        error_message: item.error_message.clone(),
        started_at: item.started_at.clone(),
        completed_at: item.completed_at.clone(),
        queued_at: item.queued_at.clone(),
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

/// GET /tama/v1/downloads/active
pub async fn get_active_pulls(
    State(state): State<Arc<ProxyState>>,
) -> Result<Json<DownloadsActiveResponse>, (StatusCode, Json<serde_json::Value>)> {
    let svc = state.pull_queue().as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_body(
                "Download queue not configured",
                Some("ServiceUnavailableError"),
            )),
        )
    })?;

    let items = svc.get_active_items_dto().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_body(e.to_string(), None)),
        )
    })?;

    let dto_items: Vec<PullQueueItemDto> = items.iter().map(item_to_dto).collect();

    Ok(Json(DownloadsActiveResponse { items: dto_items }))
}

/// GET /tama/v1/downloads/history?limit=50&offset=0
pub async fn get_pull_history(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> Result<Json<DownloadsHistoryResponse>, (StatusCode, Json<serde_json::Value>)> {
    let svc = state.pull_queue().as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_body(
                "Download queue not configured",
                Some("ServiceUnavailableError"),
            )),
        )
    })?;

    let items = svc
        .get_history_items_dto(query.limit, query.offset)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_body(e.to_string(), None)),
            )
        })?;

    let total = svc.count_history_items().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_body(e.to_string(), None)),
        )
    })?;

    let dto_items: Vec<PullQueueItemDto> = items.iter().map(item_to_dto).collect();

    Ok(Json(DownloadsHistoryResponse {
        items: dto_items,
        total,
    }))
}

/// POST /tama/v1/downloads/:job_id/cancel
pub async fn cancel_pull(
    State(state): State<Arc<ProxyState>>,
    Path(job_id): axum::extract::Path<String>,
) -> Json<DownloadCancelResponse> {
    let svc = match &state.pull_queue() {
        Some(svc) => svc,
        None => {
            return Json(DownloadCancelResponse {
                ok: false,
                message: Some("Download queue not configured".to_string()),
            })
        }
    };

    match svc.cancel(&job_id) {
        Ok(()) => Json(DownloadCancelResponse {
            ok: true,
            message: None,
        }),
        Err(e) => Json(DownloadCancelResponse {
            ok: false,
            message: Some(e.to_string()),
        }),
    }
}

/// GET /tama/v1/downloads/events — SSE stream of download lifecycle events.
pub async fn pull_events_sse(
    State(state): State<Arc<ProxyState>>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
    let svc = state
        .pull_queue()
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let mut rx = svc.subscribe_events();

    let stream = stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let sse_event = match event {
                        tama_core::proxy::pull_queue::PullEvent::Started { job_id, repo_id, filename, total_bytes } => {
                            Event::default()
                                .event("Started")
                                .json_data(serde_json::json!({
                                    "event": "Started",
                                    "job_id": job_id,
                                    "repo_id": repo_id,
                                    "filename": filename,
                                    "total_bytes": total_bytes,
                                }))
                        }
                        tama_core::proxy::pull_queue::PullEvent::Progress { job_id, bytes_pulled, total_bytes } => {
                            Event::default()
                                .event("Progress")
                                .json_data(serde_json::json!({
                                    "event": "Progress",
                                    "job_id": job_id,
                                    "bytes_pulled": bytes_pulled,
                                    "total_bytes": total_bytes,
                                }))
                        }
                        tama_core::proxy::pull_queue::PullEvent::Verifying { job_id, filename } => {
                            Event::default()
                                .event("Verifying")
                                .json_data(serde_json::json!({
                                    "event": "Verifying",
                                    "job_id": job_id,
                                    "filename": filename,
                                }))
                        }
                        tama_core::proxy::pull_queue::PullEvent::Completed { job_id, filename, size_bytes, duration_ms } => {
                            Event::default()
                                .event("Completed")
                                .json_data(serde_json::json!({
                                    "event": "Completed",
                                    "job_id": job_id,
                                    "filename": filename,
                                    "size_bytes": size_bytes,
                                    "duration_ms": duration_ms,
                                }))
                        }
                        tama_core::proxy::pull_queue::PullEvent::Failed { job_id, filename, error } => {
                            Event::default()
                                .event("Failed")
                                .json_data(serde_json::json!({
                                    "event": "Failed",
                                    "job_id": job_id,
                                    "filename": filename,
                                    "error": error,
                                }))
                        }
                        tama_core::proxy::pull_queue::PullEvent::Cancelled { job_id, filename } => {
                            Event::default()
                                .event("Cancelled")
                                .json_data(serde_json::json!({
                                    "event": "Cancelled",
                                    "job_id": job_id,
                                    "filename": filename,
                                }))
                        }
                        tama_core::proxy::pull_queue::PullEvent::Queued { job_id, repo_id, filename } => {
                            Event::default()
                                .event("Queued")
                                .json_data(serde_json::json!({
                                    "event": "Queued",
                                    "job_id": job_id,
                                    "repo_id": repo_id,
                                    "filename": filename,
                                }))
                        }
                    };

                    match sse_event {
                        Ok(e) => yield Ok(e),
                        Err(e) => yield Err(axum::Error::new(e)),
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Client fell behind; emit a marker event with the lag count.
                    yield Ok(Event::default()
                        .event("Lagged")
                        .json_data(serde_json::json!({ "lagged": n }))?);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
