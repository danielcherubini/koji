use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
use tama_core::proxy::ProxyState;

use super::types::*;
use crate::api::error::error_response;
use crate::api::helpers::open_backend_manager;
use crate::web_types::WebState;

/// GET /tama/v1/backends
pub async fn list_backends(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl IntoResponse {
    // active_job is only available when job manager is configured
    let active_job = if let Some(jobs) = &web_state.jobs {
        jobs.active()
            .await
            .filter(|j| {
                let st = j.state.try_read().ok();
                if let Some(s) = &st {
                    matches!(s.status, crate::web_types::JobStatus::Running)
                } else {
                    false
                }
            })
            .map(|j| job_to_active_dto(&j))
    } else {
        None
    };

    let mgr_result = open_backend_manager(&state).await;

    // Load backend configs from DB (keyed by (name, gpu_variant)), reusing the manager
    // opened above to avoid opening the DB twice.
    let backend_configs_map: std::collections::HashMap<
        (String, String),
        (Vec<String>, Vec<String>),
    > = mgr_result
        .as_ref()
        .ok()
        .and_then(|mgr| mgr.list_configs().ok())
        .map(|configs| {
            configs
                .into_iter()
                .map(|c| ((c.name, c.gpu_variant), (c.default_args, c.default_env)))
                .collect()
        })
        .unwrap_or_default();

    // Load cached update checks from DB (keyed by "name:variant")
    let update_checks: std::collections::HashMap<
        String,
        tama_core::db::queries::UpdateCheckRecord,
    > = match crate::api::helpers::shared_repository(&web_state) {
        Ok(repo_handle) => {
            let repo_handle = repo_handle.clone();
            match tokio::task::spawn_blocking(move || {
                let repo = repo_handle.lock().unwrap();
                repo.get_all_update_checks()
            })
            .await
            {
                Ok(Ok(records)) => records
                    .into_iter()
                    .filter(|r| r.item_type == "backend")
                    .map(|r| (r.item_id.clone(), r))
                    .collect(),
                _ => std::collections::HashMap::new(),
            }
        }
        Err(_) => std::collections::HashMap::new(),
    };

    // Build the response including available backend types
    let mut backends: Vec<BackendCardDto> = Vec::new();
    let mut custom: Vec<BackendCardDto> = Vec::new();
    let mut docker: Vec<BackendCardDto> = Vec::new();
    let mut available: Vec<String> = Vec::new();

    match mgr_result {
        Ok(mgr) => {
            // Emit one card per (backend_type, gpu_variant) pair — only if installed
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                let versions_opt = mgr.list_versions(type_, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    // Create one card per variant
                    for (variant, variant_versions) in variant_groups {
                        let (default_args, default_env) = backend_configs_map
                            .get(&(type_.to_string(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let active_version = mgr.get_active(type_, &variant).ok().flatten();

                        // Sort versions by installed_at DESC
                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        // Build version DTOs
                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
                                source: info.source.as_ref().map(|s| s.into()),
                                is_active: active_version
                                    .as_ref()
                                    .map(|a| a.version == info.version)
                                    .unwrap_or(false),
                            })
                            .collect();

                        let active_info = active_version.map(BackendInfoDto::from);

                        // Load cached update status from DB (keyed by "name:variant")
                        let update_key = format!("{}:{}", type_, variant);
                        let update_status = update_checks
                            .get(&update_key)
                            .map(|r| UpdateStatusDto {
                                checked: true,
                                latest_version: r.latest_version.clone(),
                                update_available: if r.update_available {
                                    Some(true)
                                } else {
                                    None
                                },
                            })
                            .unwrap_or_default();

                        backends.push(BackendCardDto {
                            r#type: type_.to_string(),
                            backend_name: type_.to_string(),
                            display_name: display_name.to_string(),
                            installed: true,
                            gpu_variant: variant,
                            info: active_info,
                            versions: version_dtos,
                            update: update_status,
                            release_notes_url: release_notes_url.map(String::from),
                            default_args: default_args.clone(),
                            default_env: default_env.clone(),
                            is_active: true,
                        });
                    }
                } else {
                    available.push(type_.to_string());
                }
            }

            // Custom backends — one card per (name, variant) pair
            // Collect unique custom backend names to avoid duplicate cards
            // when multiple variants are active for the same backend
            let active_backends = mgr.list_active().unwrap_or_default();
            let mut custom_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut docker_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for active in &active_backends {
                let bt = active.backend_type.to_string();
                if bt == "docker" {
                    docker_names.insert(active.name.clone());
                } else if !matches!(bt.as_str(), "llama_cpp" | "ik_llama" | "tts_kokoro") {
                    custom_names.insert(active.name.clone());
                }
            }

            for name in &custom_names {
                let versions_opt = mgr.list_versions(name, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let bt = versions
                        .first()
                        .map(|v| v.backend_type.to_string())
                        .unwrap_or_default();

                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    for (variant, variant_versions) in variant_groups {
                        let active_version = mgr.get_active(name, &variant).ok().flatten();
                        let (default_args, default_env) = backend_configs_map
                            .get(&(name.clone(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
                                source: info.source.as_ref().map(|s| s.into()),
                                is_active: active_version
                                    .as_ref()
                                    .map(|a| a.version == info.version)
                                    .unwrap_or(false),
                            })
                            .collect();

                        let active_info = active_version.map(BackendInfoDto::from);

                        // Load cached update status from DB (keyed by "name:variant")
                        let update_key = format!("{}:{}", name, variant);
                        let update_status = update_checks
                            .get(&update_key)
                            .map(|r| UpdateStatusDto {
                                checked: true,
                                latest_version: r.latest_version.clone(),
                                update_available: if r.update_available {
                                    Some(true)
                                } else {
                                    None
                                },
                            })
                            .unwrap_or_default();

                        custom.push(BackendCardDto {
                            r#type: bt.clone(),
                            backend_name: name.clone(),
                            display_name: format!("Custom ({})", name),
                            installed: true,
                            gpu_variant: variant,
                            info: active_info,
                            versions: version_dtos,
                            update: update_status,
                            release_notes_url: None,
                            default_args,
                            default_env,
                            is_active: true,
                        });
                    }
                }
            }

            for name in &docker_names {
                let versions_opt = mgr.list_versions(name, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let bt = versions
                        .first()
                        .map(|v| v.backend_type.to_string())
                        .unwrap_or_default();

                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    for (variant, variant_versions) in variant_groups {
                        let active_version = mgr.get_active(name, &variant).ok().flatten();
                        let (default_args, default_env) = backend_configs_map
                            .get(&(name.clone(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
                                source: info.source.as_ref().map(|s| s.into()),
                                is_active: active_version
                                    .as_ref()
                                    .map(|a| a.version == info.version)
                                    .unwrap_or(false),
                            })
                            .collect();

                        let active_info = active_version.map(BackendInfoDto::from);

                        // Load cached update status from DB (keyed by "name:variant")
                        let update_key = format!("{}:{}", name, variant);
                        let update_status = update_checks
                            .get(&update_key)
                            .map(|r| UpdateStatusDto {
                                checked: true,
                                latest_version: r.latest_version.clone(),
                                update_available: if r.update_available {
                                    Some(true)
                                } else {
                                    None
                                },
                            })
                            .unwrap_or_default();

                        docker.push(BackendCardDto {
                            r#type: bt.clone(),
                            backend_name: name.clone(),
                            display_name: format!("Docker ({})", name),
                            installed: true,
                            gpu_variant: variant,
                            info: active_info,
                            versions: version_dtos,
                            update: update_status,
                            release_notes_url: None,
                            default_args,
                            default_env,
                            is_active: true,
                        });
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to open backend manager: {:?}", e.status());
        }
    }

    // Get compaction config
    let compaction_config = state.with_config(|c| c.compaction.clone()).await;

    // Check if compaction backend is running (in model registry as "compaction")
    let (compaction_running, compaction_url) = match state.get_model_state("compaction").await {
        Some(s) if s.is_ready() => (true, s.backend_url().map(|u| u.to_string())),
        _ => (false, None),
    };

    let compaction_card = CompactionCardDto {
        enabled: compaction_config.enabled,
        device: compaction_config.device.as_str(),
        port: compaction_config.port,
        running: compaction_running,
        server_url: compaction_url,
        request_timeout_ms: compaction_config.request_timeout_ms,
    };

    Json(BackendListResponse {
        active_job,
        backends,
        custom,
        docker,
        available,
        compaction: compaction_card,
    })
    .into_response()
}

/// POST /tama/v1/backends/check-updates
pub async fn check_backend_updates(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl IntoResponse {
    let jobs = match &web_state.jobs {
        Some(j) => j.clone(),
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job manager not configured",
                None,
            )
        }
    };

    // Get active job if any
    let active_job = jobs
        .active()
        .await
        .filter(|j| {
            let state = j.state.try_read().ok();
            if let Some(s) = &state {
                matches!(s.status, crate::web_types::JobStatus::Running)
            } else {
                false
            }
        })
        .map(|j| job_to_active_dto(&j));

    let mgr_result = open_backend_manager(&state).await;

    // Load backend configs from DB (keyed by (name, gpu_variant)), reusing the manager
    // opened above to avoid opening the DB twice.
    let backend_configs_map: std::collections::HashMap<
        (String, String),
        (Vec<String>, Vec<String>),
    > = mgr_result
        .as_ref()
        .ok()
        .and_then(|mgr| mgr.list_configs().ok())
        .map(|configs| {
            configs
                .into_iter()
                .map(|c| ((c.name, c.gpu_variant), (c.default_args, c.default_env)))
                .collect()
        })
        .unwrap_or_default();

    let mut backends: Vec<BackendCardDto> = Vec::new();
    let mut custom: Vec<BackendCardDto> = Vec::new();
    let mut docker: Vec<BackendCardDto> = Vec::new();

    match mgr_result {
        Ok(mgr) => {
            // Emit one card per (backend_type, gpu_variant) pair
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                let versions_opt = mgr.list_versions(type_, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    // Create one card per variant
                    for (variant, variant_versions) in variant_groups {
                        let (default_args, default_env) = backend_configs_map
                            .get(&(type_.to_string(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let active_version = mgr.get_active(type_, &variant).ok().flatten();

                        // Check for updates against the active version
                        let update_check = match active_version.as_ref() {
                            Some(info) => match tama_core::backends::check_updates(info).await {
                                Ok(check) => UpdateStatusDto {
                                    checked: true,
                                    latest_version: Some(check.latest_version),
                                    update_available: Some(check.update_available),
                                },
                                Err(_) => UpdateStatusDto {
                                    checked: true,
                                    latest_version: None,
                                    update_available: None,
                                },
                            },
                            None => UpdateStatusDto::default(),
                        };

                        // Sort versions by installed_at DESC
                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
                                source: info.source.as_ref().map(|s| s.into()),
                                is_active: active_version
                                    .as_ref()
                                    .map(|a| a.version == info.version)
                                    .unwrap_or(false),
                            })
                            .collect();

                        let active_info = active_version.map(BackendInfoDto::from);

                        backends.push(BackendCardDto {
                            r#type: type_.to_string(),
                            backend_name: type_.to_string(),
                            display_name: display_name.to_string(),
                            installed: true,
                            gpu_variant: variant,
                            info: active_info,
                            versions: version_dtos,
                            update: UpdateStatusDto {
                                checked: update_check.checked,
                                latest_version: update_check.latest_version.clone(),
                                update_available: update_check.update_available,
                            },
                            release_notes_url: release_notes_url.map(String::from),
                            default_args: default_args.clone(),
                            default_env: default_env.clone(),
                            is_active: true,
                        });
                    }
                } else {
                    backends.push(BackendCardDto::default_uninstalled(
                        type_,
                        display_name,
                        *release_notes_url,
                        Vec::new(),
                    ));
                }
            }

            // Custom backends — one card per (name, variant) pair
            // Collect unique custom backend names to avoid duplicate cards
            let active_backends = mgr.list_active().unwrap_or_default();
            let mut custom_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut docker_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for active in &active_backends {
                let bt = active.backend_type.to_string();
                if bt == "docker" {
                    docker_names.insert(active.name.clone());
                } else if !matches!(bt.as_str(), "llama_cpp" | "ik_llama" | "tts_kokoro") {
                    custom_names.insert(active.name.clone());
                }
            }

            for name in &custom_names {
                let versions_opt = mgr.list_versions(name, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let bt = versions
                        .first()
                        .map(|v| v.backend_type.to_string())
                        .unwrap_or_default();

                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    for (variant, variant_versions) in variant_groups {
                        let active_version = mgr.get_active(name, &variant).ok().flatten();
                        let (default_args, default_env) = backend_configs_map
                            .get(&(name.clone(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
                                source: info.source.as_ref().map(|s| s.into()),
                                is_active: active_version
                                    .as_ref()
                                    .map(|a| a.version == info.version)
                                    .unwrap_or(false),
                            })
                            .collect();

                        let active_info = active_version.map(BackendInfoDto::from);

                        custom.push(BackendCardDto {
                            r#type: bt.clone(),
                            backend_name: name.clone(),
                            display_name: format!("Custom ({})", name),
                            installed: true,
                            gpu_variant: variant,
                            info: active_info,
                            versions: version_dtos,
                            update: UpdateStatusDto::default(),
                            release_notes_url: None,
                            default_args,
                            default_env,
                            is_active: true,
                        });
                    }
                }
            }

            for name in &docker_names {
                let versions_opt = mgr.list_versions(name, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let bt = versions
                        .first()
                        .map(|v| v.backend_type.to_string())
                        .unwrap_or_default();

                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    for (variant, variant_versions) in variant_groups {
                        let active_version = mgr.get_active(name, &variant).ok().flatten();
                        let (default_args, default_env) = backend_configs_map
                            .get(&(name.clone(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
                                source: info.source.as_ref().map(|s| s.into()),
                                is_active: active_version
                                    .as_ref()
                                    .map(|a| a.version == info.version)
                                    .unwrap_or(false),
                            })
                            .collect();

                        let active_info = active_version.map(BackendInfoDto::from);

                        docker.push(BackendCardDto {
                            r#type: bt.clone(),
                            backend_name: name.clone(),
                            display_name: format!("Docker ({})", name),
                            installed: true,
                            gpu_variant: variant,
                            info: active_info,
                            versions: version_dtos,
                            update: UpdateStatusDto::default(),
                            release_notes_url: None,
                            default_args,
                            default_env,
                            is_active: true,
                        });
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to open backend manager: {:?}", e.status());
            // On error, still return known backends as not installed
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                backends.push(BackendCardDto::default_uninstalled(
                    type_,
                    display_name,
                    *release_notes_url,
                    Vec::new(),
                ));
            }
        }
    }

    Json(CheckUpdatesResponse {
        active_job,
        backends,
        custom,
        docker,
    })
    .into_response()
}

/// GET /tama/v1/backends/:name/versions
pub async fn list_backend_versions(
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    // Validate name (prevent path traversal)
    if let Err(resp) = crate::api::backends::reject_traversal(&name, "backend name") {
        return resp;
    }

    let mgr_result = open_backend_manager(&state).await;

    match mgr_result {
        Ok(mgr) => {
            let versions_opt = match mgr.list_versions(&name, None) {
                Ok(v) => v,
                Err(e) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to list versions: {}", e),
                        None,
                    )
                }
            };

            let versions = match versions_opt {
                Some(v) => v,
                None => {
                    return error_response(
                        StatusCode::NOT_FOUND,
                        format!("Backend '{}' not found", name),
                        Some("NotFoundError"),
                    )
                }
            };

            // Get the active version for this backend, keyed by (name, gpu_variant)
            let active_backends: Vec<_> = mgr
                .list_active()
                .ok()
                .map(|backends| {
                    backends
                        .into_iter()
                        .filter(|b| b.name == name)
                        .map(|b| (b.gpu_variant, b.version))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let dto_versions: Vec<BackendVersionDto> = versions
                .iter()
                .map(|info| {
                    let is_active = active_backends.iter().any(|(variant, version)| {
                        variant == &info.gpu_variant && version == &info.version
                    });
                    BackendVersionDto {
                        name: info.name.clone(),
                        version: info.version.clone(),
                        path: info.path.to_string_lossy().to_string(),
                        installed_at: info.installed_at,
                        gpu_variant: info.gpu_variant.clone(),
                        source: info.source.as_ref().map(|s| s.into()),
                        is_active,
                    }
                })
                .collect();

            let active_version = active_backends.first().map(|(_, v)| v.clone());

            Json(BackendVersionsResponse {
                versions: dto_versions,
                active_version,
            })
            .into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to open backend manager: {:?}", e.status()),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tama_core::config::Config;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    fn test_web_state() -> crate::web_types::WebState {
        crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            repository: None,
        }
    }

    /// GET /tama/v1/backends on empty registry → 200 with empty arrays.
    #[tokio::test]
    async fn test_list_backends_empty_registry() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(config, None));

        let web_state = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/backends")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let json: serde_json::Value =
            serde_json::from_slice(&body_str).expect("body should be valid JSON");

        assert_eq!(json["backends"], serde_json::Value::Array(Vec::new()));
        assert_eq!(json["custom"], serde_json::Value::Array(Vec::new()));
    }

    /// GET /tama/v1/backends/:name/versions for unknown backend → 404.
    #[tokio::test]
    async fn test_list_backend_versions_unknown_404() {
        let config = Config::default();
        let db_dir = tempfile::tempdir().unwrap();
        let state = Arc::new(ProxyState::new(config, Some(db_dir.path().to_path_buf())));

        let web_state = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/backends/nonexistent_backend/versions")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "unknown backend versions should return 404"
        );
    }
}
