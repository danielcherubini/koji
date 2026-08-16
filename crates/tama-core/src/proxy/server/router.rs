use axum::{
    middleware,
    routing::{get, patch, post},
    Router,
};
use std::sync::Arc;

use crate::proxy::auth::{
    auth_middleware, handle_login, handle_login_callback, handle_login_error,
};
use crate::proxy::scope_middleware::scope_middleware;
#[cfg(feature = "web-ui")]
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;

use crate::proxy::handlers::chat::{handle_chat_completions, handle_stream_chat_completions};
use crate::proxy::handlers::compaction::handle_compaction;
use crate::proxy::handlers::forward::{handle_fallback, handle_forward_get, handle_forward_post};
use crate::proxy::handlers::models::{handle_get_model, handle_list_models};
use crate::proxy::handlers::status::{
    handle_health, handle_metrics, handle_reload_configs, handle_status,
};
use crate::proxy::handlers::tts::{
    handle_audio_models, handle_audio_speech, handle_audio_stream, handle_audio_voices,
};
use crate::proxy::tama_handlers::{
    handle_opencode_list_models, handle_pull_job_stream, handle_system_metrics_stream,
    handle_tama_api_keys_create, handle_tama_api_keys_list, handle_tama_api_keys_revoke,
    handle_tama_api_keys_update, handle_tama_cancel_load, handle_tama_get_pull_job,
    handle_tama_load_model, handle_tama_pull_model, handle_tama_system_gpu_devices,
    handle_tama_system_gpu_devices_refresh, handle_tama_system_restart, handle_tama_unload_model,
};
use crate::proxy::ProxyState;

/// A single proxy route: (method-label, path, method handler).
type ProxyRoute = (
    &'static str,
    &'static str,
    axum::routing::MethodRouter<Arc<ProxyState>>,
);

/// The single source of truth for proxy-owned routes: OpenAI-compatible
/// inference, model lifecycle, auth, and proxy ops. Management CRUD routes
/// live in the `tama` crate's router (`crates/tama/src/router.rs`) — do NOT
/// add them here. The ownership test in `crates/tama/tests/router_ownership_test.rs`
/// enforces the boundary.
fn proxy_routes() -> Vec<ProxyRoute> {
    vec![
        // OpenAI-compatible inference
        ("POST", "/v1", post(handle_chat_completions)),
        (
            "POST",
            "/v1/chat/completions",
            post(handle_chat_completions),
        ),
        (
            "POST",
            "/v1/chat/completions/stream",
            post(handle_stream_chat_completions),
        ),
        // Model lifecycle (load/unload/cancel only; GET is managed by tama router)
        (
            "POST",
            "/tama/v1/models/:id/load",
            post(handle_tama_load_model),
        ),
        (
            "POST",
            "/tama/v1/models/:id/unload",
            post(handle_tama_unload_model),
        ),
        (
            "POST",
            "/tama/v1/models/:id/cancel",
            post(handle_tama_cancel_load),
        ),
        // Model listing (GET /v1/models is proxy-owned; GET /tama/v1/models is tama router)
        ("GET", "/v1/models", get(handle_list_models)),
        // NOTE: `:model_id` captures only a single path segment, so aliases that
        // contain slashes (e.g., "org/model") will not match this route.
        // Chat completions are unaffected (model name arrives in the JSON body),
        // but `GET /v1/models/{id}` lookups require the alias without slashes.
        ("GET", "/v1/models/:model_id", get(handle_get_model)),
        // OpenCode plugin discovery
        (
            "GET",
            "/v1/opencode/models",
            get(handle_opencode_list_models),
        ),
        // Pull jobs
        ("POST", "/tama/v1/pulls", post(handle_tama_pull_model)),
        (
            "GET",
            "/tama/v1/pulls/:job_id",
            get(handle_tama_get_pull_job),
        ),
        (
            "GET",
            "/tama/v1/pulls/:job_id/stream",
            get(handle_pull_job_stream),
        ),
        // API keys management
        (
            "GET+POST",
            "/tama/v1/keys",
            get(handle_tama_api_keys_list).post(handle_tama_api_keys_create),
        ),
        (
            "PATCH+DELETE",
            "/tama/v1/keys/:id",
            patch(handle_tama_api_keys_update).delete(handle_tama_api_keys_revoke),
        ),
        // System management
        (
            "POST",
            "/tama/v1/system/reload-configs",
            post(handle_reload_configs),
        ),
        (
            "GET",
            "/tama/v1/system/metrics/stream",
            get(handle_system_metrics_stream),
        ),
        (
            "GET",
            "/tama/v1/system/gpu-devices",
            get(handle_tama_system_gpu_devices),
        ),
        (
            "POST",
            "/tama/v1/system/gpu-devices/refresh",
            post(handle_tama_system_gpu_devices_refresh),
        ),
        (
            "POST",
            "/tama/v1/system/restart",
            post(handle_tama_system_restart),
        ),
        // Health, status, metrics
        ("GET", "/status", get(handle_status)),
        ("GET", "/health", get(handle_health)),
        ("GET", "/metrics", get(handle_metrics)),
        // OAuth2 login flow
        ("GET", "/login", get(handle_login)),
        ("GET", "/login/callback", get(handle_login_callback)),
        ("GET", "/login/error", get(handle_login_error)),
        // TTS (OpenAI-compatible)
        ("GET", "/v1/audio/models", get(handle_audio_models)),
        ("POST", "/v1/audio/speech", post(handle_audio_speech)),
        ("POST", "/v1/audio/speech/stream", post(handle_audio_stream)),
        ("GET", "/v1/audio/voices", get(handle_audio_voices)),
        // Compaction
        ("POST", "/v1/compaction", post(handle_compaction)),
        // Wildcard forwarding
        ("POST", "/*path", post(handle_forward_post)),
    ]
}

/// (method-label, path) pairs from `proxy_routes()` — for the cross-crate
/// ownership test. Labels for multi-method routes are "GET+POST" style.
pub fn proxy_route_paths() -> Vec<(&'static str, &'static str)> {
    proxy_routes().into_iter().map(|(m, p, _)| (m, p)).collect()
}

fn fold_proxy_routes() -> Router<Arc<ProxyState>> {
    let mut router = Router::new();
    for (_, path, method_router) in proxy_routes() {
        router = router.route(path, method_router);
    }
    router
}

fn apply_shared_layers(router: Router<Arc<ProxyState>>, state: Arc<ProxyState>) -> Router {
    router
        .layer(middleware::from_fn(scope_middleware))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

/// Build the axum router with all proxy routes and shared state.
pub async fn build_router(state: Arc<ProxyState>) -> Router {
    apply_shared_layers(
        fold_proxy_routes()
            .route("/*path", get(handle_forward_get))
            .fallback(handle_fallback),
        state,
    )
}

/// Build a unified axum Router that merges proxy routes with an extra router
/// (e.g., web UI routes from `tama-web`).
///
/// Route priority is critical: proxy-specific routes (e.g., `/tama/v1/models/:id/load`)
/// must be defined before extra catch-alls (e.g., `/tama/v1/models/:id`) so that
/// axum matches the more specific handler first.
///
/// The `extra_routes` parameter is a `Router<Arc<ProxyState>>` without `.with_state()` called.
/// This function merges proxy routes first (higher priority), then extra routes,
/// and applies shared layers + state.
#[cfg(feature = "web-ui")]
pub async fn build_unified_router(
    state: Arc<ProxyState>,
    extra_routes: Router<Arc<ProxyState>>,
) -> Router {
    apply_shared_layers(
        Router::new().merge(fold_proxy_routes()).merge(extra_routes),
        state,
    )
    .layer(CatchPanicLayer::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that the proxy router returns 200 for known proxy endpoints.
    #[tokio::test]
    async fn test_proxy_router_serves_known_routes() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config, None, None));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let app = build_router(state.clone()).await;
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();

        // Health endpoint
        let resp = client
            .get(format!("http://{}/health", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Models endpoint
        let resp = client
            .get(format!("http://{}/v1/models", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Status endpoint
        let resp = client
            .get(format!("http://{}/status", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    /// Verify that proxy-specific routes take priority over extra catch-alls.
    /// The `/tama/v1/models/:id/load` endpoint should return a proxy response,
    /// not a 405 from the extra router, proving the route ordering is correct.
    #[cfg(feature = "web-ui")]
    #[tokio::test]
    async fn test_unified_router_route_priority() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config, None, None));

        // Simulate web UI routes: PUT/DELETE on /tama/v1/models/:id
        // (GET is handled by web UI in the real app, not defined here to avoid overlap)
        let extra_routes = Router::new().route(
            "/tama/v1/models/:id",
            axum::routing::put(|| async { "web put " }).delete(|| async { "web delete " }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let app = build_unified_router(state.clone(), extra_routes).await;
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();

        // POST to /tama/v1/models/test/load — should be handled by proxy's
        // handle_tama_load_model, not by extra router's catch-all.
        let resp = client
            .post(format!("http://{}/tama/v1/models/test/load", bound_addr))
            .send()
            .await
            .unwrap();
        // Must NOT be 405 (Method Not Allowed) — that would mean the extra
        // route for /tama/v1/models/:id matched instead of our proxy route.
        assert_ne!(
            resp.status(),
            405,
            "Route priority failed: extra router caught /tama/v1/models/:id/load instead of proxy handler"
        );

        // POST to /tama/v1/models/test/unload — same priority check
        let resp = client
            .post(format!("http://{}/tama/v1/models/test/unload", bound_addr))
            .send()
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            405,
            "Route priority failed: extra router caught /tama/v1/models/:id/unload instead of proxy handler"
        );

        // POST to /tama/v1/models/test/cancel — should be handled by proxy's
        // handle_tama_cancel_load, not by extra router's catch-all.
        let resp = client
            .post(format!("http://{}/tama/v1/models/test/cancel", bound_addr))
            .send()
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            405,
            "Route priority failed: extra router caught /tama/v1/models/:id/cancel instead of proxy handler"
        );

        // GET /health — proxy route
        let resp = client
            .get(format!("http://{}/health", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // GET /v1/models — proxy route
        let resp = client
            .get(format!("http://{}/v1/models", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}
