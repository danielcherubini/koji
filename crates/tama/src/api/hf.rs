use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::api::error::error_response;
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
    if !tama_core::models::is_valid_repo_id(&repo_id) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid repo_id",
            Some("ValidationError"),
        );
    }

    // Metadata requests fetch HF repo info; all others fetch quant lists.
    if path.ends_with("/metadata") {
        match tama_core::models::pull::fetch_hf_metadata(&repo_id).await {
            Ok(meta) => (StatusCode::OK, Json(meta)).into_response(),
            Err(e) => error_response(StatusCode::BAD_GATEWAY, e.to_string(), None),
        }
    } else {
        // Quant list — call fetch_blob_metadata directly instead of looping
        // through an HTTP request to the proxy (which would hit auth middleware
        // with no credentials and return 401).
        match tama_core::models::pull::fetch_blob_metadata(&repo_id).await {
            Ok(blobs) => {
                let mut quants: Vec<QuantEntry> =
                    tama_core::models::pull::group_sharded_quants(blobs)
                        .into_iter()
                        .map(|g| QuantEntry {
                            filename: g.filename,
                            quant: g.quant,
                            size_bytes: g.size_bytes,
                            kind: g.kind,
                            shards: g.shards,
                        })
                        .collect();
                quants.sort_by(|a, b| a.filename.cmp(&b.filename));
                (StatusCode::OK, Json(quants)).into_response()
            }
            Err(e) => error_response(StatusCode::BAD_GATEWAY, e.to_string(), None),
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
    shards: Vec<String>,
}

#[cfg(test)]
mod tests {
    use crate::api::error::tests::assert_error_shape;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tama_core::config::Config;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    /// GET /tama/v1/hf/*repo_id/metadata — a repo_id containing a `..` segment
    /// should be rejected with 400 and the canonical error shape.
    #[tokio::test]
    async fn test_hf_metadata_invalid_repo_id_error_shape() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(config, None));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            repository: None,
        });

        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // Path traversal: `..` segment in repo_id should be rejected with 400.
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/hf/evil/../metadata")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "hf_metadata should return 400 for path traversal in repo_id"
        );

        let detail = assert_error_shape(resp).await;
        assert_eq!(
            detail.r#type,
            Some("ValidationError".to_string()),
            "path traversal should return ValidationError type"
        );
    }
}
