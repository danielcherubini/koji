//! Benchmark dispatch to the tamad host (plan-191 Task 8).
//!
//! Benchmarks measure the *tamad's* hardware, so the runners execute on
//! the inference host (ADR-0010: the proxy spawns nothing). The proxy
//! resolves the model's provider → its tamad, computes the host-relative
//! paths (the tamad-relative roots are unknown to the proxy — only the
//! layouts are), dispatches `RunBenchmark`, and relays the tamad's
//! `StreamJob` events into the web `JobManager` log. On a succeeded
//! terminal the caller persists the benchmark history row exactly as
//! before (the proxy remains the sole DB writer).
//!
//! Fail-loud policy (ADR-0010): no local fallback. A dispatch or relay
//! failure fails the job with an actionable error.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tama_core::config::{Config, ModelConfig};
use tama_core::gpu::GpuVariant;
use tama_core::tamad::pool::TamadHandle;
use tama_core::tamad::RunBenchmarkRequest;

use crate::web_types::{Job, JobManager};

use super::BenchmarkJobContext;

/// How long the relay waits for the next tamad benchmark job event before
/// declaring the job stalled. One llama-bench subprocess can be silent
/// for a long stretch on large sweeps (pp2048 × many runs), so the
/// benchmark stall window is far more generous than the install relay's.
pub const BENCH_EVENT_STALL: Duration = Duration::from_secs(1800);

/// The resolved execution host for one benchmark dispatch.
pub(crate) struct BenchmarkHost {
    /// The provider's tamad (for RPC + streaming).
    pub handle: Arc<TamadHandle>,
    /// Model file path relative to the tamad's models dir
    /// (`<repo_id>/<filename>` — the pull layout).
    pub model_path_rel: String,
    /// Backend (llama-server) binary path relative to the tamad's install
    /// dir (`<backend_type>/<gpu_variant>/<version>/<binary>`).
    pub binary_path_rel: String,
}

/// Resolve the execution host for a benchmark: the model's provider's
/// tamad, plus the host-relative model and binary paths.
///
/// - provider resolution mirrors `proxy::lifecycle::spec::resolve_provider_for_model`
///   (model `provider_name` first, single Local-provider-with-tamad
///   fallback), using the freshly loaded model configs.
/// - `model_path_rel` — `<repo_id>/<gguf filename>` (same layout pulls
///   write under `<models-dir>/`; quant override > model quant > first
///   GGUF, as in the local path resolution). The file must exist on the
///   *tamad*; the proxy does not require it locally.
/// - `binary_path_rel` — the active installation's versioned layout under
///   the tamad's install dir.
pub(crate) async fn resolve_benchmark_host(
    ctx: &BenchmarkJobContext,
    config: &Config,
    model_configs: &HashMap<String, ModelConfig>,
    resolved_key: &str,
    backend: &str,
    gpu_variant: Option<&GpuVariant>,
) -> Result<BenchmarkHost> {
    // ── Provider → tamad (single-writer boundary, ADR-0010) ──
    let provider_name = model_configs
        .get(resolved_key)
        .and_then(|c| c.provider_name.clone());

    let provider = match provider_name {
        Some(name) => {
            let provider = tama_core::db::queries::get_provider(&ctx.db_pool, &name)
                .await?
                .ok_or_else(|| anyhow!("Provider \"{name}\" not found"))?;
            if provider.provider_type.is_remote() {
                bail!(
                    "model \"{resolved_key}\" uses remote provider \"{name}\" — \
                     benchmarks run on a local provider's tamad"
                );
            }
            provider
        }
        None => {
            let providers = tama_core::db::queries::list_providers(&ctx.db_pool).await?;
            let local: Vec<_> = providers
                .into_iter()
                .filter(|p| p.provider_type.is_local() && p.tamad_id.is_some())
                .collect();
            match local.len() {
                1 => local.into_iter().next().expect("checked len 1"),
                0 => bail!(
                    "No local provider with a tamad assigned — create one \
                     (POST /tama/v1/providers) or set provider_name on \
                     model \"{resolved_key}\""
                ),
                _ => bail!(
                    "Multiple local providers have tamads assigned — \
                     set provider_name on model \"{resolved_key}\" to disambiguate"
                ),
            }
        }
    };

    let tamad_id = provider
        .tamad_id
        .clone()
        .ok_or_else(|| anyhow!("Provider \"{}\" has no tamad assigned", provider.name))?;
    let handle = ctx
        .tamad_pool
        .handle_for_provider(Some(&tamad_id))
        .await
        .ok_or_else(|| {
            anyhow!(
                "No live stats stream for the tamad of provider \"{}\" \
                 (is it registered and online?)",
                provider.name
            )
        })?;

    // ── Model file, relative to the tamad's models dir ──
    let mc = model_configs
        .get(resolved_key)
        .context("model config not found")?;
    let rec_id = mc.db_id.context("model config has no db_id")?;
    let record = tama_core::db::queries::get_model_config(&ctx.db_pool, rec_id)
        .await?
        .with_context(|| format!("model config record (id={rec_id}) not found"))?;
    let files = tama_core::db::queries::get_model_files(&ctx.db_pool, record.id).await?;

    // Resolve the target GGUF: quant override > model quant > first GGUF.
    let first_gguf = files
        .iter()
        .find(|f| f.filename.ends_with(".gguf"))
        .map(|f| f.filename.clone());
    let quant_label = mc.quant.clone();
    let target_filename = quant_label
        .as_deref()
        .and_then(|q| mc.quants.get(q).map(|qe| qe.file.clone()))
        .or(first_gguf)
        .context("No GGUF file found for this model — benchmarks require a GGUF quant")?;
    let filename = files
        .into_iter()
        .find(|f| f.filename == target_filename)
        .context("resolved model file not found in database")?
        .filename;

    // The tamad-relative path uses the same repo layout pulls write
    // (`<models-dir>/<repo_id>/<file>`); strip the proxy's own root — the
    // relative portion is root-independent.
    let models_dir = config.models_dir()?;
    let candidate = tama_core::models::repo_path(&models_dir, &record.repo_id).join(&filename);
    let model_path_rel = candidate
        .strip_prefix(&models_dir)
        .with_context(|| {
            format!(
                "model path {} is outside the models dir",
                candidate.display()
            )
        })?
        .to_string_lossy()
        .to_string();

    // ── Backend binary, relative to the tamad's install dir ──
    let manager = tama_core::installations::InstallationManager::new(ctx.db_pool.clone());
    let variant_folder = gpu_variant
        .map(|v| v.variant_folder().to_string())
        .or_else(|| {
            config
                .backends
                .get(backend)
                .and_then(|b| b.gpu_variant.as_ref())
                .map(|v| v.variant_folder().to_string())
        })
        .unwrap_or_else(|| "cpu".to_string());

    let info = match manager.get_active(backend, &variant_folder).await? {
        Some(i) => i,
        None => manager
            .list_active()
            .await?
            .into_iter()
            .find(|v| v.name == backend)
            .ok_or_else(|| {
                anyhow!(
                    "no active installation for backend '{backend}' \
                     (variant '{variant_folder}') — install it on the tamad first"
                )
            })?,
    };
    let binary_file = std::path::Path::new(&info.path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "llama-server".to_string());
    let binary_path_rel = format!(
        "{}/{}/{}/{}",
        info.backend_type, info.gpu_variant, info.version, binary_file
    );

    Ok(BenchmarkHost {
        handle,
        model_path_rel,
        binary_path_rel,
    })
}

/// Dispatch a benchmark to the execution host and relay the tamad's
/// `StreamJob` events into the web job log until a terminal state.
///
/// Returns the terminal result JSON (the serialized report the caller
/// persists exactly as before). Fail-loud: dispatch/relay failure
/// returns `Err` with an actionable message and nothing is persisted.
pub(crate) async fn dispatch_and_relay(
    jobs: &Arc<JobManager>,
    job: &Arc<Job>,
    host: &BenchmarkHost,
    req: &RunBenchmarkRequest,
) -> Result<String> {
    let tamad_job_id = host.handle.run_benchmark(req).await.map_err(|e| {
        anyhow!(
            "benchmark dispatch to tamad '{}' failed: {e}",
            host.handle.connection.name
        )
    })?;
    tracing::info!(
        job_id = %job.id,
        tamad = %host.handle.connection.name,
        bench_job = %tamad_job_id,
        "benchmark dispatching to tamad"
    );

    // Reuse the installation relay (plan-191 Task 7): StreamJob →
    // JobManager progress/log + terminal handling incl. stall/EOF,
    // fail-loud.
    crate::api::installations::tamad_job::relay_tamad_job(
        jobs,
        job,
        &host.handle,
        &tamad_job_id,
        BENCH_EVENT_STALL,
    )
    .await
    .map_err(anyhow::Error::msg)
}

// ─── Tests (plan-191 Task 7) ─────────────────────────────────────────────────
//
// Shared testkit (seed + stub construction) is reused by the suite.rs
// dispatch tests via `crate::api::benchmarks::tamad::tests`.

#[cfg(test)]
pub mod tests {
    //! Dispatch-side tests: host resolution, the dispatch → relay →
    //! persist flow, and fail-loud on dispatch failure. Uses StubTamad
    //! (in-process gRPC) + a real Postgres schema (plan-190 fixture).

    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::web_types::{JobKind, JobStatus};
    use tama_core::gpu::VramInfo;
    use tama_core::tamad::pool::test_support::{
        grpc_conn, job_event, start_stub, terminal_success, StubTamad,
    };
    use tama_core::tamad::JobEvent;

    use super::super::bench_ctx;
    use super::resolve_benchmark_host;

    pub const TAMAD_ID: &str = "uuid-bench-tamad";

    /// Seed a model (with one GGUF file) + an active `llama_cpp`
    /// installation rooted at a host path — optionally with a local
    /// provider wired to the test tamad. Returns the model db_id.
    pub async fn seed_bench_host(
        guard: &crate::testing::postgres::SchemaGuard,
        with_provider: bool,
    ) -> i64 {
        let pool = &guard.pool;
        if with_provider {
            tama_core::db::queries::insert_provider(
                pool,
                "loc-bench",
                "local",
                "llama_cpp",
                Some(TAMAD_ID),
                None,
                None,
            )
            .await
            .expect("provider insert");
        }
        tama_core::db::queries::insert_installation(
            pool,
            &tama_core::db::queries::InstallationRecord {
                id: 0,
                name: "llama_cpp".to_string(),
                backend_type: "llama_cpp".to_string(),
                version: "b9901".to_string(),
                path: "/host/install/llama_cpp/cpu/b9901/llama-server".to_string(),
                installed_at: 1_000,
                gpu_variant: "cpu".to_string(),
                source: None,
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )
        .await
        .expect("installation insert");
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO model_configs \
                 (repo_id, display_name, backend, enabled, selected_quant, api_name) \
                 VALUES ('test/bench', 'Bench Model', 'llama_cpp', true, 'Q4_K_M', 'test--bench') \
                 RETURNING id",
        )
        .fetch_one(pool)
        .await
        .expect("model insert");
        tama_core::db::queries::upsert_model_file(
            pool,
            id,
            "test/bench",
            "bench-model-Q4_K_M.gguf",
            Some("Q4_K_M"),
            None,
            Some(1_000_000),
        )
        .await
        .expect("model file insert");
        id
    }

    /// A StubTamad with scripted event lists keyed by tamad job id
    /// (dispatches get `job-bench-1`, `job-bench-2`, … in order).
    pub fn stub_with_events(events_by_id: HashMap<String, Vec<JobEvent>>) -> StubTamad {
        let (down_tx, _rx) = tokio::sync::watch::channel(false);
        StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::new(down_tx),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            successes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-pull".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_job_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(events_by_id)),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        }
    }

    /// A serialized llama_bench report — the same shape the host returns
    /// (what the proxy itself produced pre-Task-8).
    pub fn bench_report_json() -> String {
        let report = tama_core::bench::BenchReport {
            model_info: tama_core::bench::ModelInfo {
                name: "Bench Model".to_string(),
                model_id: Some("test/bench".to_string()),
                quant: Some("Q4_K_M".to_string()),
                backend: "llama_cpp".to_string(),
                gpu_variant: "CPU".to_string(),
                context_length: Some(8192),
                gpu_layers: None,
            },
            config: tama_core::bench::BenchConfig::default(),
            summaries: vec![],
            load_time_ms: 1_234.0,
            vram: Some(VramInfo {
                used_mib: 9_000,
                total_mib: 24_000,
            }),
        };
        serde_json::to_string(&report).expect("report serializes")
    }

    /// A serialized spec benchmark result.
    pub fn spec_result_json() -> String {
        let result = tama_core::bench::llama_cli_spec::SpecBenchResult {
            baseline_tg_ts: 50.0,
            baseline_tg_stddev: 1.0,
            entries: vec![],
            vram: None,
        };
        serde_json::to_string(&result).expect("spec result serializes")
    }

    /// ProxyState + seeded DB + stub tamad in the pool.
    async fn fixture(
        events_by_id: HashMap<String, Vec<JobEvent>>,
        with_provider: bool,
        dispatch_fail: bool,
    ) -> (
        StubTamad,
        Arc<tama_core::proxy::ProxyState>,
        crate::testing::postgres::SchemaGuard,
        i64,
    ) {
        let guard = crate::testing::postgres::with_schema().await;
        let model_id = seed_bench_host(&guard, with_provider).await;
        let stub = stub_with_events(events_by_id);
        if dispatch_fail {
            *stub.bench_dispatch_fail.lock().await = true;
        }
        let addr = start_stub(stub.clone()).await;
        let conn = grpc_conn(TAMAD_ID, "bench-tamad", &format!("grpc://{addr}"));
        let pool = Arc::new(guard.pool.clone());
        let state = Arc::new(tama_core::proxy::ProxyState::new(
            tama_core::config::Config::default(),
            None,
            pool,
        ));
        state
            .tamad_pool()
            .upsert_connection(&conn)
            .await
            .expect("pool upsert");
        (stub, state, guard, model_id)
    }

    /// Host resolution: provider → tamad + DB-derived relative paths
    /// (no proxy-local file lookups).
    #[tokio::test]
    async fn test_resolve_benchmark_host_rel_paths() {
        let events: HashMap<String, Vec<JobEvent>> = HashMap::new();
        let (_stub, state, guard, _model_id) = fixture(events, true, false).await;

        let pool = state.db_pool();
        let config = tama_core::config::Config::load_from_pool(pool.as_ref())
            .await
            .expect("config");
        let model_configs = tama_core::db::load_model_configs(pool.as_ref())
            .await
            .expect("model configs");
        let ctx = bench_ctx(&pool, state.tamad_pool());

        let host = resolve_benchmark_host(
            &ctx,
            &config,
            &model_configs,
            "test--bench",
            "llama_cpp",
            None,
        )
        .await
        .expect("host resolution");

        assert_eq!(host.model_path_rel, "test/bench/bench-model-Q4_K_M.gguf");
        assert_eq!(host.binary_path_rel, "llama_cpp/cpu/b9901/llama-server");
        assert_eq!(host.handle.connection.name, "bench-tamad");
        let _ = guard;
    }

    /// No local provider with a tamad → fail loud with actionable error.
    #[tokio::test]
    async fn test_resolve_without_provider_fails() {
        let events: HashMap<String, Vec<JobEvent>> = HashMap::new();
        let (_stub, state, guard, _model_id) = fixture(events, false, false).await;

        let pool = state.db_pool();
        let config = tama_core::config::Config::load_from_pool(pool.as_ref())
            .await
            .expect("config");
        let model_configs = tama_core::db::load_model_configs(pool.as_ref())
            .await
            .expect("model configs");
        let ctx = bench_ctx(&pool, state.tamad_pool());

        let err = match resolve_benchmark_host(
            &ctx,
            &config,
            &model_configs,
            "test--bench",
            "llama_cpp",
            None,
        )
        .await
        {
            Ok(h) => panic!("should fail, got {:?}", h.model_path_rel),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("No local provider"),
            "actionable expected, got: {err}"
        );
        let _ = guard;
    }

    /// Dispatch → relay → persist: a full llama_bench run through the
    /// stub tamad, with the history row persisted exactly as before.
    #[tokio::test]
    async fn test_run_benchmark_inner_dispatches_relays_persists() {
        let report_json = bench_report_json();
        let events: HashMap<String, Vec<JobEvent>> = [(
            "job-bench-1".to_string(),
            vec![
                job_event("job-bench-1", 0, "loading model", "running"),
                job_event("job-bench-1", 40, "pp512 x1", "running"),
                terminal_success("job-bench-1", &report_json),
            ],
        )]
        .into_iter()
        .collect();
        let (stub, state, guard, model_id) = fixture(events, true, false).await;

        let jobs = Arc::new(crate::web_types::JobManager::new());
        let job = jobs
            .submit(JobKind::Benchmark, None)
            .await
            .expect("job submit");
        let temp = tempfile::tempdir().expect("tempdir");
        let req = crate::api::benchmarks::BenchmarkRunRequest {
            model_id: model_id.to_string(),
            quant: None,
            backend_name: None,
            gpu_variant: None,
            pp_sizes: vec![512],
            tg_sizes: vec![64],
            runs: 1,
            warmup: 0,
            threads: None,
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
            benchmark_type: Some("baseline".to_string()),
            suite_id: None,
        };

        crate::api::benchmarks::run::run_benchmark_inner(
            jobs.clone(),
            job.clone(),
            req,
            temp.path().join("tama.db"),
            "http://127.0.0.1:9".to_string(),
            reqwest::Client::new(),
            state.db_pool(),
            state.tamad_pool(),
        )
        .await
        .expect("benchmark inner");
        jobs.finish(&job, JobStatus::Succeeded, None).await;
        assert_eq!(job.state.read().await.status, JobStatus::Succeeded);

        // The dispatch carried host-relative paths + the proxy-resolved name.
        let reqs = stub.bench_requests.lock().await;
        assert_eq!(reqs.len(), 1, "exactly one dispatch");
        assert_eq!(reqs[0].kind, "llama_bench");
        assert_eq!(reqs[0].model_name, "Bench Model");
        assert_eq!(reqs[0].model_path_rel, "test/bench/bench-model-Q4_K_M.gguf");
        assert_eq!(reqs[0].binary_path_rel, "llama_cpp/cpu/b9901/llama-server");
        drop(reqs);

        // The history row was persisted exactly like a pre-Task-8 local run.
        let pool = state.db_pool();
        let rows = tama_core::db::queries::list_benchmarks(pool.as_ref())
            .await
            .expect("list benchmarks");
        assert_eq!(rows.len(), 1, "one history row");
        let row = &rows[0];
        assert_eq!(row.model_id, model_id.to_string());
        assert_eq!(row.display_name.as_deref(), Some("Bench Model"));
        assert_eq!(row.quant.as_deref(), Some("Q4_K_M"));
        assert_eq!(row.backend, "llama_cpp");
        assert_eq!(row.engine, "llama_bench");
        assert_eq!(row.status, "success");
        assert_eq!(row.load_time_ms, Some(1_234.0));
        assert_eq!(row.vram_used_mib, Some(9_000));
        assert_eq!(row.vram_total_mib, Some(24_000));
        let _ = guard;
    }

    /// Fail loud: a dispatch failure fails the run with an actionable
    /// error and persists NO history row.
    #[tokio::test]
    async fn test_dispatch_failure_fails_loud_without_persist() {
        let events: HashMap<String, Vec<JobEvent>> = HashMap::new();
        let (_stub, state, guard, model_id) = fixture(events, true, true).await;

        let jobs = Arc::new(crate::web_types::JobManager::new());
        let job = jobs
            .submit(JobKind::Benchmark, None)
            .await
            .expect("job submit");
        let temp = tempfile::tempdir().expect("tempdir");
        let req = crate::api::benchmarks::BenchmarkRunRequest {
            model_id: model_id.to_string(),
            quant: None,
            backend_name: None,
            gpu_variant: None,
            pp_sizes: vec![512],
            tg_sizes: vec![64],
            runs: 1,
            warmup: 0,
            threads: None,
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
            benchmark_type: None,
            suite_id: None,
        };

        let err = crate::api::benchmarks::run::run_benchmark_inner(
            jobs,
            job,
            req,
            temp.path().join("tama.db"),
            "http://127.0.0.1:9".to_string(),
            reqwest::Client::new(),
            state.db_pool(),
            state.tamad_pool(),
        )
        .await
        .expect_err("dispatch failure must fail the run");
        assert!(
            err.to_string().contains("dispatch"),
            "actionable expected, got: {err}"
        );

        let pool = state.db_pool();
        let rows = tama_core::db::queries::list_benchmarks(pool.as_ref())
            .await
            .expect("list benchmarks");
        assert!(
            rows.iter().all(|r| r.model_id != model_id.to_string()),
            "no history row on dispatch failure"
        );
        let _ = guard;
    }
}
