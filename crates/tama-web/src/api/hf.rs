use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use tama_core::config::QuantKind;
use tama_core::proxy::ProxyState;

/// GET /tama/v1/hf/*repo_id — fetch HuggingFace model metadata (API + README).
/// Wildcard captures `owner/repo/metadata`; we strip the trailing `/metadata`.
/// If path doesn't end with `/metadata`, return quant list for the repo.
pub async fn hf_metadata(
    _state: State<Arc<ProxyState>>,
    Path(path): Path<String>,
) -> axum::http::Response<axum::body::Body> {
    // Strip trailing "/metadata" from the wildcard path
    let repo_id = match path.strip_suffix("/metadata") {
        Some(r) => r.to_string(),
        None => path.clone(),
    };

    // Reject path traversal sequences (SSRF mitigation)
    if !repo_id
        .split('/')
        .all(|s| !s.is_empty() && s != ".." && !s.contains('\0'))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid repo_id" })),
        )
            .into_response();
    }

    // Metadata requests fetch HF repo info; all others fetch quant lists.
    if path.ends_with("/metadata") {
        match tama_core::models::pull::fetch_hf_metadata(&repo_id).await {
            Ok(meta) => (StatusCode::OK, Json(meta)).into_response(),
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        }
    } else {
        // Quant list — call fetch_blob_metadata directly instead of looping
        // through an HTTP request to the proxy (which would hit auth middleware
        // with no credentials and return 401).
        match tama_core::models::pull::fetch_blob_metadata(&repo_id).await {
            Ok(blobs) => {
                let mut quants: Vec<QuantEntry> = blobs
                    .into_values()
                    .map(|b| QuantEntry {
                        filename: b.filename,
                        quant: tama_core::models::pull::infer_quant_from_filename(&b.filename),
                        size_bytes: b.size,
                        kind: QuantKind::from_filename(&b.filename),
                    })
                    .collect();
                quants.sort_by(|a, b| a.filename.cmp(&b.filename));
                (StatusCode::OK, Json(quants)).into_response()
            }
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        }
    }
}

/// Response shape for quant list entries (matches what the frontend deserializes).
#[derive(serde::Serialize, Clone, Debug)]
struct QuantEntry {
    filename: String,
    quant: Option<String>,
    size_bytes: Option<i64>,
    kind: QuantKind,
}
