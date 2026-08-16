//! In-memory job state for whole-repo `hf` CLI pulls (see ADR-0007).
//!
//! Whole-repo safetensors downloads are tracked as in-memory jobs (no DB rows,
//! not in the Downloads Center). The child process is shared through
//! `Arc<Mutex<Option<Child>>>` with BRIEF lock holds only: the wait-loop uses
//! non-blocking `try_wait` and sleeps outside the lock, and cancellation takes
//! a brief lock only for `kill()`. No code path holds the child lock or the
//! job-map lock across a long `.await`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::models::pull::HfModelMetadata;

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
    pub(crate) stderr_tail: Arc<Mutex<Vec<u8>>>,
    /// Shared child handle (see the concurrency model above).
    pub(crate) child: Arc<Mutex<Option<tokio::process::Child>>>,
}

/// Recursively sum the sizes of all regular files under `dir`.
///
/// Symlinks are skipped entirely — never counted, never descended into —
/// which also makes the walk immune to symlink cycles (e.g. a directory
/// symlink pointing at an ancestor). Returns 0 if the directory does not
/// exist.
pub(crate) fn scan_dir_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            // Never follow symlinks (file_type does not): a symlinked dir to
            // an ancestor would loop forever, and `hf download` writes only
            // regular files.
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = path.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

/// Check that the `hf` CLI is available on the host.
///
/// Returns an install hint when the binary is missing or errors.
pub(crate) async fn check_hf_binary() -> Result<(), String> {
    const INSTALL_HINT: &str = "hf CLI not found. Install with: pip install -U huggingface_hub";
    match tokio::process::Command::new("hf")
        .arg("--version")
        .output()
        .await
    {
        Ok(output) if output.status.success() => Ok(()),
        _ => Err(INSTALL_HINT.to_string()),
    }
}

/// Spawn the `hf` CLI to download a whole repo into `dest`.
///
/// `binary` is injected (dependency injection) so unit tests can pass a stub
/// executable instead of requiring a real `hf` install. When `hf_token` is
/// `Some`, it is passed to the child via the `HF_TOKEN` environment variable.
pub(crate) async fn spawn_hf_download(
    binary: &str,
    repo_id: &str,
    dest: &Path,
    hf_token: Option<&str>,
) -> Result<tokio::process::Child, String> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args([
        "download",
        repo_id,
        "--local-dir",
        dest.to_string_lossy().as_ref(),
    ]);
    if let Some(token) = hf_token {
        cmd.env("HF_TOKEN", token);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    cmd.spawn()
        .map_err(|e| format!("failed to spawn hf CLI: {e}"))
}

/// Spawn a task that reads the child's stderr in bounded 4 KiB chunks and
/// keeps only the last 4096 raw bytes in `sink`. The task self-terminates
/// when stderr hits EOF.
///
/// The sink holds RAW BYTES: the cap drains from the front of a `Vec<u8>` at
/// arbitrary byte offsets (always safe — `Vec::drain` cannot panic on a
/// mid-character offset the way `String::drain` can), and decoding happens
/// exactly once at read time (`stderr_tail_str`), so a multi-byte char
/// straddling a chunk boundary stays whole instead of becoming one U+FFFD
/// per chunk. A char straddling the CAP boundary loses at most its leading
/// byte and decodes to a single U+FFFD.
///
/// Chunks (not lines) are the unit of accumulation: the tail is capped after
/// every chunk, so a single huge line — or `\r`-only progress output without
/// newlines — can never grow an unbounded per-line buffer.
pub(crate) fn start_stderr_reader(
    stderr: tokio::process::ChildStderr,
    sink: Arc<Mutex<Vec<u8>>>,
) -> tokio::task::JoinHandle<()> {
    const TAIL_CAP: usize = 4096;
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = [0u8; TAIL_CAP];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut tail = sink.lock().await;
                    tail.extend_from_slice(&buf[..n]);
                    let overflow = tail.len().saturating_sub(TAIL_CAP);
                    if overflow > 0 {
                        tail.drain(0..overflow);
                    }
                }
            }
        }
    })
}

/// Return the captured stderr tail (trailing newlines stripped) if non-empty.
///
/// The capped raw bytes are decoded exactly once, here, at read time — a
/// multi-byte char split by the cap boundary yields at most one U+FFFD.
pub(crate) async fn stderr_tail_str(sink: &Arc<Mutex<Vec<u8>>>) -> Option<String> {
    let tail = sink.lock().await;
    let decoded = String::from_utf8_lossy(&tail);
    let trimmed = decoded.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Error for starting a whole-repo `hf` CLI pull.
#[derive(Debug, thiserror::Error)]
pub enum RepoPullError {
    /// The `hf` CLI binary is missing. Payload is the install hint.
    #[error("{0}")]
    HfBinaryMissing(String),
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

/// Sync part of repo-pull completion: apply merged metadata to the model row.
///
/// MUST be synchronous — `rusqlite::Connection` is `!Sync` and this runs on an
/// owned, short-lived connection. Precedence: existing DB values survive
/// (COALESCE inside `update_model_config_hf_metadata`); where the DB and `base`
/// (HF metadata) are both NULL, `meta_tf` (config.json) fills the gap, with
/// `base` winning over `meta_tf`. `hf_format` defaults to "transformers" when
/// unknown. Quant is set unconditionally from `quantization_method`.
///
/// Returns the context length from config.json (for the job's
/// `context_length` field), if any.
pub(crate) fn apply_repo_pull_completion_with_meta(
    conn: &rusqlite::Connection,
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

    crate::models::update::update_model_config_hf_metadata(conn, model_id, &meta)?;

    if let Some(qm) = meta_tf.and_then(|tf| tf.quantization_method.as_deref()) {
        crate::models::update::update_model_config_quant(conn, model_id, qm)?;
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
/// Order matters — `rusqlite::Connection` is `!Sync`, so no `&Connection` may
/// cross an `.await`:
/// 1. re-read the job fields under a brief lock,
/// 2. compute the terminal decision (status + error),
/// 3. (Completed only) parse config.json (sync fs, soft-fail — a repo without
///    it completes), fetch HF metadata (network — no connection open), then
///    update the model row on an OWNED, short-lived connection (sync),
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
        // Sync fs read — soft-fail.
        let meta_tf = crate::models::transformers::parse_transformers_metadata(&dest).ok();

        // Network — no connection open yet.
        let base = fetch_completion_metadata(&repo_id).await;

        // DB step — owned connection, used synchronously, dropped at end of scope.
        if let Some(model_id) = model_id {
            match state.open_db() {
                Some(conn) => {
                    match apply_repo_pull_completion_with_meta(
                        &conn,
                        model_id,
                        &base,
                        meta_tf.as_ref(),
                        &dest,
                    ) {
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
                None => {
                    tracing::warn!(
                        "repo-pull completion: no database available for '{}', \
                         skipping model row update",
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

/// Start a whole-repo `hf` CLI pull job.
///
/// Validation order: repo id → duplicate → `hf` binary → repo existence →
/// (soft) byte totals → destination → spawn → register → wait-loop.
///
/// `state` is an `Arc` so the spawned wait-loop can clone it and outlive the
/// caller. `model_id` is the pre-created stub row (`None` = API-only caller,
/// no DB update on completion).
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

    // The host must have the `hf` CLI.
    if let Err(hint) = check_hf_binary().await {
        return Err(RepoPullError::HfBinaryMissing(hint));
    }

    // The repo must exist on HuggingFace. A 404 / "not found" is a missing
    // repo; anything else is an upstream/network error.
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

    // Spawn the child (stderr piped into a capped tail).
    let mut child = spawn_hf_download(
        "hf",
        repo_id,
        &dest,
        crate::models::pull::get_hf_token().as_deref(),
    )
    .await
    .map_err(RepoPullError::Upstream)?;

    let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    if let Some(stderr) = child.stderr.take() {
        let _reader = start_stderr_reader(stderr, sink.clone());
    }

    let child_handle = Arc::new(Mutex::new(Some(child)));
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
            stderr_tail: sink,
            child: child_handle.clone(),
        })
        .await;

    // Wait-loop: brief try_wait under the lock, 500 ms sleep OUTSIDE it.
    // Spawned with clones of the Arcs it needs; the stderr reader task
    // self-terminates on stderr EOF (no JoinHandle bookkeeping here).
    let state_clone = Arc::clone(state);
    let wait_job_id = job_id.clone();
    tokio::spawn(async move {
        let exit_status = loop {
            let got = {
                let mut g = child_handle.lock().await;
                g.as_mut().and_then(|c| c.try_wait().ok()).flatten()
            };
            if let Some(status) = got {
                break status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        };
        finish_repo_pull(&state_clone, &wait_job_id, exit_status.code()).await;
    });

    Ok(RepoPullStart {
        job_id,
        total_bytes,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, OpenResult};
    use crate::models::pull::HfModelMetadata;
    use crate::proxy::state::pull::PullState;
    use std::os::unix::fs::PermissionsExt;

    /// Helper: open a migrated in-memory DB and insert a model_configs row,
    /// returning the (conn, model_id) pair.
    fn conn_with_model_row(extra_sql: &str) -> (rusqlite::Connection, i64) {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES ('test/repo', 'llama_cpp')",
            [],
        )
        .unwrap();
        let model_id: i64 = conn
            .query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
            .unwrap();
        if !extra_sql.is_empty() {
            conn.execute(extra_sql, []).unwrap();
        }
        (conn, model_id)
    }

    /// Completion-relevant columns of a model_configs row.
    #[derive(Debug, PartialEq)]
    struct CompletionRow {
        hf_format: Option<String>,
        hf_architecture_type: Option<String>,
        hf_context_length: Option<i64>,
        hf_num_layers: Option<i64>,
        selected_quant: Option<String>,
    }

    /// Helper: read the completion-relevant columns for `model_id`.
    fn completion_columns(conn: &rusqlite::Connection, model_id: i64) -> CompletionRow {
        conn.query_row(
            "SELECT hf_format, hf_architecture_type, hf_context_length, hf_num_layers, \
             selected_quant FROM model_configs WHERE id = ?",
            [model_id],
            |r| {
                Ok(CompletionRow {
                    hf_format: r.get(0)?,
                    hf_architecture_type: r.get(1)?,
                    hf_context_length: r.get(2)?,
                    hf_num_layers: r.get(3)?,
                    selected_quant: r.get(4)?,
                })
            },
        )
        .unwrap()
    }

    /// Helper: tempdir dest with a config.json fixture.
    fn dest_with_config(json: &str) -> tempfile::TempDir {
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(dest.path().join("config.json"), json).unwrap();
        dest
    }

    /// Test the SYNC completion seam: with a config.json present, transformers
    /// metadata fills gaps in the base HF metadata, the model row is updated
    /// (COALESCE), quant is set from quantization_method, and the context
    /// length is returned. No network, no async.
    #[test]
    fn test_apply_repo_pull_completion_with_meta() {
        let (conn, model_id) = conn_with_model_row("");
        let dest = dest_with_config(
            r#"{"architectures": ["Qwen3ForCausalLM"], "max_position_embeddings": 32768, "num_hidden_layers": 48, "quantization_config": {"quant_method": "fp8"}}"#,
        );
        let meta_tf =
            crate::models::transformers::parse_transformers_metadata(dest.path()).unwrap();

        let context_length = apply_repo_pull_completion_with_meta(
            &conn,
            model_id,
            &HfModelMetadata::default(),
            Some(&meta_tf),
            dest.path(),
        )
        .unwrap();

        assert_eq!(context_length, Some(32768));

        let row = completion_columns(&conn, model_id);
        assert_eq!(row.hf_format.as_deref(), Some("transformers"));
        assert_eq!(
            row.hf_architecture_type.as_deref(),
            Some("Qwen3ForCausalLM")
        );
        assert_eq!(row.hf_context_length, Some(32768));
        assert_eq!(row.hf_num_layers, Some(48));
        assert_eq!(row.selected_quant.as_deref(), Some("fp8"));
    }

    /// Test that a repo WITHOUT config.json still completes: no transformers
    /// metadata, only hf_format='transformers' is written, no quant, and the
    /// context length is None.
    #[test]
    fn test_apply_repo_pull_completion_with_meta_no_config_json() {
        let (conn, model_id) = conn_with_model_row("");
        let dest = tempfile::tempdir().unwrap(); // no config.json

        let context_length = apply_repo_pull_completion_with_meta(
            &conn,
            model_id,
            &HfModelMetadata::default(),
            None,
            dest.path(),
        )
        .unwrap();

        assert_eq!(context_length, None);

        let row = completion_columns(&conn, model_id);
        assert_eq!(row.hf_format.as_deref(), Some("transformers"));
        assert!(row.hf_architecture_type.is_none());
        assert!(row.hf_context_length.is_none());
        assert!(row.hf_num_layers.is_none());
        assert!(row.selected_quant.is_none());
    }

    /// Test COALESCE semantics: existing non-NULL DB values are preserved when
    /// the incoming (merged) metadata has them as NULL, while NULL DB columns
    /// are filled from transformers metadata.
    ///
    /// config.json omits `architectures` and `max_position_embeddings` so the
    /// merged incoming value for those fields stays NULL.
    #[test]
    fn test_apply_repo_pull_completion_preserves_existing_db_values() {
        let (conn, model_id) = conn_with_model_row(
            "UPDATE model_configs SET hf_base_model = 'existing-base', \
             hf_architecture_type = 'ExistingArch', hf_context_length = 555 \
             WHERE repo_id = 'test/repo'",
        );
        let dest = dest_with_config(r#"{"num_hidden_layers": 48}"#);
        let meta_tf =
            crate::models::transformers::parse_transformers_metadata(dest.path()).unwrap();

        let context_length = apply_repo_pull_completion_with_meta(
            &conn,
            model_id,
            &HfModelMetadata::default(),
            Some(&meta_tf),
            dest.path(),
        )
        .unwrap();
        assert_eq!(context_length, None);

        let row = completion_columns(&conn, model_id);
        // Existing non-NULL DB values survive (incoming is NULL).
        assert_eq!(row.hf_architecture_type.as_deref(), Some("ExistingArch"));
        assert_eq!(row.hf_context_length, Some(555));
        // NULL DB columns are filled.
        assert_eq!(row.hf_format.as_deref(), Some("transformers"));
        assert_eq!(row.hf_num_layers, Some(48));
    }

    /// Test that a non-NULL incoming value overwrites the existing DB value
    /// (COALESCE picks the first non-NULL). Here config.json supplies an
    /// architecture that replaces the stored one.
    #[test]
    fn test_apply_repo_pull_completion_incoming_overwrites_db() {
        let (conn, model_id) = conn_with_model_row(
            "UPDATE model_configs SET hf_architecture_type = 'ExistingArch' \
             WHERE repo_id = 'test/repo'",
        );
        let dest =
            dest_with_config(r#"{"architectures": ["TfArch"], "max_position_embeddings": 100}"#);
        let meta_tf =
            crate::models::transformers::parse_transformers_metadata(dest.path()).unwrap();

        apply_repo_pull_completion_with_meta(
            &conn,
            model_id,
            &HfModelMetadata::default(),
            Some(&meta_tf),
            dest.path(),
        )
        .unwrap();

        let row = completion_columns(&conn, model_id);
        assert_eq!(row.hf_architecture_type.as_deref(), Some("TfArch"));
        assert_eq!(row.hf_context_length, Some(100));
    }

    /// Test that base (HF) metadata takes precedence over transformers metadata
    /// when both supply the same field.
    #[test]
    fn test_apply_repo_pull_completion_base_meta_wins_over_transformers() {
        let (conn, model_id) = conn_with_model_row("");
        let dest = dest_with_config(
            r#"{"architectures": ["TfArch"], "max_position_embeddings": 100, "num_hidden_layers": 7}"#,
        );
        let meta_tf =
            crate::models::transformers::parse_transformers_metadata(dest.path()).unwrap();
        let base = HfModelMetadata {
            hf_architecture_type: Some("BaseArch".to_string()),
            ..Default::default()
        };

        apply_repo_pull_completion_with_meta(&conn, model_id, &base, Some(&meta_tf), dest.path())
            .unwrap();

        let row = completion_columns(&conn, model_id);
        assert_eq!(row.hf_architecture_type.as_deref(), Some("BaseArch"));
        assert_eq!(row.hf_context_length, Some(100));
        assert_eq!(row.hf_num_layers, Some(7));
    }

    /// Write an executable shell stub to `dir` and return its path.
    fn write_stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, PermissionsExt::from_mode(0o755)).unwrap();
        path
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

    /// Test that spawn_hf_download returns Err when the binary does not exist.
    #[tokio::test]
    async fn test_spawn_hf_download_missing_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let result = spawn_hf_download(
            "definitely-not-a-real-binary-xyz",
            "foo/bar",
            tmp.path(),
            None,
        )
        .await;
        assert!(
            result.is_err(),
            "spawning a missing binary should fail, got: {result:?}"
        );
    }

    /// Test that the stub runs with the expected args, HF_TOKEN is injected
    /// only when provided, and the stderr tail captures stderr (not stdout).
    #[tokio::test]
    async fn test_spawn_hf_download_runs_stub_and_captures_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        // CLI argv is `hf download <repo> --local-dir <dest>` → dest is $4.
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\necho \"line1\"\necho \"err\" 1>&2\necho \"$HF_TOKEN\" > \"$4/.hf_token_check\"\nexit 0\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        let stub_str = stub.to_str().unwrap().to_string();

        // Run 1: with an explicit token.
        let mut child = spawn_hf_download(&stub_str, "foo/bar", &dest, Some("tok123"))
            .await
            .expect("stub should spawn");
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert!(status.success());

        let tail = stderr_tail_str(&sink)
            .await
            .expect("stderr should be captured");
        assert!(
            tail.contains("err"),
            "stderr tail should contain 'err': {tail}"
        );
        assert!(
            !tail.contains("line1"),
            "stdout must not leak into the stderr tail: {tail}"
        );
        let marker = std::fs::read_to_string(dest.join(".hf_token_check")).unwrap();
        assert_eq!(
            marker.trim(),
            "tok123",
            "HF_TOKEN env must be injected when provided"
        );

        // Run 2: without a token — HF_TOKEN must not be set by us, only
        // inherited from the test process environment (if present there).
        let mut child = spawn_hf_download(&stub_str, "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink2: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink2.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert!(status.success());

        let marker = std::fs::read_to_string(dest.join(".hf_token_check")).unwrap();
        let inherited = std::env::var("HF_TOKEN").unwrap_or_default();
        assert_eq!(
            marker.trim(),
            inherited,
            "HF_TOKEN must only be inherited, never injected"
        );
    }

    /// Test that a non-zero exit code is observable and stderr is captured.
    #[tokio::test]
    async fn test_spawn_hf_download_nonzero_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\necho \"boom\" 1>&2\nexit 3\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();

        let mut child = spawn_hf_download(stub.to_str().unwrap(), "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert_eq!(status.code(), Some(3));
        let tail = stderr_tail_str(&sink)
            .await
            .expect("stderr should be captured");
        assert!(
            tail.contains("boom"),
            "stderr tail should contain 'boom': {tail}"
        );
    }

    /// Test that a single huge stderr line (> 8 KiB) is capped to the 4096-byte
    /// tail: the sink holds exactly the LAST 4096 bytes of the stream (the
    /// tail of the line, not its head), with no unbounded per-line buffering.
    #[tokio::test]
    async fn test_stderr_reader_caps_huge_line() {
        let tmp = tempfile::tempdir().unwrap();
        // One 9000-byte line of 'x' followed by a single newline — far larger
        // than the 4096-byte cap, written as a single line to the child's
        // stderr.
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\nhead -c 9000 /dev/zero | tr '\\0' 'x' 1>&2\necho '' 1>&2\nexit 0\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();

        let mut child = spawn_hf_download(stub.to_str().unwrap(), "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert!(status.success());

        let tail = sink.lock().await;
        // 9000 'x' + 1 newline = 9001 bytes → capped to the last 4096.
        assert_eq!(
            tail.len(),
            4096,
            "sink must be capped at exactly 4096, got {}",
            tail.len()
        );
        assert!(
            tail.ends_with(b"\n"),
            "cap must keep the TAIL of the line, not the head"
        );
        assert!(
            tail[..tail.len() - 1].iter().all(|&b| b == b'x'),
            "capped tail must be all 'x' (the tail of the line)"
        );
    }

    /// Regression: a multi-byte UTF-8 character straddling the 4096-byte cap
    /// must survive intact. 4094 'a' + "é" (2 bytes) + 'b' = 4097 bytes, so
    /// the cap drops exactly one byte — the first 'a' — and the é (bytes 4095–4096
    /// of the stream) lands right on the cap boundary. Decoding happens once
    /// at read time, so the é stays a single character (the old per-chunk
    /// `from_utf8_lossy` split it into one U+FFFD per 4 KiB chunk).
    #[tokio::test]
    async fn test_stderr_reader_multibyte_char_on_cap_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\n{ head -c 4094 /dev/zero | tr '\\0' 'a'; printf '\\303\\251b'; } 1>&2\nexit 0\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();

        let mut child = spawn_hf_download(stub.to_str().unwrap(), "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader.await.expect("stderr reader must finish");
        assert!(status.success());

        let tail = stderr_tail_str(&sink)
            .await
            .expect("stderr should be captured");
        assert_eq!(
            tail,
            format!("{}éb", "a".repeat(4093)),
            "a multi-byte char on the cap boundary must stay whole, not split into U+FFFD"
        );
    }

    /// Regression: capping the sink must never panic. The old code capped a
    /// `String` with `drain(0..overflow)` where `overflow` was a BYTE offset —
    /// when that offset landed mid-character, `String::drain` PANICKED and
    /// killed the detached reader task. Here "é" opens the stream, so the
    /// first drain offset (1) lands inside the 2-byte char: the reader must
    /// survive, and the dropped leading byte decodes to at most one U+FFFD.
    #[tokio::test]
    async fn test_stderr_reader_cap_drain_mid_char_no_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = write_stub(
            tmp.path(),
            "hf-stub",
            "#!/bin/sh\n{ printf '\\303\\251'; head -c 4095 /dev/zero | tr '\\0' 'a'; } 1>&2\nexit 0\n",
        );
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();

        let mut child = spawn_hf_download(stub.to_str().unwrap(), "foo/bar", &dest, None)
            .await
            .expect("stub should spawn");
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr must be piped");
        let reader = start_stderr_reader(stderr, sink.clone());
        let status = child.wait().await.unwrap();
        reader
            .await
            .expect("stderr reader must not panic on a mid-char cap");
        assert!(status.success());

        let tail = stderr_tail_str(&sink)
            .await
            .expect("stderr should be captured");
        // 4097 bytes → 1 dropped: the leading byte of the é. The orphaned
        // trailing byte decodes to a single U+FFFD, then 4095 'a'.
        assert_eq!(
            tail,
            format!("\u{FFFD}{}", "a".repeat(4095)),
            "mid-char cap must drop bytes, not panic"
        );
    }

    /// Test the full repo-pull job lifecycle on PullState: upsert, get,
    /// running check, cancel (childless), double cancel, unknown id.
    #[tokio::test]
    async fn test_pull_state_repo_pull_lifecycle() {
        let state = PullState::new(None);
        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));
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
            child: child_arc.clone(),
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
            Arc::ptr_eq(&got.child, &child_arc),
            "cloned job must share the child handle Arc"
        );
        assert!(
            Arc::ptr_eq(&got.stderr_tail, &job_stderr_arc),
            "cloned job must share the stderr sink Arc"
        );

        assert!(state.repo_pull_running_for("owner/repo").await);
        assert!(!state.repo_pull_running_for("other/repo").await);

        // Childless Running job: cancel must tolerate the empty child handle.
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

    /// Test that the try_wait-based wait-loop and cancel do not deadlock:
    /// the wait-loop polls non-blocking and sleeps outside the lock, so
    /// cancel's brief `kill()` lock acquisition succeeds while the loop runs,
    /// and the loop terminates promptly once the child is killed.
    #[tokio::test]
    async fn test_wait_loop_cancel_race() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = write_stub(tmp.path(), "hf-stub", "#!/bin/sh\nsleep 30\n");
        let mut child = spawn_hf_download(stub.to_str().unwrap(), "foo/bar", tmp.path(), None)
            .await
            .expect("stub should spawn");
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        if let Some(stderr) = child.stderr.take() {
            let _reader = start_stderr_reader(stderr, sink.clone());
        }
        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> =
            Arc::new(Mutex::new(Some(child)));

        let state = PullState::new(None);
        state
            .upsert_repo_pull(RepoPullJob {
                job_id: "job-x".to_string(),
                repo_id: "owner/repo".to_string(),
                model_id: None,
                dest: tmp.path().to_path_buf(),
                total_bytes: None,
                status: RepoPullStatus::Running,
                error: None,
                cancel_requested: false,
                context_length: None,
                stderr_tail: sink,
                child: child_arc.clone(),
            })
            .await;

        // Wait-loop logic: brief try_wait under the lock, sleep OUTSIDE it.
        let loop_state = state.clone();
        let wait_handle = tokio::spawn(async move {
            loop {
                let got = {
                    let mut g = child_arc.lock().await;
                    g.as_mut().and_then(|c| c.try_wait().ok()).flatten()
                };
                if let Some(exit) = got {
                    let job = loop_state
                        .get_repo_pull("job-x")
                        .await
                        .expect("job must still exist");
                    return (job.cancel_requested, exit.code());
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        });

        // Let the child start, then cancel through the real cancel path
        // (brief job-map lock + brief child lock + kill).
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        state
            .cancel_repo_pull("job-x")
            .await
            .expect("cancel should acquire the child lock without deadlocking");

        let started = std::time::Instant::now();
        let (cancel_requested, _code) = wait_handle.await.expect("wait loop must finish");
        let elapsed = started.elapsed();
        assert!(
            cancel_requested,
            "wait loop must observe the cancel flag for its final decision"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "wait loop should terminate within a tick of the kill, took {elapsed:?}"
        );
    }

    // ── start_repo_pull / finish_repo_pull orchestration tests ────────────

    /// Serializes env-var mutations (PATH, HF_ENDPOINT) across tests.
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Test that an invalid repo id is rejected before any network or spawn
    /// work happens.
    #[tokio::test]
    async fn test_start_repo_pull_rejects_invalid_repo_id() {
        let state = Arc::new(crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            None,
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
            None,
        ));
        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));
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
                child: child_arc,
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

    /// Test that a missing `hf` binary (PATH pointed at an empty dir) yields
    /// HfBinaryMissing with an install hint.
    #[tokio::test]
    async fn test_start_repo_pull_missing_binary() {
        let state = Arc::new(crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            None,
            None,
        ));
        let empty_path = tempfile::tempdir().unwrap();
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("PATH", empty_path.path());
        }

        let err = start_repo_pull(&state, "owner/repo", None)
            .await
            .expect_err("missing binary must be rejected");
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("PATH");
        }

        assert!(
            matches!(err, RepoPullError::HfBinaryMissing(ref hint)
                if hint.contains("pip install")),
            "expected HfBinaryMissing with install hint, got: {err:?}"
        );
    }

    /// Test that a repo that 404s on HF is reported as RepoNotFound (the `hf`
    /// stub on PATH makes the binary check pass deterministically).
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
            None,
        ));
        let bin_dir = tempfile::tempdir().unwrap();
        let _stub = write_stub(bin_dir.path(), "hf", "#!/bin/sh\nexit 0\n");
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("PATH", bin_dir.path());
            std::env::set_var("HF_ENDPOINT", server.uri());
        }

        let err = start_repo_pull(&state, "owner/missing", None)
            .await
            .expect_err("unknown repo must be rejected");
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("PATH");
            std::env::remove_var("HF_ENDPOINT");
        }

        assert!(
            matches!(err, RepoPullError::RepoNotFound(ref msg)
                if msg.contains("not found on HuggingFace")),
            "expected RepoNotFound, got: {err:?}"
        );
    }

    /// Full lifecycle: start a pull against a wiremock HF endpoint with a stub
    /// `hf` binary that writes a config.json + weights file, wait for the
    /// wait-loop to finish, and assert the job completes with the context
    /// length and the model row gets the merged metadata + quant.
    #[tokio::test]
    async fn test_start_repo_pull_full_lifecycle() {
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
        let bin_dir = tempfile::tempdir().unwrap();
        let _stub = write_stub(
            bin_dir.path(),
            "hf",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\n\
dest=\"$4\"\n\
printf '{\"architectures\": [\"Qwen3ForCausalLM\"], \"max_position_embeddings\": 32768, \"num_hidden_layers\": 48, \"quantization_config\": {\"quant_method\": \"fp8\"}}' > \"$dest/config.json\"\n\
printf 'fake-weights' > \"$dest/model.safetensors\"\nexit 0\n",
        );

        let db_dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.general.models_dir = Some(models_root.path().to_string_lossy().to_string());
        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            Some(db_dir.path().to_path_buf()),
            None,
        ));

        // The wizard pre-creates the stub model row before starting the pull.
        let model_id: i64 = {
            let conn = state.open_db().expect("db must be available");
            conn.execute(
                "INSERT INTO model_configs (repo_id, backend) \
                 VALUES ('happy/repo', 'llama_cpp')",
                [],
            )
            .unwrap();
            conn.query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
                .unwrap()
        };

        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("PATH", bin_dir.path());
            std::env::set_var("HF_ENDPOINT", server.uri());
        }
        let start = start_repo_pull(&state, "happy/repo", Some(model_id))
            .await
            .expect("start should succeed");
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("PATH");
            std::env::remove_var("HF_ENDPOINT");
        }

        assert!(start.job_id.starts_with("hfrepo-"));
        assert!(
            start.total_bytes.is_none(),
            "stats endpoint 404s → indeterminate total"
        );

        // Poll until the wait loop takes the job to a terminal state.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let final_job = loop {
            if let Some(job) = state.pull.get_repo_pull(&start.job_id).await {
                if job.status != RepoPullStatus::Running {
                    break job;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "job did not reach a terminal state in time"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };

        assert_eq!(final_job.status, RepoPullStatus::Completed);
        assert_eq!(final_job.context_length, Some(32768));
        assert!(final_job.error.is_none());

        // The stub wrote the repo contents into <models_dir>/<repo_id>.
        let dest = crate::models::repo_path(models_root.path(), "happy/repo");
        assert!(dest.join("config.json").exists());
        assert!(dest.join("model.safetensors").exists());

        // The model row got the merged completion metadata + quant.
        let conn = state.open_db().expect("db must be available");
        let row = completion_columns(&conn, model_id);
        assert_eq!(row.hf_format.as_deref(), Some("transformers"));
        assert_eq!(
            row.hf_architecture_type.as_deref(),
            Some("Qwen3ForCausalLM")
        );
        assert_eq!(row.hf_context_length, Some(32768));
        assert_eq!(row.hf_num_layers, Some(48));
        assert_eq!(row.selected_quant.as_deref(), Some("fp8"));
    }

    /// Test that a non-zero exit with a non-empty stderr tail produces a
    /// Failed job whose error is the stderr tail.
    #[tokio::test]
    async fn test_finish_repo_pull_failure_records_stderr_tail() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("HF_ENDPOINT", server.uri());
        }
        let state = crate::proxy::ProxyState::new(crate::config::Config::default(), None, None);
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(b"boom\n".to_vec()));
        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));
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
                child: child_arc,
            })
            .await;

        finish_repo_pull(&state, "job-fail", Some(3)).await;

        let job = state
            .pull
            .get_repo_pull("job-fail")
            .await
            .expect("job must still exist");
        assert_eq!(job.status, RepoPullStatus::Failed);
        assert_eq!(job.error.as_deref(), Some("boom"));
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("HF_ENDPOINT");
        }
    }

    /// Test that a non-zero exit with an EMPTY stderr tail falls back to a
    /// synthetic error message containing the exit code.
    #[tokio::test]
    async fn test_finish_repo_pull_failure_empty_stderr_fallback() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("HF_ENDPOINT", server.uri());
        }
        let state = crate::proxy::ProxyState::new(crate::config::Config::default(), None, None);
        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));
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
                child: child_arc,
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
            "empty stderr tail must fall back to a code-based message"
        );
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("HF_ENDPOINT");
        }
    }

    /// Test that a cancel request wins over a clean exit: the final status is
    /// Cancelled with no error.
    #[tokio::test]
    async fn test_finish_repo_pull_cancel_requested() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("HF_ENDPOINT", server.uri());
        }
        let state = crate::proxy::ProxyState::new(crate::config::Config::default(), None, None);
        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));
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
                child: child_arc,
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
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("HF_ENDPOINT");
        }
    }

    /// Regression: a FAILED download must not mark the model row as
    /// configured. `hf` writes config.json EARLY (before the weights), so a
    /// failed pull can still leave a valid config.json in dest — the
    /// completion step (metadata fetch + DB update) must be gated on
    /// success, and the model row must stay untouched.
    #[tokio::test]
    async fn test_finish_repo_pull_failure_skips_db_update() {
        let server = wiremock::MockServer::start().await;
        // blobs endpoint 404s → total_bytes soft-fails to None (indeterminate).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/models/fail/repo"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // hf-hub 0.5 info() hits /api/models/{repo}/revision/{revision}.
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
        let bin_dir = tempfile::tempdir().unwrap();
        // The stub writes config.json FIRST (like a real hf), then fails.
        let _stub = write_stub(
            bin_dir.path(),
            "hf",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\n\
dest=\"$4\"\n\
printf '{\"architectures\": [\"Qwen3ForCausalLM\"], \"max_position_embeddings\": 32768, \"num_hidden_layers\": 48, \"quantization_config\": {\"quant_method\": \"fp8\"}}' > \"$dest/config.json\"\n\
echo \"download failed: connection reset by peer\" 1>&2\n\
exit 3\n",
        );

        let db_dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.general.models_dir = Some(models_root.path().to_string_lossy().to_string());
        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            Some(db_dir.path().to_path_buf()),
            None,
        ));

        // The wizard pre-creates the stub model row before starting the pull.
        let model_id: i64 = {
            let conn = state.open_db().expect("db must be available");
            conn.execute(
                "INSERT INTO model_configs (repo_id, backend) \
                 VALUES ('fail/repo', 'llama_cpp')",
                [],
            )
            .unwrap();
            conn.query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
                .unwrap()
        };

        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("PATH", bin_dir.path());
            std::env::set_var("HF_ENDPOINT", server.uri());
        }
        let start = start_repo_pull(&state, "fail/repo", Some(model_id))
            .await
            .expect("start should succeed");
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("PATH");
            std::env::remove_var("HF_ENDPOINT");
        }

        // Poll until the wait loop takes the job to a terminal state.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let final_job = loop {
            if let Some(job) = state.pull.get_repo_pull(&start.job_id).await {
                if job.status != RepoPullStatus::Running {
                    break job;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "job did not reach a terminal state in time"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };

        assert_eq!(final_job.status, RepoPullStatus::Failed);
        assert!(
            final_job
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("connection reset"),
            "failed job must carry the stderr tail as error: {final_job:?}"
        );
        assert!(final_job.context_length.is_none());

        // config.json WAS written by the stub before it failed…
        let dest = crate::models::repo_path(models_root.path(), "fail/repo");
        assert!(
            dest.join("config.json").exists(),
            "stub must have written config.json before failing"
        );

        // …but the model row must be UNCHANGED: no format, no quant, no
        // context length — a failed download is not a configured model.
        let conn = state.open_db().expect("db must be available");
        let row = completion_columns(&conn, model_id);
        assert_eq!(
            row,
            CompletionRow {
                hf_format: None,
                hf_architecture_type: None,
                hf_context_length: None,
                hf_num_layers: None,
                selected_quant: None,
            },
            "failed pull must not touch the model row"
        );
    }

    /// Regression: a CANCELLED pull must also skip the completion step, even
    /// when the child exited cleanly (cancel_requested wins over exit 0).
    /// dest contains a config.json, as hf would have written it before the
    /// user cancelled mid-download — the model row must still stay untouched.
    #[tokio::test]
    async fn test_finish_repo_pull_cancelled_skips_db_update() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("HF_ENDPOINT", server.uri());
        }

        let db_dir = tempfile::tempdir().unwrap();
        let state = crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            Some(db_dir.path().to_path_buf()),
            None,
        );
        let model_id: i64 = {
            let conn = state.open_db().expect("db must be available");
            conn.execute(
                "INSERT INTO model_configs (repo_id, backend) \
                 VALUES ('owner/repo', 'llama_cpp')",
                [],
            )
            .unwrap();
            conn.query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
                .unwrap()
        };

        // dest with a config.json, as hf would have written before the cancel.
        let dest_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dest_dir.path().join("config.json"),
            r#"{"architectures": ["TfArch"], "max_position_embeddings": 32768, "quantization_config": {"quant_method": "fp8"}}"#,
        )
        .unwrap();

        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));
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
                child: child_arc,
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
        let conn = state.open_db().expect("db must be available");
        let row = completion_columns(&conn, model_id);
        assert_eq!(
            row,
            CompletionRow {
                hf_format: None,
                hf_architecture_type: None,
                hf_context_length: None,
                hf_num_layers: None,
                selected_quant: None,
            },
            "cancelled pull must not touch the model row"
        );
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("HF_ENDPOINT");
        }
    }

    /// Test that a child killed by a signal (the wait-loop sees
    /// `exit_status == None`) with an empty stderr tail produces a Failed job
    /// with the "exited abnormally" fallback message.
    #[tokio::test]
    async fn test_finish_repo_pull_signal_death_message() {
        let state = crate::proxy::ProxyState::new(crate::config::Config::default(), None, None);
        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));
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
                child: child_arc,
            })
            .await;

        // A signal death surfaces as `exit_status = None` from the wait-loop.
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
            "signal death with empty stderr must fall back to the abnormal-exit message"
        );
        assert!(job.context_length.is_none());
    }

    /// Test that a successful pull started by an API-only caller (no
    /// pre-created model row, `model_id: None`) still completes: the DB step
    /// is skipped entirely, so `context_length` stays None and no model row is
    /// touched — even when dest holds a config.json.
    #[tokio::test]
    async fn test_finish_repo_pull_no_model_id_skips_db() {
        let server = wiremock::MockServer::start().await;
        // The completion metadata fetch soft-fails against a 404-only mock.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("HF_ENDPOINT", server.uri());
        }

        let db_dir = tempfile::tempdir().unwrap();
        let state = crate::proxy::ProxyState::new(
            crate::config::Config::default(),
            Some(db_dir.path().to_path_buf()),
            None,
        );

        // dest holds a config.json, but with model_id = None nothing may be
        // written to any model row.
        let dest_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dest_dir.path().join("config.json"),
            r#"{"max_position_embeddings": 123}"#,
        )
        .unwrap();

        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(None));
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
                child: child_arc,
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
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("HF_ENDPOINT");
        }
    }

    /// Test that a non-404 (500) error from the HF metadata/info endpoint maps
    /// to `RepoPullError::Upstream` (not `RepoNotFound`).
    #[tokio::test]
    async fn test_start_repo_pull_non_404_info_error_upstream() {
        let server = wiremock::MockServer::start().await;
        // hf-hub 0.5 info() hits /api/models/{repo}/revision/{revision}.
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
            None,
        ));
        let bin_dir = tempfile::tempdir().unwrap();
        let _stub = write_stub(bin_dir.path(), "hf", "#!/bin/sh\nexit 0\n");
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("PATH", bin_dir.path());
            std::env::set_var("HF_ENDPOINT", server.uri());
        }

        let err = start_repo_pull(&state, "broken/repo", None)
            .await
            .expect_err("a 500 from info() must surface as an error");
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::remove_var("PATH");
            std::env::remove_var("HF_ENDPOINT");
        }

        assert!(
            matches!(err, RepoPullError::Upstream(_)),
            "expected Upstream for a non-404 info error, got: {err:?}"
        );
    }
}
