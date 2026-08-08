use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::types::{BackendState, ProxyState};

/// Lifecycle state of a model as reported by the `/status` endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusModelState {
    Ready,
    #[serde(rename = "starting")]
    Loading,
    Unloading,
    Failed,
    Idle,
}

/// Per-model entry in the `/status` response `models` map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusModelEntry {
    pub id: Option<i64>,
    pub display_name: Option<String>,
    pub backend: String,
    pub backend_path: Option<String>,
    pub model: Option<String>,
    pub quant: Option<String>,
    pub context_length: Option<u32>,
    pub enabled: bool,
    pub api_name: Option<String>,
    pub state: StatusModelState,
    pub backend_pid: Option<u32>,
    pub load_time_secs: Option<u64>,
    pub last_accessed_secs_ago: Option<u64>,
    pub idle_timeout_remaining_secs: Option<u64>,
    pub consecutive_failures: Option<u32>,
}

/// VRAM usage snapshot for the `/status` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VramStatus {
    pub used_mib: u64,
    pub total_mib: u64,
}

/// Atomic counter snapshot for the `/status` endpoint (distinct from the
/// live `proxy::types::ProxyMetrics` counters).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyMetricsSnapshot {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub models_loaded: u64,
    pub models_unloaded: u64,
}

/// Typed response for the `/status` endpoint.
///
/// `vram` and `gpu_utilization_pct` use `skip_serializing_if` so that `None`
/// values are omitted from the wire JSON — matching the behaviour clients
/// received before the endpoint was fully typed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    pub cpu_usage_pct: f32,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_utilization_pct: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram: Option<VramStatus>,
    pub auto_unload: bool,
    pub idle_timeout_secs: u64,
    pub metrics: ProxyMetricsSnapshot,
    pub models: std::collections::BTreeMap<String, StatusModelEntry>,
}

impl ProxyState {
    /// Build the per-model status snapshot embedded in `MetricSample.models`.
    ///
    /// Iterates over every configured model, resolves its backends, and reports
    /// the lifecycle state (`idle`, `loading`, `ready`, `unloading`, `failed`).
    /// The returned vector is sorted by `id` so dashboard rows do not shuffle
    /// between SSE samples.
    pub async fn collect_model_state_snapshots(&self) -> Vec<crate::models::ModelStateSnapshot> {
        let config = self.config.read().await;
        let model_configs = self.registry.model_configs.read().await;

        // Borrow inference_stats before acquiring runtime to avoid lock-order issues.
        let inference_stats = self.metrics.inference_stats_snapshot();

        let runtime = self.registry.models.read().await;
        let mut out: Vec<crate::models::ModelStateSnapshot> =
            Vec::with_capacity(model_configs.len());
        for (model_id, model_cfg) in model_configs.iter() {
            // Determine the model's lifecycle state from its backend entries.
            let servers = config.resolve_backends_for_model(&model_configs, model_id);
            let mut best_state: Option<&BackendState> = None;
            for (backend_name, _, _) in servers.iter() {
                if let Some(state) = runtime.get(backend_name) {
                    match state {
                        BackendState::Ready { .. } => {
                            best_state = Some(state);
                            break; // Ready is the best possible state
                        }
                        BackendState::Starting { .. }
                        | BackendState::Unloading { .. }
                        | BackendState::Failed { .. } => {
                            if best_state.is_none() {
                                best_state = Some(state);
                            }
                        }
                    }
                }
            }

            let (model_state, error_message, is_docker) = match best_state {
                Some(BackendState::Ready { .. }) => (
                    crate::gpu::ModelState::Ready,
                    None,
                    best_state.unwrap().is_docker(),
                ),
                Some(BackendState::Starting { .. }) => (
                    crate::gpu::ModelState::Starting,
                    None,
                    best_state.unwrap().is_docker(),
                ),
                Some(BackendState::Unloading { .. }) => (
                    crate::gpu::ModelState::Unloading,
                    None,
                    best_state.unwrap().is_docker(),
                ),
                Some(BackendState::Failed { error, .. }) => (
                    crate::gpu::ModelState::Failed,
                    Some(error.clone()),
                    best_state.unwrap().is_docker(),
                ),
                None => (crate::gpu::ModelState::Idle, None, false),
            };

            // Look up the first matching backend's inference stats.
            // first-server-wins: for the current usage (one server per model) this is sufficient.
            let server_stats = servers
                .iter()
                .find_map(|(sn, _, _)| inference_stats.get(sn));
            let status = crate::models::ModelStateSnapshot {
                id: model_id.clone(),
                db_id: model_cfg.db_id,
                api_name: model_cfg.api_name.clone(),
                display_name: model_cfg.display_name.clone(),
                backend: model_cfg.backend.clone(),
                state: model_state,
                quant: model_cfg
                    .quant
                    .clone()
                    .or_else(|| model_cfg.vllm.quantization.clone()),
                context_length: model_cfg
                    .context_length
                    .or(model_cfg.vllm.max_model_len),
                hf_architecture_type: model_cfg.hf_architecture_type.clone(),
                hf_base_model: model_cfg.hf_base_model.clone(),
                hf_format: model_cfg.hf_format.clone(),
                gpu_variant: model_cfg
                    .gpu_variant
                    .as_ref()
                    .map(|v| v.variant_folder().to_string()),
                cache_type_k: model_cfg
                    .cache_type_k
                    .clone()
                    .or_else(|| model_cfg.vllm.kv_cache_dtype.clone()),
                cache_type_v: model_cfg
                    .cache_type_v
                    .clone()
                    .or_else(|| model_cfg.vllm.kv_cache_dtype.clone()),
                spec_types: model_cfg.spec_decoding.spec_types.clone(),
                gpu_device: model_cfg.gpu_device.clone(),
                error_message,
                tps: server_stats.and_then(|s| s.tps),
                prompt_tps: server_stats.and_then(|s| s.prompt_tps),
                is_docker,
            };
            out.push(status);
        }
        // Stable order so dashboard rows don't shuffle between samples.
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Build a comprehensive status response for the proxy.
    ///
    /// Returns a typed `StatusResponse` suitable for JSON serialization.
    /// Models are an object keyed by name, fields are flat per model
    /// (not nested in a `runtime` sub-object), and `idle_timeout_secs`
    /// is at the top level.
    pub async fn build_status_response(&self) -> StatusResponse {
        use std::sync::atomic::Ordering::Relaxed;

        let sys_metrics = self.metrics.system_metrics_snapshot().await;

        let config = self.config.read().await;
        let model_configs = self.registry.model_configs.read().await;
        let auto_unload = config.proxy.auto_unload;
        let idle_timeout_secs = config.proxy.idle_timeout_secs;
        let models = self.registry.models.read().await;
        let mut models_obj = std::collections::BTreeMap::new();

        for (model_name, model_config) in model_configs.iter() {
            let backend_path = config
                .backends
                .get(&model_config.backend)
                .and_then(|b| b.path.clone());

            let model_state = models.get(model_name);

            let entry = match model_state {
                Some(BackendState::Ready {
                    backend_pid,
                    load_time,
                    last_accessed,
                    consecutive_failures,
                    ..
                }) => {
                    let now = Instant::now();
                    let last_accessed_secs_ago = now.duration_since(*last_accessed).as_secs();
                    let idle_timeout_remaining_secs: Option<u64> = if auto_unload {
                        let timeout = Duration::from_secs(idle_timeout_secs);
                        let elapsed = now.duration_since(*last_accessed);
                        if elapsed < timeout {
                            Some((timeout - elapsed).as_secs())
                        } else {
                            Some(0)
                        }
                    } else {
                        // Auto-unload disabled — no countdown
                        None
                    };
                    let load_time_secs = load_time
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    StatusModelEntry {
                        id: model_config.db_id,
                        display_name: model_config.display_name.clone(),
                        backend: model_config.backend.clone(),
                        backend_path: backend_path.clone(),
                        model: model_config.model.clone(),
                        quant: model_config.quant.clone(),
                        context_length: model_config.context_length,
                        enabled: model_config.enabled,
                        api_name: model_config.api_name.clone(),
                        state: StatusModelState::Ready,
                        backend_pid: Some(*backend_pid),
                        load_time_secs: Some(load_time_secs),
                        last_accessed_secs_ago: Some(last_accessed_secs_ago),
                        idle_timeout_remaining_secs,
                        consecutive_failures: Some(consecutive_failures.load(Relaxed)),
                    }
                }
                Some(BackendState::Starting {
                    consecutive_failures,
                    ..
                }) => StatusModelEntry {
                    id: model_config.db_id,
                    display_name: model_config.display_name.clone(),
                    backend: model_config.backend.clone(),
                    backend_path: backend_path.clone(),
                    model: model_config.model.clone(),
                    quant: model_config.quant.clone(),
                    context_length: model_config.context_length,
                    enabled: model_config.enabled,
                    api_name: model_config.api_name.clone(),
                    state: StatusModelState::Loading,
                    backend_pid: None,
                    load_time_secs: None,
                    last_accessed_secs_ago: None,
                    idle_timeout_remaining_secs: None,
                    consecutive_failures: Some(consecutive_failures.load(Relaxed)),
                },
                Some(BackendState::Unloading { .. }) => StatusModelEntry {
                    id: model_config.db_id,
                    display_name: model_config.display_name.clone(),
                    backend: model_config.backend.clone(),
                    backend_path: backend_path.clone(),
                    model: model_config.model.clone(),
                    quant: model_config.quant.clone(),
                    context_length: model_config.context_length,
                    enabled: model_config.enabled,
                    api_name: model_config.api_name.clone(),
                    state: StatusModelState::Unloading,
                    backend_pid: None,
                    load_time_secs: None,
                    last_accessed_secs_ago: None,
                    idle_timeout_remaining_secs: None,
                    consecutive_failures: None,
                },
                Some(BackendState::Failed { .. }) => StatusModelEntry {
                    id: model_config.db_id,
                    display_name: model_config.display_name.clone(),
                    backend: model_config.backend.clone(),
                    backend_path: backend_path.clone(),
                    model: model_config.model.clone(),
                    quant: model_config.quant.clone(),
                    context_length: model_config.context_length,
                    enabled: model_config.enabled,
                    api_name: model_config.api_name.clone(),
                    state: StatusModelState::Failed,
                    backend_pid: None,
                    load_time_secs: None,
                    last_accessed_secs_ago: None,
                    idle_timeout_remaining_secs: None,
                    consecutive_failures: None,
                },
                _ => StatusModelEntry {
                    id: model_config.db_id,
                    display_name: model_config.display_name.clone(),
                    backend: model_config.backend.clone(),
                    backend_path: backend_path.clone(),
                    model: model_config.model.clone(),
                    quant: model_config.quant.clone(),
                    context_length: model_config.context_length,
                    enabled: model_config.enabled,
                    api_name: model_config.api_name.clone(),
                    state: StatusModelState::Idle,
                    backend_pid: None,
                    load_time_secs: None,
                    last_accessed_secs_ago: None,
                    idle_timeout_remaining_secs: None,
                    consecutive_failures: None,
                },
            };

            models_obj.insert(model_name.clone(), entry);
        }

        drop(models);

        let metrics = &self.metrics.counters;

        StatusResponse {
            cpu_usage_pct: sys_metrics.cpu_usage_pct,
            ram_used_mib: sys_metrics.ram_used_mib,
            ram_total_mib: sys_metrics.ram_total_mib,
            gpu_utilization_pct: sys_metrics.gpu_utilization_pct,
            vram: sys_metrics.vram.map(|v| VramStatus {
                used_mib: v.used_mib,
                total_mib: v.total_mib,
            }),
            auto_unload,
            idle_timeout_secs,
            metrics: ProxyMetricsSnapshot {
                total_requests: metrics.total_requests.load(Relaxed),
                successful_requests: metrics.successful_requests.load(Relaxed),
                failed_requests: metrics.failed_requests.load(Relaxed),
                models_loaded: metrics.models_loaded.load(Relaxed),
                models_unloaded: metrics.models_unloaded.load(Relaxed),
            },
            models: models_obj,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendConfig, Config, ModelConfig};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn make_model_config(backend: &str) -> ModelConfig {
        ModelConfig {
            backend: backend.to_string(),
            args: vec![],
            sampling: None,
            model: None,
            quant: None,

            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            num_parallel: Some(1),
            kv_unified: false,
            profile: None,
            api_name: None,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            quants: BTreeMap::new(),
            modalities: None,
            display_name: None,
            db_id: None,
            ..Default::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_model_config_with_vllm(
        backend: &str,
        quant: Option<String>,
        context_length: Option<u32>,
        cache_type_k: Option<String>,
        cache_type_v: Option<String>,
        vllm_quantization: Option<String>,
        vllm_max_model_len: Option<u32>,
        vllm_kv_cache_dtype: Option<String>,
    ) -> ModelConfig {
        ModelConfig {
            backend: backend.to_string(),
            args: vec![],
            sampling: None,
            model: None,
            quant,
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length,
            num_parallel: Some(1),
            kv_unified: false,
            profile: None,
            api_name: None,
            gpu_layers: None,
            cache_type_k,
            cache_type_v,
            quants: BTreeMap::new(),
            modalities: None,
            display_name: None,
            db_id: None,
            vllm: crate::config::VllmConfig {
                quantization: vllm_quantization,
                kv_cache_dtype: vllm_kv_cache_dtype,
                max_model_len: vllm_max_model_len,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// When `state.models()` has no runtime entries, every configured model
    /// should be reported as `loaded == false`, with the returned vector
    /// sorted by id ascending and the `backend` field matching the
    /// corresponding `ModelConfig.backend` value.
    #[tokio::test]
    async fn test_collect_model_state_snapshots_reports_idle_when_no_runtime_entry() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Populate model_configs
        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert("zephyr".to_string(), make_model_config("llama_cpp"));
            mc.insert("alpha".to_string(), make_model_config("vllm"));
        }

        // Sanity check: no runtime entries.
        assert!(state.registry.models.read().await.is_empty());

        let statuses = state.collect_model_state_snapshots().await;

        // Length matches the number of configured models.
        assert_eq!(statuses.len(), 2);

        // Every entry is reported as not loaded.
        assert!(
            !statuses
                .iter()
                .any(|s| matches!(s.state, crate::gpu::ModelState::Ready)),
            "expected every status to not be ready, got: {:?}",
            statuses
        );

        // Entries are sorted by id ascending.
        let ids: Vec<&str> = statuses.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "zephyr"]);

        // Backend field matches the configured backend name for each model.
        assert_eq!(statuses[0].id, "alpha");
        assert_eq!(statuses[0].backend, "vllm");
        assert_eq!(statuses[1].id, "zephyr");
        assert_eq!(statuses[1].backend, "llama_cpp");
    }

    /// When `state.models()` contains a `BackendState::Ready` entry under the
    /// server name that resolves for one of the configured models, that
    /// model should be reported as `loaded == true` while all other
    /// configured models remain `loaded == false`. The returned vector
    /// must still be sorted by id ascending and carry the configured
    /// `backend` value.
    #[tokio::test]
    async fn test_collect_model_state_snapshots_reports_loaded_when_server_is_ready() {
        use std::sync::atomic::AtomicU32;
        use std::sync::Arc;
        use std::time::{Instant, SystemTime};

        let mut config = Config::default();
        // Add backends so resolve_backends_for_model can match models.
        config.backends.insert(
            "vllm".to_string(),
            BackendConfig {
                path: None,
                version: None,
                gpu_variant: None,
            },
        );
        config.backends.insert(
            "llama_cpp".to_string(),
            BackendConfig {
                path: None,
                version: None,
                gpu_variant: None,
            },
        );
        let state = ProxyState::new(config, None);

        // Populate model_configs
        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert("zephyr".to_string(), make_model_config("llama_cpp"));
            mc.insert("alpha".to_string(), make_model_config("vllm"));
        }

        // Insert a Ready entry for "alpha" under the server name that
        // `resolve_backends_for_model("alpha")` will return — the config key
        // itself, since `make_model_config` leaves `model` as `None`.
        {
            let mut runtime = state.registry.models.write().await;
            runtime.insert(
                "alpha".to_string(),
                BackendState::Ready {
                    model_name: "alpha".to_string(),
                    backend: "vllm".to_string(),
                    backend_pid: 12345,
                    backend_url: "http://127.0.0.1:8000".to_string(),
                    load_time: SystemTime::now(),
                    last_accessed: Instant::now(),
                    consecutive_failures: Arc::new(AtomicU32::new(0)),
                    failure_timestamp: None,
                    is_docker: false,
                    restart_count: 0,
                },
            );
        }

        let statuses = state.collect_model_state_snapshots().await;

        // Length matches the number of configured models.
        assert_eq!(statuses.len(), 2);

        // Entries are sorted by id ascending.
        let ids: Vec<&str> = statuses.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "zephyr"]);

        // Exactly one model is reported as loaded.
        let loaded_count = statuses
            .iter()
            .filter(|s| matches!(s.state, crate::gpu::ModelState::Ready))
            .count();
        assert_eq!(
            loaded_count, 1,
            "expected exactly one loaded model, got: {:?}",
            statuses
        );

        // alpha is loaded with the configured backend.
        assert_eq!(statuses[0].id, "alpha");
        assert_eq!(
            statuses[0].state,
            crate::gpu::ModelState::Ready,
            "expected alpha to be ready"
        );
        assert_eq!(statuses[0].backend, "vllm");

        // zephyr is not loaded but still carries its configured backend.
        assert_eq!(statuses[1].id, "zephyr");
        assert_eq!(
            statuses[1].state,
            crate::gpu::ModelState::Idle,
            "expected zephyr to not be ready"
        );
        assert_eq!(statuses[1].backend, "llama_cpp");
    }

    /// `collect_model_state_snapshots` should only treat `BackendState::Ready` as
    /// "loaded". Other variants like `Starting` and `Failed` must be
    /// reported as `loaded == false` so the dashboard does not falsely
    /// claim a model is serving traffic while it is still booting or has
    /// crashed.
    #[tokio::test]
    async fn test_collect_model_state_snapshots_ignores_non_ready_states() {
        use std::sync::atomic::AtomicU32;
        use std::sync::Arc;
        use std::time::Instant;

        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Populate model_configs
        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert("alpha".to_string(), make_model_config("llama_cpp"));
        }

        // The server name `resolve_backends_for_model("alpha")` returns is
        // the config key itself, since `make_model_config` leaves
        // `model` as `None`.
        let backend_name = "alpha".to_string();

        // --- Case 1: Starting must NOT count as loaded ---------------------
        {
            let mut runtime = state.registry.models.write().await;
            runtime.insert(
                backend_name.clone(),
                BackendState::Starting {
                    model_name: "alpha".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_url: "http://127.0.0.1:8000".to_string(),
                    backend_pid: 0,
                    last_accessed: Instant::now(),
                    start_time: Instant::now(),
                    consecutive_failures: Arc::new(AtomicU32::new(0)),
                    is_docker: false,
                    failure_timestamp: None,
                },
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        assert_eq!(
            statuses.len(),
            1,
            "expected one status entry per configured model, got: {:?}",
            statuses
        );
        let alpha = statuses
            .iter()
            .find(|s| s.id == "alpha")
            .expect("alpha entry missing from collect_model_state_snapshots output");
        assert_eq!(
            alpha.state,
            crate::gpu::ModelState::Starting,
            "BackendState::Starting must not be reported as ready, got: {:?}",
            alpha
        );

        // --- Case 2: Failed must NOT count as loaded -----------------------
        {
            let mut runtime = state.registry.models.write().await;
            runtime.insert(
                backend_name.clone(),
                BackendState::Failed {
                    model_name: "alpha".to_string(),
                    backend: "llama_cpp".to_string(),
                    error: "backend exited with status 1".to_string(),
                },
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        assert_eq!(
            statuses.len(),
            1,
            "expected one status entry per configured model, got: {:?}",
            statuses
        );
        let alpha = statuses
            .iter()
            .find(|s| s.id == "alpha")
            .expect("alpha entry missing from collect_model_state_snapshots output");
        assert_eq!(
            alpha.state,
            crate::gpu::ModelState::Failed,
            "BackendState::Failed must not be reported as ready, got: {:?}",
            alpha
        );
    }

    // ── Drift-guard: /status response round-trip ──────────────────────────────

    /// The StatusResponse struct must faithfully represent the full wire shape.
    /// Deserializing the serialized body back into StatusResponse and comparing
    /// against the raw Value ensures no fields are silently dropped or invented.
    #[tokio::test]
    async fn test_status_response_roundtrip_lossless() {
        use crate::gpu::{SystemMetrics, VramInfo};
        use std::sync::atomic::AtomicU32;
        use std::time::{Instant, UNIX_EPOCH};

        // Build a minimal fixture with one ready model and vram present.
        let mut config = Config::default();
        config.backends.insert(
            "llama_cpp".to_string(),
            BackendConfig {
                path: Some("/opt/llama".into()),
                version: None,
                gpu_variant: None,
            },
        );
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "ready-model".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    display_name: Some("Ready".to_string()),
                    model: Some("test/model".to_string()),
                    db_id: Some(42),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        {
            let mut runtime = state.registry.models.write().await;
            runtime.insert(
                "ready-model".to_string(),
                BackendState::Ready {
                    model_name: "ready-model".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_pid: 1234,
                    backend_url: "http://127.0.0.1:8080".to_string(),
                    load_time: UNIX_EPOCH,
                    last_accessed: Instant::now(),
                    consecutive_failures: Arc::new(AtomicU32::new(0)),
                    failure_timestamp: None,
                    is_docker: false,
                    restart_count: 0,
                },
            );
        }

        {
            state
                .metrics
                .set_system_metrics(SystemMetrics {
                    cpu_usage_pct: 10.0,
                    ram_used_mib: 512,
                    ram_total_mib: 4096,
                    gpu_utilization_pct: Some(50),
                    vram: Some(VramInfo {
                        used_mib: 100,
                        total_mib: 8192,
                    }),
                    ..Default::default()
                })
                .await;
        }

        let response = state.build_status_response().await;
        let body_bytes = serde_json::to_vec(&response).unwrap();

        // Deserialize into the typed struct.
        let parsed: StatusResponse =
            serde_json::from_slice(&body_bytes).expect("body must deserialize into StatusResponse");

        // Lossless round-trip: re-serialize parsed and compare to original Value.
        let raw_value: serde_json::Value =
            serde_json::from_slice(&body_bytes).expect("body must be valid JSON");
        assert_eq!(
            serde_json::to_value(&parsed).expect("parsed must serialize"),
            raw_value,
            "StatusResponse round-trip must be lossless — struct fields must match wire shape exactly"
        );
    }

    // ── vLLM field resolution in ModelStateSnapshot ──────────────────────────

    /// When GGUF `quant` is `Some`, it takes priority over `vllm.quantization`.
    #[tokio::test]
    async fn test_vllm_quant_fallback_gguf_wins() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm",
                    Some("Q4_K_M".to_string()), // GGUF quant
                    None,
                    None,
                    None,
                    Some("fp8".to_string()), // vLLM quantization
                    None,
                    None,
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.quant,
            Some("Q4_K_M".to_string()),
            "GGUF quant should take priority over vLLM quantization"
        );
    }

    /// When GGUF `quant` is `None`, falls back to `vllm.quantization`.
    #[tokio::test]
    async fn test_vllm_quant_fallback_uses_vllm() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm",
                    None, // GGUF quant is None
                    None,
                    None,
                    None,
                    Some("fp8".to_string()), // vLLM quantization
                    None,
                    None,
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.quant,
            Some("fp8".to_string()),
            "Should fall back to vLLM quantization when GGUF quant is None"
        );
    }

    /// When both GGUF `quant` and `vllm.quantization` are `None`, returns `None`.
    #[tokio::test]
    async fn test_vllm_quant_fallback_both_none() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm", None, // GGUF quant
                    None, None, None, None, // vLLM quantization
                    None, None,
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.quant, None,
            "Should be None when both GGUF quant and vLLM quantization are None"
        );
    }

    /// When GGUF `context_length` is `Some`, it takes priority over `vllm.max_model_len`.
    #[tokio::test]
    async fn test_vllm_context_length_fallback_gguf_wins() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm",
                    None,
                    Some(4096), // GGUF context_length
                    None,
                    None,
                    None,
                    Some(8192), // vLLM max_model_len
                    None,
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.context_length,
            Some(4096),
            "GGUF context_length should take priority over vLLM max_model_len"
        );
    }

    /// When GGUF `context_length` is `None`, falls back to `vllm.max_model_len`.
    #[tokio::test]
    async fn test_vllm_context_length_fallback_uses_vllm() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm",
                    None,
                    None, // GGUF context_length is None
                    None,
                    None,
                    None,
                    Some(8192), // vLLM max_model_len
                    None,
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.context_length,
            Some(8192),
            "Should fall back to vLLM max_model_len when GGUF context_length is None"
        );
    }

    /// When both GGUF `context_length` and `vllm.max_model_len` are `None`, returns `None`.
    #[tokio::test]
    async fn test_vllm_context_length_fallback_both_none() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm", None, None, // GGUF context_length
                    None, None, None, None, // vLLM max_model_len
                    None,
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.context_length, None,
            "Should be None when both GGUF context_length and vLLM max_model_len are None"
        );
    }

    /// When GGUF `cache_type_k` is `Some`, it takes priority over `vllm.kv_cache_dtype`.
    #[tokio::test]
    async fn test_vllm_cache_type_k_fallback_gguf_wins() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm",
                    None,
                    None,
                    Some("q4_0".to_string()), // GGUF cache_type_k
                    None,
                    None,
                    None,
                    Some("fp8".to_string()), // vLLM kv_cache_dtype
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.cache_type_k,
            Some("q4_0".to_string()),
            "GGUF cache_type_k should take priority over vLLM kv_cache_dtype"
        );
    }

    /// When GGUF `cache_type_k` is `None`, falls back to `vllm.kv_cache_dtype`.
    #[tokio::test]
    async fn test_vllm_cache_type_k_fallback_uses_vllm() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm",
                    None,
                    None,
                    None, // GGUF cache_type_k is None
                    None,
                    None,
                    None,
                    Some("fp8".to_string()), // vLLM kv_cache_dtype
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.cache_type_k,
            Some("fp8".to_string()),
            "Should fall back to vLLM kv_cache_dtype when GGUF cache_type_k is None"
        );
    }

    /// When GGUF `cache_type_v` is `Some`, it takes priority over `vllm.kv_cache_dtype`.
    #[tokio::test]
    async fn test_vllm_cache_type_v_fallback_gguf_wins() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm",
                    None,
                    None,
                    None,
                    Some("q8_0".to_string()), // GGUF cache_type_v
                    None,
                    None,
                    Some("fp8".to_string()), // vLLM kv_cache_dtype
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.cache_type_v,
            Some("q8_0".to_string()),
            "GGUF cache_type_v should take priority over vLLM kv_cache_dtype"
        );
    }

    /// When GGUF `cache_type_v` is `None`, falls back to `vllm.kv_cache_dtype`.
    #[tokio::test]
    async fn test_vllm_cache_type_v_fallback_uses_vllm() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm",
                    None,
                    None,
                    None,
                    None, // GGUF cache_type_v is None
                    None,
                    None,
                    Some("fp8".to_string()), // vLLM kv_cache_dtype
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.cache_type_v,
            Some("fp8".to_string()),
            "Should fall back to vLLM kv_cache_dtype when GGUF cache_type_v is None"
        );
    }

    /// `cache_type_k` and `cache_type_v` resolve independently — each falls back
    /// to `vllm.kv_cache_dtype` only when its own column is `None`.
    #[tokio::test]
    async fn test_vllm_cache_types_resolve_independently() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        // cache_type_k is Some, cache_type_v is None — only V should fall back
        {
            let mut mc = state.registry.model_configs.write().await;
            mc.insert(
                "safetensors-model".to_string(),
                make_model_config_with_vllm(
                    "vllm",
                    None,
                    None,
                    Some("q4_0".to_string()), // GGUF cache_type_k
                    None,                     // GGUF cache_type_v is None
                    None,
                    None,
                    Some("fp8".to_string()), // vLLM kv_cache_dtype
                ),
            );
        }

        let statuses = state.collect_model_state_snapshots().await;
        let snap = statuses
            .iter()
            .find(|s| s.id == "safetensors-model")
            .unwrap();
        assert_eq!(
            snap.cache_type_k,
            Some("q4_0".to_string()),
            "cache_type_k should use GGUF value"
        );
        assert_eq!(
            snap.cache_type_v,
            Some("fp8".to_string()),
            "cache_type_v should fall back to vLLM kv_cache_dtype"
        );
    }
}
