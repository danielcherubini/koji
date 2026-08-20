//! In-memory job state for whole-repo `hf` CLI pulls (see ADR-0007).
//!
//! Whole-repo safetensors downloads are dispatched to the pull host (a
//! tamad) via `PullModel`/`StreamJob` (ADR-0010: the proxy never downloads
//! or spawns children itself). The jobs are tracked as in-memory state (no
//! DB rows, not in the Downloads Center); the relay task mirrors the
//! tamad's job events into [`RepoPullJob`] and finalizes the model row on
//! success. Job-map lock holds are brief: the relay never holds the map
//! lock across an `.await`.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::models::pull::hf_cli; // shared hf-CLI helpers (plan-191 Task 6)
use crate::models::pull::{HfModelMetadata, TamadRepoPullResult};

pub(crate) use hf_cli::{scan_dir_bytes, stderr_tail_str};

/// Status of a whole-repo pull job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RepoPullStatus {
    /// The `hf` child process is still running.
    Running,
    /// The child exited with code 0.
    Completed,
    /// The child exited non-zero (or crashed) without a cancel request.
    Failed,
    /// The job was cancelled by the user.
    Cancelled,
}

/// A whole-repo `hf` CLI pull job, tracked in memory (no DB rows).
#[derive(Debug, Clone)]
pub(crate) struct RepoPullJob {
    /// Unique job identifier.
    pub job_id: String,
    /// Hugging Face repo id (e.g. `owner/repo`).
    pub repo_id: String,
    /// Associated model id, if the pull was started from the model wizard.
    pub model_id: Option<i64>,
    /// Local destination directory for the repo contents.
    pub dest: PathBuf,
    /// Expected total size in bytes from HF sibling sizes, if known.
    pub total_bytes: Option<u64>,
    /// Current job status.
    pub status: RepoPullStatus,
    /// Error message (capped stderr tail) for failed jobs.
    pub error: Option<String>,
    /// Set by cancel_repo_pull BEFORE killing, so the wait-loop's final
    /// status decision can distinguish "killed by user" from "crashed".
    pub cancel_requested: bool,
    /// From config.json max_position_embeddings, populated on completion.
    pub context_length: Option<u32>,
    /// Capped tail of the child's stderr (last 4096 RAW BYTES), updated by
    /// the reader task. Raw bytes so the cap can drain at any byte offset
    /// (`Vec::drain` never panics, unlike `String::drain` mid-character);
    /// decoded lossy, once, at read time in `stderr_tail_str`.
    /// Relayed host pulls reuse this sink to render the tamad's error text
    /// through the existing error path.
    pub(crate) stderr_tail: Arc<Mutex<Vec<u8>>>,
    /// The tamad-side job id when the pull is hosted on a tamad via
    /// `PullModel`/`StreamJob` (plan-191 follow-up B). Used by the cancel
    /// endpoint to dispatch `CancelJob`.
    pub(crate) tamad_job_id: Option<String>,
    /// Bytes downloaded, mirrored from the tamad's job events (plan-191
    /// follow-up B) — the relay is the single source for progress; the
    /// directory-size scan is only a fallback.
    pub(crate) bytes_done: u64,
}

/// Error for starting a whole-repo `hf` CLI pull.
#[derive(Debug, thiserror::Error)]
pub enum RepoPullError {
    /// A whole-repo pull for this repo is already running.
    #[error("a whole-repo pull for this repo is already running")]
    DuplicatePull,
    /// The repo does not exist on HuggingFace.
    #[error("{0}")]
    RepoNotFound(String),
    /// Upstream (network / HF API / config / spawn) error.
    #[error("{0}")]
    Upstream(String),
    /// The repo id is not a valid HuggingFace repo id.
    #[error("invalid repo id: '{0}'")]
    InvalidRepoId(String),
}

/// Result of successfully starting a whole-repo pull.
///
/// The web layer maps this to its own response shape (the status is always
/// "running" — the job was just created).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoPullStart {
    /// Job id to poll / cancel with.
    pub job_id: String,
    /// Expected total size in bytes from HF sibling sizes, if known.
    pub total_bytes: Option<u64>,
}

/// Public DTO describing a whole-repo pull job (web-crate-visible).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoPullStatusDto {
    /// Job id.
    pub job_id: String,
    /// Status, lowercase: running | completed | failed | cancelled.
    pub status: String,
    /// Bytes downloaded so far (sum of file sizes in the destination dir).
    pub bytes_done: u64,
    /// Expected total size in bytes, if known at start.
    pub total_bytes: Option<u64>,
    /// Error message for failed jobs (capped stderr tail).
    pub error: Option<String>,
    /// Context length from config.json, populated on completion.
    pub context_length: Option<u32>,
}

/// Lowercase status string for the DTO.
impl std::fmt::Display for RepoPullStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RepoPullStatus::Running => "running",
            RepoPullStatus::Completed => "completed",
            RepoPullStatus::Failed => "failed",
            RepoPullStatus::Cancelled => "cancelled",
        };
        f.write_str(s)
    }
}

/// Network part of repo-pull completion: fetch HF metadata for the repo.
///
/// Soft-fails to `HfModelMetadata::default()` so a metadata-fetch error never
/// fails the job (the download already succeeded). No DB access here — this
/// must be awaited BEFORE any connection is opened (see `finish_repo_pull`).
pub(crate) async fn fetch_completion_metadata(repo_id: &str) -> HfModelMetadata {
    match crate::models::pull::lookup_hf_metadata(repo_id).await {
        Ok(meta) => meta,
        Err(e) => {
            tracing::warn!(
                "repo-pull completion: HF metadata fetch failed for '{}': {e}",
                repo_id
            );
            HfModelMetadata::default()
        }
    }
}

/// Async part of repo-pull completion: apply merged metadata to the model row.
///
/// Precedence: existing DB values survive
/// (COALESCE inside `update_model_config_hf_metadata`); where the DB and `base`
/// (HF metadata) are both NULL, `meta_tf` (config.json) fills the gap, with
/// `base` winning over `meta_tf`. `hf_format` defaults to "transformers" when
/// unknown. Quant is set unconditionally from `quantization_method`.
///
/// Returns the context length from config.json (for the job's
/// `context_length` field), if any.
pub(crate) async fn apply_repo_pull_completion_with_meta(
    pool: &sqlx::PgPool,
    model_id: i64,
    base: &HfModelMetadata,
    meta_tf: Option<&crate::models::transformers::TransformersMetadata>,
    // Kept for call-site symmetry with the completion flow (and for future
    // per-file verification against dest); unused by the current merge logic.
    _dest: &std::path::Path,
) -> anyhow::Result<Option<u32>> {
    let mut meta = base.clone();
    if let Some(tf) = meta_tf {
        if meta.hf_architecture_type.is_none() {
            meta.hf_architecture_type = tf.architectures.first().cloned();
        }
        if meta.hf_context_length.is_none() {
            meta.hf_context_length = tf.max_position_embeddings;
        }
        if meta.hf_num_layers.is_none() {
            meta.hf_num_layers = tf.num_hidden_layers;
        }
    }
    if meta.hf_format.is_none() {
        meta.hf_format = Some("transformers".to_string());
    }

    crate::models::update::update_model_config_hf_metadata(pool, model_id, &meta).await?;

    if let Some(qm) = meta_tf.and_then(|tf| tf.quantization_method.as_deref()) {
        crate::models::update::update_model_config_quant(pool, model_id, qm).await?;
    }

    Ok(meta_tf.and_then(|tf| tf.max_position_embeddings))
}

/// Finalize a whole-repo pull job after the child exits. Called from the
/// wait-loop.
///
/// The terminal decision is made FIRST (`cancel_requested` → Cancelled,
/// `exit_status == Some(0)` → Completed, anything else → Failed with the
/// capped stderr-tail error). The model-row completion step — config.json
/// parse + HF metadata fetch + DB update — runs ONLY for Completed jobs: a
/// failed or cancelled download can still leave a valid config.json in dest
/// (hf writes it early, before the weights), and must not mark the model
/// row as configured. Non-Completed jobs skip the network fetch entirely.
///
/// Order matters so the network/DB awaits never hold the job lock:
/// 1. re-read the job fields under a brief lock,
/// 2. compute the terminal decision (status + error),
/// 3. (Completed only) parse config.json (sync fs, soft-fail — a repo without
///    it completes), fetch HF metadata (network), then update the model row
///    on the Postgres pool,
/// 4. write the terminal status under a brief lock.
///
/// DB errors are logged but never fail the job — metadata is informational,
/// the download itself already succeeded.
pub(crate) async fn finish_repo_pull(
    state: &crate::proxy::ProxyState,
    job_id: &str,
    exit_status: Option<i32>,
) {
    // 1. Brief lock: snapshot the fields we need.
    let mut job = match state.pull.get_repo_pull(job_id).await {
        Some(job) => job,
        None => return, // job cleared (e.g. shutdown) — nothing to finalize
    };
    let repo_id = job.repo_id.clone();
    let dest = job.dest.clone();
    let model_id = job.model_id;
    let cancel_requested = job.cancel_requested;
    let stderr_tail = job.stderr_tail.clone();

    // 2. Terminal decision FIRST: cancel wins, then the exit code.
    let (status, error) = if cancel_requested {
        (RepoPullStatus::Cancelled, None)
    } else if exit_status == Some(0) {
        (RepoPullStatus::Completed, None)
    } else {
        let error = match stderr_tail_str(&stderr_tail).await {
            Some(tail) => Some(tail),
            None => Some(match exit_status {
                Some(code) => format!("hf download exited with code {code}"),
                None => "hf download exited abnormally".to_string(),
            }),
        };
        (RepoPullStatus::Failed, error)
    };

    // 3. Completion (metadata + model row) ONLY for successful downloads.
    //    A failed/cancelled job may have a partial config.json in dest —
    //    letting it reach the DB would leave a misleadingly "configured"
    //    model row behind. `context_length` stays None in that case.
    let mut context_length: Option<u32> = None;
    if status == RepoPullStatus::Completed {
        // Best-effort local read: in a single-host layout the repo (incl.
        // config.json) lands next door and the parse supplies the context
        // length + transformers metadata. In a remote-host layout nothing
        // exists at `dest` on this machine — the parse soft-fails and the
        // completion proceeds from the HF metadata fetch alone. The relay
        // never requires proxy-local files.
        let meta_tf = crate::models::transformers::parse_transformers_metadata(&dest).ok();

        // Network — no connection open yet.
        let base = fetch_completion_metadata(&repo_id).await;

        // DB step — Postgres pool (plan-190 Task 5).
        if let Some(model_id) = model_id {
            let pool = state.db_pool();
            match apply_repo_pull_completion_with_meta(
                &pool,
                model_id,
                &base,
                meta_tf.as_ref(),
                &dest,
            )
            .await
            {
                Ok(context_length_tf) => context_length = context_length_tf,
                Err(e) => {
                    // Metadata is informational — the job still completes.
                    tracing::warn!(
                        "repo-pull completion: DB update failed for '{}': {e}",
                        repo_id
                    );
                }
            }
        }
    }

    // 4. Brief lock: terminal status + error + context length.
    job.status = status;
    job.error = error;
    job.context_length = context_length;
    state.pull.upsert_repo_pull(job).await;
}

/// Start a whole-repo `hf` CLI pull job (ADR-0010: execution on the pull
/// host).
///
/// Validation order: repo id → duplicate → repo existence →
/// (soft) byte totals → destination → dispatch `PullModel(repo_pull=true)`
/// to `proxy.pull_backend`'s tamad → register → relay `StreamJob` events.
///
/// `state` is an `Arc` so the spawned relay task can clone it and outlive
/// the caller. `model_id` is the pre-created stub row (`None` = API-only
/// caller, no DB update on completion).
pub(crate) async fn start_repo_pull(
    state: &Arc<crate::proxy::ProxyState>,
    repo_id: &str,
    model_id: Option<i64>,
) -> Result<RepoPullStart, RepoPullError> {
    // Validate the repo id (charset / path traversal).
    if !crate::models::is_valid_repo_id(repo_id) {
        return Err(RepoPullError::InvalidRepoId(repo_id.to_string()));
    }

    // Reject concurrent pulls of the same repo.
    if state.pull.repo_pull_running_for(repo_id).await {
        return Err(RepoPullError::DuplicatePull);
    }

    // The repo must exist on HuggingFace. A 404 / "not found" is a missing
    // repo; anything else is an upstream/network error. (Proxy-side HTTP
    // metadata only — the download itself runs on the pull host, ADR-0010.)
    let api = crate::models::pull::hf_api()
        .await
        .map_err(|e| RepoPullError::Upstream(e.to_string()))?;
    if let Err(e) = api.model(repo_id.to_string()).info().await {
        let msg = e.to_string();
        let lower = msg.to_lowercase();
        if lower.contains("404") || lower.contains("not found") {
            return Err(RepoPullError::RepoNotFound(format!(
                "'{repo_id}' not found on HuggingFace"
            )));
        }
        return Err(RepoPullError::Upstream(msg));
    }

    // Expected total size — soft-fail (progress becomes indeterminate).
    let total_bytes = match crate::models::pull::lookup_repo_stats(repo_id).await {
        Ok(stats) => Some(stats.total_bytes),
        Err(e) => {
            tracing::debug!(
                "repo-pull start: repo stats lookup failed for '{}': {e}",
                repo_id
            );
            None
        }
    };

    // Resolve the destination directory under the models root.
    let models_dir = {
        let cfg = state.config.read().await;
        cfg.models_dir()
            .map_err(|e| RepoPullError::Upstream(e.to_string()))?
    };
    let dest = crate::models::repo_path(models_dir, repo_id);
    tokio::fs::create_dir_all(&dest)
        .await
        .map_err(|e| RepoPullError::Upstream(e.to_string()))?;

    // ── Dispatch the whole-repo pull to the pull host (plan-191 follow-up
    // B; ADR-0010) ── the `hf` CLI runs on the tamad (`repo_pull = true`);
    // this process only relays `StreamJob` progress into the in-memory
    // job and finalizes the metadata on success.
    let pull_backend = state
        .config
        .read()
        .await
        .proxy
        .pull_backend
        .clone()
        .ok_or_else(|| {
            RepoPullError::Upstream(
                "no pull host configured: set proxy.pull_backend (the proxy itself never downloads — ADR-0010)"
                    .to_string(),
            )
        })?;
    let handle = state.tamad_pool.get(&pull_backend).await.ok_or_else(|| {
        RepoPullError::Upstream(format!(
            "pull_backend '{pull_backend}' is not a registered tamad"
        ))
    })?;

    let hf_token = crate::models::pull::get_hf_token().unwrap_or_default();
    let request = crate::tamad::PullModelRequest {
        repo_id: repo_id.to_string(),
        quants: Vec::new(),
        model_name: String::new(),
        backend: String::new(),
        hf_token,
        repo_pull: true,
        dest_dir: dest.to_string_lossy().to_string(),
    };
    tracing::info!(
        repo = repo_id,
        tamad = %pull_backend,
        "dispatching whole-repo pull to tamad"
    );
    let tamad_job_id = handle
        .pull_model(&request)
        .await
        .map_err(|e| RepoPullError::Upstream(format!("tamad pull dispatch failed: {e:#}")))?;

    let job_id = format!("hfrepo-{}", uuid::Uuid::new_v4().hyphenated());
    state
        .pull
        .upsert_repo_pull(RepoPullJob {
            job_id: job_id.clone(),
            repo_id: repo_id.to_string(),
            model_id,
            dest,
            total_bytes,
            status: RepoPullStatus::Running,
            error: None,
            cancel_requested: false,
            context_length: None,
            stderr_tail: Arc::new(Mutex::new(Vec::new())),
            tamad_job_id: Some(tamad_job_id.clone()),
            bytes_done: 0,
        })
        .await;

    let state_clone = Arc::clone(state);
    let relay_job_id = job_id.clone();
    tokio::spawn(async move {
        relay_repo_pull(&state_clone, &relay_job_id, &handle, &tamad_job_id).await;
    });

    Ok(RepoPullStart {
        job_id,
        total_bytes,
    })
}

/// Relay a whole-repo `PullModel` job until it reaches a terminal state,
/// mirroring progress into the in-memory [`RepoPullJob`] and finalizing
/// (metadata + model row) on success. Shared by every relayed host pull
/// (plan-191 follow-up B).
pub(crate) async fn relay_repo_pull(
    state: &crate::proxy::ProxyState,
    job_id: &str,
    handle: &Arc<crate::tamad::pool::TamadHandle>,
    tamad_job_id: &str,
) {
    const EVENT_TIMEOUT_SECS: u64 = 120;
    let stream = match handle.stream_job(tamad_job_id).await {
        Ok(s) => s,
        Err(e) => {
            finalize_failed(state, job_id, format!("tamad job stream failed: {e:#}")).await;
            return;
        }
    };

    let mut stream = stream;
    loop {
        let next = tokio::time::timeout(
            std::time::Duration::from_secs(EVENT_TIMEOUT_SECS),
            stream.message(),
        )
        .await;
        let ev = match next {
            Err(_elapsed) => {
                finalize_failed(
                    state,
                    job_id,
                    format!("pull stalled: no tamad progress for {EVENT_TIMEOUT_SECS}s"),
                )
                .await;
                return;
            }
            Ok(Err(e)) => {
                finalize_failed(state, job_id, format!("tamad job stream error: {e:?}")).await;
                return;
            }
            Ok(Ok(None)) => {
                finalize_failed(
                    state,
                    job_id,
                    "tamad disconnected mid-pull (no terminal job event)".to_string(),
                )
                .await;
                return;
            }
            Ok(Ok(Some(e))) => e,
        };

        // Mirror progress into the in-memory job (brief lock hold, no
        // `.await` inside).
        state
            .pull
            .with_repo_pull(job_id, |job| {
                if ev.total_bytes > 0 {
                    job.total_bytes = Some(ev.total_bytes as u64);
                }
                if ev.bytes_downloaded > 0 {
                    job.bytes_done = ev.bytes_downloaded as u64;
                }
            })
            .await;

        match ev.status.as_str() {
            "succeeded" => {
                // Consume the host's terminal payload (`{"dir","ok"}`): a
                // payload that explicitly declares failure is a contract
                // violation — fail the job rather than trusting the status
                // flag alone. An empty/legacy payload is tolerated: the
                // "succeeded" status is the source of truth.
                if let Some(false) = parse_repo_result_ok(&ev.result_json) {
                    finalize_failed(
                        state,
                        job_id,
                        "tamad repo pull result payload reported ok=false".to_string(),
                    )
                    .await;
                    return;
                }
                finish_repo_pull(state, job_id, Some(0)).await;
                return;
            }
            "cancelled" => {
                // The user cancelled from the UI; the cancel endpoint has
                // already flagged `cancel_requested` — the final decision
                // (cancel wins) happens in finish_repo_pull.
                finish_repo_pull(state, job_id, None).await;
                return;
            }
            "failed" => {
                // Mirror the host error into the capped stderr tail so the
                // existing error-path logic renders it. Clone the sink Arc
                // under a brief job-guard, then lock the sink (no map-lock
                // held across the await).
                let sink_arc = state
                    .pull
                    .with_repo_pull(job_id, |job| job.stderr_tail.clone())
                    .await;
                if let Some(sink_arc) = sink_arc {
                    let cap: usize = 4096;
                    let mut bytes = ev.error.trim().to_string().into_bytes();
                    if bytes.len() > cap {
                        bytes.drain(0..(bytes.len() - cap));
                    }
                    let mut sink = sink_arc.lock().await;
                    sink.clear();
                    sink.extend_from_slice(&bytes);
                }
                finish_repo_pull(state, job_id, Some(1)).await;
                return;
            }
            _ => {}
        }
    }
}

/// The `ok` flag of the host's terminal repo-pull payload
/// (`{"dir","ok"}`), when the payload is present and parseable.
fn parse_repo_result_ok(result_json: &str) -> Option<bool> {
    if result_json.trim().is_empty() {
        return None;
    }
    let result: TamadRepoPullResult = serde_json::from_str(result_json).ok()?;
    Some(result.ok)
}

/// Finalize a relayed host pull as FAILED before/at stream teardown:
/// mirror the error into the capped stderr tail (so the existing render
/// logic picks it up) and run the normal terminal decision (cancel still
/// wins if the user cancelled in the meantime).
async fn finalize_failed(state: &crate::proxy::ProxyState, job_id: &str, error: String) {
    // Mirror the relay error into the capped stderr tail, then run the
    // normal terminal decision (cancel still wins if the user cancelled in
    // the meantime).
    if let Some(sink_arc) = state
        .pull
        .with_repo_pull(job_id, |job| job.stderr_tail.clone())
        .await
    {
        let mut sink = sink_arc.lock().await;
        sink.clear();
        sink.extend_from_slice(&error.into_bytes());
    }
    finish_repo_pull(state, job_id, None).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::state::pull::PullState;
    use crate::tamad::pool::test_support::{
        grpc_conn, job_event_bytes, job_event_failed, start_stub, terminal_success, StubTamad,
    };

    const TAMAD_ID: &str = "uuid-repo-tamad";
    const JOB_ID: &str = "job-repo";

    /// StubTamad with scripted pull events (repo-pull relay tests).
    fn make_stub(
        events: Vec<crate::tamad::JobEvent>,
        pull_model_fail: bool,
    ) -> (StubTamad, Arc<tokio::sync::watch::Sender<bool>>) {
        let (down, _) = tokio::sync::watch::channel(false);
        let stub = StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::new(down.clone()),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            successes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: JOB_ID.to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(pull_model_fail)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(events)),
            stream_job_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_processes: vec![],
            logs_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            log_messages: vec![],
        };
        (stub, Arc::new(down))
    }

    /// ProxyState with `pull_backend` pointing at the stub tamad, an isolated
    /// models dir, an in-memory config home, and a fresh Postgres schema
    /// (the completion step writes the model row through the pool).
    async fn state_with_stub(
        models_tmp: &tempfile::TempDir,
        xdg_tmp: &tempfile::TempDir,
        stub_addr: std::net::SocketAddr,
    ) -> (
        Arc<crate::proxy::ProxyState>,
        crate::testing::postgres::SchemaGuard,
    ) {
        std::env::set_var("XDG_CONFIG_HOME", xdg_tmp.path().to_str().unwrap());
        std::env::set_var("HOME", xdg_tmp.path().to_str().unwrap());
        std::env::remove_var("HF_TOKEN");

        let mut config = crate::config::Config::default();
        config.general.models_dir = Some(models_tmp.path().to_string_lossy().to_string());
        config.proxy.pull_backend = Some(TAMAD_ID.to_string());

        let guard = crate::testing::postgres::with_schema().await;
        let pool = std::sync::Arc::new(guard.pool.clone());
        let state = crate::proxy::ProxyState::new(config, Some(xdg_tmp.path().to_path_buf()), pool);
        let conn = grpc_conn(TAMAD_ID, "stub", &format!("grpc://{stub_addr}"));
        state.tamad_pool.upsert_connection(&conn).await.unwrap();
        (Arc::new(state), guard)
    }

    fn restore_env() {
        std::env::remove_var("HF_ENDPOINT");
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("HOME");
    }

    /// Insert a pre-created stub model row (the wizard does this before
    /// starting a repo pull) and return its id.
    async fn insert_stub_model(state: &crate::proxy::ProxyState, repo_id: &str) -> i64 {
        let pool = state.db_pool();
        let record = crate::db::queries::ModelConfigRecord {
            repo_id: repo_id.to_string(),
            backend: "llama_cpp".to_string(),
            ..Default::default()
        };
        crate::db::queries::upsert_model_config(&pool, &record)
            .await
            .unwrap()
    }

    /// Poll the job until it reaches a terminal state (the relay task is
    /// spawned by start_repo_pull and runs concurrently).
    async fn wait_terminal(state: &crate::proxy::ProxyState, job_id: &str) -> RepoPullJob {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if let Some(job) = state.pull.get_repo_pull(job_id).await {
                if job.status != RepoPullStatus::Running {
                    return job;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "job did not reach a terminal state in time"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Test that RepoPullStatus serializes to lowercase strings.
    #[test]
    fn test_repo_pull_status_serde_lowercase() {
        let statuses = [
            (RepoPullStatus::Running, "\"running\""),
            (RepoPullStatus::Completed, "\"completed\""),
            (RepoPullStatus::Failed, "\"failed\""),
            (RepoPullStatus::Cancelled, "\"cancelled\""),
        ];
        for (status, expected) in statuses {
            assert_eq!(serde_json::to_string(&status).unwrap(), expected);
        }
    }

    /// Test that scan_dir_bytes sums files recursively and returns 0 for missing dirs.
    #[test]
    fn test_scan_dir_bytes_nested() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.bin"), vec![0u8; 100]).unwrap();
        let nested = tmp.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("b.bin"), vec![1u8; 50]).unwrap();
        let deep = nested.join("deep");
        std::fs::create_dir(&deep).unwrap();
        std::fs::write(deep.join("c.bin"), vec![2u8; 25]).unwrap();

        assert_eq!(scan_dir_bytes(tmp.path()), 175);
        assert_eq!(scan_dir_bytes(&tmp.path().join("does-not-exist")), 0);
    }

    /// Test that scan_dir_bytes skips symlinks entirely: a symlinked file is
    /// not counted, a symlinked dir is not descended into, and a symlink to
    /// the scanned root (a cycle) cannot loop the walk forever.
    #[test]
    fn test_scan_dir_bytes_skips_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        // A second tempdir OUTSIDE the scanned root, reachable only via symlink.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outer.bin"), vec![1u8; 200]).unwrap();

        let real = tmp.path().join("real.bin");
        std::fs::write(&real, vec![0u8; 100]).unwrap();

        std::os::unix::fs::symlink(&real, tmp.path().join("link-file")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link-dir")).unwrap();
        // A cycle: a directory symlink pointing back at the scanned root.
        std::os::unix::fs::symlink(tmp.path(), tmp.path().join("cycle")).unwrap();

        // Only the real file counts: 100 bytes (not 100 + 100 + 200).
        assert_eq!(scan_dir_bytes(tmp.path()), 100);
    }

    /// Test the full repo-pull job lifecycle on PullState: upsert, get,
    /// running check, cancel, double cancel, unknown id. The relay is the
    /// single host-side executions path, so jobs are childless.
    #[tokio::test]
    async fn test_pull_state_repo_pull_lifecycle() {
        let state = PullState::new(None);
        let job_stderr_arc: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let job = RepoPullJob {
            job_id: "job-1".to_string(),
            repo_id: "owner/repo".to_string(),
            model_id: Some(42),
            dest: PathBuf::from("/tmp/models/owner/repo"),
            total_bytes: Some(1234),
            status: RepoPullStatus::Running,
            error: None,
            cancel_requested: false,
            context_length: None,
            stderr_tail: job_stderr_arc.clone(),
            tamad_job_id: None,
            bytes_done: 0,
        };
        state.upsert_repo_pull(job).await;

        let got = state
            .get_repo_pull("job-1")
            .await
            .expect("job should exist");
        assert_eq!(got.job_id, "job-1");
        assert_eq!(got.repo_id, "owner/repo");
        assert_eq!(got.model_id, Some(42));
        assert_eq!(got.dest, PathBuf::from("/tmp/models/owner/repo"));
        assert_eq!(got.total_bytes, Some(1234));
        assert_eq!(got.status, RepoPullStatus::Running);
        assert!(got.error.is_none());
        assert!(!got.cancel_requested);
        assert!(got.context_length.is_none());
        assert!(
            Arc::ptr_eq(&got.stderr_tail, &job_stderr_arc),
            "cloned job must share the stderr sink Arc"
        );

        assert!(state.repo_pull_running_for("owner/repo").await);
        assert!(!state.repo_pull_running_for("other/repo").await);

        state
            .cancel_repo_pull("job-1")
            .await
            .expect("cancel should succeed");
        let after = state
            .get_repo_pull("job-1")
            .await
            .expect("job should exist");
        assert!(after.cancel_requested, "cancel flag must be set");
        assert_eq!(after.status, RepoPullStatus::Cancelled);
        assert!(!state.repo_pull_running_for("owner/repo").await);

        assert_eq!(
            state.cancel_repo_pull("job-1").await,
            Err("already finished".to_string())
        );
        assert_eq!(
            state.cancel_repo_pull("missing").await,
            Err("not found".to_string())
        );
    }

    // ── start_repo_pull validation tests (no pull host, no network) ────────

    /// Test that an invalid repo id is rejected before any network or
    /// dispatch work happens.
    #[tokio::test]
    async fn test_start_repo_pull_rejects_invalid_repo_id() {
        let state = Arc::new(crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        ));

        let err = start_repo_pull(&state, "../evil", None)
            .await
            .expect_err("invalid repo id must be rejected");
        assert!(
            matches!(err, RepoPullError::InvalidRepoId(_)),
            "expected InvalidRepoId, got: {err:?}"
        );
    }

    /// Test that a second pull for the same repo while one is running is
    /// rejected with DuplicatePull (no payload).
    #[tokio::test]
    async fn test_start_repo_pull_rejects_duplicate() {
        let state = Arc::new(crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        ));
        state
            .pull
            .upsert_repo_pull(RepoPullJob {
                job_id: "job-dup".to_string(),
                repo_id: "owner/repo".to_string(),
                model_id: None,
                dest: PathBuf::from("/tmp/models/owner/repo"),
                total_bytes: None,
                status: RepoPullStatus::Running,
                error: None,
                cancel_requested: false,
                context_length: None,
                stderr_tail: Arc::new(Mutex::new(Vec::new())),
                tamad_job_id: Some("job-tamad".to_string()),
                bytes_done: 0,
            })
            .await;

        let err = start_repo_pull(&state, "owner/repo", None)
            .await
            .expect_err("running duplicate must be rejected");
        assert!(
            matches!(err, RepoPullError::DuplicatePull),
            "expected DuplicatePull, got: {err:?}"
        );
    }

    /// ADR-0010: with a valid repo but no pull host configured, the start
    /// must fail loudly (the proxy never downloads locally, and no local
    /// `hf` binary check remains). HF is mocked so the test is offline.
    #[tokio::test]
    async fn test_start_repo_pull_no_pull_host_configured() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/api/models/owner/repo/revision/main",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sha": "abc123",
                    "siblings": []
                })),
            )
            .mount(&server)
            .await;
        std::env::set_var("HF_ENDPOINT", server.uri());

        let models_tmp = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.general.models_dir = Some(models_tmp.path().to_string_lossy().to_string());
        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            None,
            crate::db::pool::test_dummy_pool(),
        ));

        let err = start_repo_pull(&state, "owner/repo", None)
            .await
            .expect_err("no pull host → must fail");
        std::env::remove_var("HF_ENDPOINT");

        assert!(
            matches!(err, RepoPullError::Upstream(ref msg)
                if msg.contains("no pull host configured")),
            "expected 'no pull host configured', got: {err:?}"
        );
    }

    /// Tamad unreachable at dispatch → clean, actionable failure
    /// (fail-loud: no local fallback, ADR-0010).
    #[tokio::test]
    async fn test_start_repo_pull_tamad_offline_dispatch_fails() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/api/models/happy/repo/revision/main",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sha": "abc123",
                    "siblings": [{"rfilename": "model.safetensors"}]
                })),
            )
            .mount(&server)
            .await;

        let (stub, _down) = make_stub(Vec::new(), true);
        let addr = start_stub(stub).await;
        let models_tmp = tempfile::tempdir().unwrap();
        let xdg_tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());

        let (state, _guard) = state_with_stub(&models_tmp, &xdg_tmp, addr).await;

        let err = start_repo_pull(&state, "happy/repo", None)
            .await
            .expect_err("offline tamad must fail dispatch");
        restore_env();

        assert!(
            matches!(err, RepoPullError::Upstream(ref msg)
                if msg.contains("tamad pull dispatch failed")),
            "expected dispatch failure, got: {err:?}"
        );
    }

    /// Test that a repo that 404s on HF is reported as RepoNotFound.
    #[tokio::test]
    async fn test_start_repo_pull_repo_not_found() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                r"^/api/models/owner/missing",
            ))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let state = Arc::new(crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        ));
        std::env::set_var("HF_ENDPOINT", server.uri());

        let err = start_repo_pull(&state, "owner/missing", None)
            .await
            .expect_err("unknown repo must be rejected");
        restore_env();

        assert!(
            matches!(err, RepoPullError::RepoNotFound(ref msg)
                if msg.contains("not found on HuggingFace")),
            "expected RepoNotFound, got: {err:?}"
        );
    }

    /// Test that a non-404 (500) error from the HF info endpoint maps to
    /// `RepoPullError::Upstream` (not `RepoNotFound`).
    #[tokio::test]
    async fn test_start_repo_pull_non_404_info_error_upstream() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/api/models/broken/repo/revision/main",
            ))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let state = Arc::new(crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        ));
        std::env::set_var("HF_ENDPOINT", server.uri());

        let err = start_repo_pull(&state, "broken/repo", None)
            .await
            .expect_err("a 500 from info() must surface as an error");
        restore_env();

        assert!(
            matches!(err, RepoPullError::Upstream(_)),
            "expected Upstream for a non-404 info error, got: {err:?}"
        );
    }

    // ── relay tests (dispatch + StreamJob convergence, pull host) ──────────

    /// Full relayed lifecycle: the proxy dispatches `PullModel(repo_pull=true)`
    /// to the pull host and relays `StreamJob` progress + terminal success;
    /// the completion phase (config.json parse + HF metadata + model row)
    /// runs PROXY-side from the files the pull host placed in
    /// `<models_dir>/<repo>` (single-host layout). The `hf` CLI call happens
    /// on the host — the proxy never spawns it.
    #[tokio::test]
    async fn test_repo_pull_relay_success_updates_model_row() {
        let server = wiremock::MockServer::start().await;
        // blobs endpoint (tama's own URL helper) 404s → total_bytes soft-fails
        // to None (indeterminate).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/models/happy/repo"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // hf-hub 0.5 info() hits /api/models/{repo}/revision/{revision}.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/api/models/happy/repo/revision/main",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sha": "abc123",
                    "siblings": [{"rfilename": "model.safetensors"}]
                })),
            )
            .mount(&server)
            .await;

        let models_root = tempfile::tempdir().unwrap();
        let xdg_tmp = tempfile::tempdir().unwrap();

        // The "pull host" (stub tamad) has already written the repo contents
        // into <models_dir>/<repo> before the terminal success event —
        // the relay only relays; it never downloads.
        let dest = crate::models::repo_path(models_root.path(), "happy/repo");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(
            dest.join("config.json"),
            r#"{"architectures": ["Qwen3ForCausalLM"], "max_position_embeddings": 32768, "num_hidden_layers": 48, "quantization_config": {"quant_method": "fp8"}}"#,
        )
        .unwrap();
        std::fs::write(dest.join("model.safetensors"), b"fake-weights").unwrap();

        std::env::set_var("HF_ENDPOINT", server.uri());

        let events = vec![
            job_event_bytes(JOB_ID, 40, "downloading", "running", 50_000, 120_000),
            job_event_bytes(JOB_ID, 90, "downloading", "running", 108_000, 120_000),
            terminal_success(JOB_ID, r#"{"dir": "ok", "ok": true}"#),
        ];
        let (stub, _down) = make_stub(events, false);
        let addr = start_stub(stub.clone()).await;

        let (state, guard) = state_with_stub(&models_root, &xdg_tmp, addr).await;

        // The wizard pre-creates the stub model row before starting the pull.
        let model_id = insert_stub_model(&state, "happy/repo").await;

        let start = start_repo_pull(&state, "happy/repo", Some(model_id))
            .await
            .expect("start should succeed");
        assert!(start.job_id.starts_with("hfrepo-"));
        assert!(
            start.total_bytes.is_none(),
            "stats endpoint 404s → indeterminate total"
        );

        // The dispatch must carry the repo-pull shape.
        let requests = stub.pull_requests.lock().await;
        assert_eq!(requests.len(), 1, "exactly one PullModel dispatch");
        let req = &requests[0];
        assert!(req.repo_pull, "dispatch must be a whole-repo pull");
        assert_eq!(req.repo_id, "happy/repo");
        assert_eq!(
            req.dest_dir,
            dest.to_string_lossy().to_string(),
            "dest is the proxy's models layout (single-host convention)"
        );

        let final_job = wait_terminal(&state, &start.job_id).await;
        assert_eq!(final_job.status, RepoPullStatus::Completed);
        assert_eq!(final_job.context_length, Some(32768));
        assert!(final_job.error.is_none());
        assert_eq!(
            final_job.bytes_done, 108_000,
            "progress bytes must be mirrored from the relay"
        );
        assert_eq!(final_job.total_bytes, Some(120_000));

        // The status DTO serves the relay-mirrored progress (single host:
        // the local scan sees the same bytes and the max wins).
        let dto = state
            .get_repo_pull_status(&start.job_id)
            .await
            .expect("status DTO must exist");
        assert_eq!(dto.status, "completed");
        assert_eq!(
            dto.bytes_done, 108_000,
            "relayed progress (108k) beats the local scan (162 bytes)"
        );
        assert_eq!(dto.context_length, Some(32768));

        // The model row got the merged completion metadata + quant (Postgres).
        let pool = state.db_pool();
        let record = crate::db::queries::get_model_config(&pool, model_id)
            .await
            .unwrap()
            .expect("model row should exist");
        assert_eq!(record.hf_format.as_deref(), Some("transformers"));
        assert_eq!(
            record.hf_architecture_type.as_deref(),
            Some("Qwen3ForCausalLM")
        );
        assert_eq!(record.hf_context_length, Some(32768));
        assert_eq!(record.hf_num_layers, Some(48));
        assert_eq!(record.selected_quant.as_deref(), Some("fp8"));

        restore_env();
        guard.finish().await;
    }

    /// Regression: a FAILED relayed download must not mark the model row as
    /// configured. `hf` writes config.json EARLY (before the weights), so a
    /// failed pull can still leave a valid config.json in dest — the
    /// completion step must be gated on success, and the host error must
    /// surface through the job error.
    #[tokio::test]
    async fn test_repo_pull_relay_failure_keeps_model_row_untouched() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/models/fail/repo"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/api/models/fail/repo/revision/main",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sha": "abc123",
                    "siblings": [{"rfilename": "model.safetensors"}]
                })),
            )
            .mount(&server)
            .await;

        let models_root = tempfile::tempdir().unwrap();
        let xdg_tmp = tempfile::tempdir().unwrap();

        // config.json WAS written on the host before the failure (hf writes
        // it early) — it must not leak into the DB through a failed job.
        let dest = crate::models::repo_path(models_root.path(), "fail/repo");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(
            dest.join("config.json"),
            r#"{"architectures": ["Qwen3ForCausalLM"], "max_position_embeddings": 32768, "num_hidden_layers": 48, "quantization_config": {"quant_method": "fp8"}}"#,
        )
        .unwrap();

        std::env::set_var("HF_ENDPOINT", server.uri());

        let events = vec![job_event_failed(JOB_ID, "connection reset by peer")];
        let (stub, _down) = make_stub(events, false);
        let addr = start_stub(stub.clone()).await;

        let (state, guard) = state_with_stub(&models_root, &xdg_tmp, addr).await;
        let model_id = insert_stub_model(&state, "fail/repo").await;

        let start = start_repo_pull(&state, "fail/repo", Some(model_id))
            .await
            .expect("start should succeed");

        let final_job = wait_terminal(&state, &start.job_id).await;
        assert_eq!(final_job.status, RepoPullStatus::Failed);
        assert!(
            final_job
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("connection reset"),
            "failed job must carry the host error as error: {final_job:?}"
        );
        assert!(final_job.context_length.is_none());

        // The model row must be UNCHANGED: no format, no quant, no
        // context length — a failed download is not a configured model.
        let pool = state.db_pool();
        let record = crate::db::queries::get_model_config(&pool, model_id)
            .await
            .unwrap()
            .expect("model row should exist");
        assert!(record.hf_format.is_none());
        assert!(record.hf_architecture_type.is_none());
        assert!(record.hf_context_length.is_none());
        assert!(record.hf_num_layers.is_none());
        assert!(
            record.selected_quant.is_none(),
            "failed pull must not touch the model row"
        );

        restore_env();
        guard.finish().await;
    }

    /// Remote-host invariant: the relayed repo's weights + config.json live
    /// on the tamad's disk and are ABSENT on the proxy's. The relay must
    /// still complete the job and update the model row from HF metadata
    /// alone — the config.json read is strictly best-effort (single-host
    /// layout convenience), never a requirement.
    #[tokio::test]
    async fn test_repo_pull_relay_success_without_local_files() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/models/happy/repo"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/api/models/happy/repo/revision/main",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sha": "abc123",
                    "siblings": [{"rfilename": "model.safetensors"}]
                })),
            )
            .mount(&server)
            .await;

        let models_root = tempfile::tempdir().unwrap();
        let xdg_tmp = tempfile::tempdir().unwrap();
        // No local files written: the "pull host" is remote.

        std::env::set_var("HF_ENDPOINT", server.uri());

        let events = vec![
            job_event_bytes(JOB_ID, 40, "downloading", "running", 50_000, 120_000),
            terminal_success(
                JOB_ID,
                r#"{"dir": "/remote/models/happy/repo", "ok": true}"#,
            ),
        ];
        let (stub, _down) = make_stub(events, false);
        let addr = start_stub(stub.clone()).await;
        let (state, guard) = state_with_stub(&models_root, &xdg_tmp, addr).await;
        let model_id = insert_stub_model(&state, "happy/repo").await;

        let start = start_repo_pull(&state, "happy/repo", Some(model_id))
            .await
            .expect("start should succeed");

        let final_job = wait_terminal(&state, &start.job_id).await;
        assert_eq!(
            final_job.status,
            RepoPullStatus::Completed,
            "remote-host pull must complete without local files: {:?}",
            final_job.error
        );
        assert!(final_job.error.is_none());
        assert_eq!(
            final_job.context_length, None,
            "no local config.json → no context length (HF metadata only)"
        );
        assert_eq!(
            final_job.bytes_done, 50_000,
            "relayed progress is the source of truth on a remote host"
        );

        // Model row: HF metadata merged (format defaults to transformers),
        // nothing from the (absent) local config.json.
        let pool = state.db_pool();
        let record = crate::db::queries::get_model_config(&pool, model_id)
            .await
            .unwrap()
            .expect("model row should exist");
        assert_eq!(record.hf_format.as_deref(), Some("transformers"));
        assert!(record.hf_context_length.is_none());
        assert!(record.selected_quant.is_none());

        restore_env();
        guard.finish().await;
    }

    /// A terminal "succeeded" event whose payload declares `ok: false` is a
    /// contract violation (the host returns Err on failure instead): the
    /// relay consumes the host's payload and fails the job rather than
    /// trusting the status flag alone.
    #[tokio::test]
    async fn test_repo_pull_relay_success_payload_ok_false_fails() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/models/happy/repo"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/api/models/happy/repo/revision/main",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sha": "abc123",
                    "siblings": [{"rfilename": "model.safetensors"}]
                })),
            )
            .mount(&server)
            .await;

        let models_root = tempfile::tempdir().unwrap();
        let xdg_tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());

        let events = vec![terminal_success(
            JOB_ID,
            r#"{"dir": "/remote/models/happy/repo", "ok": false}"#,
        )];
        let (stub, _down) = make_stub(events, false);
        let addr = start_stub(stub.clone()).await;
        let (state, guard) = state_with_stub(&models_root, &xdg_tmp, addr).await;
        let model_id = insert_stub_model(&state, "happy/repo").await;

        let start = start_repo_pull(&state, "happy/repo", Some(model_id))
            .await
            .expect("start should succeed");

        let final_job = wait_terminal(&state, &start.job_id).await;
        assert_eq!(
            final_job.status,
            RepoPullStatus::Failed,
            "ok=false payload must fail the job, error: {:?}",
            final_job.error
        );
        assert!(
            final_job
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("ok=false"),
            "job error must mention the payload contradiction, got: {:?}",
            final_job.error
        );

        // A contradictory success must not mark the model row configured.
        let pool = state.db_pool();
        let record = crate::db::queries::get_model_config(&pool, model_id)
            .await
            .unwrap()
            .expect("model row should exist");
        assert!(record.hf_format.is_none());
        assert!(
            record.selected_quant.is_none(),
            "contradictory success must not touch the model row"
        );

        restore_env();
        guard.finish().await;
    }
    // ── finish_repo_pull terminal-decision tests ───────────────────────────

    /// Test that a failed relayed job whose mirrored error is in the
    /// stderr-tail sink surfaces that error (not an exit-code message).
    #[tokio::test]
    async fn test_finish_repo_pull_failure_records_error_tail() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        std::env::set_var("HF_ENDPOINT", server.uri());
        let state = crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        );
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(b"boom\n".to_vec()));
        state
            .pull
            .upsert_repo_pull(RepoPullJob {
                job_id: "job-fail".to_string(),
                repo_id: "owner/missing".to_string(),
                model_id: None,
                dest: PathBuf::from("/tmp/nowhere"),
                total_bytes: None,
                status: RepoPullStatus::Running,
                error: None,
                cancel_requested: false,
                context_length: None,
                stderr_tail: sink,
                tamad_job_id: Some(JOB_ID.to_string()),
                bytes_done: 0,
            })
            .await;

        finish_repo_pull(&state, "job-fail", Some(1)).await;

        let job = state
            .pull
            .get_repo_pull("job-fail")
            .await
            .expect("job must still exist");
        assert_eq!(job.status, RepoPullStatus::Failed);
        assert_eq!(job.error.as_deref(), Some("boom"));
        restore_env();
    }

    /// Test that a failed job with an EMPTY mirrored error falls back to a
    /// synthetic message containing the exit code.
    #[tokio::test]
    async fn test_finish_repo_pull_failure_empty_error_fallback() {
        let state = crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        );
        state
            .pull
            .upsert_repo_pull(RepoPullJob {
                job_id: "job-fail-empty".to_string(),
                repo_id: "owner/missing".to_string(),
                model_id: None,
                dest: PathBuf::from("/tmp/nowhere"),
                total_bytes: None,
                status: RepoPullStatus::Running,
                error: None,
                cancel_requested: false,
                context_length: None,
                stderr_tail: Arc::new(Mutex::new(Vec::new())),
                tamad_job_id: None,
                bytes_done: 0,
            })
            .await;

        finish_repo_pull(&state, "job-fail-empty", Some(2)).await;

        let job = state
            .pull
            .get_repo_pull("job-fail-empty")
            .await
            .expect("job must still exist");
        assert_eq!(job.status, RepoPullStatus::Failed);
        assert_eq!(
            job.error.as_deref(),
            Some("hf download exited with code 2"),
            "empty error tail must fall back to a code-based message"
        );
    }

    /// Test that a cancel request wins over a clean terminal event: the
    /// final status is Cancelled with no error.
    #[tokio::test]
    async fn test_finish_repo_pull_cancel_requested() {
        let state = crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        );
        state
            .pull
            .upsert_repo_pull(RepoPullJob {
                job_id: "job-cancelled".to_string(),
                repo_id: "owner/repo".to_string(),
                model_id: None,
                dest: PathBuf::from("/tmp/nowhere"),
                total_bytes: None,
                status: RepoPullStatus::Running,
                error: None,
                cancel_requested: true,
                context_length: None,
                stderr_tail: Arc::new(Mutex::new(b"killed\n".to_vec())),
                tamad_job_id: None,
                bytes_done: 0,
            })
            .await;

        finish_repo_pull(&state, "job-cancelled", Some(0)).await;

        let job = state
            .pull
            .get_repo_pull("job-cancelled")
            .await
            .expect("job must still exist");
        assert_eq!(job.status, RepoPullStatus::Cancelled);
        assert!(job.error.is_none());
    }

    /// Regression: a CANCELLED pull must also skip the completion step, even
    /// when the terminal event reports success (cancel_requested wins over
    /// the clean terminal). dest contains a config.json, as the host would
    /// have written it before the cancel — the model row must stay untouched.
    #[tokio::test]
    async fn test_finish_repo_pull_cancelled_skips_db_update() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        std::env::set_var("HF_ENDPOINT", server.uri());

        let xdg_tmp = tempfile::tempdir().unwrap();
        let guard = crate::testing::postgres::with_schema().await;
        let pool = std::sync::Arc::new(guard.pool.clone());
        let state = crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            Some(xdg_tmp.path().to_path_buf()),
            pool.clone(),
        );
        let model_id: i64 = {
            let record = crate::db::queries::ModelConfigRecord {
                repo_id: "owner/repo".to_string(),
                backend: "llama_cpp".to_string(),
                ..Default::default()
            };
            crate::db::queries::upsert_model_config(&pool, &record)
                .await
                .unwrap()
        };

        // dest with a config.json, as the host would have written it before
        // the cancel.
        let dest_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dest_dir.path().join("config.json"),
            r#"{"architectures": ["TfArch"], "max_position_embeddings": 32768, "quantization_config": {"quant_method": "fp8"}}"#,
        )
        .unwrap();

        state
            .pull
            .upsert_repo_pull(RepoPullJob {
                job_id: "job-cancelled-db".to_string(),
                repo_id: "owner/repo".to_string(),
                model_id: Some(model_id),
                dest: dest_dir.path().to_path_buf(),
                total_bytes: None,
                status: RepoPullStatus::Running,
                error: None,
                cancel_requested: true,
                context_length: None,
                stderr_tail: Arc::new(Mutex::new(b"killed\n".to_vec())),
                tamad_job_id: None,
                bytes_done: 0,
            })
            .await;

        finish_repo_pull(&state, "job-cancelled-db", Some(0)).await;

        let job = state
            .pull
            .get_repo_pull("job-cancelled-db")
            .await
            .expect("job must still exist");
        assert_eq!(job.status, RepoPullStatus::Cancelled);
        assert!(job.error.is_none());
        assert!(job.context_length.is_none());

        // The model row must be UNCHANGED despite the config.json in dest.
        let record = crate::db::queries::get_model_config(&pool, model_id)
            .await
            .unwrap()
            .expect("model row should exist");
        assert!(record.hf_format.is_none());
        assert!(record.hf_architecture_type.is_none());
        assert!(record.hf_context_length.is_none());
        assert!(record.hf_num_layers.is_none());
        assert!(
            record.selected_quant.is_none(),
            "cancelled pull must not touch the model row"
        );
        restore_env();
        guard.finish().await;
    }

    /// Test that a signal/death-style terminal with an empty error tail
    /// produces a Failed job with the fallback message.
    #[tokio::test]
    async fn test_finish_repo_pull_death_message() {
        let state = crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        );
        state
            .pull
            .upsert_repo_pull(RepoPullJob {
                job_id: "job-signal".to_string(),
                repo_id: "owner/repo".to_string(),
                model_id: None,
                dest: PathBuf::from("/tmp/nowhere"),
                total_bytes: None,
                status: RepoPullStatus::Running,
                error: None,
                cancel_requested: false,
                context_length: None,
                stderr_tail: Arc::new(Mutex::new(Vec::new())),
                tamad_job_id: None,
                bytes_done: 0,
            })
            .await;

        // The relay passes None when no exit status is known.
        finish_repo_pull(&state, "job-signal", None).await;

        let job = state
            .pull
            .get_repo_pull("job-signal")
            .await
            .expect("job must still exist");
        assert_eq!(job.status, RepoPullStatus::Failed);
        assert_eq!(
            job.error.as_deref(),
            Some("hf download exited abnormally"),
            "no status with empty error tail must fall back to the abnormal-exit message"
        );
        assert!(job.context_length.is_none());
    }

    /// Test that a successful relayed pull started by an API-only caller
    /// (no pre-created model row, `model_id: None`) still completes: the DB
    /// step is skipped entirely, so `context_length` stays None and no model
    /// row is touched — even when dest holds a config.json.
    #[tokio::test]
    async fn test_finish_repo_pull_no_model_id_skips_db() {
        let server = wiremock::MockServer::start().await;
        // The completion metadata fetch soft-fails against a 404-only mock.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        std::env::set_var("HF_ENDPOINT", server.uri());

        let xdg_tmp = tempfile::tempdir().unwrap();
        let state = crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            Some(xdg_tmp.path().to_path_buf()),
            crate::db::pool::test_dummy_pool(),
        );

        // dest holds a config.json, but with model_id = None nothing may be
        // written to any model row.
        let dest_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dest_dir.path().join("config.json"),
            r#"{"max_position_embeddings": 123}"#,
        )
        .unwrap();

        state
            .pull
            .upsert_repo_pull(RepoPullJob {
                job_id: "job-no-model".to_string(),
                repo_id: "owner/repo".to_string(),
                model_id: None,
                dest: dest_dir.path().to_path_buf(),
                total_bytes: None,
                status: RepoPullStatus::Running,
                error: None,
                cancel_requested: false,
                context_length: None,
                stderr_tail: Arc::new(Mutex::new(Vec::new())),
                tamad_job_id: None,
                bytes_done: 0,
            })
            .await;

        finish_repo_pull(&state, "job-no-model", Some(0)).await;

        let job = state
            .pull
            .get_repo_pull("job-no-model")
            .await
            .expect("job must still exist");
        assert_eq!(job.status, RepoPullStatus::Completed);
        assert!(job.error.is_none());
        assert!(
            job.context_length.is_none(),
            "no model row → no completion step → no context length"
        );
        restore_env();
    }

    /// Cancel through the PUBLIC delegate while a relay is in flight: the
    /// local job is flagged and marked cancelled immediately (the relay
    /// converges when the host sends its terminal `cancelled` event), and a
    /// second cancel is rejected.
    #[tokio::test]
    async fn test_cancel_repo_public_delegate_flags_job() {
        let state = Arc::new(crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        ));
        state
            .pull
            .upsert_repo_pull(RepoPullJob {
                job_id: "job-cancel-pub".to_string(),
                repo_id: "owner/repo".to_string(),
                model_id: None,
                dest: PathBuf::from("/tmp/nowhere"),
                total_bytes: None,
                status: RepoPullStatus::Running,
                error: None,
                cancel_requested: false,
                context_length: None,
                stderr_tail: Arc::new(Mutex::new(Vec::new())),
                tamad_job_id: Some(JOB_ID.to_string()),
                bytes_done: 0,
            })
            .await;

        state
            .cancel_repo_pull("job-cancel-pub")
            .await
            .expect("cancel should succeed");

        let job = state
            .pull
            .get_repo_pull("job-cancel-pub")
            .await
            .expect("job must exist");
        assert!(job.cancel_requested);
        assert_eq!(job.status, RepoPullStatus::Cancelled);

        assert_eq!(
            state.cancel_repo_pull("job-cancel-pub").await,
            Err("already finished".to_string())
        );
    }
}
