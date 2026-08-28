use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode, Uri},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use include_dir::{include_dir, Dir};
use std::sync::Arc;
use tower_http::{catch_panic::CatchPanicLayer, cors::CorsLayer};

use crate::api;
use crate::api::aliases::{create_alias, delete_alias, get_alias, list_aliases, update_alias};
use crate::api::backup::{create_backup, restore_preview, start_restore};
use crate::api::benchmarks::{
    benchmark_events, delete_benchmark, get_benchmark_result, list_benchmark_history,
    run_benchmark, run_benchmark_suite, run_mtp_benchmark, run_spec_benchmark,
};
use crate::api::installations::{
    activate_installation_version, check_installation_updates, compaction::update_compaction,
    get_job, install_installation, job_events_sse, list_installation_versions, list_installations,
    patch_installation, register_installation, remove_installation, remove_installation_version,
    rename_installation, system_capabilities, update_installation,
    update_installation_default_args, update_installation_default_env, update_installation_source,
};
use crate::api::providers::{
    create_provider, delete_provider, get_provider, list_providers, update_provider,
};
use crate::api::tamads::{
    create_tamad, delete_tamad, get_tamad, list_tamads, trigger_health_check, update_tamad,
};
use tama_core::proxy::{
    forward_to_backend,
    tama_handlers::{
        handle_tama_system_health,
        logs_api::{
            handle_delete_logs, handle_log_events_sse, handle_log_export, handle_log_query,
            handle_log_sources, handle_log_status, handle_log_stream, handle_log_summary,
            LogsApiState,
        },
    },
    ProxyState,
};

/// Embedded dist/ directory for serving the web UI.
static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/dist");

/// Serve a static file from the embedded `dist/` directory.
async fn serve_static(path: Option<Path<String>>) -> Response {
    let file_path = path.map(|p| p.0).unwrap_or_else(|| "index.html".into());
    let file_path = if file_path.is_empty() || file_path == "/" {
        "index.html".to_string()
    } else {
        file_path
    };

    match DIST.get_file(&file_path) {
        Some(f) => {
            let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
            Response::builder()
                .header("Content-Type", mime.as_ref())
                .body(Body::from(f.contents()))
                .unwrap()
        }
        None => {
            // SPA fallback: return index.html for unknown paths
            match DIST.get_file("index.html") {
                Some(f) => Html(std::str::from_utf8(f.contents()).unwrap_or("")).into_response(),
                None => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
            }
        }
    }
}

/// Dedicated handler for the root path — avoids Axum type-inference issues with inline closures.
async fn serve_index() -> Response {
    serve_static(None).await
}

/// Redirect old /ui/* paths to /tama, preserving query strings.
async fn redirect_to_tama(path: Path<String>, uri: Uri) -> Response {
    let query = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();
    let target = if path.0.is_empty() {
        format!("/tama{}", query)
    } else {
        format!("/tama/{}{}", path.0, query)
    };
    (
        StatusCode::SEE_OTHER,
        [(axum::http::header::LOCATION, target)],
    )
        .into_response()
}

/// Redirect /ui root to /tama, preserving query strings.
async fn redirect_ui_root(uri: Uri) -> Response {
    let query = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();
    (
        StatusCode::SEE_OTHER,
        [(axum::http::header::LOCATION, format!("/tama{}", query))],
    )
        .into_response()
}

/// Forward root-level paths to the backend.
/// All static/web files live under /tama/*; this catches /slots, /tokenize,
/// /health, etc. and forwards them to the first available backend server.
async fn handle_root_forward(State(state): State<Arc<ProxyState>>, req: Request<Body>) -> Response {
    let (parts, body) = req.into_parts();
    forward_to_backend(&state, parts, body).await
}

/// Middleware that adds `Deprecation: true` header to responses from deprecated
/// alias routes (old `/tama/v1/backends/*` paths).
async fn deprecated_alias_middleware(req: Request<Body>, next: axum::middleware::Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert("Deprecation", "true".parse().unwrap());
    resp
}

/// Build the web UI routes without attaching state.
///
/// This table owns ALL `/tama/v1` management routes. Core's router
/// (`tama_core::proxy::server::router`) owns only inference/lifecycle/auth —
/// `crates/tama/tests/router_ownership_test.rs` asserts the two tables stay disjoint.
///
/// The caller (e.g., the proxy server) merges this router with proxy routes
/// and calls `.with_state(state)` on the merged result.
///
/// `web_state` is added as an Extension layer on sub-routers so that
/// handlers can extract it via `Extension<WebState>`.
pub fn build_web_routes(
    web_state: Arc<crate::web_types::WebState>,
) -> Router<Arc<tama_core::proxy::ProxyState>> {
    // Build sub-router for installations API with CORS and origin enforcement.
    // CorsLayer must be outermost (applied last) so it runs before same-origin check.
    let backend_routes = Router::new()
        .route("/tama/v1/system/capabilities", get(system_capabilities))
        // Installation routes
        .route(
            "/tama/v1/installations",
            post(register_installation).get(list_installations),
        )
        // Install/update endpoints: 16MB body limit
        .route(
            "/tama/v1/installations/install",
            post(install_installation)
                .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/tama/v1/installations/:name/update",
            post(update_installation).layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/tama/v1/installations/:name",
            delete(remove_installation).patch(patch_installation),
        )
        .route(
            "/tama/v1/installations/:name/default-args",
            post(update_installation_default_args),
        )
        .route(
            "/tama/v1/installations/:name/default-env",
            post(update_installation_default_env),
        )
        .route(
            "/tama/v1/installations/:name/versions/:version",
            delete(remove_installation_version),
        )
        .route(
            "/tama/v1/installations/check-updates",
            post(check_installation_updates),
        )
        .route(
            "/tama/v1/installations/:name/versions",
            get(list_installation_versions),
        )
        .route(
            "/tama/v1/installations/:name/activate",
            post(activate_installation_version),
        )
        .route(
            "/tama/v1/installations/:name/source",
            post(update_installation_source),
        )
        .route(
            "/tama/v1/installations/:name/rename",
            post(rename_installation),
        )
        .route("/tama/v1/installations/jobs/:id", get(get_job))
        .route(
            "/tama/v1/installations/jobs/:id/events",
            get(job_events_sse),
        )
        .route("/tama/v1/installations/compaction", post(update_compaction));
    // Deprecated aliases for old /tama/v1/backends/* paths
    let deprecated_aliases = Router::new()
        .route(
            "/tama/v1/backends",
            post(register_installation).get(list_installations),
        )
        .route(
            "/tama/v1/backends/install",
            post(install_installation)
                .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/tama/v1/backends/:name/update",
            post(update_installation).layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/tama/v1/backends/:name",
            delete(remove_installation).patch(patch_installation),
        )
        .route(
            "/tama/v1/backends/:name/default-args",
            post(update_installation_default_args),
        )
        .route(
            "/tama/v1/backends/:name/default-env",
            post(update_installation_default_env),
        )
        .route(
            "/tama/v1/backends/:name/versions/:version",
            delete(remove_installation_version),
        )
        .route(
            "/tama/v1/backends/check-updates",
            post(check_installation_updates),
        )
        .route(
            "/tama/v1/backends/:name/versions",
            get(list_installation_versions),
        )
        .route(
            "/tama/v1/backends/:name/activate",
            post(activate_installation_version),
        )
        .route(
            "/tama/v1/backends/:name/source",
            post(update_installation_source),
        )
        .route("/tama/v1/backends/:name/rename", post(rename_installation))
        .route("/tama/v1/backends/jobs/:id", get(get_job))
        .route("/tama/v1/backends/jobs/:id/events", get(job_events_sse))
        .route("/tama/v1/backends/compaction", post(update_compaction))
        // Add Deprecation header to all deprecated alias responses
        .layer(middleware::from_fn(deprecated_alias_middleware));

    // Merge deprecated aliases into backend_routes (which has CORS + same-origin)
    let backend_routes = backend_routes
        .merge(deprecated_aliases)
        // Backup download (GET passes through CSRF token issuance)
        .route("/tama/v1/backup", get(create_backup))
        // Restore routes (CSRF-protected)
        .route(
            "/tama/v1/restore/preview",
            post(restore_preview).layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route("/tama/v1/restore", post(start_restore))
        // Self-update POST is inside backend_routes for CSRF protection
        .route(
            "/tama/v1/self-update/update",
            post(api::self_update::trigger_update),
        )
        .route("/tama/v1/updates/check", post(api::updates::trigger_check))
        .route(
            "/tama/v1/updates/check/:item_type/:item_id",
            post(api::updates::check_item_for_update),
        )
        .route(
            "/tama/v1/updates/events",
            get(api::updates::update_events_sse),
        )
        .route(
            "/tama/v1/updates/apply/backend/:name",
            post(api::updates::apply_backend_update),
        )
        .route(
            "/tama/v1/updates/apply/model/:id",
            post(api::updates::apply_model_update),
        )
        .route("/tama/v1/updates", get(api::updates::get_updates))
        // CORS layer outermost (applied last) so it runs before same-origin enforcement
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::DELETE,
                    axum::http::Method::PATCH,
                ])
                .allow_headers(tower_http::cors::Any)
                // Expose X-CSRF-Token so JS can read it from GET responses
                .expose_headers([axum::http::HeaderName::from_static("x-csrf-token")]),
        )
        .layer(middleware::from_fn(api::middleware::enforce_same_origin));

    // 1MB body limit for all JSON API endpoints
    let json_body_limit = axum::extract::DefaultBodyLimit::max(1024 * 1024);

    // Sub-router for non-backend state-changing endpoints with CSRF enforcement
    let csrf_routes = Router::new()
        .route("/tama/v1/logs", delete(handle_delete_logs))
        .route(
            "/tama/v1/config",
            get(api::get_config)
                .post(api::save_config)
                .layer(json_body_limit),
        )
        .route(
            "/tama/v1/config/structured",
            get(api::get_structured_config)
                .post(api::save_structured_config)
                .patch(api::patch_structured_config)
                .layer(json_body_limit),
        )
        .route(
            "/tama/v1/models",
            get(api::list_models)
                .post(api::create_model)
                .layer(json_body_limit),
        )
        .route(
            "/tama/v1/models/:id",
            get(api::get_model)
                .put(api::update_model)
                .patch(api::patch_model)
                .delete(api::delete_model),
        )
        .route(
            "/tama/v1/models/:id/rename",
            post(api::rename_model).layer(json_body_limit),
        )
        .route(
            "/tama/v1/models/:id/refresh",
            post(api::refresh_model_metadata).layer(json_body_limit),
        )
        .route(
            "/tama/v1/models/:id/verify",
            post(api::verify_model_files).layer(json_body_limit),
        )
        .route(
            "/tama/v1/models/:id/quants/:quant_key",
            delete(api::delete_quant),
        )
        .route(
            "/tama/v1/benchmarks/run",
            post(run_benchmark).layer(json_body_limit),
        )
        .route(
            "/tama/v1/benchmarks/spec-run",
            post(run_spec_benchmark).layer(json_body_limit),
        )
        .route(
            "/tama/v1/benchmarks/mtp-run",
            post(run_mtp_benchmark).layer(json_body_limit),
        )
        .route(
            "/tama/v1/benchmarks/suite",
            post(run_benchmark_suite).layer(json_body_limit),
        )
        .route(
            "/tama/v1/pulls/:job_id/cancel",
            post(api::pulls::cancel_pull).layer(json_body_limit),
        )
        // Whole-repo `hf` CLI pull routes (safetensors / transformers wizard)
        .route(
            "/tama/v1/pulls/repo",
            post(api::repo_pulls::start_repo_pull).layer(json_body_limit),
        )
        .route(
            "/tama/v1/pulls/repo/:job_id",
            delete(api::repo_pulls::delete_repo_pull),
        )
        // Alias CRUD routes
        .route(
            "/tama/v1/aliases",
            get(list_aliases).post(create_alias).layer(json_body_limit),
        )
        .route(
            "/tama/v1/aliases/:id",
            get(get_alias)
                .put(update_alias)
                .delete(delete_alias)
                .layer(json_body_limit),
        )
        // Provider CRUD routes
        .route(
            "/tama/v1/providers",
            get(list_providers)
                .post(create_provider)
                .layer(json_body_limit),
        )
        .route(
            "/tama/v1/providers/:name",
            get(get_provider)
                .patch(update_provider)
                .delete(delete_provider)
                .layer(json_body_limit),
        )
        // Tamad CRUD routes
        .route(
            "/tama/v1/tamads",
            get(list_tamads).post(create_tamad).layer(json_body_limit),
        )
        .route(
            "/tama/v1/tamads/:id",
            get(get_tamad)
                .patch(update_tamad)
                .delete(delete_tamad)
                .layer(json_body_limit),
        )
        .route("/tama/v1/tamads/:id/health", post(trigger_health_check))
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                    axum::http::Method::PATCH,
                ])
                .allow_headers(tower_http::cors::Any)
                // Expose X-CSRF-Token so JS can read it from GET responses
                .expose_headers([axum::http::HeaderName::from_static("x-csrf-token")]),
        )
        .layer(middleware::from_fn(api::middleware::enforce_same_origin));

    Router::new()
        // HF metadata endpoint — wildcard captures `owner/repo` with embedded slash
        .route("/tama/v1/hf/*repo_id", get(api::hf::hf_metadata))
        // Self-update GET routes (safe methods, no CSRF protection needed)
        .route(
            "/tama/v1/self-update/check",
            get(api::self_update::check_update),
        )
        .route(
            "/tama/v1/self-update/events",
            get(api::self_update::update_events),
        )
        // Benchmark GET routes (no CSRF needed)
        .route("/tama/v1/benchmarks/jobs/:id", get(get_benchmark_result))
        .route("/tama/v1/benchmarks/jobs/:id/events", get(benchmark_events))
        .route("/tama/v1/benchmarks/history", get(list_benchmark_history))
        .route("/tama/v1/benchmarks/history/:id", delete(delete_benchmark))
        // Pulls Center routes
        .route("/tama/v1/pulls/active", get(api::pulls::get_active_pulls))
        .route("/tama/v1/pulls/history", get(api::pulls::get_pull_history))
        .route("/tama/v1/pulls/events", get(api::pulls::pull_events_sse))
        .route(
            "/tama/v1/pulls/repo/:job_id",
            get(api::repo_pulls::get_repo_pull),
        )
        // API documentation (OpenAPI 3.1.0 spec)
        .route("/tama/v1/docs", get(api::openapi::serve_spec))
        // System health: core proxy handler mounted explicitly (it is part of
        // the management API surface but implemented in tama-core).
        .route("/tama/v1/system/health", get(handle_tama_system_health))
        // Structured log store read API (plan-195 task 4) — the legacy
        // all-tail handler (`handle_all_logs`) is retired with the Type II
        // consumer in task 5.
        .route("/tama/v1/logs", get(handle_log_query))
        .route("/tama/v1/logs/sources", get(handle_log_sources))
        .route("/tama/v1/logs/summary", get(handle_log_summary))
        .route("/tama/v1/logs/status", get(handle_log_status))
        .route("/tama/v1/logs/stream", get(handle_log_stream))
        .route("/tama/v1/logs/events", get(handle_log_events_sse))
        .route("/tama/v1/logs/export", get(handle_log_export))
        .merge(csrf_routes)
        .merge(backend_routes)
        // Redirect old /ui paths to /tama
        .route("/ui", get(redirect_ui_root))
        .route("/ui/*path", get(redirect_to_tama))
        // Web UI — mounted at /tama (SPA fallback, /tama/v1/* takes priority)
        .route("/tama", get(serve_index))
        .route(
            "/tama/*path",
            get(|Path(p): Path<String>| async move { serve_static(Some(Path(p))).await }),
        )
        // Backend forwarding for root-level paths (/slots, /tokenize, /health, etc.)
        .route("/*path", get(handle_root_forward))
        // Add WebState as Extension for ALL routes (must be after .merge() so it
        // wraps merged sub-routers too — axum layers before .merge() don't apply
        // to the merged routes)
        .layer(axum::extract::Extension(web_state.as_ref().clone()))
        // Read-endpoint log API state (same fields, shaped for the
        // tama-core handlers that can't name WebState directly).
        .layer(axum::extract::Extension(LogsApiState {
            log_read: web_state.log_read.clone(),
            log_tail: web_state.log_tail.clone(),
            log_status: web_state.log_status.clone(),
            log_events_tx: web_state.log_events_tx.clone(),
        }))
        .layer(CatchPanicLayer::new())
}
