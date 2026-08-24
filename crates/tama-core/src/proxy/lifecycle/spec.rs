//! Shared load-spec builders + tamad dispatch (plan-191 Tasks 5 and 10).
//!
//! The proxy resolves a *fully resolved launch spec* for a local model from
//! the central DB (installation config, binary path, model file path, GPU
//! device) and ships it to the model's provider tamad via `LoadModel`. This
//! module is the single home of that resolution so the request path
//! (`ensure_model_loaded`), the TTS/compaction handlers, the management API
//! (load/unload/cancel handlers), and the reconciler all build identical
//! specs.
//!
//! The proxy never spawns, kills, or samples the host (ADR-0010, enforced
//! by the dependency graph since plan-191 Task 10): after the RPC succeeds
//! it records *desired* state in `desired_models` and keeps a local
//! `BackendState` mirror (staging mirror) so the forward path and management
//! API keep working. GPU isolation env vars are resolved by the *tamad*
//! from the `gpu_device` field it compares against its own hardware.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use tracing::warn;

use crate::gpu::GpuVariant;
use crate::providers::Provider;
use crate::tamad::{LoadModelRequest, SystemStats};

use super::ProxyState;

/// Env key the proxy uses to carry the *proxy's* models directory in the
/// launch spec. The tamad rewrites any arg that references it to its own
/// `models_dir` and strips the key before spawning.
pub const PROXY_MODELS_DIR_ENV: &str = "TAMA_MODELS_DIR";

/// A fully resolved launch spec for a local model (or auxiliary backend).
#[derive(Debug, Clone)]
pub struct LoadSpec {
    /// Config backend name (the mirror key / `resolve_backends_for_model`
    /// key). For auxiliary backends this is the well-known key
    /// ("tts_kokoro" / "compaction").
    pub backend_name: String,
    /// Backend key (e.g. "llama_cpp", "tts_kokoro", "compaction").
    pub backend: String,
    /// The configured GPU device for this model (e.g. "GPU1"); `None` for
    /// CPU-only / non-GPU backends. Forwarded to the tamad, which resolves
    /// the vendor env var against its own hardware.
    pub gpu_device: Option<String>,
    /// The resolved `LoadModelRequest` to send to the tamad.
    pub request: LoadModelRequest,
}

/// Resolve the effective GPU device: model config > model card default.
/// Pure string logic — no local GPU discovery (the tamad owns hardware).
pub fn resolve_gpu_device(config: Option<String>, card_default: Option<String>) -> Option<String> {
    let normalize = |s: Option<String>| {
        s.and_then(|v| {
            let t = v.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        })
    };
    normalize(config).or_else(|| normalize(card_default))
}

/// Resolved GPU variant + device for a model (the old `load_model`
/// resolution, without the local GPU-discovery step — variant and device
/// come from the model config / card, not from scanning local devices).
pub async fn model_gpu_info(
    state: &ProxyState,
    model_name: &str,
    model_toml: Option<&crate::models::ModelToml>,
) -> Result<(GpuVariant, Option<String>)> {
    let (model_config, card_default) = {
        let config = state.config.read().await;
        let model_configs = state.registry.model_configs.read().await;
        let backends = config.resolve_backends_for_model(&model_configs, model_name);
        let backend_name = backends
            .first()
            .map(|(name, _, _)| name.clone())
            .ok_or_else(|| anyhow!("Failed to resolve backend for model {model_name}"))?;
        let (model_config, _) = config.resolve_backend(&model_configs, &backend_name)?;
        (
            model_config.clone(),
            model_toml.and_then(|toml| toml.model.default_gpu_device.clone()),
        )
    };
    let device = resolve_gpu_device(model_config.gpu_device.clone(), card_default);
    let variant = model_config
        .gpu_variant
        .clone()
        .unwrap_or(GpuVariant::CpuOnly);
    Ok((variant, device))
}

/// Build the full launch spec for a local model from the central DB.
///
/// The spec carries everything the tamad needs to spawn the process:
///
/// - `command` — resolved backend binary path (installation DB > config)
/// - `args` — `build_full_args` output (installation defaults, model args,
///   `-m`/`--mmproj`/`--spec-draft-model` injection, sampling) with absolute
///   model paths plus the `--host`/`--port` overrides (fresh port allocated
///   here)
/// - `model_path` — the model file relative to the models dir (metadata)
/// - `env` — installation `default_env` + `TAMA_MODELS_DIR` (the proxy's
///   models dir, so the tamad can remap the absolute model paths in `args`
///   to its own disk)
/// - `health_url` / `health_timeout_ms` — installation health URL (port
///   substituted) / `proxy.startup_timeout_secs`
///
/// GPU isolation env vars are *not* resolved here — the proxy would have to
/// sample local hardware for that, which is the tamad's job. The configured
/// device is forwarded in `gpu_device` and the tamad resolves the vendor
/// env var against its own GPU list.
pub async fn build_load_spec(
    state: &ProxyState,
    model_name: &str,
    model_toml: Option<&crate::models::ModelToml>,
) -> Result<LoadSpec> {
    let config = state.config.read().await.clone();

    // Resolve backend + model config (borrow, then take owned clones).
    let (backend_name, model_config, backend_config) = {
        let model_configs = state.registry.model_configs.read().await;
        let backends = config.resolve_backends_for_model(&model_configs, model_name);
        let backend_name = backends
            .first()
            .map(|(name, _, _)| name.clone())
            .ok_or_else(|| anyhow!("Failed to resolve backend for model {model_name}"))?;
        let (model_config, backend_config) =
            config.resolve_backend(&model_configs, &backend_name)?;
        (backend_name, model_config.clone(), backend_config.clone())
    };

    let (variant, device) = model_gpu_info(state, model_name, model_toml).await?;

    let manager = crate::installations::InstallationManager::new(state.db_pool());
    let variant_folder = variant.variant_folder().to_string();

    // Command: installation DB path > config.path (same resolution as the
    // former local load_model).
    let command = config
        .resolve_backend_path(&model_config.backend, Some(&variant), Some(&manager))
        .await
        .with_context(|| format!("resolving backend path for '{}'", model_config.backend))?;

    // Docker backends: the proxy ships the DockerConfig from the active
    // install row so the tamad (which owns no DB) can spawn the container.
    // `resolve_backend_path` yields only the image string as `command`; the
    // full mount/devices/shm/capability config lives on the install row
    // (ADDR-0006 / plan-080 style), which we re-read here.
    let docker_config_json = match manager
        .get_active(&model_config.backend, &variant_folder)
        .await
    {
        Ok(Some(info)) => info
            .docker_config
            .as_ref()
            .and_then(|dc| serde_json::to_string(dc).ok()),
        _ => None,
    };

    // Args: installation defaults → model args → injected model paths.
    let default_args = manager
        .get_default_args(&model_config.backend, &variant_folder)
        .await;
    let mut args = config.build_full_args(&model_config, &backend_config, None, &default_args)?;

    // Fresh port for the backend (single-node staging: proxy and tamad
    // share the port namespace). Multi-host port planning lands with the
    // provider abstraction follow-ups.
    let port = find_free_port_with_retry(3).await?;
    crate::process::override_arg(&mut args, "--host", "127.0.0.1");
    crate::process::override_arg(&mut args, "--port", &port.to_string());

    // Model paths stay absolute under the proxy's models dir: the tamad
    // re-anchors them to its own `models_dir` using TAMA_MODELS_DIR (see
    // PROXY_MODELS_DIR_ENV), so the same spec works across hosts.
    let models_dir = config.models_dir()?;

    let models_dir_str = models_dir.to_string_lossy().to_string();

    // Model path (relative) for the request — mirrors what build_full_args
    // injected (quant file for GGUF, repo dir for transformers).
    let is_transformers = model_config.hf_format.as_deref() == Some("transformers");
    let model_path = match (&model_config.model, &model_config.quant) {
        (Some(model_id), _) if is_transformers => {
            let p = crate::models::repo_path(&models_dir, model_id);
            p.strip_prefix(&models_dir)
                .map(|r| r.to_string_lossy().to_string())
                .unwrap_or_default()
        }
        (Some(model_id), Some(quant_name)) => {
            match model_config
                .quants
                .get(quant_name.as_str())
                .map(|q| q.file.clone())
            {
                Some(quant_file) => {
                    let p = crate::models::repo_path(&models_dir, model_id).join(quant_file);
                    p.strip_prefix(&models_dir)
                        .map(|r| r.to_string_lossy().to_string())
                        .unwrap_or_default()
                }
                None => {
                    warn!(
                        quant = %quant_name,
                        model = %model_id,
                        "Quant not found in ModelConfig; model_path left empty"
                    );
                    String::new()
                }
            }
        }
        _ => String::new(),
    };

    // Env: installation defaults + proxy models dir. (GPU isolation vars
    // are resolved on the tamad — see the module docs.)
    let mut env: HashMap<String, String> = HashMap::new();
    let default_env = manager
        .get_default_env(&model_config.backend, &variant_folder)
        .await;
    for env_var in &default_env {
        if let Some((key, value)) = env_var.split_once('=') {
            env.insert(key.to_string(), value.to_string());
        } else if !env_var.is_empty() {
            warn!("Skipping malformed env var (missing '='): {env_var}");
        }
    }
    env.insert(PROXY_MODELS_DIR_ENV.to_string(), models_dir_str.clone());

    // Health URL: installation template with the fresh port substituted.
    let health_url = match manager
        .get_health_check_url(&model_config.backend, &variant_folder)
        .await
    {
        Some(template) => {
            let mut url = url::Url::parse(&template)
                .with_context(|| format!("invalid health_check_url '{template}'"))?;
            url.set_host(Some("127.0.0.1")).ok();
            url.set_port(Some(port)).ok();
            url.to_string()
        }
        None => format!("http://127.0.0.1:{port}/health"),
    };

    let request = LoadModelRequest {
        provider_name: model_config.backend.clone(),
        model_path,
        gpu_variant: variant_folder,
        // params is wire-compat only: command/args/env carry the fully
        // resolved spec and are authoritative.
        params: HashMap::new(),
        // Canonical identity: the config key (== `backend_name`), NOT the raw
        // request name. The reconciler joins desired_models.model_name with the
        // tamad process's model_name; the forward path passes the raw
        // repo_id/api_name while the management path passes the config key, so
        // both must normalise to the config key here (the same key the mirror
        // and active_models already use) or the reconciler flaps.
        model_name: backend_name.clone(),
        command: command.to_string_lossy().into_owned(),
        args,
        env,
        health_url,
        health_timeout_ms: (config.proxy.startup_timeout_secs as i64) * 1000,
        // The tamad resolves the isolation env var for this device against
        // its own GPU list (the proxy never samples local hardware).
        gpu_device: device.clone().unwrap_or_default(),
        // Docker config (native = None): the tamad spawns a container when
        // this is present, instead of the host binary in `command`.
        docker_config_json: docker_config_json.unwrap_or_default(),
    };

    Ok(LoadSpec {
        backend_name,
        backend: model_config.backend.clone(),
        gpu_device: device,
        request,
    })
}

/// Build the launch spec for the TTS backend (Kokoro-FastAPI uvicorn
/// server). Same shape as the LLM spec: the proxy builds command/args/env
/// from the *central DB* installation row (whose `path` is the tamad host's
/// install dir, plan-191 Task 7) and the tamad spawns it.
///
/// `backend_name` is the installation name (e.g. "tts_kokoro"); the mirror
/// is keyed by it, matching the TTS handler's lookup.
pub async fn build_tts_load_spec(state: &ProxyState, backend_name: &str) -> Result<LoadSpec> {
    let port = find_free_port_with_retry(3).await?;
    let health_url = format!("http://127.0.0.1:{port}/health");

    // Load the installation row from the central DB (the path stored there
    // is the tamad host's install dir — plan-191 Task 7).
    let pool = state.db_pool();
    let mgr = crate::installations::InstallationManager::new(pool);
    let variants = mgr
        .list_versions(backend_name, None)
        .await
        .with_context(|| format!("Failed to list versions for '{backend_name}'"))?
        .ok_or_else(|| anyhow!("Backend '{backend_name}' not installed"))?;
    let variant = variants
        .first()
        .map(|v| v.gpu_variant.clone())
        .unwrap_or_else(|| "cpu".to_string());
    let info = mgr
        .get_active(backend_name, &variant)
        .await
        .with_context(|| format!("Backend '{backend_name}' not found in manager"))?
        .ok_or_else(|| anyhow!("Backend '{backend_name}' not installed"))?;

    // Derive paths from InstallationInfo.path (base_dir = <install>/tts_kokoro/).
    // The repo root is the kokoro-fastapi subdirectory, and venv is a sibling.
    // Env paths are absolute so the tamad's spawn working dir (next to the
    // binary) does not change their meaning.
    let base_path = info.path.as_path();
    let repo_root = base_path.join("kokoro-fastapi");
    let python_bin = base_path.join("venv").join("bin").join("python");

    let startup_timeout_ms =
        { (state.config.read().await.proxy.startup_timeout_secs as i64) * 1000 };

    let args: Vec<String> = vec![
        "-m".into(),
        "uvicorn".into(),
        "api.src.main:app".into(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
    ];
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert(
        "PYTHONPATH".into(),
        repo_root.to_string_lossy().into_owned(),
    );
    env.insert(
        "MODEL_DIR".into(),
        repo_root
            .join("api/src/models")
            .to_string_lossy()
            .into_owned(),
    );
    env.insert(
        "VOICES_DIR".into(),
        repo_root
            .join("api/src/voices/v1_0")
            .to_string_lossy()
            .into_owned(),
    );

    let request = LoadModelRequest {
        provider_name: info.name.clone(),
        model_path: String::new(),
        gpu_variant: variant,
        params: HashMap::new(),
        model_name: backend_name.to_string(),
        command: python_bin.to_string_lossy().into_owned(),
        args,
        env,
        health_url,
        health_timeout_ms: startup_timeout_ms,
        gpu_device: String::new(),
        docker_config_json: String::new(),
    };

    Ok(LoadSpec {
        backend_name: backend_name.to_string(),
        backend: info.name.clone(),
        gpu_device: None,
        request,
    })
}

/// Build the launch spec for the compaction backend (embedded LLMLingua-2
/// Python server, plan-191 Task 10).
///
/// The Python source is embedded in the *tamad* binary; the proxy can only
/// send the generic launch shape (`uv run uvicorn ...`) plus the config
/// values, and the tamad injects its own `--project` path before spawning.
/// The proxy's `compaction.server_path` override no longer applies: the
/// server file now lives on the tamad host (it is the tamad's own
/// embedded copy).
pub async fn build_compaction_load_spec(state: &ProxyState) -> Result<LoadSpec> {
    let compaction = state.config.read().await.compaction.clone();
    if !compaction.enabled {
        bail!("Compaction is not enabled in config");
    }

    // Honor a fixed config port; otherwise allocate a fresh one
    // (single-node staging: proxy and tamad share the port namespace).
    let port = match compaction.port {
        Some(p) => p,
        None => find_free_port_with_retry(3).await?,
    };

    let startup_timeout_ms = (state.config.read().await.proxy.startup_timeout_secs as i64) * 1000;

    let health_url = format!("http://127.0.0.1:{port}/health");
    let args: Vec<String> = vec![
        "run".into(),
        "uvicorn".into(),
        "main:app".into(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
    ];
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert("COMPACTION_PORT".into(), port.to_string());
    env.insert(
        "COMPACTION_DEVICE".into(),
        compaction.device.as_str().to_string(),
    );

    let request = LoadModelRequest {
        provider_name: "compaction".into(),
        model_path: String::new(),
        gpu_variant: "cpu".into(),
        params: HashMap::new(),
        model_name: "compaction".into(),
        // The tamad resolves "uv" from its own PATH and injects
        // `--project <embedded server dir>` for provider_name "compaction".
        command: "uv".into(),
        args,
        env,
        health_url,
        health_timeout_ms: startup_timeout_ms,
        gpu_device: String::new(),
        docker_config_json: String::new(),
    };

    Ok(LoadSpec {
        backend_name: "compaction".into(),
        backend: "compaction".into(),
        gpu_device: None,
        request,
    })
}

/// Resolve a model's owning provider (plan-191 Task 5).
///
/// 1. `model_configs.provider_name` (when set) → that provider.
/// 2. Otherwise fall back to the single Local provider with a tamad
///    assigned (single-node deployments have exactly one).
///
/// Errors are user-facing: a model whose provider is missing, or a
/// provider without a tamad, cannot be loaded.
pub async fn resolve_provider_for_model(state: &ProxyState, model_name: &str) -> Result<Provider> {
    let provider_name = {
        state
            .registry
            .model_configs
            .read()
            .await
            .get(model_name)
            .and_then(|c| c.provider_name.clone())
    };

    match provider_name {
        Some(name) => {
            let provider = crate::db::queries::get_provider(&state.db_pool, &name)
                .await?
                .ok_or_else(|| anyhow!("Provider \"{name}\" not found"))?;
            if provider.provider_type.is_remote() {
                bail!(
                    "model \"{model_name}\" uses remote provider \"{name}\" — no tamad load applicable"
                );
            }
            Ok(provider)
        }
        None => {
            let providers = crate::db::queries::list_providers(&state.db_pool).await?;
            let local: Vec<Provider> = providers
                .into_iter()
                .filter(|p| p.provider_type.is_local() && p.tamad_id.is_some())
                .collect();
            match local.len() {
                1 => Ok(local.into_iter().next().expect("checked len 1")),
                0 => bail!(
                    "No local provider with a tamad assigned — create one \
                     (POST /tama/v1/providers) or set provider_name on model \"{model_name}\""
                ),
                _ => bail!(
                    "Multiple local providers have tamads assigned — \
                     set provider_name on model \"{model_name}\" to disambiguate"
                ),
            }
        }
    }
}

/// Fail-fast VRAM guard: refuse to load a GPU model when the target
/// tamad reports all of its GPUs ≥ 95% VRAM used.
///
/// No-GPU data (empty `gpus`) is treated as "unknown" and allowed — the
/// tamad may legitimately not report VRAM.
pub fn vram_load_ok(
    stats: &SystemStats,
    variant: &GpuVariant,
    gpu_device: Option<&str>,
) -> Result<()> {
    if matches!(variant, GpuVariant::CpuOnly) || gpu_device.is_none() {
        return Ok(());
    }
    if stats.gpus.is_empty() {
        return Ok(());
    }
    let all_full = stats.gpus.iter().all(|g| {
        g.vram_total_bytes > 0 && (g.vram_used_bytes as f64 / g.vram_total_bytes as f64) >= 0.95
    });
    if all_full {
        bail!(
            "target tamad has no free VRAM: all GPUs are >=95% used — \
             unload another model or add capacity"
        );
    }
    Ok(())
}

/// Insert (or replace) the `Starting` placeholder mirror for `spec` so the
/// dashboard and the models API expose the load window as "loading".
///
/// The `LoadModel` RPC in [`load_spec_on_tamad`] blocks until the tamad
/// finishes spawning the backend and it passes its health poll — minutes
/// for a large model. Without a runtime entry, `collect_model_state_snapshots`
/// would report the model as `idle` for the whole window. The key is the
/// canonical config key (`spec.backend_name`) — the same key the `Ready`
/// mirror is inserted under on success, so it replaces the placeholder in
/// place — and a `Failed` mirror replaces it if the RPC fails.
pub(crate) async fn insert_starting_mirror(state: &ProxyState, spec: &LoadSpec) {
    state.registry.models.write().await.insert(
        spec.backend_name.clone(),
        crate::proxy::BackendState::Starting {
            model_name: spec.backend_name.clone(),
            backend: spec.backend.clone(),
            backend_url: String::new(),
            backend_pid: 0,
            last_accessed: std::time::Instant::now(),
            start_time: std::time::Instant::now(),
            consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            failure_timestamp: None,
            is_docker: false,
        },
    );
}

/// The shared tamad-load tail: resolve the provider's live tamad, run the
/// capacity/VRAM guards, issue `LoadModel`, mark the model *desired* in the
/// DB, and refresh the local mirror.
///
/// `evict_lru` enables the LRU capacity guard (LLM loads only — auxiliary
/// backends like TTS/compaction are excluded from the capacity count and
/// must not trigger eviction).
///
/// Returns the mirror key for the model.
pub async fn load_spec_on_tamad(
    state: &ProxyState,
    spec: &LoadSpec,
    evict_lru: bool,
) -> Result<String> {
    let provider = resolve_provider_for_model(state, &spec.backend_name).await?;
    let tamad_id = provider
        .tamad_id
        .clone()
        .ok_or_else(|| anyhow!("Provider \"{}\" has no tamad assigned", provider.name))?;

    let pool = state.tamad_pool();
    let handle = pool
        .handle_for_provider(Some(&tamad_id))
        .await
        .ok_or_else(|| {
            anyhow!(
                "No live stats stream for the tamad of provider \"{}\" (is it registered and online?)",
                provider.name
            )
        })?;

    if evict_lru {
        // LRU capacity check: the mirror now tracks the tamads' process
        // tables (plan-191 Task 5), so capacity/eviction operates on the
        // proxy's view of per-tamad state. Best-effort: an eviction failure
        // must not block the load itself. (Runs for CPU loads too — the
        // target device `None` matches the CPU-model bucket, as before.)
        if let Err(e) = state.evict_lru_if_needed(spec.gpu_device.clone()).await {
            warn!(error = %e, model = %spec.backend_name, "LRU eviction check failed");
        }

        // VRAM fail-fast on the tamad's latest snapshot (best-effort: a
        // missing snapshot means "unknown", not "full").
        if let Some(latest) = handle.latest().await {
            let variant: GpuVariant = spec
                .request
                .gpu_variant
                .parse()
                .unwrap_or(GpuVariant::CpuOnly);
            vram_load_ok(&latest, &variant, spec.gpu_device.as_deref())?;
        }
    }

    // The `LoadModel` RPC blocks until the tamad's spawn + health poll
    // completes (minutes for a large model); insert a `Starting` placeholder
    // mirror first so the dashboard and the models API expose that window as
    // "loading" instead of "idle". The `Ready` mirror inserted below (same
    // key) replaces it on success; a `Failed` mirror replaces it if the RPC
    // fails (see below).
    insert_starting_mirror(state, spec).await;

    let resp = match handle.load_model(&spec.request).await.with_context(|| {
        format!(
            "LoadModel RPC to the tamad of provider \"{}\" failed",
            provider.name
        )
    }) {
        Ok(resp) => resp,
        // A failed load must surface as `failed` in the dashboard / models
        // API instead of silently going back to `idle`: leave a `Failed`
        // mirror carrying the error, then re-return the original error so
        // the caller still sees it.
        Err(e) => {
            state.registry.models.write().await.insert(
                spec.backend_name.clone(),
                crate::proxy::BackendState::Failed {
                    model_name: spec.backend_name.clone(),
                    backend: spec.backend.clone(),
                    error: e.to_string(),
                },
            );
            return Err(e);
        }
    };

    // Desired state in the central DB (single-writer rule: only the proxy).
    // Keyed by the canonical config key so the reconciler's join with the
    // tamad process name (also the config key) is consistent.
    crate::db::queries::set_desired(&state.db_pool, &spec.backend_name, &tamad_id)
        .await
        .with_context(|| format!("marking model \"{}\" as desired", spec.backend_name))?;

    // Staging mirror: keep the forward path + management API working.
    let port = resp
        .endpoint_url
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<i64>().ok())
        .unwrap_or(0);
    let mirror = crate::proxy::BackendState::Ready {
        model_name: spec.backend_name.clone(),
        backend: spec.backend.clone(),
        backend_pid: resp.pid.max(0) as u32,
        backend_url: resp.endpoint_url.clone(),
        load_time: std::time::SystemTime::now(),
        last_accessed: std::time::Instant::now(),
        consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        failure_timestamp: None,
        restart_count: 0,
        is_docker: false,
    };
    state
        .registry
        .models
        .write()
        .await
        .insert(spec.backend_name.clone(), mirror);

    // Best-effort active_models row (existing convention).
    let _ = crate::db::queries::insert_active_model(
        &state.db_pool,
        &spec.backend_name,
        &spec.backend_name,
        &spec.backend,
        resp.pid.max(0) as i64,
        port,
        &resp.endpoint_url,
    )
    .await;

    state
        .metrics
        .counters
        .models_loaded
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    tracing::info!(
        model = %spec.backend_name,
        provider = %provider.name,
        tamad = %handle.connection.name,
        pid = resp.pid,
        endpoint = %resp.endpoint_url,
        "model loaded on tamad"
    );

    Ok(spec.backend_name.clone())
}

/// Full LLM load path via the model's provider tamad (plan-191 Task 5):
/// build the launch spec, then run the shared tamad-load tail.
///
/// Returns the mirror (config backend) key for the model.
pub async fn load_model_on_tamad(state: &ProxyState, model_name: &str) -> Result<String> {
    let model_toml = state.get_model_toml(model_name).await;
    let spec = build_load_spec(state, model_name, model_toml.as_ref()).await?;
    load_spec_on_tamad(state, &spec, true).await
}

/// Load the TTS backend described by `backend_name` (a `tts_*`
/// installation, e.g. "tts_kokoro") on the model's provider tamad.
///
/// Replaces the old local-spawn path (plan-191 Task 10): the spec is built
/// from the central DB installation row and the uvicorn process is spawned
/// by the tamad.
pub async fn load_tts_on_tamad(state: &ProxyState, backend_name: &str) -> Result<String> {
    // Fast path — already loaded (live row, plan-193 T4 flip).
    let live = crate::proxy::live_rows(state.tamad_pool().as_ref()).await;
    if live
        .row(backend_name)
        .map(|r| r.status == "ready")
        .unwrap_or(false)
    {
        return Ok(backend_name.to_string());
    }
    let spec = build_tts_load_spec(state, backend_name).await?;
    load_spec_on_tamad(state, &spec, false).await
}

/// Load the compaction backend (LLMLingua-2) on the model's provider
/// tamad. Replaces the old local-spawn path (plan-191 Task 10).
pub async fn load_compaction_on_tamad(state: &ProxyState) -> Result<()> {
    if !state.config.read().await.compaction.enabled {
        anyhow::bail!("Compaction is not enabled in config");
    }
    // Fast path — already loaded (live row, plan-3 flip).
    let live = crate::proxy::live_rows(state.tamad_pool().as_ref()).await;
    if live
        .row("compaction")
        .map(|r| r.status == "ready")
        .unwrap_or(false)
    {
        return Ok(());
    }
    let spec = build_compaction_load_spec(state).await?;
    load_spec_on_tamad(state, &spec, false).await?;
    Ok(())
}

/// Resolve a model name to its canonical config key (the mirror /
/// `active_models` / `desired_models` join key). Falls back to the input when
/// the model isn't in the registry (e.g. an already-unloaded model),
/// preserving the previous pass-through behaviour for the unload path.
async fn canonical_model_key(state: &ProxyState, model_name: &str) -> String {
    let config = state.config.read().await;
    let model_configs = state.registry.model_configs.read().await;
    config
        .resolve_backends_for_model(&model_configs, model_name)
        .into_iter()
        .next()
        .map(|(name, _, _)| name)
        .unwrap_or_else(|| model_name.to_string())
}

/// Unload a model on its provider's tamad and clear its desired state.
///
/// Returns `true` when the tamad actually had the model loaded, `false`
/// when the model was unknown to the tamad (idempotent unload).
pub async fn unload_model_on_tamad(state: &ProxyState, model_name: &str) -> Result<bool> {
    let provider = resolve_provider_for_model(state, model_name).await?;
    let tamad_id = provider
        .tamad_id
        .clone()
        .ok_or_else(|| anyhow!("Provider \"{}\" has no tamad assigned", provider.name))?;
    let handle = state
        .tamad_pool()
        .handle_for_provider(Some(&tamad_id))
        .await
        .ok_or_else(|| {
            anyhow!(
                "No live stats stream for the tamad of provider \"{}\"",
                provider.name
            )
        })?;

    // Normalise to the config key so the RPC target and the desired-state
    // row line up with the load path and the reconciler's join.
    let key = canonical_model_key(state, model_name).await;
    match handle.unload_model(&key).await {
        Ok(()) => {
            let _ = crate::db::queries::clear_desired(&state.db_pool, &key).await;
            Ok(true)
        }
        Err(e) if is_not_loaded_err(&e) => Ok(false),
        Err(e) => Err(e).with_context(|| {
            format!(
                "UnloadModel RPC to the tamad of provider \"{}\" failed",
                provider.name
            )
        }),
    }
}

/// Whether a tamad unload error means "model not loaded here".
fn is_not_loaded_err(e: &anyhow::Error) -> bool {
    e.to_string().contains("not loaded on this tamad")
}

/// Find a free port by binding to 127.0.0.1:0 and releasing the listener.
async fn find_free_port() -> Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Find a free port with retry (bind to 0.0.0.0:0 never collides, but the
/// retry keeps parity with the former local-spawn helper).
async fn find_free_port_with_retry(max_attempts: u32) -> Result<u16> {
    let mut last_err: Option<anyhow::Error> = None;
    for _attempt in 1..=max_attempts.max(1) {
        match find_free_port().await {
            Ok(port) => return Ok(port),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("failed to find a free port")))
}

#[cfg(test)]
mod tests;
