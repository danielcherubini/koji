use super::*;
use crate::api::benchmarks::run::{resolve_model_path, unload_model_before_benchmark};
use crate::api::benchmarks::{derive_status, BenchmarkProgressSink};
use anyhow::Context;

// ── Handler: Submit spec benchmark job ────────────────────────────────

pub async fn run_spec_benchmark(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<SpecBenchmarkRunRequest>,
) -> impl IntoResponse {
    // Validate spec_types is not empty
    if req.spec_types.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "spec_types must not be empty",
            Some("ValidationError"),
        );
    }

    // Apply minimum guards
    let runs = req.runs.max(1);
    let gen_tokens = req.gen_tokens.max(1);

    // Build config to validate sweep dimensions
    let model_path = std::path::PathBuf::from("/tmp/validation_model.gguf");
    let validation_config = SpecBenchConfig {
        model_path,
        spec_types: req.spec_types.clone(),
        draft_max_values: req.draft_max_values.clone(),
        ngram_n_values: req.ngram_n_values.clone(),
        ngram_m_values: req.ngram_m_values.clone(),
        ngram_min_values: req.ngram_min_values.clone(),
        ngram_max_values: req.ngram_max_values.clone(),
        ngram_min_hits: req.ngram_min_hits,
        gen_tokens,
        runs,
        ngl: req.ngl,
        flash_attn: req.flash_attn,
    };

    // Validate sweep matrix would produce entries
    if let Err(e) = validate_spec_sweep(&validation_config) {
        return error_response(
            StatusCode::BAD_REQUEST,
            e.to_string(),
            Some("ValidationError"),
        );
    }

    let (job_id, _jobs) =
        match submit_benchmark_job(&state, &web_state, req, run_spec_benchmark_inner).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };
    (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
}

/// Validate that the spec sweep configuration would produce at least one entry.
pub fn validate_spec_sweep(config: &SpecBenchConfig) -> Result<()> {
    tama_core::bench::llama_cli_spec::validate_sweep_config(config)
}

#[allow(clippy::too_many_arguments)]
pub async fn run_spec_benchmark_inner(
    jobs: Arc<JobManager>,
    job: Arc<crate::web_types::Job>,
    req: SpecBenchmarkRunRequest,
    db_path: std::path::PathBuf,
    proxy_base_url: String,
    client: reqwest::Client,
    db_pool: std::sync::Arc<sqlx::PgPool>,
) -> Result<()> {
    use tama_core::bench::llama_cli_spec;

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
    let spec_types_for_trace = req.spec_types.clone();
    let draft_max_for_trace = req.draft_max_values.clone();
    let ngram_n_for_trace = req.ngram_n_values.clone();
    let ngram_m_for_trace = req.ngram_m_values.clone();

    // Load the global config from Postgres (plan-190 Task 3).
    let pool = db_pool.as_ref();
    let config = tama_core::config::Config::load_from_pool(pool).await?;

    // Resolve model path — model configs and files come from Postgres.
    let db_dir = db_path.parent().context("db_path has no parent")?;
    let model_configs = tama_core::db::load_model_configs(pool).await?;

    // If model_id is an integer db_id, resolve it to the config key first.
    let resolved_id = if let Ok(db_id) = model_id.parse::<i64>() {
        model_configs
            .iter()
            .find(|(_, mc)| mc.db_id == Some(db_id))
            .map(|(key, _)| key.clone())
            .unwrap_or(model_id.clone())
    } else {
        model_id.clone()
    };

    let (model_config, _) = config
        .resolve_backend(&model_configs, &resolved_id)
        .context("Failed to resolve server config for benchmark")?;
    let model_path = resolve_model_path(
        &config,
        db_dir,
        pool,
        &model_configs,
        &resolved_id,
        quant.as_deref(),
    )
    .await?;
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
    let resolved_id_owned = resolved_id;

    // Apply minimum guards
    let runs = req.runs.max(1);
    let gen_tokens = req.gen_tokens.max(1);

    // Build SpecBenchConfig
    let spec_config = SpecBenchConfig {
        model_path: model_path.clone(),
        spec_types: req.spec_types,
        draft_max_values: req.draft_max_values,
        ngram_n_values: req.ngram_n_values,
        ngram_m_values: req.ngram_m_values,
        ngram_min_values: req.ngram_min_values,
        ngram_max_values: req.ngram_max_values,
        ngram_min_hits: req.ngram_min_hits,
        gen_tokens,
        runs,
        ngl: req.ngl,
        flash_attn: req.flash_attn,
    };

    // Create progress sink
    let sink = Arc::new(BenchmarkProgressSink {
        name: "spec-bench",
        job: job.clone(),
        jobs: jobs.clone(),
    });

    // Resolve backend path via the Postgres-backed InstallationManager.
    let manager = tama_core::installations::InstallationManager::new(db_pool.clone());
    let backend_path = config
        .resolve_backend_path(&target_backend, gpu_variant.as_ref(), Some(&manager))
        .await?;

    // Discover llama-server binary
    // The resolved path may be a file (llama-server) rather than the backend directory.
    // Use its parent as the search base for llama-server.
    let backend_dir = backend_path.parent().unwrap_or(&backend_path);
    tracing::info!(job_id = %job.id, backend_dir = %backend_dir.display(), "Resolving llama-server for benchmark");
    let server_binary = llama_cli_spec::find_llama_server(backend_dir).context(format!(
        "llama-server not found for backend '{}'. Install llama.cpp from source or set LLAMA_SERVER_PATH",
        target_backend
    ))?;
    tracing::info!(
        job_id = %job.id,
        model = %resolved_id_owned,
        backend = %target_backend,
        spec_types = ?spec_types_for_trace,
        draft_max = ?draft_max_for_trace,
        ngram_n = ?ngram_n_for_trace,
        ngram_m = ?ngram_m_for_trace,
        gen_tokens = gen_tokens,
        runs = runs,
        "Starting speculative decoding benchmark",
    );
    tracing::info!(job_id = %job.id, llama_server = %server_binary.display(), "Using llama-server binary");

    // Run spec benchmark
    let result =
        llama_cli_spec::run_spec_bench(&spec_config, Some(server_binary), sink.clone()).await?;

    // Serialize the full result for storage
    let results_json =
        serde_json::to_string(&result).context("Failed to serialize spec benchmark result")?;
    let pp_sizes_json = "[]";
    let tg_sizes_json =
        serde_json::to_string(&[gen_tokens]).context("Failed to serialize gen_tokens")?;

    // Derive run status from per-entry results: count entries with any non-success status.
    let entries_failed = result
        .entries
        .iter()
        .filter(|e| e.status != "success")
        .count();
    let run_status = derive_status(result.entries.len() - entries_failed, entries_failed, false);

    // Get VRAM info
    let vram = query_vram();

    // Insert into Postgres (plan-190 Task 8).
    let params = tama_core::db::queries::BenchmarkInsertParams {
        model_id: &model_id,
        display_name: display_name.as_deref(),
        quant: quant.as_deref(),
        backend: &target_backend,
        engine: "llama_cli_spec",
        pp_sizes_json,
        tg_sizes_json: &tg_sizes_json,
        threads_json: None,
        ngl_range: None,
        runs,
        warmup: 0,
        results_json: &results_json,
        load_time_ms: None,
        vram_used_mib: vram.as_ref().map(|v| v.used_mib as i64),
        vram_total_mib: vram.as_ref().map(|v| v.total_mib as i64),
        duration_seconds: 0.0,
        status: run_status,
        benchmark_type: benchmark_type.as_deref(),
        suite_id: req.suite_id.as_deref(),
    };
    tama_core::db::queries::insert_benchmark(pool, &params).await?;

    tracing::info!(
        job_id = %job.id,
        entries = result.entries.len(),
        baseline_tg_ts = result.baseline_tg_ts,
        "Speculative decoding benchmark completed",
    );

    Ok(())
}
