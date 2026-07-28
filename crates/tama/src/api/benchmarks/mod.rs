//! Benchmark API endpoints.
//!
//! Provides REST endpoints for triggering llama-bench benchmarks,
//! streaming progress via SSE, and managing benchmark history.

mod history;
mod mtp;
mod run;
mod spec;

// ── Shared imports (re-exported for sub-modules) ─────────────────────

use anyhow::Result;
use axum::extract::{Extension, Path, State};
use axum::response::sse::Event;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Sse},
    Json,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::error::error_response;
use crate::api::helpers::shared_repository;
use crate::gpu::query_vram;
use crate::web_types::{JobEvent, JobKind, JobManager, JobStatus, WebState};
use tama_core::backends::ProgressSink;
use tama_core::bench::llama_cli_spec::{SpecBenchConfig, SpecType};
use tama_core::proxy::ProxyState;

// ── Request/Response DTOs ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkRunRequest {
    pub model_id: String,
    /// Optional quant label (e.g. "Q6_K"). When provided, the benchmark uses
    /// the GGUF file for this specific quant instead of the default.
    #[serde(default)]
    pub quant: Option<String>,
    /// Optional backend name to use for llama-bench. If not provided, the
    /// backend is resolved from the model config.
    #[serde(default)]
    pub backend_name: Option<String>,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub warmup: u32,
    #[serde(default)]
    pub threads: Option<Vec<u32>>,
    #[serde(default)]
    pub ngl_range: Option<String>,
    #[serde(default)]
    pub ctx_override: Option<u32>,
    #[serde(default)]
    pub batch_sizes: Vec<u32>,
    #[serde(default)]
    pub ubatch_sizes: Vec<u32>,
    #[serde(default)]
    pub kv_cache_type: Option<String>,
    #[serde(default)]
    pub depth: Vec<u32>,
    #[serde(default)]
    pub flash_attn: Option<bool>,
    /// Identifies what kind of benchmark was run (e.g., "baseline", "pp_sweep").
    #[serde(default)]
    pub benchmark_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkRunResponse {
    pub job_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpecBenchmarkRunRequest {
    pub model_id: String,
    /// Optional quant label (e.g. "Q6_K"). When provided, the benchmark uses
    /// the GGUF file for this specific quant instead of the default.
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub backend_name: Option<String>,
    /// Optional GPU variant to use for the backend (e.g. "cpu", "cuda", "rocm", "vulkan").
    /// When provided, overrides config/DB resolution for the backend path.
    #[serde(default)]
    pub gpu_variant: Option<String>,
    pub spec_types: Vec<SpecType>,
    #[serde(default)]
    pub draft_max_values: Vec<u32>,
    #[serde(default)]
    pub ngram_n_values: Vec<u32>,
    #[serde(default)]
    pub ngram_m_values: Vec<u32>,
    /// N-gram minimum match values for n-gram-mod.
    #[serde(default)]
    pub ngram_min_values: Vec<u32>,
    /// N-gram maximum match values for n-gram-mod.
    #[serde(default)]
    pub ngram_max_values: Vec<u32>,
    #[serde(default = "default_min_hits")]
    pub ngram_min_hits: u32,
    #[serde(default = "default_gen_tokens")]
    pub gen_tokens: u32,
    #[serde(default = "default_runs")]
    pub runs: u32,
    #[serde(default)]
    pub ngl: Option<u32>,
    #[serde(default = "default_flash_attn")]
    pub flash_attn: bool,
    /// Identifies what kind of benchmark was run (e.g., "spec_scan", "spec_sweep").
    #[serde(default)]
    pub benchmark_type: Option<String>,
}

fn default_min_hits() -> u32 {
    1
}
fn default_gen_tokens() -> u32 {
    256
}
fn default_runs() -> u32 {
    3
}
fn default_flash_attn() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct BenchmarkHistoryEntry {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    #[serde(default)]
    pub engine: Option<String>,
    /// Identifies what kind of benchmark was run (e.g., "baseline", "pp_sweep").
    #[serde(default)]
    pub benchmark_type: Option<String>,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub results_count: usize,
    pub status: String,
    pub results: serde_json::Value,
}

// ── Re-exports from sub-modules ───────────────────────────────────────

pub use history::{
    benchmark_events, delete_benchmark, get_benchmark_result, list_benchmark_history,
};
pub use mtp::run_mtp_benchmark;
pub use run::{run_benchmark, run_benchmark_inner};
pub use spec::{run_spec_benchmark, run_spec_benchmark_inner, validate_spec_sweep};

// ── Shared helpers ────────────────────────────────────────────────────

/// A generic progress sink for benchmark jobs.
/// Logs progress lines and broadcasts results over the job event channel.
#[derive(Clone)]
pub struct BenchmarkProgressSink {
    pub name: &'static str,
    pub job: Arc<crate::web_types::Job>,
    pub jobs: Arc<JobManager>,
}

impl ProgressSink for BenchmarkProgressSink {
    fn log(&self, line: &str) {
        tracing::debug!("[{}] {}", self.name, line);
        let job = self.job.clone();
        let jobs = self.jobs.clone();
        let line = line.to_string();
        tokio::spawn(async move {
            jobs.append_log(&job, line).await;
        });
    }

    fn result(&self, json: &str) {
        let job = self.job.clone();
        let data = json.to_string();
        tracing::info!("[{}] result: {}", self.name, json);

        // Broadcast over the shared job event channel so live SSE
        // subscribers get the result immediately. Send synchronously —
        // `broadcast::Sender::send` is non-blocking.
        if let Err(e) = job.log_tx.send(JobEvent::Result(data.clone())) {
            tracing::warn!("Failed to broadcast result for job {}: {}", job.id, e);
        }

        tokio::spawn(async move {
            // Also store in job state so late subscribers can pick it
            // up on replay and the REST endpoint can return it.
            let mut results = job.benchmark_results.write().await;
            *results = Some(data);
            tracing::info!("Stored benchmark results in job state");
        });
    }
}

/// Generic job submission helper for benchmark handlers.
/// Takes ownership of the request to avoid borrow-checker issues with
/// `tokio::spawn` (borrowed references can't escape into `'static` tasks).
pub async fn submit_benchmark_job<F, Fut, R>(
    state: &tama_core::proxy::ProxyState,
    web_state: &WebState,
    req: R,
    run_inner: F,
) -> std::result::Result<(String, Arc<JobManager>), axum::response::Response>
where
    R: Send + 'static,
    F: FnOnce(
            Arc<JobManager>,
            Arc<crate::web_types::Job>,
            R,
            std::path::PathBuf,
            String,
            reqwest::Client,
            std::sync::Arc<std::sync::Mutex<tama_core::db::repository::Repository>>,
        ) -> Fut
        + Send
        + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    let jobs = match &web_state.jobs {
        Some(j) => j.clone(),
        None => return Err(job_manager_unavailable_response()),
    };

    let job = jobs
        .submit(JobKind::Benchmark, None)
        .await
        .map_err(|_| job_conflict_response())?;
    let job_id = job.id.clone();
    let db_path = match crate::api::helpers::resolve_config_dir(state) {
        Ok(d) => d.join("tama.db"),
        Err(resp) => return Err(resp),
    };
    let proxy_base_url = state.config().read().await.proxy_url();
    let client = state.client().clone();

    let repo_handle = match shared_repository(web_state) {
        Ok(h) => h,
        Err(resp) => return Err(resp),
    };

    let jobs_for_spawn = jobs.clone();
    let job_for_spawn = job.clone();
    tokio::spawn(async move {
        if let Err(e) = run_inner(
            jobs_for_spawn.clone(),
            job_for_spawn.clone(),
            req,
            db_path,
            proxy_base_url,
            client,
            repo_handle,
        )
        .await
        {
            tracing::error!(job_id = %job_for_spawn.id, error = %e, "Benchmark job failed");
            jobs_for_spawn
                .finish(&job_for_spawn, JobStatus::Failed, Some(e.to_string()))
                .await;
        } else {
            jobs_for_spawn
                .finish(&job_for_spawn, JobStatus::Succeeded, None)
                .await;
        }
    });

    Ok((job_id, jobs))
}

/// Build the shared error response for job manager unavailability.
pub fn job_manager_unavailable_response() -> axum::response::Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "Job manager not available",
        None,
    )
}

/// Build the shared error response for job submission conflicts.
pub fn job_conflict_response() -> axum::response::Response {
    error_response(
        StatusCode::CONFLICT,
        "Another job is already running",
        Some("ConflictError"),
    )
}
