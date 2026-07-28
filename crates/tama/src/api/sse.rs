//! Shared SSE stream builders for the management API.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::response::sse::Event;
use futures_util::Stream;
use serde_json::json;
use tokio::sync::broadcast;

use crate::api::error::error_body;
use crate::web_types::{Job, JobEvent, JobStatus};

/// Drive a broadcast receiver into an SSE stream: map each domain event with
/// `to_event`, emit a `Lagged` marker (`{"lagged": n}`) when the receiver falls
/// behind, and end the stream when the channel closes.
///
/// This is the single receive-loop scaffolding shared by all broadcast-backed
/// SSE endpoints (pulls, updates, jobs, …).
pub fn broadcast_to_sse<E, F>(
    mut rx: broadcast::Receiver<E>,
    to_event: F,
) -> impl Stream<Item = Result<Event, axum::Error>>
where
    E: Clone + Send + 'static,
    F: Fn(&E) -> Result<Event, serde_json::Error> + Send + 'static,
{
    async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => match to_event(&event) {
                    Ok(e) => yield Ok(e),
                    Err(e) => yield Err(axum::Error::new(e)),
                },
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    match Event::default()
                        .event("Lagged")
                        .json_data(json!({ "lagged": n }))
                    {
                        Ok(e) => yield Ok(e),
                        Err(e) => yield Err(axum::Error::new(e)),
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

/// Build the SSE event stream for a job: replay the log snapshot
/// (head → skipped-marker → tail), replay any stored result, emit the
/// terminal status/error if the job already finished, then stream live
/// `JobEvent`s until a terminal status or channel close.
///
/// Subscribes BEFORE snapshotting so no line emitted in between is lost.
pub fn job_event_stream(job: Arc<Job>) -> impl Stream<Item = Result<Event, axum::Error>> {
    let mut rx = job.log_tx.subscribe();

    async_stream::stream! {
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
        if let Some(ref results_json) = stored_result {
            yield Ok(Event::default().event("result")
                .json_data(json!({ "results": results_json}))?);
        }

        // Emit final status if terminal
        if status != JobStatus::Running {
            yield Ok(Event::default().event("status")
                .json_data(json!({ "status": status}))?);
            if let Some(err) = error {
                yield Ok(Event::default().event("error")
                    .json_data(error_body(err, None))?);
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
                        Ok(JobEvent::Result(results_json)) => {
                            yield Ok(Event::default().event("result")
                                .json_data(json!({ "results": results_json}))?);
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            // Emit dropped marker
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::sse::Event;
    use futures_util::StreamExt;
    use std::sync::{Arc, Mutex};
    use tokio::sync::broadcast;

    #[derive(Debug, Clone)]
    enum TestEvent {
        Hello,
        World,
    }

    fn make_to_event(
        received: Arc<Mutex<Vec<String>>>,
    ) -> impl Fn(&TestEvent) -> Result<Event, serde_json::Error> + Send + 'static {
        move |e: &TestEvent| {
            let name = match e {
                TestEvent::Hello => "Hello",
                TestEvent::World => "World",
            };
            received.lock().unwrap().push(name.to_string());
            Ok(Event::default().event(name).data(format!("data-{name}")))
        }
    }

    #[tokio::test]
    async fn test_broadcast_to_sse_maps_events() {
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let (tx, rx) = broadcast::channel::<TestEvent>(16);
        let stream = broadcast_to_sse(rx, make_to_event(received.clone()));
        tokio::pin!(stream);

        tx.send(TestEvent::Hello).unwrap();
        tx.send(TestEvent::World).unwrap();
        drop(tx);

        let mut count = 0;
        while let Some(item) = stream.next().await {
            let _ = item.unwrap();
            count += 1;
        }

        assert_eq!(count, 2);
        assert_eq!(*received.lock().unwrap(), vec!["Hello", "World"]);
    }

    #[tokio::test]
    async fn test_broadcast_to_sse_lagged_marker() {
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let (tx, rx) = broadcast::channel::<TestEvent>(1);
        let stream = broadcast_to_sse(rx, make_to_event(received.clone()));
        tokio::pin!(stream);

        tx.send(TestEvent::Hello).unwrap();
        tx.send(TestEvent::World).unwrap();
        drop(tx);

        let mut count = 0;
        while let Some(item) = stream.next().await {
            let _ = item.unwrap();
            count += 1;
        }

        // Lagged(1) → Ok(World) → Closed  →  2 items, to_event called once (World only)
        assert_eq!(count, 2);
        assert_eq!(*received.lock().unwrap(), vec!["World"]);
    }

    #[tokio::test]
    async fn test_broadcast_to_sse_closed_ends_stream() {
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let (tx, rx) = broadcast::channel::<TestEvent>(16);
        let stream = broadcast_to_sse(rx, make_to_event(received.clone()));
        tokio::pin!(stream);

        drop(tx);

        let mut count = 0;
        while let Some(item) = stream.next().await {
            let _ = item.unwrap();
            count += 1;
        }

        assert_eq!(count, 0);
        assert!(received.lock().unwrap().is_empty());
    }
}
