//! Engine container log tails (plan-195 task 6, stage 2a).
//!
//! [`TailSupervisor`] drives one `docker logs -f -t <container>` child
//! per *running, container-backed* model. **Process-table discovery is
//! a 1 s poll** — `ProcessTable` exposes no watch/notify channel, so
//! this cannot be event-driven.
//!
//! * Container identity: [`container_name_for`] (`tama-<model_name>`),
//!   the same helper the legacy `Logs` RPC uses. The "has a container"
//!   discriminator is `ProcessEntry.spec.docker_config_json` being
//!   non-empty — native-host backends get NO tail (they match today).
//! * Each physical line becomes ONE `PushEvent` (multi-line stderr
//!   tracebacks therefore land as several entries — by design); EMPTY
//!   lines are noise and dropped, every non-empty line is kept.
//! * The `-t` flag prefixes each line with its container timestamp:
//!   [`parse_ts_prefix`] splits off a leading RFC3339 stamp (→ `ts`).
//!   Malformed/absent prefixes (continuation lines, unspooled/unbuffered
//!   content, plain pre-1.13 docker) fall back to capture time and
//!   `level = -1` (unknown).
//! * `level` is always `-1` for engine lines — the proxy maps that to
//!   level 2 + `level_known: false` when ingesting (task 7).
//! * Flow control lives in the child's stdout/stderr pipes: if the push
//!   channel is full and `try_send` drops, the pipe fills, `docker logs`
//!   blocks — the TAIL stalls but the tamad NEVER does. A child that
//!   has produced no output for 30 s is considered blocked: it is
//!   killed and restarted ONCE per boot per model (WARN
//!   `tail_child_reattach`).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::LinesStream;
use tokio_stream::StreamExt;
use tracing::{debug, warn};

use crate::host_installs::docker::runner::{container_name_for, logs_follow_args};
use crate::host_installs::docker::runtime::ContainerRuntime;
use crate::process_table::{ProcessEntry, ProcessTable};

use super::{model_source, now_unix_ms, PushEvent};

/// Process-table poll cadence (and supervisor loop tick).
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// A tail child silent for this long is considered blocked and is
/// killed + restarted exactly once per boot per model
/// (`tail_child_reattach`).
const STALL_KILL: Duration = Duration::from_secs(30);

/// Min interval between spawn retries for a model whose `docker logs`
/// could not be spawned at all (no docker on PATH, etc.) — avoiding a
/// 1 s spawn storm on docker-less hosts.
const SPAWN_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// Parse a leading RFC3339 timestamp (from `docker logs -t`) off a line.
///
/// Returns `(Some(unix_ms), body)` when the line starts with a valid
/// RFC3339 stamp followed by whitespace, else `(None, full line)`.
/// The body keeps any original indentation after the one-space
/// separator.
pub fn parse_ts_prefix(line: &str) -> (Option<i64>, String) {
    let Some(sp) = line.find(' ') else {
        return (None, line.to_string());
    };
    let (tok, rest) = line.split_at(sp);
    match chrono::DateTime::parse_from_rfc3339(tok) {
        Ok(dt) => (Some(dt.timestamp_millis()), rest[1..].to_string()),
        Err(_) => (None, line.to_string()),
    }
}

/// True when the line is empty/whitespace noise that must be dropped.
pub fn is_noise(rest: &str) -> bool {
    rest.trim().is_empty()
}

/// The set of model names that HAVE a container tail right now (non-empty
/// `docker_config_json` in the stored spec).
pub fn tail_models(entries: &[ProcessEntry]) -> HashSet<String> {
    entries
        .iter()
        .filter(|e| !e.spec.docker_config_json.is_empty())
        .map(|e| e.model_name.clone())
        .collect()
}

/// One tail child's bookkeeping.
struct TailSlot {
    /// Stop signal for the child task (kill on drop).
    kill: tokio::sync::watch::Sender<bool>,
    /// Last time the child produced output (any line).
    last_data: Arc<Mutex<Instant>>,
    /// Flips false when the child exits on its own.
    child_alive: Arc<AtomicBool>,
}

/// Supervisor process: one per tamad, started from `main` when a proxy
/// is configured (auto G6).
pub struct TailSupervisor {
    stop: tokio::sync::watch::Sender<bool>,
}

impl TailSupervisor {
    /// Start the supervisor: polls `table` every [`POLL_INTERVAL`] and
    /// feeds each container tail's lines to `events_tx`. Call
    /// [`stop`](Self::stop) at shutdown (killing all children).
    pub fn start(table: Arc<ProcessTable>, events_tx: mpsc::Sender<PushEvent>) -> Arc<Self> {
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(run(table.clone(), events_tx, stop_rx));
        Arc::new(Self { stop: stop_tx })
    }

    /// Stop the supervisor and all tail children.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

/// True when the entry has a container (the tail discriminator).
#[cfg(test)]
fn has_container(entry: &ProcessEntry) -> bool {
    !entry.spec.docker_config_json.is_empty()
}

/// Supervisor loop: one per tamad.
async fn run(
    table: Arc<ProcessTable>,
    events_tx: mpsc::Sender<PushEvent>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut slots: HashMap<String, TailSlot> = HashMap::new();
    let mut reattached: HashSet<String> = HashSet::new();
    // Models whose (blocked/exited) child has ALREADY been reattached
    // once this boot — at most one reattach per model.
    let mut spawn_blocked: HashMap<String, Instant> = HashMap::new();
    // Last spawn-failure time per model (spawn-retry backoff).

    loop {
        tokio::select! {
            _ = stop_rx.changed() => break,
            _ = interval.tick() => {}
        }
        if *stop_rx.borrow_and_update() {
            break;
        }

        let snapshot = table.snapshot().await;
        let desired = tail_models(&snapshot);

        // Stop tails whose model lost its container (or disappeared).
        for (model, slot) in slots.iter() {
            if !desired.contains(model) {
                let _ = slot.kill.send(true);
            }
        }
        slots.retain(|model, _| desired.contains(model));

        for model in desired {
            // Reattach (once per boot) when the child exited on its own
            // or has been silent for over STALL_KILL.
            let reattach = match slots.get(&model) {
                Some(slot) => {
                    !slot.child_alive.load(Ordering::Relaxed)
                        || slot.last_data.lock().unwrap().elapsed() > STALL_KILL
                }
                None => false,
            };
            if reattach {
                if let Some(slot) = slots.remove(&model) {
                    let _ = slot.kill.send(true);
                }
                if reattached.insert(model.clone()) {
                    warn!(
                        model = %model,
                        "tail_child_reattach: engine log tail child blocked or exited; \
                         killing and restarting (once per boot)"
                    );
                }
            }

            if slots.contains_key(&model) {
                continue;
            }
            // Spawn backoff: no more than once per SPAWN_RETRY_INTERVAL
            // while `docker` is unavailable on this host.
            if let Some(last) = spawn_blocked.get(&model) {
                if last.elapsed() < SPAWN_RETRY_INTERVAL {
                    continue;
                }
            }
            match spawn_tail_child(&model, events_tx.clone()).await {
                Ok((kill, last_data, child_alive)) => {
                    spawn_blocked.remove(&model);
                    slots.insert(
                        model.clone(),
                        TailSlot {
                            kill,
                            last_data,
                            child_alive,
                        },
                    );
                }
                Err(e) => {
                    spawn_blocked.insert(model.clone(), Instant::now());
                    debug!(model = %model, error = %e, "engine log tail spawn failed; will retry");
                }
            }
        }
    }

    // Shutdown: kill all children.
    for (_, slot) in slots.drain() {
        let _ = slot.kill.send(true);
    }
}

/// Read one pipe line-by-line into the push channel (shared by the
/// stdout and stderr feeds — `docker logs -f` merges both into the
/// container's output streams; both are tailed so nothing is lost).
async fn pipe_reader(
    pipe: impl AsyncRead + Send + 'static,
    source: String,
    tx: mpsc::Sender<PushEvent>,
    last_data: Arc<Mutex<Instant>>,
) {
    let raw_lines = tokio::io::BufReader::new(pipe).lines();
    let lines = LinesStream::new(raw_lines);
    tokio::pin!(lines);
    while let Some(Ok(raw)) = lines.next().await {
        // Touch liveness BEFORE the (possibly dropped) send — a silent
        // container with a full channel is NOT a stalled child.
        *last_data.lock().unwrap() = Instant::now();
        let (ts, rest) = parse_ts_prefix(&raw);
        if is_noise(&rest) {
            continue;
        }
        // Bounded flow control in the transport: channel full ⇒ drop
        // this line (newest) and let the pipe backpressure the
        // `docker logs` child, never the tamad.
        let _ = tx.try_send(PushEvent {
            ts: ts.unwrap_or_else(now_unix_ms),
            level: -1,
            source: source.clone(),
            message: json!({ "message": rest }).to_string(),
        });
    }
}

/// Spawn the `docker logs -f -t` child for `model` (container
/// `tama-<model>`) and return its bookkeeping handles. The spawned task
/// owns the child's lifecycle: kill signal ⇒ SIGKILL + wait; child exit
/// ⇒ mark dead (the supervisor reattaches at most once per boot per
/// model, then arms fresh children for NEW container generations).
async fn spawn_tail_child(
    model: &str,
    events_tx: mpsc::Sender<PushEvent>,
) -> std::io::Result<(
    tokio::sync::watch::Sender<bool>,
    Arc<Mutex<Instant>>,
    Arc<AtomicBool>,
)> {
    let container = container_name_for(model);
    let source = model_source(model);
    let mut child = Command::new(ContainerRuntime::default().command())
        .args(logs_follow_args(&container))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let last_data = Arc::new(Mutex::new(Instant::now()));
    let child_alive = Arc::new(AtomicBool::new(true));
    let (kill_tx, mut kill_rx) = tokio::sync::watch::channel(false);

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let last_data_task = Arc::clone(&last_data);
    let child_alive_task = Arc::clone(&child_alive);

    tokio::spawn(async move {
        let reader_a = tokio::spawn(pipe_reader(
            stdout,
            source.clone(),
            events_tx.clone(),
            Arc::clone(&last_data_task),
        ));
        let reader_b = tokio::spawn(pipe_reader(
            stderr,
            source,
            events_tx,
            Arc::clone(&last_data_task),
        ));

        tokio::select! {
            _ = kill_rx.changed() => {
                let _ = child.kill().await;
            }
            r = child.wait() => {
                let _ = r; // exits on its own (container stopped, daemon
                // gone); readers drain whatever remains and EOF.
            }
        }
        reader_a.abort();
        reader_b.abort();
        child_alive_task.store(false, Ordering::Relaxed);
    });

    Ok((kill_tx, last_data, child_alive))
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A `docker logs -t` line: RFC3339 prefix → ts; body preserved.
    #[test]
    fn test_parse_ts_prefix_docker_line() {
        let (ts, body) = parse_ts_prefix("2025-01-02T03:04:05.123456789Z llava ready");
        assert_eq!(
            ts,
            Some(
                chrono::DateTime::parse_from_rfc3339("2025-01-02T03:04:05.123456789Z")
                    .unwrap()
                    .timestamp_millis()
            )
        );
        assert_eq!(body, "llava ready");
    }

    /// Continuation lines (no valid prefix) keep the full line and
    /// report `None` → the caller falls back to capture time.
    #[test]
    fn test_parse_ts_prefix_continuation() {
        let (ts, body) = parse_ts_prefix("    at frame line (stacktrace.py:42)");
        assert_eq!(ts, None);
        assert_eq!(body, "    at frame line (stacktrace.py:42)");
    }

    /// Malformed prefixes degrade to `None` (not a panic).
    #[test]
    fn test_parse_ts_prefix_malformed() {
        assert_eq!(
            parse_ts_prefix("not-a-timestamp foo"),
            (None, "not-a-timestamp foo".to_string())
        );
        assert_eq!(
            parse_ts_prefix("2025-13-45T00:00:00Z oops"),
            (None, "2025-13-45T00:00:00Z oops".to_string())
        );
        // No space at all → the whole line is the body.
        assert_eq!(
            parse_ts_prefix("singlescamp"),
            (None, "singlescamp".to_string())
        );
    }

    /// A pathological >16 KiB single line parses cleanly (prefix split
    /// + full body intact — no mid-line truncation).
    #[test]
    fn test_parse_ts_prefix_huge_line() {
        let payload = "x".repeat(16 * 1024 + 1);
        let input = format!("2025-01-02T03:04:05Z {payload}");
        let (ts, body) = parse_ts_prefix(&input);
        assert!(ts.is_some());
        assert_eq!(body.len(), 16 * 1024 + 1);
        assert_eq!(&body[..4], "xxxx");
    }

    /// Empty / whitespace-only lines are noise (dropped); any real
    /// content is kept.
    #[test]
    fn test_is_noise() {
        assert!(is_noise(""));
        assert!(is_noise("   \t  "));
        assert!(!is_noise("keep me"));
        assert!(!is_noise("0"));
    }

    /// The container discriminator: only non-empty `docker_config_json`
    /// entries are tailed (native backends are not).
    #[test]
    fn test_tail_models_native_vs_docker() {
        fn entry(name: &str, docker_json: &str) -> ProcessEntry {
            let spec = tama_core::tamad::LoadModelRequest {
                docker_config_json: docker_json.to_string(),
                ..Default::default()
            };
            ProcessEntry {
                model_name: name.to_string(),
                provider_name: "llama.cpp".to_string(),
                pid: 1,
                endpoint_url: String::new(),
                status: "ready".to_string(),
                started_at: Instant::now(),
                spec,
                restart_count: 0,
                window_starts: Vec::new(),
                user_flagged: false,
            }
        }
        let entries = vec![
            entry("with-container", r#"{"image":"x"}"#),
            entry("native-host", ""),
            entry("another-container", r#"{"image":"y"}"#),
        ];
        let models = tail_models(&entries);
        assert_eq!(models.len(), 2);
        assert!(models.contains("with-container"));
        assert!(models.contains("another-container"));
        assert!(!models.contains("native-host"));
    }

    /// `container_name_for` is the SAME helper the legacy Logs RPC uses
    /// (`tama-<model>`), and the discriminator helper agrees.
    #[test]
    fn test_container_identity_matches_legacy_logs_rpc() {
        assert_eq!(container_name_for("rel-aso"), "tama-rel-aso");
        assert!(has_container(&process_entry_with_docker()));
    }

    fn process_entry_with_docker() -> ProcessEntry {
        let spec = tama_core::tamad::LoadModelRequest {
            docker_config_json: r#"{"image":"x"}"#.to_string(),
            ..Default::default()
        };
        ProcessEntry {
            model_name: "m".to_string(),
            provider_name: "llama.cpp".to_string(),
            pid: 1,
            endpoint_url: String::new(),
            status: "ready".to_string(),
            started_at: Instant::now(),
            spec,
            restart_count: 0,
            window_starts: Vec::new(),
            user_flagged: false,
        }
    }
}
