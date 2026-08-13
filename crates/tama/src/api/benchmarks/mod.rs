//! Benchmark API endpoints.
//!
//! Provides REST endpoints for triggering llama-bench benchmarks,
//! streaming progress via SSE, and managing benchmark history.

mod history;
mod mtp;
mod run;
mod spec;
mod suite;

// ── Shared imports (re-exported for sub-modules) ─────────────────────

use anyhow::Result;
use axum::extract::{Extension, Path, State};
use axum::response::sse::{Event, KeepAlive};
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
use tama_core::bench::llama_cli_spec::{SpecBenchConfig, SpecType};
use tama_core::installations::ProgressSink;
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
    /// Optional GPU variant to use for the backend (e.g. "cpu", "cuda", "rocm", "vulkan").
    /// When provided, overrides config/DB resolution for the backend path.
    #[serde(default)]
    pub gpu_variant: Option<String>,
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
    /// Suite identifier for grouping related benchmark runs within a suite.
    #[serde(skip, default)]
    pub suite_id: Option<String>,
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
    /// Suite identifier for grouping related benchmark runs within a suite.
    #[serde(skip, default)]
    pub suite_id: Option<String>,
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

// Re-export the shared BenchmarkHistoryEntry so external consumers can use it.
pub use crate::pages::benchmarks::types::BenchmarkHistoryEntry;

// ── Re-exports from sub-modules ───────────────────────────────────────

pub use history::{
    benchmark_events, delete_benchmark, get_benchmark_result, list_benchmark_history,
};
pub use mtp::run_mtp_benchmark;
pub use run::{run_benchmark, run_benchmark_inner};
pub use spec::{run_spec_benchmark, run_spec_benchmark_inner, validate_spec_sweep};
pub use suite::{run_benchmark_suite, select_suite_types, BenchmarkSuiteRequest};

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

/// Shared setup data extracted during job submission.
#[derive(Clone)]
pub struct BenchmarkJobContext {
    pub db_path: std::path::PathBuf,
    pub proxy_base_url: String,
    pub client: reqwest::Client,
    pub repo_handle: std::sync::Arc<std::sync::Mutex<tama_core::db::repository::Repository>>,
}

/// Resolve shared context needed for benchmark execution.
pub async fn resolve_benchmark_context(
    state: &tama_core::proxy::ProxyState,
    web_state: &WebState,
) -> std::result::Result<BenchmarkJobContext, axum::response::Response> {
    let db_path = match crate::api::helpers::resolve_config_dir(state) {
        Ok(d) => d.join("tama.db"),
        Err(resp) => return Err(resp),
    };
    let proxy_base_url = state.with_config(|c| c.proxy_url()).await;
    let client = state.client().clone();

    let repo_handle = match shared_repository(web_state) {
        Ok(h) => h,
        Err(resp) => return Err(resp),
    };

    Ok(BenchmarkJobContext {
        db_path,
        proxy_base_url,
        client,
        repo_handle,
    })
}

/// Spawn a benchmark task: create a job, spawn a worker task, and mark it
/// finished when done. The `work` closure runs inside the spawned task
/// and receives the jobs handle, job reference, and resolved context.
pub async fn spawn_benchmark_task<F, Fut>(
    web_state: &WebState,
    ctx: BenchmarkJobContext,
    work: F,
) -> std::result::Result<(String, Arc<JobManager>), axum::response::Response>
where
    F: FnOnce(Arc<JobManager>, Arc<crate::web_types::Job>, BenchmarkJobContext) -> Fut
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

    let jobs_for_spawn = jobs.clone();
    let job_for_spawn = job.clone();
    let work_ctx = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = work(jobs_for_spawn.clone(), job_for_spawn.clone(), work_ctx).await {
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

/// Generic job submission helper for benchmark handlers.
/// Delegates to `resolve_benchmark_context` + `spawn_benchmark_task` to avoid
/// duplicating the shared setup / spawn / finish logic.
pub async fn submit_benchmark_job<F, Fut, R>(
    state: &tama_core::proxy::ProxyState,
    web_state: &WebState,
    req: R,
    run_inner: F,
) -> std::result::Result<(String, Arc<JobManager>), axum::response::Response>
where
    R: Send + Clone + 'static,
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
        + Clone
        + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    // Resolve shared context using the dedicated helper.
    let ctx = resolve_benchmark_context(state, web_state).await?;

    // Delegate to the generic spawn helper.
    spawn_benchmark_task(web_state, ctx, move |jobs, job, ctx| {
        let req = req.clone();
        let run_inner = run_inner.clone();
        async move {
            run_inner(
                jobs,
                job,
                req,
                ctx.db_path,
                ctx.proxy_base_url,
                ctx.client,
                ctx.repo_handle,
            )
            .await
        }
    })
    .await
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

// ── Status derivation helper ──────────────────────────────────────────

/// Derive the canonical run status from per-entry counts.
///
/// - `run_errored` → `"failed"` (run failed before producing results)
/// - `entries_failed > 0` → `"partial"` (some entries succeeded, some failed)
/// - otherwise → `"success"` (all entries ok)
pub fn derive_status(_entries_ok: usize, entries_failed: usize, run_errored: bool) -> &'static str {
    if run_errored {
        "failed"
    } else if entries_failed > 0 {
        "partial"
    } else {
        "success"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_status_success() {
        assert_eq!(derive_status(5, 0, false), "success");
        assert_eq!(derive_status(1, 0, false), "success");
        assert_eq!(derive_status(0, 0, false), "success");
    }

    #[test]
    fn test_derive_status_partial() {
        assert_eq!(derive_status(5, 1, false), "partial");
        assert_eq!(derive_status(0, 1, false), "partial");
        assert_eq!(derive_status(3, 2, false), "partial");
    }

    #[test]
    fn test_derive_status_failed() {
        assert_eq!(derive_status(5, 0, true), "failed");
        assert_eq!(derive_status(0, 0, true), "failed");
        assert_eq!(derive_status(5, 3, true), "failed");
    }
}
