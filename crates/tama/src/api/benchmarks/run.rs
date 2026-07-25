use super::*;
use crate::api::benchmarks::BenchmarkProgressSink;
use anyhow::Context;

// ── Handler: Submit benchmark job ─────────────────────────────────────

pub async fn run_benchmark(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<BenchmarkRunRequest>,
) -> impl IntoResponse {
    let (job_id, _jobs) =
        match submit_benchmark_job(&state, &web_state, req, run_benchmark_inner).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };
    (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
}

pub async fn run_benchmark_inner(
    jobs: Arc<JobManager>,
    job: Arc<crate::web_types::Job>,
    req: BenchmarkRunRequest,
    db_path: std::path::PathBuf,
    proxy_base_url: String,
    client: reqwest::Client,
    repo_handle: std::sync::Arc<std::sync::Mutex<tama_core::db::repository::Repository>>,
) -> Result<()> {
    use tama_core::bench::llama_bench::{self, LlamaBenchConfig};

    // Unload any active server for this model before running the benchmark.
    // This prevents GPU memory conflicts when the model is already loaded.
    unload_model_before_benchmark(&client, &proxy_base_url, &req.model_id, &job.id).await;

    // Clone fields we need after consuming `req`
    let model_id = req.model_id.clone();
    let backend_name = req.backend_name.clone();
    let quant = req.quant.clone();
    let benchmark_type = req.benchmark_type.clone();
    let ngl_range = req.ngl_range.clone();
    let ngl_range_for_insert = ngl_range.clone();
    let pp_sizes_for_trace = req.pp_sizes.clone();
    let pp_sizes_for_serial = pp_sizes_for_trace.clone();
    let tg_sizes_for_trace = req.tg_sizes.clone();
    let tg_sizes_for_serial = tg_sizes_for_trace.clone();
    let threads_for_trace = req.threads.clone();

    // Load config - clone db_path for the blocking task
    let db_path_for_load = db_path.clone();

    let config = tokio::task::spawn_blocking(move || {
        tama_core::config::Config::load_from(&db_path_for_load)
    })
    .await??;

    // Create progress sink
    let sink = BenchmarkProgressSink {
        name: "llama-bench",
        job: job.clone(),
        jobs: jobs.clone(),
    };

    // Build llama-bench config
    let bench_config = LlamaBenchConfig {
        pp_sizes: req.pp_sizes,
        tg_sizes: req.tg_sizes,
        runs: req.runs,
        warmup: req.warmup,
        threads: req.threads,
        ngl_range,
        ctx_override: req.ctx_override,
        batch_sizes: req.batch_sizes,
        ubatch_sizes: req.ubatch_sizes,
        kv_cache_type: req.kv_cache_type,
        depth: req.depth,
        flash_attn: req.flash_attn,
    };

    tracing::info!(
        job_id = %job.id,
        model_id = %model_id,
        backend = ?backend_name,
        pp_sizes = ?pp_sizes_for_trace,
        tg_sizes = ?tg_sizes_for_trace,
        runs = req.runs,
        "Starting llama-bench benchmark",
    );

    // Run benchmark
    let report = llama_bench::run_llama_bench(
        &config,
        &model_id,
        quant.as_deref(),
        backend_name.as_deref(),
        &bench_config,
        &sink,
    )
    .await?;

    // Store results in database — pool the blocking SQLite calls.
    // Segment 1: load model configs for display-name lookup.
    let repo_handle_for_load = repo_handle.clone();
    let model_configs: std::collections::HashMap<String, tama_core::config::ModelConfig> =
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let repo = repo_handle_for_load.lock().unwrap();
            repo.load_model_configs_for_benchmarks()
        })
        .await??;

    // Get model display name from config. The request carries the db_id as a
    // string (e.g. "4") because that's what the model dropdown submits, so we
    // resolve it to the config key first — otherwise `.get("4")` never hits.
    let resolved_key = if let Ok(db_id) = model_id.parse::<i64>() {
        model_configs
            .iter()
            .find(|(_, mc)| mc.db_id == Some(db_id))
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| model_id.clone())
    } else {
        model_id.clone()
    };
    let display_name = model_configs.get(&resolved_key).and_then(|mc| {
        mc.display_name
            .clone()
            .or_else(|| mc.api_name.clone())
            .or_else(|| mc.model.clone())
    });

    // Serialize the full report for storage so history can reconstruct model
    // metadata (backend, GPU, VRAM, load time, batch/ubatch/KV cache choices),
    // not just the per-test summary rows.
    let results_json =
        serde_json::to_string(&report).context("Failed to serialize benchmark report")?;
    let pp_sizes_json =
        serde_json::to_string(&pp_sizes_for_serial).context("Failed to serialize pp_sizes")?;
    let tg_sizes_json =
        serde_json::to_string(&tg_sizes_for_serial).context("Failed to serialize tg_sizes")?;
    let threads_json = threads_for_trace
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("Failed to serialize threads")?;

    // Get VRAM info
    let vram = query_vram();

    // Clone values before moving into the spawn_blocking closure.
    let display_name_for_trace = display_name.clone();
    let backend_for_trace = report.model_info.backend.clone();

    // Segment 2: insert benchmark record on the blocking pool.
    let repo_for_insert = repo_handle.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let repo = repo_for_insert.lock().unwrap();
        repo.insert_benchmark(&tama_core::db::repository::BenchmarkParams {
            model_id: req.model_id.clone(),
            display_name: display_name.clone(),
            quant: report.model_info.quant.clone(),
            backend: report.model_info.backend.clone(),
            engine: "llama_bench".to_string(),
            pp_sizes_json,
            tg_sizes_json,
            threads_json,
            ngl_range: ngl_range_for_insert,
            runs: req.runs,
            warmup: req.warmup,
            results_json,
            load_time_ms: Some(report.load_time_ms),
            vram_used_mib: vram.as_ref().map(|v| v.used_mib as i64),
            vram_total_mib: vram.as_ref().map(|v| v.total_mib as i64),
            duration_seconds: 0.0, // duration tracked by job system
            status: "success".to_string(),
            benchmark_type: benchmark_type.clone(),
        })?;
        Ok(())
    })
    .await??;

    tracing::info!(
        job_id = %job.id,
        display_name = ?display_name_for_trace,
        backend = %backend_for_trace,
        entries = report.summaries.len(),
        "llama-bench benchmark completed",
    );

    Ok(())
}

// ── Shared helpers ────────────────────────────────────────────────────

/// Best-effort unload of any active proxy server for the given model.
/// Used before benchmarks to prevent GPU memory conflicts. Errors are logged
/// at debug level and never block the benchmark — the model may not be loaded,
/// or the proxy may be unreachable.
pub(super) async fn unload_model_before_benchmark(
    client: &reqwest::Client,
    proxy_base_url: &str,
    model_id: &str,
    job_id: &str,
) {
    let unload_url = format!("{}/tama/v1/models/{}/unload", proxy_base_url, model_id);
    match client.post(&unload_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!(job_id = %job_id, "Unloaded active model before benchmark");
        }
        Ok(resp) => {
            tracing::debug!(
                job_id = %job_id,
                status = %resp.status(),
                "Model unload returned non-success (model may not be loaded)"
            );
        }
        Err(e) => {
            tracing::debug!(
                job_id = %job_id,
                error = %e,
                "Failed to call model unload (may not be reachable)"
            );
        }
    }
}

/// Resolve a model's file path from config and database.
/// `quant_override` takes priority over `mc.quant` when resolving the target file.
pub(super) fn resolve_model_path(
    config: &tama_core::config::Config,
    db_dir: &std::path::Path,
    repo: &tama_core::db::repository::Repository,
    model_configs: &std::collections::HashMap<String, tama_core::config::ModelConfig>,
    resolved_id: &str,
    quant_override: Option<&str>,
) -> Result<std::path::PathBuf> {
    let mc = model_configs
        .get(resolved_id)
        .with_context(|| format!("Model config '{}' not found", resolved_id))?;
    let rec_id = mc.db_id.context("Model config has no db_id")?;
    let record = repo
        .get_model_config(rec_id)?
        .with_context(|| format!("Model config record (id={}) not found in database", rec_id))?;
    let files = repo.get_model_files(record.id)?;

    // Resolve the target filename: prefer quant_override, then mc.quant from config,
    // falling back to the first .gguf if quants map is empty (legacy configs).
    let first_gguf = files
        .iter()
        .find(|f| f.filename.ends_with(".gguf"))
        .map(|f| f.filename.clone());

    let target_filename = quant_override
        .or(mc.quant.as_deref())
        .and_then(|quant_label| mc.quants.get(quant_label).map(|qe| qe.file.clone()))
        .or(first_gguf)
        .context("No model file found for this config")?;

    let model_file = files
        .into_iter()
        .find(|f| f.filename == target_filename)
        .context("Resolved model file not found in database")?;

    let model_data_dir = config.models_dir()?;
    let candidate = model_data_dir
        .join(&record.repo_id)
        .join(&model_file.filename);
    if candidate.exists() {
        return Ok(candidate);
    }

    let legacy = db_dir.join("models");
    let legacy_candidate = legacy.join(&record.repo_id).join(&model_file.filename);
    if legacy_candidate.exists() {
        return Ok(legacy_candidate);
    }

    anyhow::bail!(
        "Model file not found: {} (searched {:?} and {:?})",
        model_file.filename,
        candidate,
        legacy_candidate
    )
}
