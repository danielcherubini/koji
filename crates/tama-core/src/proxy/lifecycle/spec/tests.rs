//! Tests for the shared load-spec builder (plan-191 Task 5).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{BackendConfig, Config, ModelConfig, QuantEntry, QuantKind};
use crate::gpu::GpuVariant;
use crate::proxy::lifecycle::spec::*;
use crate::proxy::types::BackendState;
use crate::proxy::ProxyState;
use crate::tamad::pool::test_support::{grpc_conn, start_stub, stub_default, wait_for};
use crate::tamad::{GpuInfo, LoadModelRequest};
use crate::testing::postgres::with_schema;

fn test_config(models_dir: &std::path::Path) -> crate::config::Config {
    let mut config = crate::config::Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.backends.insert(
        "llama_cpp".to_string(),
        BackendConfig {
            path: Some("/usr/local/bin/llama-server".to_string()),
            version: None,
            gpu_variant: None,
        },
    );
    config
}

fn gguf_model_config() -> ModelConfig {
    let mut quants = BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "m.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: None,
            context_length: None,
        },
    );
    ModelConfig {
        backend: "llama_cpp".to_string(),
        gpu_variant: Some(GpuVariant::Cuda {
            version: "12.4".to_string(),
        }),
        gpu_device: Some("GPU0".to_string()),
        model: Some("owner/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        quants,
        enabled: true,
        ..Default::default()
    }
}

/// build_load_spec produces a complete LaunchSpec: command from the
/// installation/config path, relative model path in args + model_path,
/// gpu env var present, health URL with the allocated port.
#[tokio::test]
async fn test_build_load_spec_fields() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let models_dir = tempfile::tempdir().unwrap();
    let config = test_config(models_dir.path());
    let state = Arc::new(ProxyState::new(config.clone(), None, pool.clone()));

    state
        .registry
        .model_configs
        .write()
        .await
        .insert("test-model".to_string(), gguf_model_config());

    // Installation config: default args/env + health URL template.
    let manager = crate::installations::InstallationManager::new(pool.clone());
    manager
        .save_config(
            "llama_cpp",
            "cuda",
            &["--flash-attn".to_string()],
            &["OMP_THREADS_BIND=0-7".to_string()],
            Some("http://localhost:5801/health"),
        )
        .await
        .unwrap();

    let spec = build_load_spec(&state, "test-model", None)
        .await
        .expect("spec builds");

    assert_eq!(spec.backend_name, "test-model");
    assert_eq!(spec.backend, "llama_cpp");

    let req = &spec.request;
    assert_eq!(req.command, "/usr/local/bin/llama-server");
    assert_eq!(req.model_name, "test-model");
    assert_eq!(req.provider_name, "llama_cpp");
    assert_eq!(req.gpu_variant, "cuda");
    assert!(req.params.is_empty(), "params is wire-compat only");
    assert_eq!(req.model_path, "owner/repo/m.gguf");

    // Args: model path stays absolute under the proxy's models dir — the
    // tamad re-anchors it to its own models_dir via TAMA_MODELS_DIR.
    let args = &req.args;
    let m_pos = args
        .iter()
        .position(|a| a == "-m")
        .expect("-m flag injected");
    assert_eq!(
        args[m_pos + 1],
        models_dir
            .path()
            .join("owner/repo/m.gguf")
            .to_string_lossy()
            .to_string()
    );
    // Installation default args survive.
    assert!(args.contains(&"--flash-attn".to_string()));
    // --host/--port overrides present with a real port.
    let port_pos = args
        .iter()
        .position(|a| a == "--port")
        .expect("--port override");
    let port: u16 = args[port_pos + 1].parse().expect("numeric port");
    assert!(port > 0);
    let host_pos = args
        .iter()
        .position(|a| a == "--host")
        .expect("--host override");
    assert_eq!(args[host_pos + 1], "127.0.0.1");

    // Env: installation default + proxy models dir. GPU isolation vars are
    // resolved on the tamad (the proxy never samples local hardware) — they
    // must be absent here, and the device is forwarded instead.
    assert_eq!(req.env["OMP_THREADS_BIND"], "0-7");
    assert!(
        !req.env.contains_key("CUDA_VISIBLE_DEVICES"),
        "the proxy must not resolve GPU env vars itself"
    );
    assert_eq!(req.gpu_device, "GPU0");
    assert_eq!(
        req.env[PROXY_MODELS_DIR_ENV],
        models_dir.path().to_string_lossy()
    );

    // Health URL: template with the allocated port.
    assert!(
        req.health_url.contains(&format!(":{port}")),
        "health url must use the allocated port: {}",
        req.health_url
    );
    assert!(req.health_url.ends_with("/health"));

    // Health timeout mirrors the proxy startup timeout.
    assert_eq!(
        req.health_timeout_ms,
        (config.proxy.startup_timeout_secs as i64) * 1000
    );

    let _ = guard.finish().await;
}

/// The `LoadModelRequest.model_name` must be the canonical config key, not the
/// raw request name. The reconciler joins `desired_models.model_name` with the
/// tamad process's `model_name`; if the forward path (which sees the raw
/// repo_id/api_name) and the management path (which sees the config key)
/// populated those with different strings, the reconciler would flap
/// (load + unload every tick). Normalising to the config key — the same key
/// the mirror and `active_models` already use — makes the join consistent.
#[tokio::test]
async fn test_build_load_spec_normalises_model_name_to_config_key() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let models_dir = tempfile::tempdir().unwrap();
    let config = test_config(models_dir.path());
    let state = Arc::new(ProxyState::new(config.clone(), None, pool.clone()));

    // Config key is `owner--repo` (from_repo_id lowercases + `/` → `--`), but
    // the raw name the forward path passes is the repo_id `owner/repo`.
    state
        .registry
        .model_configs
        .write()
        .await
        .insert("owner--repo".to_string(), gguf_model_config());

    let manager = crate::installations::InstallationManager::new(pool.clone());
    manager
        .save_config(
            "llama_cpp",
            "cuda",
            &[],
            &[],
            Some("http://localhost:5801/health"),
        )
        .await
        .unwrap();

    let spec = build_load_spec(&state, "owner/repo", None)
        .await
        .expect("spec builds from the raw repo_id");

    assert_eq!(spec.backend_name, "owner--repo");
    // The tamad-side identity is the config key, so desired + process name
    // line up regardless of which path initiated the load.
    assert_eq!(spec.request.model_name, "owner--repo");

    let _ = guard.finish().await;
}

/// Transformers-format models: repo dir as relative model_path, no `-m`.
#[tokio::test]
async fn test_build_load_spec_transformers() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let models_dir = tempfile::tempdir().unwrap();
    let mut config = test_config(models_dir.path());
    config.backends.insert(
        "vllm".to_string(),
        BackendConfig {
            path: Some("/usr/local/bin/vllm".to_string()),
            version: None,
            gpu_variant: None,
        },
    );
    let state = Arc::new(ProxyState::new(config, None, pool.clone()));

    let mut model_config = gguf_model_config();
    model_config.backend = "vllm".to_string();
    model_config.hf_format = Some("transformers".to_string());
    model_config.quant = None;
    state
        .registry
        .model_configs
        .write()
        .await
        .insert("tf-model".to_string(), model_config);

    let spec = build_load_spec(&state, "tf-model", None)
        .await
        .expect("spec builds");

    assert_eq!(spec.request.model_path, "owner/repo");
    assert!(
        !spec.request.args.iter().any(|a| a == "-m"),
        "transformers format must not inject -m"
    );

    let _ = guard.finish().await;
}

/// A Local provider without a tamad → clear error from the load path.
#[tokio::test]
async fn test_load_model_on_tamad_provider_without_tamad() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let models_dir = tempfile::tempdir().unwrap();
    let config = test_config(models_dir.path());
    let state = Arc::new(ProxyState::new(config, None, pool.clone()));

    crate::db::queries::insert_provider(
        pool.as_ref(),
        "myprov",
        "local",
        "llama_cpp",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let mut mc = gguf_model_config();
    mc.provider_name = Some("myprov".to_string());
    state
        .registry
        .model_configs
        .write()
        .await
        .insert("test-model".to_string(), mc);

    let err = load_model_on_tamad(&state, "test-model")
        .await
        .expect_err("must fail: provider has no tamad");
    assert!(
        err.to_string()
            .contains("Provider \"myprov\" has no tamad assigned"),
        "clear error expected, got: {err}"
    );

    let _ = guard.finish().await;
}

/// ensure_model_loaded surfaces the no-tamad error through on_load_error.
#[tokio::test]
async fn test_ensure_model_loaded_local_provider_without_tamad() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let models_dir = tempfile::tempdir().unwrap();
    let config = test_config(models_dir.path());
    let state = Arc::new(ProxyState::new(config, None, pool.clone()));

    crate::db::queries::insert_provider(
        pool.as_ref(),
        "notamad",
        "local",
        "llama_cpp",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let mut mc = gguf_model_config();
    mc.provider_name = Some("notamad".to_string());
    state
        .registry
        .model_configs
        .write()
        .await
        .insert("test-model".to_string(), mc);

    let err = crate::proxy::lifecycle::ensure_model_loaded(&state, "test-model", |_, e| Err(e))
        .await
        .expect_err("must fail: provider has no tamad");
    assert!(
        err.to_string().contains("has no tamad assigned"),
        "clear error expected, got: {err}"
    );

    let _ = guard.finish().await;
}

/// No local provider with a tamad at all → clear error.
#[tokio::test]
async fn test_load_model_on_tamad_no_local_provider() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let models_dir = tempfile::tempdir().unwrap();
    let config = test_config(models_dir.path());
    let state = Arc::new(ProxyState::new(config, None, pool.clone()));

    state
        .registry
        .model_configs
        .write()
        .await
        .insert("test-model".to_string(), gguf_model_config());

    let err = load_model_on_tamad(&state, "test-model")
        .await
        .expect_err("must fail: no local provider");
    assert!(
        err.to_string()
            .contains("No local provider with a tamad assigned"),
        "clear error expected, got: {err}"
    );

    let _ = guard.finish().await;
}

/// provider_name set → that provider is used (and its missing tamad
/// reported under its name).
#[tokio::test]
async fn test_resolve_provider_by_name() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let models_dir = tempfile::tempdir().unwrap();
    let config = test_config(models_dir.path());
    let state = Arc::new(ProxyState::new(config, None, pool.clone()));

    crate::db::queries::insert_provider(
        pool.as_ref(),
        "prov-a",
        "local",
        "llama_cpp",
        Some("tamad-1"),
        None,
        None,
    )
    .await
    .unwrap();

    let mut mc = gguf_model_config();
    mc.provider_name = Some("prov-a".to_string());
    state
        .registry
        .model_configs
        .write()
        .await
        .insert("test-model".to_string(), mc);

    let provider = resolve_provider_for_model(&state, "test-model")
        .await
        .expect("provider resolves");
    assert_eq!(provider.name, "prov-a");
    assert_eq!(provider.tamad_id.as_deref(), Some("tamad-1"));

    // Unknown provider name → error.
    let mut mc2 = gguf_model_config();
    mc2.provider_name = Some("ghost".to_string());
    state
        .registry
        .model_configs
        .write()
        .await
        .insert("ghost-model".to_string(), mc2);
    let err = resolve_provider_for_model(&state, "ghost-model")
        .await
        .expect_err("unknown provider must fail");
    assert!(err.to_string().contains("not found"));

    let _ = guard.finish().await;
}

/// Fallback: a single local provider with a tamad is used when the model
/// has no provider_name.
#[tokio::test]
async fn test_resolve_provider_fallback_single_local() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let models_dir = tempfile::tempdir().unwrap();
    let config = test_config(models_dir.path());
    let state = Arc::new(ProxyState::new(config, None, pool.clone()));

    crate::db::queries::insert_provider(
        pool.as_ref(),
        "only-local",
        "local",
        "llama_cpp",
        Some("tamad-9"),
        None,
        None,
    )
    .await
    .unwrap();

    state
        .registry
        .model_configs
        .write()
        .await
        .insert("test-model".to_string(), gguf_model_config());

    let provider = resolve_provider_for_model(&state, "test-model")
        .await
        .expect("fallback provider resolves");
    assert_eq!(provider.name, "only-local");
    assert_eq!(provider.tamad_id.as_deref(), Some("tamad-9"));

    let _ = guard.finish().await;
}

fn gpu(index: i32, total: i64, used: i64) -> GpuInfo {
    GpuInfo {
        index,
        name: "GPU".to_string(),
        driver_version: String::new(),
        vram_total_bytes: total,
        vram_used_bytes: used,
        utilization_percent: 0.0,
        temperature_c: 0.0,
        power_w: 0.0,
    }
}

fn stats_with_gpus(gpus: Vec<GpuInfo>) -> crate::tamad::SystemStats {
    crate::tamad::SystemStats {
        cpu_usage_percent: 0.0,
        memory_total_bytes: 0,
        memory_used_bytes: 0,
        swap_total_bytes: 0,
        swap_used_bytes: 0,
        disk_total_bytes: 0,
        disk_free_bytes: 0,
        gpus,
        processes: vec![],
    }
}

/// All GPUs ≥ 95% used → GPU model load must fail fast.
#[test]
fn test_vram_load_ok_all_full() {
    let stats = stats_with_gpus(vec![
        gpu(0, 24_000, 22_800), // 95%
        gpu(1, 24_000, 24_000), // 100%
    ]);
    let err = vram_load_ok(
        &stats,
        &GpuVariant::Cuda {
            version: "12.4".to_string(),
        },
        Some("GPU0"),
    )
    .expect_err("all-full must fail");
    assert!(err.to_string().contains("no free VRAM"));
}

/// Some GPU has headroom → allowed.
#[test]
fn test_vram_load_ok_partial() {
    let stats = stats_with_gpus(vec![gpu(0, 24_000, 23_900), gpu(1, 24_000, 12_000)]);
    assert!(vram_load_ok(
        &stats,
        &GpuVariant::Cuda {
            version: "12.4".to_string(),
        },
        Some("GPU0"),
    )
    .is_ok());
}

/// CPU-only models and unassigned devices are never blocked.
#[test]
fn test_vram_load_ok_cpu_only() {
    let stats = stats_with_gpus(vec![gpu(0, 24_000, 24_000)]);
    assert!(vram_load_ok(&stats, &GpuVariant::CpuOnly, Some("GPU0")).is_ok());
    assert!(vram_load_ok(&stats, &GpuVariant::CpuOnly, None).is_ok());
}

/// No GPU data reported → unknown, allowed.
#[test]
fn test_vram_load_ok_no_gpu_data() {
    let stats = stats_with_gpus(vec![]);
    assert!(vram_load_ok(
        &stats,
        &GpuVariant::Cuda {
            version: "12.4".to_string(),
        },
        Some("GPU0"),
    )
    .is_ok());
}

/// build_tts_load_spec builds the uvicorn launch shape from the central DB
/// installation row (plan-191 Task 10: the tamad spawns the process).
#[tokio::test]
async fn test_build_tts_load_spec_fields() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let tempdir = tempfile::tempdir().unwrap();
    let base_dir = tempdir.path().join("backends");
    let backend_dir = base_dir.join("tts_kokoro");
    std::fs::create_dir_all(&backend_dir).unwrap();

    let config = test_config(tempdir.path());
    let state = Arc::new(ProxyState::new(config, None, pool.clone()));

    let mgr = crate::installations::InstallationManager::new(pool.clone());
    mgr.add_installation(&crate::installations::InstallationInfo {
        name: "tts_kokoro".into(),
        backend_type: crate::installations::InstallationType::TtsKokoro,
        version: "1.0.0".into(),
        path: backend_dir.clone(),
        installed_at: 0,
        gpu_variant: "cpu".into(),
        source: None,
        docker_config: None,
    })
    .await
    .unwrap();

    let spec = build_tts_load_spec(&state, "tts_kokoro")
        .await
        .expect("TTS spec builds");

    assert_eq!(spec.backend_name, "tts_kokoro");
    assert_eq!(spec.backend, "tts_kokoro");
    assert!(spec.gpu_device.is_none());

    let req = &spec.request;
    // Command = the venv python, under the install dir from the central DB.
    assert_eq!(
        req.command,
        backend_dir
            .join("venv/bin/python")
            .to_string_lossy()
            .to_string()
    );
    // uvicorn target + host/port override + /health on the same port.
    assert!(req.args.iter().any(|a| a == "api.src.main:app"));
    let port_pos = req
        .args
        .iter()
        .position(|a| a == "--port")
        .expect("--port present");
    let port = &req.args[port_pos + 1];
    assert!(req.health_url.contains(&format!(":{port}")));
    assert!(req.health_url.ends_with("/health"));
    // Env: PYTHONPATH is absolute (the tamad's cwd differs from the
    // legacy proxy's), MODEL_DIR/VOICES_DIR are absolute paths.
    assert_eq!(
        req.env["PYTHONPATH"],
        backend_dir
            .join("kokoro-fastapi")
            .to_string_lossy()
            .to_string()
    );
    assert_eq!(
        req.env["MODEL_DIR"],
        backend_dir
            .join("kokoro-fastapi/api/src/models")
            .to_string_lossy()
            .to_string()
    );
    assert!(req.gpu_device.is_empty());

    let _ = guard.finish().await;
}

/// build_compaction_load_spec produces the `uv run uvicorn` shape with the
/// config's port and device env — the tamad injects `--project` itself.
#[tokio::test]
async fn test_build_compaction_load_spec_fields() {
    let pool = crate::db::pool::test_dummy_pool();
    let mut config = Config::default();
    config.compaction.enabled = true;
    config.compaction.port = Some(41_234);
    let state = Arc::new(ProxyState::new(config, None, pool.clone()));

    let spec = build_compaction_load_spec(&state)
        .await
        .expect("compaction spec builds");

    assert_eq!(spec.backend_name, "compaction");
    let req = &spec.request;
    assert_eq!(req.command, "uv");
    assert!(req.args.iter().any(|a| a == "main:app"));
    assert_eq!(req.args[0], "run");
    // No --project: the tamad owns its embedded server dir and injects it.
    assert!(!req.args.iter().any(|a| a == "--project"));
    assert!(req.args.iter().any(|a| a == "41234"));
    assert_eq!(req.env["COMPACTION_PORT"], "41234");
    assert_eq!(req.health_url, "http://127.0.0.1:41234/health");
    assert_eq!(req.provider_name, "compaction");
    assert_eq!(req.model_name, "compaction");
}

/// Compaction disabled → spec build fails with a clear error.
#[tokio::test]
async fn test_build_compaction_load_spec_disabled() {
    let pool = crate::db::pool::test_dummy_pool();
    let config = Config::default(); // compaction disabled by default
    let state = Arc::new(ProxyState::new(config, None, pool));

    let err = build_compaction_load_spec(&state)
        .await
        .expect_err("disabled compaction must fail");
    assert!(err.to_string().contains("not enabled"));
}

// ─── tamad load-window mirror visibility (issue #192) ────────────────────

/// A minimal launch spec for the mirror-visibility tests: the stub tamad
/// answers any `LoadModel` for `model_name = "test-model"`.
fn test_load_spec() -> LoadSpec {
    LoadSpec {
        backend_name: "test-model".to_string(),
        backend: "llama_cpp".to_string(),
        gpu_device: None,
        request: LoadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_name: "test-model".to_string(),
            ..Default::default()
        },
    }
}

/// Stand up the DB rows (provider + tamad), the stats-stream pool handle,
/// and the model config needed for `load_spec_on_tamad` to reach a stub
/// tamad.
///
/// `load_model_fail` makes the stub's `LoadModel` RPC fail; `load_delay`
/// makes it sleep before answering (simulating a minutes-long tamad load
/// in milliseconds).
async fn setup_stub_load(
    load_model_fail: bool,
    load_delay: Option<Duration>,
) -> (crate::testing::postgres::SchemaGuard, Arc<ProxyState>) {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let models_dir = tempfile::tempdir().unwrap();
    let config = test_config(models_dir.path());

    let mut stub = stub_default();
    if load_model_fail {
        *stub.load_model_fail.lock().await = true;
    }
    if let Some(delay) = load_delay {
        stub.load_delays.insert("test-model".to_string(), delay);
    }
    let addr = start_stub(stub).await;
    let url = format!("grpc://{addr}");

    crate::db::queries::insert_tamad(&pool, "tamad-1", "stub", &url, "grpc", Some("secret"))
        .await
        .unwrap();
    crate::db::queries::insert_provider(
        pool.as_ref(),
        "myprov",
        "local",
        "llama_cpp",
        Some("tamad-1"),
        None,
        None,
    )
    .await
    .unwrap();

    let state = Arc::new(ProxyState::new(config, None, pool));
    state
        .tamad_pool()
        .upsert_connection(&grpc_conn("tamad-1", "stub", &url))
        .await
        .unwrap();

    let mut mc = gguf_model_config();
    mc.provider_name = Some("myprov".to_string());
    state
        .registry
        .model_configs
        .write()
        .await
        .insert("test-model".to_string(), mc);

    (guard, state)
}

/// While the tamad-side load is in flight, the runtime must hold a
/// `Starting` mirror for the model — the dashboard and the models API
/// report it as "loading" for the whole window (issue #192). When the RPC
/// returns, the mirror flips to `Ready` under the same key (replace, not
/// duplicate).
#[tokio::test]
async fn test_load_spec_on_tamad_exposes_starting_then_ready() {
    let (guard, state) = setup_stub_load(false, Some(Duration::from_millis(1_000))).await;

    let spec = Arc::new(test_load_spec());
    let load = tokio::spawn({
        let state = state.clone();
        let spec = Arc::clone(&spec);
        async move { load_spec_on_tamad(&state, &spec, false).await }
    });

    // Observe the Starting placeholder during the load window: no PID and
    // no endpoint yet (the tamad owns those until the RPC returns).
    let saw_starting = wait_for(|| async {
        state
            .registry
            .models
            .read()
            .await
            .get("test-model")
            .is_some_and(|s| {
                matches!(
                    s,
                    BackendState::Starting {
                        backend_pid: 0,
                        backend_url,
                        ..
                    } if backend_url.is_empty()
                )
            })
    })
    .await;
    assert!(
        saw_starting,
        "expected a Starting mirror during the load window"
    );

    // The load succeeds and the mirror flips to Ready on the same key.
    let key = load
        .await
        .expect("load task panicked")
        .expect("load should succeed");
    assert_eq!(key, "test-model");

    let models = state.registry.models.read().await;
    assert_eq!(
        models.len(),
        1,
        "Ready mirror must replace the placeholder, not add a second entry: {models:?}"
    );
    let mirror = models
        .get("test-model")
        .expect("mirror present after load")
        .clone();
    let BackendState::Ready { .. } = mirror else {
        panic!("expected Ready mirror after load, got: {mirror:?}");
    };
    // Stub tamad answers with endpoint 5801 / pid 1234 — proves the Ready
    // mirror replaced the Starting placeholder in place.
    assert_eq!(mirror.backend_pid(), Some(1234));
    assert_eq!(mirror.backend_url(), Some("http://127.0.0.1:5801"));

    let _ = guard.finish().await;
}

/// A failed `LoadModel` RPC must leave a `Failed` mirror carrying the error
/// instead of silently going back to `idle`; the error is still propagated
/// to the caller, and the model is NOT marked desired (the RPC is the
/// gating step).
#[tokio::test]
async fn test_load_spec_on_tamad_failed_rpc_leaves_failed_mirror() {
    let (guard, state) = setup_stub_load(true, None).await;

    let spec = test_load_spec();
    let err = load_spec_on_tamad(&state, &spec, false)
        .await
        .expect_err("load must fail when the tamad rejects it");

    let mirror = state
        .registry
        .models
        .read()
        .await
        .get("test-model")
        .cloned()
        .expect("a Failed mirror must remain after a failed load");
    let BackendState::Failed {
        model_name,
        backend,
        error,
    } = mirror
    else {
        panic!("expected a Failed mirror, got: {mirror:?}");
    };
    assert_eq!(model_name, "test-model");
    assert_eq!(backend, "llama_cpp");
    assert!(
        error.contains("LoadModel RPC to the tamad of provider \"myprov\" failed"),
        "Failed mirror must carry the RPC error, got: {error}"
    );
    assert!(
        err.to_string().contains("LoadModel RPC to the tamad"),
        "the original RPC error must still be propagated, got: {err}"
    );

    // The load never became desired (set_desired runs after the RPC).
    assert!(
        crate::db::queries::get_desired(state.db_pool().as_ref(), "test-model")
            .await
            .unwrap()
            .is_none(),
        "a failed load must not be marked desired"
    );

    let _ = guard.finish().await;
}
