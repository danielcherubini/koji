use super::*;
use crate::api::benchmarks::derive_status;
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

#[allow(clippy::too_many_arguments)]
pub async fn run_benchmark_inner(
    jobs: Arc<JobManager>,
    job: Arc<crate::web_types::Job>,
    req: BenchmarkRunRequest,
    _db_path: std::path::PathBuf,
    proxy_base_url: String,
    client: reqwest::Client,
    db_pool: std::sync::Arc<sqlx::PgPool>,
    tamad_pool: std::sync::Arc<tama_core::tamad::pool::TamadPool>,
) -> Result<()> {
    use tama_core::bench::llama_bench::{LlamaBenchConfig, LlamaBenchConfigJson};
    use tama_core::bench::{BenchReport, ModelInfo};

    // Unload any active server for this model before running the benchmark.
    // This prevents GPU memory conflicts when the model is already loaded.
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
    let suite_id = req.suite_id.clone();
    let ngl_range = req.ngl_range.clone();
    let ngl_range_for_insert = ngl_range.clone();
    let pp_sizes_for_trace = req.pp_sizes.clone();
    let pp_sizes_for_serial = pp_sizes_for_trace.clone();
    let tg_sizes_for_trace = req.tg_sizes.clone();
    let tg_sizes_for_serial = tg_sizes_for_trace.clone();
    let threads_for_trace = req.threads.clone();

    // Load the global config from Postgres (plan-190 Task 3) and the model
    // configs for key/display resolution (plan-190 Task 5).
    let pool = db_pool.as_ref();
    let config = tama_core::config::Config::load_from_pool(pool).await?;
    let model_configs: std::collections::HashMap<String, tama_core::config::ModelConfig> =
        tama_core::db::load_model_configs(pool).await?;

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
    let model_config = config
        .resolve_backend(&model_configs, &resolved_key)?
        .0
        .clone();
    let target_backend = backend_name.unwrap_or_else(|| model_config.backend.clone());

    // Resolve the execution host (plan-191 Task 8, ADR-0010: the proxy
    // spawns nothing): the model's provider's tamad + host-relative paths.
    let ctx = super::bench_ctx(&db_pool, tamad_pool);
    let host = super::tamad::resolve_benchmark_host(
        &ctx,
        &config,
        &model_configs,
        &resolved_key,
        &target_backend,
        gpu_variant.as_ref(),
    )
    .await?;

    // Build llama-bench config + the proxy-resolved model metadata
    // envelope (the tamad has no DB — it fills the report from this).
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
    let model_name_for_host = display_name.clone().unwrap_or_else(|| model_id.clone());
    let envelope = LlamaBenchConfigJson {
        bench: bench_config,
        model_info: ModelInfo {
            name: model_name_for_host.clone(),
            model_id: model_config.model.clone(),
            // Request quant override, else the model config's selected quant.
            quant: quant.clone().or_else(|| model_config.quant.clone()),
            backend: target_backend.clone(),
            // The host derives the label from the binary path on its disk.
            gpu_variant: String::new(),
            context_length: model_config.context_length,
            gpu_layers: None,
        },
    };
    let config_json =
        serde_json::to_string(&envelope).context("Failed to serialize llama_bench config")?;

    tracing::info!(
        job_id = %job.id,
        model_id = %model_id,
        backend = %target_backend,
        pp_sizes = ?pp_sizes_for_trace,
        tg_sizes = ?tg_sizes_for_trace,
        tamad = %host.handle.connection.name,
        "Starting llama-bench benchmark on tamad",
    );

    // Dispatch to the tamad host and relay progress into this job.
    let bench_req = tama_core::tamad::RunBenchmarkRequest {
        model_name: model_name_for_host,
        kind: "llama_bench".to_string(),
        config_json,
        model_path_rel: host.model_path_rel.clone(),
        binary_path_rel: host.binary_path_rel.clone(),
    };
    let result_json = super::tamad::dispatch_and_relay(&jobs, &job, &host, &bench_req).await?;

    // The tamad ran the same runner as before — parse the report back into
    // the persistence struct.
    let report: BenchReport = serde_json::from_str(&result_json)
        .context("tamad returned an invalid llama_bench result")?;

    // Serialize the full report for storage so history can reconstruct model
    // metadata (backend, GPU, VRAM, load time, batch/ubatch/KV cache choices),
    // not just the per-test summary rows. Re-serializing the parsed struct
    // keeps the stored JSON shape identical to pre-Task-8 runs.
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

    // Llama-bench has no per-test failure concept — parse_bench_json only emits
    // successful tests and failed runs bail! before insert. Always derive success.
    let run_status = derive_status(report.summaries.len(), 0, false);

    // VRAM as reported by the execution host (the tamad sampled its own
    // GPUs — ADR-0010, the proxy no longer samples host hardware here).
    let vram = report.vram.clone();

    // Clone values used for tracing after the insert.
    let display_name_for_trace = display_name.clone();
    let backend_for_trace = report.model_info.backend.clone();
    let suite_id_cloned = suite_id.clone();

    // Insert the benchmark record into Postgres (plan-190 Task 8).
    let params = tama_core::db::queries::BenchmarkInsertParams {
        model_id: &model_id,
        display_name: display_name.as_deref(),
        quant: report.model_info.quant.as_deref(),
        backend: &report.model_info.backend,
        engine: "llama_bench",
        pp_sizes_json: &pp_sizes_json,
        tg_sizes_json: &tg_sizes_json,
        threads_json: threads_json.as_deref(),
        ngl_range: ngl_range_for_insert.as_deref(),
        runs: req.runs,
        warmup: req.warmup,
        results_json: &results_json,
        load_time_ms: Some(report.load_time_ms),
        vram_used_mib: vram.as_ref().map(|v| v.used_mib as i64),
        vram_total_mib: vram.as_ref().map(|v| v.total_mib as i64),
        duration_seconds: 0.0, // duration tracked by job system
        status: run_status,
        benchmark_type: benchmark_type.as_deref(),
        suite_id: suite_id_cloned.as_deref(),
    };
    tama_core::db::queries::insert_benchmark(pool, &params).await?;

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

/// Resolve a model's file path from config and database (Postgres, plan-190 Task 5).
/// `quant_override` takes priority over `mc.quant` when resolving the target file.
pub(super) async fn resolve_model_path(
    config: &tama_core::config::Config,
    db_dir: &std::path::Path,
    pool: &sqlx::PgPool,
    model_configs: &std::collections::HashMap<String, tama_core::config::ModelConfig>,
    resolved_id: &str,
    quant_override: Option<&str>,
) -> Result<std::path::PathBuf> {
    let mc = model_configs
        .get(resolved_id)
        .with_context(|| format!("Model config '{}' not found", resolved_id))?;
    let rec_id = mc.db_id.context("Model config has no db_id")?;
    let record = tama_core::db::queries::get_model_config(pool, rec_id)
        .await?
        .with_context(|| format!("Model config record (id={}) not found in database", rec_id))?;
    let files = tama_core::db::queries::get_model_files(pool, record.id).await?;

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
