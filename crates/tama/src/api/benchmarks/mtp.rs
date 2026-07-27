use super::*;
use crate::api::benchmarks::run::{resolve_model_path, unload_model_before_benchmark};
use crate::api::benchmarks::BenchmarkProgressSink;
use anyhow::Context;

// ── Request DTO ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct MtpBenchmarkRunRequest {
    pub model_id: String,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub backend_name: Option<String>,
    #[serde(default)]
    pub gpu_variant: Option<String>,
    #[serde(default = "default_draft_max_values")]
    pub draft_max_values: Vec<u32>,
    #[serde(default = "default_ngl")]
    pub ngl: Option<u32>,
    #[serde(default = "default_draft_ngl")]
    pub draft_ngl: Option<u32>,
    #[serde(default = "default_flash_attn")]
    pub flash_attn: bool,
    #[serde(default = "default_context_size")]
    pub context_size: Option<u32>,
    #[serde(default)]
    pub benchmark_type: Option<String>,
}

fn default_draft_max_values() -> Vec<u32> {
    vec![0, 1, 2, 3, 4, 5, 6, 7, 8]
}
fn default_ngl() -> Option<u32> {
    Some(99)
}
fn default_draft_ngl() -> Option<u32> {
    Some(99)
}
fn default_flash_attn() -> bool {
    true
}
fn default_context_size() -> Option<u32> {
    Some(32768)
}

// ── Handler: Submit MTP benchmark job ─────────────────────────────────

pub async fn run_mtp_benchmark(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<MtpBenchmarkRunRequest>,
) -> impl IntoResponse {
    // Validate draft_max_values is not empty
    if req.draft_max_values.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "draft_max_values must not be empty",
            Some("ValidationError"),
        );
    }

    let (job_id, _jobs) =
        match submit_benchmark_job(&state, &web_state, req, run_mtp_benchmark_inner).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };
    (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
}

pub async fn run_mtp_benchmark_inner(
    jobs: Arc<JobManager>,
    job: Arc<crate::web_types::Job>,
    req: MtpBenchmarkRunRequest,
    db_path: std::path::PathBuf,
    proxy_base_url: String,
    client: reqwest::Client,
    repo_handle: std::sync::Arc<std::sync::Mutex<tama_core::db::repository::Repository>>,
) -> Result<()> {
    use tama_core::bench::llama_cli_mtp;

    // Unload any active server for this model before running the benchmark.
    unload_model_before_benchmark(&client, &proxy_base_url, &req.model_id, &job.id).await;

    // Parse gpu_variant from wire string to GpuVariant enum.
    let gpu_variant: Option<tama_core::gpu::GpuVariant> = match req.gpu_variant {
        Some(ref s) => Some(<tama_core::gpu::GpuVariant as std::str::FromStr>::from_str(
            s,
        )?),
        None => None,
    };

    // Clone fields we need after consuming `req`
    let model_id = req.model_id.clone();
    let backend_name = req.backend_name.clone();
    let quant = req.quant.clone();
    let benchmark_type = req.benchmark_type.clone();
    let draft_max_for_trace = req.draft_max_values.clone();

    // Load config - clone db_path for the blocking task
    let db_path_for_load = db_path.clone();

    let config = tokio::task::spawn_blocking(move || {
        tama_core::config::Config::load_from(&db_path_for_load)
    })
    .await??;

    // Resolve model path — pool the blocking SQLite calls.
    let db_dir = db_path.parent().context("db_path has no parent")?;
    // Clone values before moving into the spawn_blocking closure.
    let model_id_for_pool = model_id.clone();
    let quant_for_pool = quant.clone();
    let config_for_pool = config.clone();
    let db_dir_for_pool = db_dir.to_path_buf();
    let repo_handle_for_pool = repo_handle.clone();
    let (model_path, target_backend, display_name, resolved_id_owned) =
        tokio::task::spawn_blocking(
            move || -> anyhow::Result<(std::path::PathBuf, String, Option<String>, String)> {
                let model_configs = {
                    let repo = repo_handle_for_pool.lock().unwrap();
                    repo.load_model_configs_for_benchmarks()
                }?;

                // If model_id is an integer db_id, resolve it to the config key first.
                let resolved_id = if let Ok(db_id) = model_id_for_pool.parse::<i64>() {
                    model_configs
                        .iter()
                        .find(|(_, mc)| mc.db_id == Some(db_id))
                        .map(|(key, _)| key.clone())
                        .unwrap_or(model_id_for_pool.clone())
                } else {
                    model_id_for_pool.clone()
                };

                let (model_config, _) = config_for_pool
                    .resolve_backend(&model_configs, &resolved_id)
                    .context("Failed to resolve server config for benchmark")?;

                let repo = repo_handle_for_pool.lock().unwrap();
                let model_path = resolve_model_path(
                    &config_for_pool,
                    &db_dir_for_pool,
                    &repo,
                    &model_configs,
                    &resolved_id,
                    quant_for_pool.as_deref(),
                )?;
                let display_name = model_configs.get(&resolved_id).and_then(|mc| {
                    mc.display_name
                        .clone()
                        .or_else(|| mc.api_name.clone())
                        .or_else(|| mc.model.clone())
                });
                let target_backend = backend_name
                    .as_deref()
                    .unwrap_or(&model_config.backend)
                    .to_string();
                Ok((model_path, target_backend, display_name, resolved_id))
            },
        )
        .await??;

    // Build MtpBenchConfig
    let mtp_config = llama_cli_mtp::MtpBenchConfig {
        model_path: model_path.clone(),
        draft_max_values: req.draft_max_values,
        ngl: req.ngl,
        draft_ngl: req.draft_ngl,
        flash_attn: req.flash_attn,
        context_size: req.context_size,
    };

    // Create progress sink
    let sink = Arc::new(BenchmarkProgressSink {
        name: "mtp-bench",
        job: job.clone(),
        jobs: jobs.clone(),
    });

    // Resolve backend path — pool the blocking SQLite calls.
    let db_dir_for_pm = db_dir.to_path_buf();
    let target_backend_for_pm = target_backend.clone();
    let gpu_variant_for_pm = gpu_variant.clone();
    let backend_path = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let manager = tama_core::backends::BackendManager::open(&db_dir_for_pm)?;
        config.resolve_backend_path(
            &target_backend_for_pm,
            gpu_variant_for_pm.as_ref(),
            &manager,
        )
    })
    .await??;

    // Discover llama-server binary
    // The resolved path may be a file (llama-server) rather than the backend directory.
    // Use its parent as the search base for llama-server.
    let backend_dir = backend_path.parent().unwrap_or(&backend_path);
    tracing::info!(job_id = %job.id, backend_dir = %backend_dir.display(), "Resolving llama-server for MTP benchmark");
    let server_binary = llama_cli_mtp::find_llama_server(backend_dir).context(format!(
        "llama-server not found for backend '{}'. Install llama.cpp from source or set LLAMA_SERVER_PATH",
        target_backend
    ))?;
    tracing::info!(
        job_id = %job.id,
        model = %resolved_id_owned,
        backend = %target_backend,
        draft_max = ?draft_max_for_trace,
        "Starting MTP benchmark",
    );
    tracing::info!(job_id = %job.id, llama_server = %server_binary.display(), "Using llama-server binary");

    // Run MTP benchmark
    let result =
        llama_cli_mtp::run_mtp_bench(&mtp_config, Some(server_binary), sink.clone()).await?;

    // Serialize the full result for storage
    let results_json =
        serde_json::to_string(&result).context("Failed to serialize MTP benchmark result")?;
    let pp_sizes_json = "[]";
    let tg_sizes_json = "[]";

    // Get VRAM info
    let vram = query_vram();

    // Clone values before moving into the spawn_blocking closure.
    let display_name_for_trace = display_name.clone();
    let model_id_for_trace = model_id.clone();
    let quant_for_trace = quant.clone();
    let target_backend_for_trace = target_backend.clone();

    // Insert into database — pool the blocking SQLite call.
    let repo_handle_for_insert = repo_handle.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let repo = repo_handle_for_insert.lock().unwrap();
        repo.insert_benchmark(&tama_core::db::repository::BenchmarkParams {
            model_id: model_id_for_trace,
            display_name: display_name_for_trace,
            quant: quant_for_trace,
            backend: target_backend_for_trace.to_string(),
            engine: "llama_cli_mtp".to_string(),
            pp_sizes_json: pp_sizes_json.to_string(),
            tg_sizes_json: tg_sizes_json.to_string(),
            threads_json: None,
            ngl_range: None,
            runs: 1,
            warmup: 0,
            results_json,
            load_time_ms: None,
            vram_used_mib: vram.as_ref().map(|v| v.used_mib as i64),
            vram_total_mib: vram.as_ref().map(|v| v.total_mib as i64),
            duration_seconds: 0.0,
            status: "success".to_string(),
            benchmark_type: benchmark_type.clone(),
        })?;
        Ok(())
    })
    .await??;

    tracing::info!(
        job_id = %job.id,
        entries = result.entries.len(),
        total_predicted = result.aggregate.total_predicted,
        total_draft = result.aggregate.total_draft,
        accept_rate = result.aggregate.aggregate_accept_rate,
        "MTP benchmark completed",
    );

    Ok(())
}
