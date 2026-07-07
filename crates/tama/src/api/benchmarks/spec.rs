use super::*;
use crate::api::benchmarks::run::{resolve_model_path, unload_model_before_benchmark};
use crate::api::benchmarks::{
    job_conflict_response, job_manager_unavailable_response, BenchmarkProgressSink,
};

// ── Handler: Submit spec benchmark job ────────────────────────────────

pub async fn run_spec_benchmark(
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<SpecBenchmarkRunRequest>,
) -> impl IntoResponse {
    let jobs = match state.web_jobs() {
        Some(j) => j.clone(),
        None => return job_manager_unavailable_response(),
    };

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

    let job = match jobs.submit(JobKind::Benchmark, None).await {
        Ok(j) => j,
        Err(_) => return job_conflict_response(),
    };

    let job_id = job.id.clone();

    let db_path = state
        .db_dir()
        .clone()
        .unwrap_or_else(|| {
            tama_core::config::Config::config_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
        })
        .join("tama.db");
    let proxy_base_url = state.config().read().await.proxy_url();
    let client = state.client().clone();

    // Spawn the benchmark in the background
    tokio::spawn(async move {
        if let Err(e) = run_spec_benchmark_inner(
            jobs.clone(),
            &job,
            req,
            Some(db_path),
            proxy_base_url,
            client,
        )
        .await
        {
            tracing::error!(job_id = %job.id, error = %e, "Spec benchmark failed");
            jobs.finish(&job, JobStatus::Failed, Some(e.to_string()))
                .await;
        } else {
            jobs.finish(&job, JobStatus::Succeeded, None).await;
        }
    });

    (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
}

/// Validate that the spec sweep configuration would produce at least one entry.
pub fn validate_spec_sweep(config: &SpecBenchConfig) -> Result<()> {
    tama_core::bench::llama_cli_spec::validate_sweep_config(config)
}

pub async fn run_spec_benchmark_inner(
    jobs: Arc<JobManager>,
    job: &Arc<tama_core::web_types::Job>,
    req: SpecBenchmarkRunRequest,
    db_path: Option<std::path::PathBuf>,
    proxy_base_url: String,
    client: reqwest::Client,
) -> Result<()> {
    use tama_core::bench::llama_cli_spec;

    // Unload any active server for this model before running the benchmark.
    unload_model_before_benchmark(&client, &proxy_base_url, &req.model_id, &job.id).await;

    // Clone fields we need after consuming `req`
    let model_id = req.model_id.clone();
    let backend_name = req.backend_name.clone();
    let quant = req.quant.clone();
    let benchmark_type = req.benchmark_type.clone();
    let gpu_variant = req.gpu_variant.clone();
    let spec_types_for_trace = req.spec_types.clone();
    let draft_max_for_trace = req.draft_max_values.clone();
    let ngram_n_for_trace = req.ngram_n_values.clone();
    let ngram_m_for_trace = req.ngram_m_values.clone();

    // Load config - clone db_path for the blocking task
    let db_path: std::path::PathBuf = db_path.context("Cannot determine db path")?;
    let db_path_for_load = db_path.clone();

    let config = tokio::task::spawn_blocking(move || {
        tama_core::config::Config::load_from(&db_path_for_load)
    })
    .await??;

    // Resolve model path (same pattern as llama_bench)
    let db_dir = db_path.parent().context("db_path has no parent")?;
    let repo = tama_core::db::repository::Repository::open(db_dir)?;
    let model_configs = tama_core::db::load_model_configs(repo.conn())?;

    // If model_id is an integer db_id, resolve it to the config key first.
    let resolved_id = if let Ok(db_id) = model_id.parse::<i64>() {
        model_configs
            .iter()
            .find(|(_, mc)| mc.db_id == Some(db_id))
            .map(|(key, _)| key.as_str())
            .unwrap_or(&model_id)
    } else {
        &model_id
    };

    let (server_config, _) = config
        .resolve_backend(&model_configs, resolved_id)
        .context("Failed to resolve server config for benchmark")?;

    let model_path = resolve_model_path(
        &config,
        db_dir,
        &repo,
        &model_configs,
        resolved_id,
        quant.as_deref(),
    )?;

    // Get model display name from config
    let display_name = model_configs.get(resolved_id).and_then(|mc| {
        mc.display_name
            .clone()
            .or_else(|| mc.api_name.clone())
            .or_else(|| mc.model.clone())
    });

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

    // Resolve backend path for llama-server discovery
    let target_backend = backend_name.as_deref().unwrap_or(&server_config.backend);
    let manager = tama_core::backends::BackendManager::open(db_dir)?;
    let backend_path =
        config.resolve_backend_path(target_backend, gpu_variant.as_deref(), &manager)?;

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
        model = %resolved_id,
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

    // Store results in database
    let db_dir = db_path.parent().context("db_path has no parent")?;
    let repo = tama_core::db::repository::Repository::open(db_dir)?;

    // Serialize the full result for storage
    let results_json =
        serde_json::to_string(&result).context("Failed to serialize spec benchmark result")?;
    let pp_sizes_json = "[]";
    let tg_sizes_json =
        serde_json::to_string(&[gen_tokens]).context("Failed to serialize gen_tokens")?;

    // Get VRAM info
    let vram = query_vram();

    // Insert into database
    let _id = repo.insert_benchmark(&tama_core::db::repository::BenchmarkParams {
        model_id: model_id.clone(),
        display_name: display_name.clone(),
        quant: quant.clone(),
        backend: target_backend.to_string(),
        engine: "llama_cli_spec".to_string(),
        pp_sizes_json: pp_sizes_json.to_string(),
        tg_sizes_json: tg_sizes_json.clone(),
        threads_json: None,
        ngl_range: None,
        runs,
        warmup: 0,
        results_json,
        load_time_ms: None,
        vram_used_mib: vram.as_ref().map(|v| v.used_mib as i64),
        vram_total_mib: vram.as_ref().map(|v| v.total_mib as i64),
        duration_seconds: 0.0,
        status: "success".to_string(),
        benchmark_type: benchmark_type.clone(),
    })?;

    tracing::info!(
        job_id = %job.id,
        entries = result.entries.len(),
        baseline_tg_ts = result.baseline_tg_ts,
        "Speculative decoding benchmark completed",
    );

    Ok(())
}
