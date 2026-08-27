//! Backend log endpoints: file-based reading and grouped listing.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Json, Sse};
use futures_util::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::super::ProxyState;
use crate::logging;

/// Response from GET /tama/v1/logs — grouped by source.
#[derive(Debug, Clone, Serialize)]
pub struct AllLogsResponse {
    pub sources: Vec<SourceLogs>,
}

/// Logs for a single source (e.g. "tama", "llama_cpp_1").
#[derive(Debug, Clone, Serialize)]
pub struct SourceLogs {
    pub name: String,
    pub lines: Vec<String>,
}

/// Query params for GET /tama/v1/logs.
#[derive(Deserialize)]
pub struct AllLogsQuery {
    /// Number of lines per source (default: 200).
    #[serde(default = "default_lines")]
    pub lines: usize,
}

fn default_lines() -> usize {
    200
}

/// GET /tama/v1/logs — return grouped logs from all configured sources.
pub async fn handle_all_logs(
    State(state): State<Arc<ProxyState>>,
    Query(query): Query<AllLogsQuery>,
) -> impl IntoResponse {
    let n = query.lines.min(10_000);

    // Get logs_dir from config, with fallback to base_dir/logs if configured path
    // doesn't exist or contains no log files.
    let mut logs_dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(d) = state.config.read().await.logs_dir() {
        logs_dirs.push(d.clone());
        // Also try base_dir/logs as fallback (handles custom log directory)
        if let Ok(base) = crate::config::Config::base_dir() {
            let fallback = base.join("logs");
            if fallback != d && !logs_dirs.contains(&fallback) {
                logs_dirs.push(fallback);
            }
        }
    } else if let Ok(base) = crate::config::Config::base_dir() {
        logs_dirs.push(base.join("logs"));
    }

    // Collect logs from each candidate directory
    let mut sources = Vec::new();
    let mut seen_sources = std::collections::HashSet::new();
    for dir in &logs_dirs {
        if !dir.exists() {
            continue;
        }

        // Collect tama.log
        let tama_path = dir.join("tama.log");
        if tama_path.exists() && seen_sources.insert("tama".to_string()) {
            let lines: Vec<String> =
                match tokio::task::spawn_blocking(move || logging::tail_lines(&tama_path, n)).await
                {
                    Ok(Ok(l)) => l,
                    _ => Vec::new(),
                }
                .into_iter()
                .map(|line| logging::format_log_line(&line))
                .collect();
            if !lines.is_empty() {
                sources.push(SourceLogs {
                    name: "tama".to_string(),
                    lines,
                });
            }
        }

        // Collect backend logs (named {backend}_{backend_name}.log)
        let mut entries = match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        // Sort by modification time (newest first — unstable sort is fine for file list)
        entries.sort_by(|a, b| {
            let a_mod = a.metadata().ok().and_then(|m| m.modified().ok());
            let b_mod = b.metadata().ok().and_then(|m| m.modified().ok());
            b_mod.cmp(&a_mod) // newest first
        });

        for entry in entries {
            let fname = entry.file_name();
            let fname_str = fname.to_string_lossy();
            if fname_str.ends_with(".log") && fname_str != "tama.log" {
                let source_name = fname_str[..fname_str.len() - 4].to_string();
                if seen_sources.insert(source_name.clone()) {
                    let path = entry.path();
                    let lines: Vec<String> =
                        match tokio::task::spawn_blocking(move || logging::tail_lines(&path, n))
                            .await
                        {
                            Ok(Ok(l)) => l,
                            _ => Vec::new(),
                        }
                        .into_iter()
                        .map(|line| logging::format_log_line(&line))
                        .collect();
                    if !lines.is_empty() {
                        sources.push(SourceLogs {
                            name: source_name,
                            lines,
                        });
                    }
                }
            }
        }
    }

    for src in collect_tamad_log_sources(state.tamad_pool().as_ref(), n).await {
        // Dedupe against the file-based sources (a name collision would
        // otherwise double-list one model).
        if seen_sources.insert(src.name.clone()) {
            sources.push(src);
        }
    }

    // Sort sources: tama first, then alphabetical
    sources.sort_by(|a, b| {
        if a.name == "tama" {
            std::cmp::Ordering::Less
        } else if b.name == "tama" {
            std::cmp::Ordering::Greater
        } else {
            a.name.cmp(&b.name)
        }
    });

    Json(serde_json::json!({ "sources": sources }))
}

/// Type-erased SSE stream for backend logs.
type LogStream = BoxStream<'static, Result<axum::response::sse::Event, axum::Error>>;

/// Per-tamad budget for the engine-log tail (open + drain).
/// A wedged RPC must not stall the whole endpoint.
const TAMAD_LOGS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Tail engine logs from every online tamad.
///
/// For each online handle with a latest stats snapshot, every process in
/// `"starting"` / `"ready"` state is tailed through the `Logs` RPC: the
/// tamad tails the `tama-<model>` container its backend runs in. Source
/// names are `{tamad-host}:{model}` — exactly the `source` value the
/// dashboard's per-model log links select in `/tama/logs?source=...`.
///
/// Empty tails (native host backends, gone containers) yield no source.
/// Any per-source failure (timeout, connect error, stream error) is
/// logged and skipped — this endpoint never fails because of a remote
/// tamad.
pub(crate) async fn collect_tamad_log_sources(
    pool: &crate::tamad::pool::TamadPool,
    max_lines: usize,
) -> Vec<SourceLogs> {
    let mut out = Vec::new();
    for handle in pool.list_handles().await {
        if !handle.is_online().await {
            continue;
        }
        let Some(stats) = handle.latest().await else {
            // Online but no snapshot yet — nothing to enumerate.
            continue;
        };
        for p in &stats.processes {
            if !matches!(p.status.as_str(), "starting" | "ready") {
                continue;
            }
            let host = handle.connection.name.clone();
            let model = p.model_name.clone();
            let req = crate::tamad::LogsRequest {
                provider_name: p.provider_name.clone(),
                model_name: model.clone(),
            };

            let open = tokio::time::timeout(TAMAD_LOGS_TIMEOUT, handle.logs(&req)).await;
            let mut stream = match open {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    tracing::warn!(host, %model, error = %e, "tamad logs RPC failed; skipping source");
                    continue;
                }
                Err(_) => {
                    tracing::warn!(host, %model, "tamad logs RPC timed out; skipping source");
                    continue;
                }
            };

            let drained = tokio::time::timeout(TAMAD_LOGS_TIMEOUT, async {
                let mut lines: Vec<String> = Vec::new();
                while lines.len() < max_lines {
                    match stream.message().await {
                        Ok(Some(entry)) => {
                            // Skip blank lines (container log noise).
                            if !entry.message.trim().is_empty() {
                                lines.push(entry.message);
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!(host, %model, error = %e, "tamad log stream error; keeping partial tail");
                            break;
                        }
                    }
                }
                lines
            })
            .await;
            let lines = match drained {
                Ok(lines) => lines,
                Err(_) => {
                    tracing::warn!(host, %model, "tamad log stream stalled; skipping source");
                    continue;
                }
            };

            if !lines.is_empty() {
                out.push(SourceLogs {
                    name: format!("{host}:{model}"),
                    lines,
                });
            }
        }
    }
    out
}

/// GET /tama/v1/logs/:backend/events — SSE stream of backend log lines.
pub async fn handle_backend_log_sse(
    State(state): State<Arc<ProxyState>>,
    Path(backend): Path<String>,
) -> Sse<LogStream> {
    let backend_logs = &state.backend_logs;

    let stream: LogStream = match backend_logs.get(&backend).await {
        Some(log_stream) => {
            let rx = log_stream.subscribe();
            let head = log_stream.snapshot().await;
            futures_util::stream::iter(head.into_iter().map(|line| {
                Ok(axum::response::sse::Event::default()
                    .event("log")
                    .json_data(json!({ "line": line }))
                    .expect("SSE Event json_data serialization should not fail for valid JSON"))
            }))
            .chain(futures_util::stream::unfold(rx, move |mut rx| async move {
                loop {
                    match rx.recv().await {
                        Ok(line) => {
                            return Some((
                                Ok(axum::response::sse::Event::default()
                                    .event("log")
                                    .json_data(json!({ "line": line }))
                                    .expect("SSE Event json_data serialization should not fail for valid JSON")),
                                rx,
                            ));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::debug!("Backend log subscriber lagged by {} lines", n);
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return None;
                        }
                    }
                }
            }))
            .boxed()
        }
        None => {
            // Return an empty stream that stays open but sends no data.
            // This keeps the SSE connection alive without spamming requests
            // when there's no active backend to stream from.
            futures_util::stream::empty().boxed()
        }
    };

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::collect_tamad_log_sources;
    use crate::tamad::pool::test_support::{grpc_conn, start_stub, wait_for, StubTamad};
    use crate::tamad::pool::TamadPool;

    /// A `StubTamad` wired for scripted stats processes + log tails.
    /// `fail_first_n = usize::MAX` simulates a never-online tamad.
    /// (Full literal — the shared harness has no builder.)
    fn logs_stub(
        fail_first_n: usize,
        stats_processes: Vec<crate::tamad::ProcessInfo>,
        log_messages: Vec<String>,
    ) -> StubTamad {
        let (down_tx, _) = tokio::sync::watch::channel(false);
        StubTamad {
            fail_first_n,
            succeed_until: usize::MAX,
            down: Arc::new(down_tx),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            successes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-stub".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_job_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            stats_processes,
            logs_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            log_messages,
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        }
    }

    fn process(model: &str, status: &str) -> crate::tamad::ProcessInfo {
        crate::tamad::ProcessInfo {
            model_name: model.to_string(),
            provider_name: "llama.cpp".to_string(),
            pid: 4242,
            alive: true,
            endpoint_url: "http://127.0.0.1:18099".to_string(),
            status: status.to_string(),
            desired: false,
            restart_count: 0,
            max_restarts: 0,
            spec_accept_pct: None,
            spec_decoding_active: false,
        }
    }

    /// No registered handles → no sources (and no failure).
    #[tokio::test]
    async fn test_collect_tamad_log_sources_empty_pool() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = TamadPool::new(Arc::new(guard.pool.clone()));
        let sources = collect_tamad_log_sources(&pool, 200).await;
        assert!(sources.is_empty(), "empty pool must yield no sources");
        guard.finish().await;
    }

    /// A never-online tamad is skipped entirely: no `Logs` RPC is even
    /// attempted, and the endpoint stays healthy.
    #[tokio::test]
    async fn test_collect_tamad_log_sources_offline_tamad_skipped() {
        let guard = crate::testing::postgres::with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());
        let stub = logs_stub(
            usize::MAX, // stream_stats fails forever → never online
            vec![process("model-x", "ready")],
            vec!["line".into()],
        );
        let addr = start_stub(stub.clone()).await;
        let pool = TamadPool::new(db_pool).with_backoff_base(std::time::Duration::from_millis(20));
        let conn = grpc_conn("uuid-logs-off", "hostA", &format!("grpc://{addr}"));
        pool.upsert_connection(&conn).await.unwrap();

        // Give the stream task a couple of (failing) reconnect attempts.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let handle = pool.get("uuid-logs-off").await.expect("handle");
        assert!(!handle.is_online().await, "stub must stay offline");

        let sources = collect_tamad_log_sources(&pool, 200).await;
        assert!(sources.is_empty(), "offline tamad must yield no sources");
        assert!(
            stub.logs_requests.lock().await.is_empty(),
            "no Logs RPC may be attempted for an offline tamad"
        );

        guard.finish().await;
    }

    /// Happy path: one online tamad reporting one `ready` process → the
    /// tail becomes a `{tamad-host}:{model}` source; the RPC carries both
    /// the provider name and the model name (precise routing).
    #[tokio::test]
    async fn test_collect_tamad_log_sources_happy_path() {
        let guard = crate::testing::postgres::with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());
        let stub = logs_stub(
            0,
            vec![process("model-x", "ready")],
            vec!["engine line one".into(), "engine line two".into()],
        );
        let addr = start_stub(stub.clone()).await;
        let pool = TamadPool::new(db_pool).with_backoff_base(std::time::Duration::from_millis(20));
        let conn = grpc_conn("uuid-logs-hp", "hostA", &format!("grpc://{addr}"));
        pool.upsert_connection(&conn).await.unwrap();
        let handle = pool.get("uuid-logs-hp").await.expect("handle");

        assert!(
            wait_for(|| async {
                handle.is_online().await
                    && handle
                        .latest()
                        .await
                        .map(|s| s.processes.len() == 1)
                        .unwrap_or(false)
            })
            .await,
            "handle should come online with the process snapshot"
        );

        let sources = collect_tamad_log_sources(&pool, 200).await;
        assert_eq!(sources.len(), 1, "one source for one ready process");
        assert_eq!(sources[0].name, "hostA:model-x");
        assert_eq!(
            sources[0].lines,
            vec!["engine line one".to_string(), "engine line two".to_string()]
        );

        // The RPC carried both fields (model_name precise + provider_name).
        let reqs = stub.logs_requests.lock().await;
        assert_eq!(reqs.len(), 1, "exactly one Logs RPC");
        assert_eq!(reqs[0].model_name, "model-x");
        assert_eq!(reqs[0].provider_name, "llama.cpp");

        guard.finish().await;
    }

    /// Non-`starting`/`ready` processes are never tailed, and blank lines
    /// from the tail are filtered out of the source.
    #[tokio::test]
    async fn test_collect_tamad_log_sources_skips_inactive_and_blanks() {
        let guard = crate::testing::postgres::with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());
        let stub = logs_stub(
            0,
            vec![process("model-a", "ready"), process("model-b", "failed")],
            vec![
                "keep me".into(),
                String::new(),
                "   ".into(),
                "keep too".into(),
            ],
        );
        let addr = start_stub(stub.clone()).await;
        let pool = TamadPool::new(db_pool).with_backoff_base(std::time::Duration::from_millis(20));
        let conn = grpc_conn("uuid-logs-skip", "hostB", &format!("grpc://{addr}"));
        pool.upsert_connection(&conn).await.unwrap();
        let handle = pool.get("uuid-logs-skip").await.expect("handle");

        assert!(
            wait_for(|| async {
                handle
                    .latest()
                    .await
                    .map(|s| s.processes.len() == 2)
                    .unwrap_or(false)
            })
            .await,
            "snapshot with both processes should arrive"
        );

        let sources = collect_tamad_log_sources(&pool, 200).await;
        assert_eq!(sources.len(), 1, "failed process must not be tailed");
        assert_eq!(sources[0].name, "hostB:model-a");
        assert_eq!(
            sources[0].lines,
            vec!["keep me".to_string(), "keep too".to_string()],
            "blank lines must be filtered"
        );
        let reqs = stub.logs_requests.lock().await;
        assert_eq!(reqs.len(), 1, "only the ready process is tailed");
        assert_eq!(reqs[0].model_name, "model-a");

        guard.finish().await;
    }
}
