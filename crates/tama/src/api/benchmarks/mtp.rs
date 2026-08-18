use super::*;
use crate::api::benchmarks::derive_status;
use crate::api::benchmarks::run::unload_model_before_benchmark;
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
    /// Suite identifier for grouping related benchmark runs within a suite.
    #[serde(skip, default)]
    pub suite_id: Option<String>,
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

#[allow(clippy::too_many_arguments)]
pub async fn run_mtp_benchmark_inner(
    jobs: Arc<JobManager>,
    job: Arc<crate::web_types::Job>,
    req: MtpBenchmarkRunRequest,
    _db_path: std::path::PathBuf,
    proxy_base_url: String,
    client: reqwest::Client,
    db_pool: std::sync::Arc<sqlx::PgPool>,
    tamad_pool: std::sync::Arc<tama_core::tamad::pool::TamadPool>,
) -> Result<()> {
    use tama_core::bench::llama_cli_mtp;
    use tama_core::bench::llama_cli_mtp::MtpBenchResult;

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
    let suite_id = req.suite_id.clone();

    // Load the global config from Postgres (plan-190 Task 3).
    let pool = db_pool.as_ref();
    let config = tama_core::config::Config::load_from_pool(pool).await?;

    // Resolve model key + metadata — model configs come from Postgres.
    let model_configs = tama_core::db::load_model_configs(pool).await?;

    // If model_id is an integer db_id, resolve it to the config key first.
    let resolved_key = if let Ok(db_id) = model_id.parse::<i64>() {
        model_configs
            .iter()
            .find(|(_, mc)| mc.db_id == Some(db_id))
            .map(|(key, _)| key.clone())
            .unwrap_or(model_id.clone())
    } else {
        model_id.clone()
    };

    let (model_config, _) = config
        .resolve_backend(&model_configs, &resolved_key)
        .context("Failed to resolve server config for benchmark")?;
    let display_name = model_configs.get(&resolved_key).and_then(|mc| {
        mc.display_name
            .clone()
            .or_else(|| mc.api_name.clone())
            .or_else(|| mc.model.clone())
    });
    let target_backend = backend_name.unwrap_or_else(|| model_config.backend.clone());

    // Resolve the execution host (plan-191 Task 8, ADR-0010): the
    // model's provider's tamad + host-relative paths. The host resolves
    // its own model file/binary; the config's model_path is a placeholder
    // the tamad overwrites with its resolved path.
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

    // Build MtpBenchConfig (model_path is host-relative on the wire; the
    // tamad re-anchors it to its own models dir).
    let mtp_config = llama_cli_mtp::MtpBenchConfig {
        model_path: std::path::PathBuf::new(),
        draft_max_values: req.draft_max_values,
        ngl: req.ngl,
        draft_ngl: req.draft_ngl,
        flash_attn: req.flash_attn,
        context_size: req.context_size,
    };
    let config_json =
        serde_json::to_string(&mtp_config).context("Failed to serialize mtp config")?;

    tracing::info!(
        job_id = %job.id,
        model = %resolved_key,
        backend = %target_backend,
        draft_max = ?draft_max_for_trace,
        tamad = %host.handle.connection.name,
        "Starting MTP benchmark on tamad",
    );

    // Dispatch to the tamad host and relay progress into this job.
    let bench_req = tama_core::tamad::RunBenchmarkRequest {
        model_name: display_name.clone().unwrap_or_else(|| model_id.clone()),
        kind: "mtp".to_string(),
        config_json,
        model_path_rel: host.model_path_rel.clone(),
        binary_path_rel: host.binary_path_rel.clone(),
    };
    let result_json = super::tamad::dispatch_and_relay(&jobs, &job, &host, &bench_req).await?;

    // The tamad ran the same runner as before — parse the result back into
    // the persistence struct.
    let result: MtpBenchResult =
        serde_json::from_str(&result_json).context("tamad returned an invalid mtp result")?;

    // Serialize the full result for storage
    let results_json =
        serde_json::to_string(&result).context("Failed to serialize mtp benchmark result")?;
    let pp_sizes_json = "[]";
    let tg_sizes_json = "[]";

    // Derive run status from per-entry results: count entries with non-null error.
    let entries_failed = result.entries.iter().filter(|e| e.error.is_some()).count();
    let run_status = derive_status(result.entries.len() - entries_failed, entries_failed, false);

    // VRAM: the execution host (tamad) sampled it — plan-191 Task 10.
    let vram = result.vram.clone();

    // Insert into Postgres (plan-190 Task 8).
    let params = tama_core::db::queries::BenchmarkInsertParams {
        model_id: &model_id,
        display_name: display_name.as_deref(),
        quant: quant.as_deref(),
        backend: &target_backend,
        engine: "llama_cli_mtp",
        pp_sizes_json,
        tg_sizes_json,
        threads_json: None,
        ngl_range: None,
        runs: 1,
        warmup: 0,
        results_json: &results_json,
        load_time_ms: None,
        vram_used_mib: vram.as_ref().map(|v| v.used_mib as i64),
        vram_total_mib: vram.as_ref().map(|v| v.total_mib as i64),
        duration_seconds: 0.0,
        status: run_status,
        benchmark_type: benchmark_type.as_deref(),
        suite_id: suite_id.as_deref(),
    };
    tama_core::db::queries::insert_benchmark(pool, &params).await?;

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
