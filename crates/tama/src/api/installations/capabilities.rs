use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::api::error::error_response;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// GET /tama/v1/system/capabilities
///
/// Reports the proxy host's **toolchain** facts (os/arch/git/cmake/compiler) —
/// the install wizard uses them as build-from-source hints. The facts
/// describe the *reporting* (proxy) host only: backend installs execute on a
/// tamad host (plan-191 Task 7, ADR-0010), and the provider's tamad probes
/// its own host before building from source (see `install_from_source` in
/// the tamad crate), so in multi-host topologies these flags are
/// informational wizard hints. It does NOT probe local GPU hardware, so
/// `detected_cuda_version` is always absent (per-tamad GPU facts live on
/// `GET /tama/v1/system/gpu-devices` and the metrics stream's `hosts[]`).
pub async fn system_capabilities(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl IntoResponse {
    let cache = match &web_state.capabilities {
        Some(c) => c.clone(),
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "capabilities cache not configured",
                None,
            )
        }
    };

    match cache
        .get_or_compute(tama_core::gpu::detect_build_prerequisites)
        .await
    {
        Ok(caps) => Json(caps).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// plan-191 Task 9: the endpoint no longer probes local GPU hardware
    /// (`nvidia-smi`/`nvcc` run on the proxy host) — installs execute on a
    /// tamad, so `detected_cuda_version` is always absent. The non-hardware
    /// toolchain facts remain as proxy-host hints for the install wizard.
    #[tokio::test]
    async fn test_capabilities_does_not_detect_local_cuda() {
        let state = Arc::new(tama_core::proxy::ProxyState::new(
            tama_core::config::Config::default(),
            None,
            tama_test_support::test_dummy_pool(),
        ));
        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: Some(Arc::new(crate::web_types::CapabilitiesCache::new())),
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            db_pool: tama_test_support::test_dummy_pool(),
        });
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/system/capabilities")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body readable");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        // No local CUDA hardware probing anymore (plan-191 Task 9).
        assert!(
            json.get("detected_cuda_version").is_none(),
            "detected_cuda_version must be absent (no local GPU probe), got: {json}"
        );
        // Non-hardware facts remain.
        assert!(json["os"].is_string());
        assert!(json["arch"].is_string());
        assert!(json["supported_cuda_versions"].is_array());
    }
}
