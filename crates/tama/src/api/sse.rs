//! Shared SSE stream builders for the management API.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_stream::stream;
use axum::response::sse::Event;
use futures_util::Stream;
use serde_json::json;

use crate::web_types::{Job, JobEvent, JobStatus};
use tokio::sync::broadcast;

/// Build the SSE event stream for a job: replay the log snapshot
/// (head → skipped-marker → tail), replay any stored result, emit the
/// terminal status/error if the job already finished, then stream live
/// `JobEvent`s until a terminal status or channel close.
///
/// Subscribes BEFORE snapshotting so no line emitted in between is lost.
pub fn job_event_stream(job: Arc<Job>) -> impl Stream<Item = Result<Event, axum::Error>> {
    let mut rx = job.log_tx.subscribe();

    stream! {
        // Snapshot inside the async stream so tokio::join! works
        let (head, tail, dropped, status, _finished_at, error, stored_result) = {
            let (state, log_head, log_tail, bench_results) = tokio::join!(
                job.state.read(),
                job.log_head.read(),
                job.log_tail.read(),
                job.benchmark_results.read()
            );
            (
                log_head.iter().cloned().collect::<Vec<_>>(),
                log_tail.iter().cloned().collect::<Vec<_>>(),
                job.log_dropped.load(Ordering::Relaxed),
                state.status,
                state.finished_at,
                state.error.clone(),
                bench_results.clone(),
            )
        };

        // Replay head
        for line in head {
            yield Ok(Event::default().event("log").json_data(json!({ "line": line}))?);
        }

        // Emit skipped marker if dropped > 0
        if dropped > 0 && !tail.is_empty() {
            yield Ok(Event::default().event("log")
                .json_data(json!({ "line": format!("[... {} lines skipped ...]", dropped)}))?);
        }

        // Replay tail
        for line in tail {
            yield Ok(Event::default().event("log").json_data(json!({ "line": line}))?);
        }

        // Replay any stored job result (for benchmark jobs — late subscribers).
        if let Some(results_str) = &stored_result {
            if let Ok(results_value) = serde_json::from_str::<serde_json::Value>(results_str) {
                yield Ok(Event::default().event("result")
                    .json_data(json!({ "results": results_value}))?);
            }
        }

        // Emit final status if terminal
        if status != JobStatus::Running {
            yield Ok(Event::default().event("status")
                .json_data(json!({ "status": status}))?);
            if let Some(err) = error {
                yield Ok(Event::default().event("error")
                    .json_data(crate::api::error::error_body(err, None))?);
            }
            return; // Close after terminal job
        }

        // Live stream
        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Ok(JobEvent::Log(line)) => {
                            yield Ok(Event::default().event("log")
                                .json_data(json!({ "line": line}))?);
                        }
                        Ok(JobEvent::Status(s)) => {
                            yield Ok(Event::default().event("status")
                                .json_data(json!({ "status": s}))?);
                            if s != JobStatus::Running {
                                return; // Close on terminal status
                            }
                        }
                        Ok(JobEvent::Result(results_str)) => {
                            if let Ok(results_value) = serde_json::from_str::<serde_json::Value>(&results_str) {
                                yield Ok(Event::default().event("result")
                                    .json_data(json!({ "results": results_value}))?);
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            yield Ok(Event::default().event("log")
                                .json_data(json!({ "line": format!("[{} lines dropped]", n)}))?);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return;
                        }
                    }
                }
            }
        }
    }
}
