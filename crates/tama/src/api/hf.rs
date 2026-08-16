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
        match tama_core::models::pull::lookup_hf_metadata(&repo_id).await {
            Ok(meta) => (StatusCode::OK, Json(meta)).into_response(),
            Err(e) => error_response(StatusCode::BAD_GATEWAY, e.to_string(), None),
        }
    } else {
        // Quant list — call lookup_blob_metadata directly instead of looping
        // through an HTTP request to the proxy (which would hit auth middleware
        // with no credentials and return 401).
        match tama_core::models::pull::lookup_blob_metadata(&repo_id).await {
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
    use wiremock::matchers::{method, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// GET /tama/v1/hf/*repo_id/metadata — a repo_id containing a `..` segment
    /// should be rejected with 400 and the canonical error shape.
    #[tokio::test]
    async fn test_hf_metadata_invalid_repo_id_error_shape() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_core::db::pool::test_dummy_pool(),
        ));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: tama_core::db::pool::test_dummy_pool(),
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

    /// GET /tama/v1/hf/*repo_id/metadata — happy path with wiremock.
    ///
    /// The blobs call (`?blobs=true`) is a second request to the same path; the
    /// blobs mock takes precedence via `with_priority(1)`, and its sibling sizes
    /// are surfaced as `hf_total_size_bytes`/`hf_file_count`.
    #[tokio::test]
    async fn test_hf_metadata_happy_path() {
        let mock_server = MockServer::start().await;

        // Mock the HF API repo info endpoint: {endpoint}/api/models/{repo_id}
        let hf_response = serde_json::json!({
            "lastModified": "2024-01-15T00:00:00.000Z",
            "tags": ["gguf", "text-generation"],
            "pipeline_tag": "text-generation"
        });
        // Match any GET path starting with /api/models.
        Mock::given(method("GET"))
            .and(path_regex("^/api/models/.*$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&hf_response))
            .mount(&mock_server)
            .await;

        // Blobs endpoint (same path + ?blobs=true) — higher priority so it
        // wins over the base mock for requests carrying the query parameter.
        let blobs_response = serde_json::json!({
            "siblings": [
                { "rfilename": "Qwen3-8B-Q4_K_M.gguf", "size": 5_000_000_000_i64 },
                { "rfilename": "README.md", "size": 3_100_000_i64 }
            ]
        });
        Mock::given(method("GET"))
            .and(path_regex("^/api/models/.*$"))
            .and(query_param("blobs", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&blobs_response))
            .with_priority(1)
            .mount(&mock_server)
            .await;

        // Set HF_ENDPOINT env var so the handler uses our mock server.
        std::env::set_var("HF_ENDPOINT", mock_server.uri());

        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_core::db::pool::test_dummy_pool(),
        ));

        // Build a minimal router with just the hf route for isolation.
        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: tama_core::db::pool::test_dummy_pool(),
        });

        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // Request metadata for a repo.
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/hf/bartowski/Qwen3-8B-GGUF/metadata")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let json: serde_json::Value =
            serde_json::from_slice(&body_str).expect("body should be valid JSON");

        // Assert parsed metadata fields (HfModelMetadata serializes with hf_ prefix).
        assert_eq!(json["hf_pipeline_tag"], "text-generation");
        assert_eq!(json["hf_last_modified"], "2024-01-15T00:00:00.000Z");

        // Repo stats from the blobs call: 5_000_000_000 + 3_100_000 bytes, 2 files.
        assert_eq!(json["hf_total_size_bytes"], 5_003_100_000u64);
        assert_eq!(json["hf_file_count"], 2u32);

        // Clean up env var.
        std::env::remove_var("HF_ENDPOINT");
    }

    /// GET /tama/v1/hf/*repo_id/metadata — a failing blobs endpoint (500) must
    /// soft-fail: the response is still 200 and `hf_total_size_bytes`/
    /// `hf_file_count` are null.
    #[tokio::test]
    async fn test_hf_metadata_soft_fails_when_blobs_error() {
        let mock_server = MockServer::start().await;

        // Base model info for the repo.
        let hf_response = serde_json::json!({
            "lastModified": "2024-01-15T00:00:00.000Z",
            "tags": ["gguf"],
            "pipeline_tag": "text-generation"
        });
        Mock::given(method("GET"))
            .and(path_regex("^/api/models/.*$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&hf_response))
            .mount(&mock_server)
            .await;

        // Blobs endpoint errors — higher priority so it wins for ?blobs=true.
        Mock::given(method("GET"))
            .and(path_regex("^/api/models/.*$"))
            .and(query_param("blobs", "true"))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(1)
            .mount(&mock_server)
            .await;

        std::env::set_var("HF_ENDPOINT", mock_server.uri());

        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_core::db::pool::test_dummy_pool(),
        ));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: tama_core::db::pool::test_dummy_pool(),
        });

        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/hf/bartowski/Qwen3-8B-GGUF/metadata")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let json: serde_json::Value =
            serde_json::from_slice(&body_str).expect("body should be valid JSON");

        // Metadata still served; repo stats soft-failed to null.
        assert_eq!(json["hf_pipeline_tag"], "text-generation");
        assert!(json["hf_total_size_bytes"].is_null(), "size must be null");
        assert!(json["hf_file_count"].is_null(), "file count must be null");

        std::env::remove_var("HF_ENDPOINT");
    }

    /// GET /tama/v1/hf/*repo_id — a repo_id containing `..` should return 400.
    #[tokio::test]
    async fn test_hf_metadata_rejects_traversal() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_core::db::pool::test_dummy_pool(),
        ));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: tama_core::db::pool::test_dummy_pool(),
        });

        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // Path with `..` in repo_id.
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/hf/evil/../secret/metadata")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "hf_metadata should reject path traversal"
        );
    }
}
