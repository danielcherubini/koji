use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use super::types::*;
use crate::api::error::error_response;
use crate::api::helpers::open_backend_manager;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// Query params for POST /tama/v1/backends/:name/update
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// POST /tama/v1/backends/:name/update
pub async fn update_backend(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<UpdateQuery>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name: path separators or traversal sequences not allowed",
            Some("ValidationError"),
        );
    }

    let jobs = match web_state.jobs.as_ref() {
        Some(j) => j.clone(),
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job manager not configured",
                None,
            )
        }
    };

    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });
    let config_dir_clone = config_dir.clone();

    let mgr = match open_backend_manager(&state).await {
        Ok(mgr) => mgr,
        Err(e) => return e,
    };

    // Determine gpu_variant: use explicit value or auto-infer from manager
    let lookup_variant = match query.gpu_variant {
        Some(v) => v,
        None => {
            // Auto-infer: find unique variant for this backend
            let versions = match mgr.list_versions(&name, None) {
                Ok(Some(v)) => v,
                Ok(None) => {
                    return error_response(
                        StatusCode::NOT_FOUND,
                        format!("Backend '{}' not found", name),
                        Some("NotFoundError"),
                    )
                }
                Err(e) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to query backend: {}", e),
                        None,
                    )
                }
            };
            let mut variants: Vec<String> =
                versions.iter().map(|v| v.gpu_variant.clone()).collect();
            variants.sort();
            variants.dedup();
            match variants.len() {
                1 => variants.into_iter().next().unwrap(),
                _ => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": format!(
                                "Backend '{}' has multiple variants. Please specify gpu_variant. Available: {}",
                                name,
                                variants.join(", ")
                            )
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    let backend_info = match mgr.get_active(&name, &lookup_variant) {
        Ok(Some(info)) => info,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                format!("Backend '{}' not found", name),
                Some("NotFoundError"),
            )
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get backend: {}", e),
                None,
            )
        }
    };

    let backend_type = backend_info.backend_type.clone();

    // Check latest version
    let latest_version =
        match tama_core::backends::check_latest_version(&backend_type, None, None).await {
            Ok(v) => v,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to check latest version: {}", e),
                    None,
                )
            }
        };

    // Submit job
    let job = match jobs
        .submit(
            crate::web_types::JobKind::Update,
            Some(backend_type.clone()),
        )
        .await
    {
        Ok(j) => j,
        Err(crate::web_types::JobError::AlreadyRunning(existing_id)) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "another backend job is already running",
                    "job_id": existing_id
                })),
            )
                .into_response();
        }
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create job",
                None,
            )
        }
    };

    // Use versioned path structure for the update target
    let target_dir = match tama_core::backends::backends_dir() {
        Ok(d) => tama_core::backends::get_backend_install_path(
            &d,
            &backend_type,
            &backend_info.gpu_variant,
            &latest_version,
        ),
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get backends dir: {}", e),
                None,
            )
        }
    };

    // Build update options — always use latest_version, not the old version from the registry.
    let source = match backend_info.source.clone() {
        Some(src) => match src {
            tama_core::backends::BackendSource::Prebuilt { .. } => {
                tama_core::backends::BackendSource::Prebuilt {
                    version: latest_version.clone(),
                }
            }
            tama_core::backends::BackendSource::SourceCode {
                git_url, commit: _, ..
            } => tama_core::backends::BackendSource::SourceCode {
                version: latest_version.clone(),
                git_url,
                commit: None,
            },
        },
        None => {
            // Fallback: use source code if no source recorded
            tama_core::backends::BackendSource::SourceCode {
                version: latest_version.clone(),
                git_url: match &backend_type {
                    tama_core::backends::BackendType::LlamaCpp => {
                        "https://github.com/ggml-org/llama.cpp.git"
                    }
                    tama_core::backends::BackendType::IkLlama => {
                        "https://github.com/ikawrakow/ik_llama.cpp.git"
                    }
                    other => {
                        tracing::warn!(
                            "No source URL configured for backend type {:?}, using llama.cpp fallback",
                            other
                        );
                        "https://github.com/ggml-org/llama.cpp.git"
                    }
                }
                .to_string(),
                commit: None,
            }
        }
    };

    let options = tama_core::backends::InstallOptions {
        backend_type: backend_type.clone(),
        source,
        target_dir,
        gpu_variant: backend_info.gpu_variant.clone(),
        allow_overwrite: true,
    };

    // Clone variables needed for the post-update check
    let checker = web_state.update_checker.clone();
    let backend_type_clone = backend_type.clone();

    // Spawn the update task
    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    let name_clone = name.clone();
    let latest_version_clone = latest_version.clone();
    let gpu_variant_clone = backend_info.gpu_variant.clone();
    tokio::spawn(async move {
        let adapter = Arc::new(JobAdapter {
            jobs: jobs_clone.clone(),
            job: job_clone.clone(),
        });

        let client = reqwest::Client::builder()
            .user_agent("tama-backend-manager")
            .timeout(std::time::Duration::from_secs(300))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        let result = match tama_core::backends::update_backend_with_progress(
            mgr,
            &client,
            &name_clone,
            &gpu_variant_clone,
            options,
            latest_version_clone,
            Some(adapter),
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string()),
        };

        match result {
            Ok(_) => {
                let _ = jobs_clone
                    .finish(&job_clone, crate::web_types::JobStatus::Succeeded, None)
                    .await;
                // Refresh the update check record so the Updates Center reflects the new version
                let _ = checker
                    .check_backend(
                        &config_dir_clone,
                        &name_clone,
                        &backend_type_clone,
                        &gpu_variant_clone,
                    )
                    .await;
            }
            Err(e) => {
                let _ = jobs_clone
                    .finish(&job_clone, crate::web_types::JobStatus::Failed, Some(e))
                    .await;
            }
        }
    });

    Json(InstallResponse {
        job_id: job.id.to_string(),
        kind: "update".to_string(),
        backend_type: format!("{}", backend_type),
        notices: vec![],
    })
    .into_response()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// Create a minimal WebState for tests.
    fn test_web_state() -> crate::web_types::WebState {
        crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Path traversal in update_backend name should return 400.
    #[tokio::test]
    async fn test_update_backend_path_traversal_rejected() {
        let config = tama_core::config::Config::default();
        let state = Arc::new(tama_core::proxy::ProxyState::new(config, None));

        let web_state_for_test = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state_for_test.clone())
            .with_state(state)
            .layer(axum::extract::Extension(
                web_state_for_test.as_ref().clone(),
            ));

        // Valid CSRF token pair — cookie and header must match.
        let csrf_token = "test-csrf-token-12345";
        let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

        // Test with `\` in name — backslash won't be normalized by Axum.
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends/foo\\bar/update")
            .header(axum::http::header::COOKIE, cookie_header.as_str())
            .header("X-CSRF-Token", csrf_token)
            .body(Body::empty())
            .unwrap();

        let resp = router
            .clone()
            .oneshot(req)
            .await
            .expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "update_backend should reject names containing '\\' with 400"
        );

        // Test with `..` in name — Axum normalizes `../` segments but not `..`
        // embedded within a segment. The validation catches this.
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends/foo..bar/update")
            .header(axum::http::header::COOKIE, cookie_header.as_str())
            .header("X-CSRF-Token", csrf_token)
            .body(Body::empty())
            .unwrap();

        let resp = router
            .clone()
            .oneshot(req)
            .await
            .expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "update_backend should reject names containing '..' with 400"
        );
    }

    /// Path traversal in update_backend_source name should return 400.
    #[tokio::test]
    async fn test_update_backend_source_path_traversal_rejected() {
        let config = tama_core::config::Config::default();
        let state = Arc::new(tama_core::proxy::ProxyState::new(config, None));

        let web_state_for_test = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state_for_test.clone())
            .with_state(state)
            .layer(axum::extract::Extension(
                web_state_for_test.as_ref().clone(),
            ));

        let csrf_token = "test-csrf-token-12345";
        let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

        let body = serde_json::json!({"build_from_source": true}).to_string();

        // Test with `..` in name — Axum normalizes `../` segments but not `..`
        // embedded within a segment. The validation catches this.
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends/foo..bar/source")
            .header(axum::http::header::COOKIE, cookie_header.as_str())
            .header("X-CSRF-Token", csrf_token)
            .header("Content-Type", "application/json")
            .body(Body::from(body.clone()))
            .unwrap();

        let resp = router
            .clone()
            .oneshot(req)
            .await
            .expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "update_backend_source should reject names containing '..' with 400"
        );

        // Test with `\` in name — backslash won't be normalized by Axum.
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends/foo\\bar/source")
            .header(axum::http::header::COOKIE, cookie_header.as_str())
            .header("X-CSRF-Token", csrf_token)
            .header("Content-Type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = router
            .clone()
            .oneshot(req)
            .await
            .expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "update_backend_source should reject names containing '\\' with 400"
        );
    }

    /// Missing backend in update_backend_source should return 404.
    #[tokio::test]
    async fn test_update_backend_source_missing_backend() {
        let config = tama_core::config::Config::default();
        let state = Arc::new(tama_core::proxy::ProxyState::new(config, None));

        let web_state_for_test = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state_for_test.clone())
            .with_state(state)
            .layer(axum::extract::Extension(
                web_state_for_test.as_ref().clone(),
            ));

        let csrf_token = "test-csrf-token-12345";
        let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

        let body = serde_json::json!({"build_from_source": true}).to_string();

        // POST to a non-existent backend
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends/nonexistent_backend/source")
            .header(axum::http::header::COOKIE, cookie_header.as_str())
            .header("X-CSRF-Token", csrf_token)
            .header("Content-Type", "application/json")
            .body(Body::from(body))
            .unwrap();

        let resp = router
            .clone()
            .oneshot(req)
            .await
            .expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "update_backend_source should return 404 for non-existent backend"
        );
    }
}

/// Query params for DELETE /tama/v1/backends/:name/versions/:version
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoveVersionQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// DELETE /tama/v1/backends/:name/versions/:version
pub async fn remove_backend_version(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path((name, version)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<RemoveVersionQuery>,
) -> impl IntoResponse {
    // Validate path params (prevent path traversal)
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name: path separators or traversal sequences not allowed",
            Some("ValidationError"),
        );
    }
    if version.contains('/') || version.contains('\\') || version.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid version: path separators or traversal sequences not allowed",
            Some("ValidationError"),
        );
    }

    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    // Open manager and get the specific version
    let config_dir_clone = config_dir.clone();
    let mgr_result: Result<tama_core::backends::BackendManager, _> =
        tokio::task::spawn_blocking(move || {
            tama_core::backends::BackendManager::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    let mgr = match mgr_result {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to open manager: {}", e),
                None,
            )
        }
    };

    // Use gpu_variant from query param if provided
    let gpu_variant_filter = query.gpu_variant.clone();

    // Get the specific version record before deleting
    let versions = match mgr.list_versions(&name, gpu_variant_filter.as_deref()) {
        Ok(Some(v)) => v,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Backend '{}' version '{}' not found", name, version)
                })),
            )
                .into_response();
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query backend: {}", e),
                None,
            )
        }
    };

    // Find matching versions and check for ambiguity
    let matches: Vec<_> = versions.iter().filter(|v| v.version == version).collect();
    let info = match matches.len() {
        0 => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Backend '{}' version '{}' not found", name, version)
                })),
            )
                .into_response();
        }
        1 => matches[0].clone(),
        _ if gpu_variant_filter.is_some() => matches[0].clone(),
        _ => {
            // Multiple variants have the same version - require gpu_variant
            let variant_list: Vec<String> = matches.iter().map(|v| v.gpu_variant.clone()).collect();
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "Version '{}' exists in multiple variants for backend '{}'. Please specify gpu_variant. Available: {}",
                        version, name, variant_list.join(", ")
                    )
                })),
            )
                .into_response();
        }
    };

    // Delete files FIRST (before any DB changes)
    let info_to_remove = tama_core::backends::BackendInfo {
        name: info.name.clone(),
        backend_type: info.backend_type.clone(),
        version: info.version.clone(),
        path: std::path::PathBuf::from(&info.path),
        installed_at: info.installed_at,
        gpu_variant: info.gpu_variant.clone(),
        source: None,
    };

    // Check if a job is running for this backend
    if let Some(jobs) = web_state.jobs.as_ref() {
        if let Some(active_job) = jobs.active().await {
            let active_type = active_job
                .backend_type
                .as_ref()
                .map(|b| b.to_string())
                .unwrap_or_default();
            if active_type == info.backend_type.to_string() {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "a job is currently running for this backend"
                    })),
                )
                    .into_response();
            }
        }
    }

    if info_to_remove.path.exists() {
        if let Err(e) = tama_core::backends::safe_remove_installation(&info_to_remove) {
            let err_msg = e.to_string();
            if err_msg.contains("outside the managed backends directory") {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "path is outside the managed backends directory; remove manually"
                    })),
                )
                    .into_response();
            }
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to remove files: {}", e),
                None,
            );
        }
    }

    // Remove from DB (activates another version if this was active)
    if let Err(e) = mgr.remove_version(&name, &info.gpu_variant, &version) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to remove version: {}", e),
            None,
        );
    }

    // Clean up update_check records — use LIKE pattern to match all variants
    // (e.g., "llama_cpp:cpu", "llama_cpp:cuda") plus legacy format.
    if let Ok(repo) = tama_core::db::repository::Repository::open(&config_dir) {
        let escaped_name = name
            .replace('\\', "\\\\")
            .replace('_', "\\_")
            .replace('%', "\\%");
        let pattern = format!("{}:%", escaped_name);
        let _ = repo.delete_update_checks_by_pattern("backend", &pattern);
        // Also delete legacy format (no variant separator)
        let _ = repo.delete_update_check("backend", &name);
    }

    Json(DeleteResponse { removed: true }).into_response()
}

/// Query params for POST /tama/v1/backends/:name/activate
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivateQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// POST /tama/v1/backends/:name/activate
pub async fn activate_backend_version(
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ActivateQuery>,
    Json(req): Json<ActivateRequest>,
) -> impl IntoResponse {
    // Validate name
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name",
            Some("ValidationError"),
        );
    }

    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    // Determine gpu_variant: use explicit value or auto-infer from manager
    let gpu_variant = match query.gpu_variant {
        Some(v) => v,
        None => {
            let config_dir_clone = config_dir.clone();
            let name_clone = name.clone();
            let version_clone = req.version.clone();
            let infer_result: Result<Option<Vec<tama_core::backends::BackendInfo>>, anyhow::Error> =
                tokio::task::spawn_blocking(move || {
                    let mgr = tama_core::backends::BackendManager::open(&config_dir_clone)?;
                    mgr.list_versions(&name_clone, None)
                })
                .await
                .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
                .and_then(|r| r);

            let versions = match infer_result {
                Ok(Some(v)) => v,
                Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(serde_json::json!({
                            "error": format!("Backend '{}' not found", name)
                        })),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": format!("Failed to query backend: {}", e)
                        })),
                    )
                        .into_response();
                }
            };

            // Collect unique variants
            let mut variants: Vec<String> =
                versions.iter().map(|v| v.gpu_variant.clone()).collect();
            variants.sort();
            variants.dedup();

            if variants.len() == 1 {
                // Only one variant exists — use it
                variants.into_iter().next().unwrap()
            } else {
                // Multiple variants — find the one that has the requested version
                let matching: Vec<String> = versions
                    .iter()
                    .filter(|v| v.version == version_clone)
                    .map(|v| v.gpu_variant.clone())
                    .collect();
                let mut matching = matching;
                matching.sort();
                matching.dedup();

                match matching.len() {
                    1 => matching.into_iter().next().unwrap(),
                    0 => {
                        return (
                            StatusCode::NOT_FOUND,
                            Json(serde_json::json!({
                                "error": format!(
                                    "Version '{}' not found for backend '{}'. Available variants: {}",
                                    version_clone,
                                    name,
                                    variants.join(", ")
                                )
                            })),
                        )
                            .into_response();
                    }
                    _ => {
                        // Multiple variants have the same version — ambiguous
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({
                                "error": format!(
                                    "Version '{}' exists in multiple variants for backend '{}'. Please specify gpu_variant. Available variants: {}",
                                    version_clone,
                                    name,
                                    matching.join(", ")
                                )
                            })),
                        )
                            .into_response();
                    }
                }
            }
        }
    };

    let config_dir_clone = config_dir.clone();
    let version_clone = req.version.clone();
    let name_clone = name.clone();
    let version_for_error = version_clone.clone();
    let gpu_variant_clone = gpu_variant.to_string();
    let mgr_result: Result<(tama_core::backends::BackendManager, bool), _> =
        tokio::task::spawn_blocking(move || {
            let mgr = tama_core::backends::BackendManager::open(&config_dir_clone)?;
            let activated = mgr.activate(&name_clone, &gpu_variant_clone, &version_clone)?;
            Ok((mgr, activated))
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    match mgr_result {
        Ok((_, activated)) => {
            if !activated {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": format!("Version '{}' not found for backend '{}'", version_for_error, name)
                    })),
                )
                    .into_response();
            }

            Json(ActivateResponse {
                version: req.version,
                is_active: true,
            })
            .into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to activate: {}", e),
            None,
        ),
    }
}

/// POST /tama/v1/backends/:name/default-args
/// Update default_args for a backend in the backend_configs DB table.
#[derive(Deserialize)]
pub struct UpdateDefaultArgsRequest {
    pub default_args: Vec<String>,
}

/// Query params for POST /tama/v1/backends/:name/default-args
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DefaultArgsQuery {
    pub gpu_variant: String,
}

pub async fn update_backend_default_args(
    State(state): State<Arc<ProxyState>>,
    Path(backend_name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DefaultArgsQuery>,
    Json(req): Json<UpdateDefaultArgsRequest>,
) -> impl IntoResponse {
    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let backend_name = backend_name.clone();
    let gpu_variant = query.gpu_variant.clone();
    let default_args = req.default_args.clone();

    let result: Result<(), anyhow::Error> = tokio::task::spawn_blocking(move || {
        let mgr = tama_core::backends::BackendManager::open(&config_dir)?;
        // Preserve existing default_env when updating default_args
        let existing_env = mgr.get_default_env(&backend_name, &gpu_variant);
        mgr.save_config(
            &backend_name,
            &gpu_variant,
            &default_args,
            &existing_env,
            None,
        )?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
    .and_then(|r| r);

    match result {
        Ok(()) => Json(serde_json::json!({"success": true})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update backend config: {}", e),
            None,
        ),
    }
}

/// POST /tama/v1/backends/:name/default-env
/// Update default_env for a backend in the backend_configs DB table.
#[derive(Deserialize)]
pub struct UpdateDefaultEnvRequest {
    pub default_env: Vec<String>,
}

/// Query params for POST /tama/v1/backends/:name/default-env
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DefaultEnvQuery {
    pub gpu_variant: String,
}

pub async fn update_backend_default_env(
    State(state): State<Arc<ProxyState>>,
    Path(backend_name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DefaultEnvQuery>,
    Json(req): Json<UpdateDefaultEnvRequest>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if backend_name.contains('/') || backend_name.contains('\\') || backend_name.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name: path separators or traversal sequences not allowed",
            Some("ValidationError"),
        );
    }

    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let backend_name = backend_name.clone();
    let gpu_variant = query.gpu_variant.clone();
    let default_env = req.default_env.clone();

    let result: Result<(), anyhow::Error> = tokio::task::spawn_blocking(move || {
        let mgr = tama_core::backends::BackendManager::open(&config_dir)?;
        // Preserve existing default_args when updating default_env
        let existing_args = mgr.get_default_args(&backend_name, &gpu_variant);
        mgr.save_config(
            &backend_name,
            &gpu_variant,
            &existing_args,
            &default_env,
            None,
        )?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
    .and_then(|r| r);

    match result {
        Ok(()) => Json(serde_json::json!({"success": true})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update backend config: {}", e),
            None,
        ),
    }
}

/// Query params for POST /tama/v1/backends/:name/source
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SourceQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// POST /tama/v1/backends/:name/source
/// Updates the build method (source vs prebuilt) for a backend.
pub async fn update_backend_source(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<SourceQuery>,
    Json(req): Json<UpdateSourceRequest>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name",
            Some("ValidationError"),
        );
    }

    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    // Open manager and determine gpu_variant
    let config_dir_clone = config_dir.clone();
    let name_clone = name.clone();
    let query_gpu_variant = query.gpu_variant.clone();
    let mgr_result: Result<(tama_core::backends::BackendManager, String), _> =
        tokio::task::spawn_blocking(move || {
            let mgr = tama_core::backends::BackendManager::open(&config_dir_clone)?;

            // Determine gpu_variant: use explicit value or auto-infer from manager
            let gpu_variant = match query_gpu_variant {
                Some(v) => v,
                None => {
                    let versions = mgr.list_versions(&name_clone, None)?;
                    let versions = match versions {
                        Some(v) => v,
                        None => {
                            return Err(anyhow::anyhow!(
                                "Backend '{}' not found",
                                name_clone
                            ));
                        }
                    };
                    let mut variants: Vec<String> =
                        versions.iter().map(|v| v.gpu_variant.clone()).collect();
                    variants.sort();
                    variants.dedup();
                    match variants.len() {
                        1 => variants.into_iter().next().unwrap(),
                        _ => {
                            return Err(anyhow::anyhow!(
                                "Backend '{}' has multiple variants. Please specify gpu_variant. Available: {}",
                                name_clone,
                                variants.join(", ")
                            ));
                        }
                    }
                }
            };

            // Validate resolved gpu_variant for path traversal
            if gpu_variant.contains('/') || gpu_variant.contains('\\') || gpu_variant.contains("..")
            {
                return Err(anyhow::anyhow!("Invalid gpu_variant: path separators or traversal sequences not allowed"));
            }

            Ok((mgr, gpu_variant))
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    let (mgr, gpu_variant) = match mgr_result {
        Ok(r) => r,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not found") {
                return error_response(StatusCode::NOT_FOUND, err_msg, Some("NotFoundError"));
            }
            if err_msg.contains("Invalid gpu_variant") || err_msg.contains("multiple variants") {
                return error_response(StatusCode::BAD_REQUEST, err_msg, Some("ValidationError"));
            }
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to open manager: {}", e),
                None,
            );
        }
    };

    // Check for active job conflict
    if let Some(jobs) = web_state.jobs.as_ref() {
        if let Some(active_job) = jobs.active().await {
            if active_job.backend_type.as_ref().map(|b| b.to_string()) == Some(name.clone()) {
                return error_response(
                    StatusCode::CONFLICT,
                    "another backend job is already running",
                    Some("ConflictError"),
                );
            }
        }
    }

    let name_for_update = name.clone();
    let gpu_variant_for_update = gpu_variant.clone();
    let build_from_source = req.build_from_source;

    let update_result: Result<(), anyhow::Error> = tokio::task::spawn_blocking(move || {
        mgr.update_build_method(&name_for_update, &gpu_variant_for_update, build_from_source)
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
    .and_then(|r| r);

    match update_result {
        Ok(()) => Json(UpdateSourceResponse { build_from_source }).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update build method: {}", e),
            None,
        ),
    }
}
