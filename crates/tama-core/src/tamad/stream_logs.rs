//! Proxy-side `StreamLogs` ingest (plan-195 task 7, stage 2b).
//!
//! One long-lived ingest task per tamad connection (spawned alongside
//! `run_stream_task` by [`crate::tamad::pool::TamadPool`] with the same
//! cancel/backoff lifecycle): the PROXY dials the tamad's `StreamLogs`
//! RPC (`StreamInit`, then replay, then live), dedupes the
//! (instance_id, seq) replays through the shared pure
//! [`crate::logstore::DedupState`], and enqueues the accepted lines into
//! the SAME bounded `LogRecord` channel the proxy's own tracing layer
//! uses — the rows are indistinguishable from the proxy's own once
//! stored.
//!
//! ## Enqueue shape
//!
//! `LogRecord { ts: line.ts, level: mapped, source: Source::tamad(host)
//! | Source::tamad_model(host, model), msg: parsed doc }`. The wire
//! `message` field is a JSON doc string (`{"message":…, "target":…, …}`);
//! it is parsed with `serde_json::from_str` and falls back to
//! `{"message": <raw>}` on parse failure. FLAT top-level keys
//! `instance_id`, `host`, and `seq` are added to the doc (flat, not
//! nested — friendlier to FTS; the `source` label already carries the
//! host prefix). Lines with `level == -1` (unknown: engine container
//! lines) map to [`LogstoreLevel::INFO`] and carry
//! `level_known: false` in the doc; known levels (0..=4) map straight
//! through and carry no `level_known` key.
//!
//! ## Shutdown criteria
//!
//! - cancel (pool remove / connection replacement) — like
//!   `run_stream_task`.
//! - dial status `UNIMPLEMENTED` / `NOT_FOUND` / `UNAUTHENTICATED` — the
//!   tamad is too old, untouched, or refuses the token permanently; the
//!   task STOPS (logged once), no reconnect loop.
//! - the shared log channel is closed (writer torn down) — the task
//!   stops.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::logstore::{Decision, DedupState, LogRecord, LogstoreLevel, Source};
use crate::providers::TamadConnection;
use crate::tamad::client::TamadClient;
use crate::tamad::tamad_service::stream_log_message::Kind;
use crate::tamad::{LoggedLine, StreamLogMessage};
use tokio::sync::mpsc;

/// Maximum reconnect backoff delay (mirrors `run_stream_task`).
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Poll cadence while the shared log channel is not wired yet (None).
const LOG_TX_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Map a wire level to the store domain.
///
/// Returns `(level, level_known)`: `-1` (unknown — engine container
/// lines) and any other out-of-domain value map to
/// [`LogstoreLevel::INFO`] with `level_known = false`; `0..=4` pass
/// through as-is with `level_known = true`.
pub fn map_level(level: i32) -> (LogstoreLevel, bool) {
    if level == -1 {
        return (LogstoreLevel::INFO, false);
    }
    match LogstoreLevel::from_u8(level as u8) {
        Some(l) if level >= 0 => (l, true),
        _ => (LogstoreLevel::INFO, false),
    }
}

/// Classify the wire line's HOST-RELATIVE `source` (`"tamad"` |
/// `"model:<model>"`) into the store's host-prefixed taxonomy:
/// `tamad:<host>` for control lines, `tamad:<host>:model:<model>` for
/// engine lines. Defensive fallback: anything not exactly `"tamad"` or
/// a `"model:<name>"` prefix lands on the host line.
pub fn line_source(host_name: &str, host_relative_source: &str) -> Source {
    if host_relative_source == "tamad" {
        Source::tamad(host_name)
    } else if let Some(model) = host_relative_source.strip_prefix("model:") {
        Source::tamad_model(host_name, model)
    } else {
        Source::tamad(host_name)
    }
}

/// Build the enqueued [`LogRecord`] for one accepted tamad line (pure —
/// see the module docs for the doc shape and flat keys).
pub fn line_to_record(line: &LoggedLine, instance_id: &str, host_name: &str) -> LogRecord {
    let (level, level_known) = map_level(line.level);
    let mut doc: serde_json::Value = serde_json::from_str(&line.message)
        .unwrap_or_else(|_| json!({ "message": line.message.clone() }));
    // A parsed but non-object doc (e.g. a bare array) cannot carry the
    // flat keys — degrade to the fallback doc around the raw text.
    if !doc.is_object() {
        doc = json!({ "message": line.message.clone() });
    }
    let obj = doc.as_object_mut().expect("object doc above");
    obj.insert("instance_id".to_string(), json!(instance_id));
    obj.insert("host".to_string(), json!(host_name));
    obj.insert("seq".to_string(), json!(line.seq));
    if !level_known {
        obj.insert("level_known".to_string(), json!(false));
    }
    LogRecord {
        ts: line.ts,
        level,
        source: line_source(host_name, &line.source),
        msg: doc,
    }
}

/// Dial statuses that make the ingest task STOP (log once, no
/// reconnect): the tamad does not implement `StreamLogs` (old binary —
/// the task-6 flag/refusal contract), the path is not found, or the
/// stored token is permanently refused.
pub(crate) fn is_terminal_ingest_status(code: tonic::Code) -> bool {
    matches!(
        code,
        tonic::Code::Unimplemented | tonic::Code::NotFound | tonic::Code::Unauthenticated
    )
}

/// Spawn the per-tamad `StreamLogs` ingest task (the pool-side entry
/// point; see the module docs).
///
/// - `tamad_id` — the registry id of the tamad (dedupe key + logs).
/// - `conn` — the registered connection record (same TLS/auth options
///   the rest of the pool's client uses).
/// - `log_tx` — the pool-owned holder for the shared `LogRecord`
///   channel sender (set once at boot; `None` = ingest disabled, the
///   task polls until it is wired).
/// - `dedupe` — the pool-owned shared [`DedupState`] (one mutex for all
///   tamads; lock-per-frame).
/// - `host_name` — the tamad's registered host label (the store's
///   `tamad:<host>` prefix).
/// - `cancel` — the pool handle's cancel watch (stopped on
///   remove / connection replacement).
/// - `backoff_base` — reconnect backoff base (mirrors
///   `run_stream_task`).
pub(crate) fn spawn_stream_logs_ingest(
    tamad_id: String,
    conn: TamadConnection,
    log_tx: Arc<Mutex<Option<mpsc::Sender<LogRecord>>>>,
    dedupe: Arc<tokio::sync::Mutex<DedupState>>,
    host_name: String,
    cancel: tokio::sync::watch::Receiver<bool>,
    backoff_base: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_stream_logs_ingest(
        tamad_id,
        conn,
        log_tx,
        dedupe,
        host_name,
        cancel,
        backoff_base,
    ))
}

/// The ingest loop itself (see [`spawn_stream_logs_ingest`]).
async fn run_stream_logs_ingest(
    tamad_id: String,
    conn: TamadConnection,
    log_tx: Arc<Mutex<Option<mpsc::Sender<LogRecord>>>>,
    dedupe: Arc<tokio::sync::Mutex<DedupState>>,
    host_name: String,
    cancel: tokio::sync::watch::Receiver<bool>,
    backoff_base: Duration,
) {
    // HTTP-protocol connections have no log stream.
    if !conn.protocol.is_grpc() {
        tracing::debug!(tamad_id = %tamad_id, "tamad uses HTTP protocol; no log stream");
        return;
    }
    let mut cancel_rx = cancel;
    let client = TamadClient::new(&conn);
    let mut backoff = backoff_base;

    loop {
        if *cancel_rx.borrow_and_update() {
            return;
        }

        // Defensively disabled until the boot wiring sets the channel:
        // poll the holder on a short beat.
        let Some(tx) = log_tx.lock().unwrap().clone() else {
            tracing::debug!(
                tamad_id = %tamad_id,
                "log channel not wired yet; holding off StreamLogs ingest"
            );
            if sleep_or_cancel(&mut cancel_rx, LOG_TX_POLL_INTERVAL).await {
                return;
            }
            continue;
        };

        let mut stream = match client.stream_logs().await {
            Ok(stream) => stream,
            Err(e) => {
                let code = e
                    .downcast_ref::<tonic::Status>()
                    .map(|status| status.code());
                if code.is_some_and(is_terminal_ingest_status) {
                    tracing::debug!(
                        tamad_id = %tamad_id,
                        code = ?code,
                        "StreamLogs unavailable from tamad (old tamad / bad token); stopping ingest"
                    );
                    return;
                }
                tracing::debug!(tamad_id = %tamad_id, error = %e, "StreamLogs dial failed");
                if sleep_or_cancel(&mut cancel_rx, backoff).await {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        // Stream open — reset the backoff and the per-stream identity.
        tracing::info!(tamad_id = %tamad_id, "tamad StreamLogs connected");
        backoff = backoff_base;
        let mut init_instance: Option<String> = None;

        'frames: loop {
            tokio::select! {
                _ = cancel_rx.changed() => break 'frames,
                item = stream.message() => {
                    match item {
                        Ok(Some(msg)) => {
                            if !consume_frame(&mut init_instance, &tamad_id, &host_name, &tx, &dedupe, &msg).await {
                                // The shared channel closed (writer
                                // torn down) — stop the task.
                                return;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                tamad_id = %tamad_id,
                                error = %e,
                                "StreamLogs error; reconnecting"
                            );
                            break 'frames;
                        }
                        Ok(None) => {
                            tracing::debug!(
                                tamad_id = %tamad_id,
                                "StreamLogs closed by tamad; reconnecting"
                            );
                            break 'frames;
                        }
                    }
                }
            }
        }

        if *cancel_rx.borrow() {
            return;
        }
        if sleep_or_cancel(&mut cancel_rx, backoff).await {
            return;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Consume one stream frame: `StreamInit` → `on_init` per source;
/// `line` → (defensive `on_init` for the line's source) + dedupe
/// decision + optional enqueue. Returns `false` only when the shared
/// channel is closed (stop the task).
async fn consume_frame(
    init_instance: &mut Option<String>,
    tamad_id: &str,
    host_name: &str,
    tx: &mpsc::Sender<LogRecord>,
    dedupe: &Arc<tokio::sync::Mutex<DedupState>>,
    msg: &StreamLogMessage,
) -> bool {
    let Some(kind) = msg.kind.as_ref() else {
        return true; // defensive: empty envelope
    };
    match kind {
        Kind::Init(init) => {
            let id = init.instance_id.clone();
            let mut state = dedupe.lock().await;
            for source in init.start_seq_by_source.keys() {
                state.on_init(tamad_id, source, &id);
            }
            *init_instance = Some(id);
            true
        }
        Kind::Line(line) => {
            // StreamInit ALWAYS precedes the lines of a (re)connected
            // stream; a line that arrives none is a protocol violation —
            // skip it (log once-tolerant: debug-level per frame).
            let Some(instance_id) = init_instance.as_deref() else {
                tracing::debug!(
                    tamad_id,
                    host = %host_name,
                    "StreamLogs line before StreamInit; skipped"
                );
                return true;
            };
            let mut state = dedupe.lock().await;
            // Defensive: a source that the init did not announce (e.g. a
            // freshly-created model source) is announced here — the
            // judgement underneath is unchanged (the line's id is the
            // stream's own).
            state.on_init(tamad_id, &line.source, instance_id);
            let decision = state.on_message(tamad_id, &line.source, instance_id, line.seq);
            drop(state);
            match decision {
                Decision::Duplicate => true,
                _ => {
                    let record = line_to_record(line, instance_id, host_name);
                    // A blocked send applies gentle backpressure to the
                    // tamad (its send buffer fills; the tamad drop-newest
                    // policy handles it). A CLOSED channel means the
                    // writer is gone — stop.
                    if tx.send(record).await.is_err() {
                        tracing::info!(
                            tamad_id,
                            "shared log channel closed; stopping StreamLogs ingest"
                        );
                        return false;
                    }
                    true
                }
            }
        }
    }
}

/// Sleep for `duration`, waking early when cancel is set; returns
/// `true` when cancelled.
async fn sleep_or_cancel(
    cancel_rx: &mut tokio::sync::watch::Receiver<bool>,
    duration: Duration,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        _ = cancel_rx.changed() => true,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use crate::tamad::pool::test_support::{
        grpc_conn as test_grpc_conn, start_stub, stub_default, wait_for,
    };
    use crate::tamad::tamad_service::StreamInit;

    fn init_frame(instance_id: &str, sources: &[(&str, i64)]) -> StreamLogMessage {
        StreamLogMessage {
            kind: Some(Kind::Init(StreamInit {
                instance_id: instance_id.to_string(),
                start_seq_by_source: sources.iter().map(|(s, v)| (s.to_string(), *v)).collect(),
            })),
        }
    }

    fn line_frame(seq: i64, level: i32, source: &str, message: &str) -> StreamLogMessage {
        StreamLogMessage {
            kind: Some(Kind::Line(LoggedLine {
                ts: 1_700_000_000_000 + seq * 100,
                level,
                source: source.to_string(),
                message: message.to_string(),
                seq,
                dropped: false,
                dropped_count: 0,
                dropped_since_ts: 0,
            })),
        }
    }

    fn empty_cancel() -> (
        tokio::sync::watch::Sender<bool>,
        tokio::sync::watch::Receiver<bool>,
    ) {
        tokio::sync::watch::channel(false)
    }

    /// Test channel holder wiring: builds the `log_tx` holder (already
    /// set) + hands back the receiver on the other end.
    type TxHolder = Arc<Mutex<Option<mpsc::Sender<LogRecord>>>>;
    fn wired_tx() -> (TxHolder, mpsc::Receiver<LogRecord>) {
        let (tx, rx) = mpsc::channel::<LogRecord>(64);
        (Arc::new(Mutex::new(Some(tx))), rx)
    }

    /// Empty (unwired) holder.
    fn unwired_tx() -> TxHolder {
        Arc::new(Mutex::new(None))
    }

    // ── pure unit tests ──

    /// `-1 → INFO + level_known:false`; `0..=4` pass through; other
    /// out-of-domain values degrade to `INFO, unknown`.
    #[test]
    fn test_level_mapping() {
        assert_eq!(
            map_level(-1),
            (LogstoreLevel::INFO, false),
            "-1 is the engine-container unknown"
        );
        assert_eq!(map_level(0), (LogstoreLevel::TRACE, true));
        assert_eq!(map_level(1), (LogstoreLevel::DEBUG, true));
        assert_eq!(map_level(2), (LogstoreLevel::INFO, true));
        assert_eq!(map_level(3), (LogstoreLevel::WARN, true));
        assert_eq!(map_level(4), (LogstoreLevel::ERROR, true));
        assert_eq!(
            map_level(7),
            (LogstoreLevel::INFO, false),
            "out-of-domain → unknown"
        );
        assert_eq!(map_level(-2), (LogstoreLevel::INFO, false));
    }

    /// Source taxonomy: the host label is added to the host-relative
    /// wire source.
    #[test]
    fn test_line_source_taxonomy() {
        assert_eq!(line_source("gpu-box", "tamad").as_str(), "tamad:gpu-box");
        assert_eq!(
            line_source("gpu-box", "model:qwen3").as_str(),
            "tamad:gpu-box:model:qwen3"
        );
        // Defensive fallbacks land on the host line.
        assert_eq!(line_source("gpu-box", "").as_str(), "tamad:gpu-box");
        assert_eq!(
            line_source("gpu-box", "model:").as_str(),
            "tamad:gpu-box:model:"
        );
        assert_eq!(line_source("gpu-box", "other").as_str(), "tamad:gpu-box");
    }

    /// Enqueue doc shape: parsed doc keeps its own keys + flat
    /// `instance_id`/`host`/`seq`; `level_known:false` only for -1;
    /// unparseable raw → fallback `{"message": raw}` + flat keys.
    #[test]
    fn test_line_to_record_shape() {
        let line = LoggedLine {
            ts: 42,
            level: -1,
            source: "model:m".to_string(),
            message: r#"{"message":"engine boom","target":"eng"}"#.to_string(),
            seq: 7,
            dropped: false,
            dropped_count: 0,
            dropped_since_ts: 0,
        };
        let rec = line_to_record(&line, "boot-1", "gpu-box");
        assert_eq!(rec.ts, 42);
        assert_eq!(rec.level, LogstoreLevel::INFO, "-1 maps to INFO");
        assert_eq!(rec.source.as_str(), "tamad:gpu-box:model:m");
        let msg = rec.msg.as_object().expect("doc object");
        assert_eq!(msg.get("message"), Some(&json!("engine boom")));
        assert_eq!(msg.get("target"), Some(&json!("eng")));
        assert_eq!(msg.get("instance_id"), Some(&json!("boot-1")));
        assert_eq!(msg.get("host"), Some(&json!("gpu-box")));
        assert_eq!(msg.get("seq"), Some(&json!(7)));
        assert_eq!(msg.get("level_known"), Some(&json!(false)));
        assert_eq!(
            msg.len(),
            6,
            "message, target, instance_id, host, seq, level_known"
        );

        // Known level: no level_known key.
        let mut l4 = line.clone();
        l4.level = 4;
        let rec4 = line_to_record(&l4, "boot-1", "gpu-box");
        assert_eq!(rec4.level, LogstoreLevel::ERROR);
        assert!(rec4.msg.get("level_known").is_none());

        // Unparseable raw → fallback doc around the raw text.
        let mut raw = line.clone();
        raw.level = 2;
        raw.source = "tamad".to_string();
        raw.message = "plain engine line".to_string();
        let recraw = line_to_record(&raw, "boot-1", "gpu-box");
        assert_eq!(recraw.source.as_str(), "tamad:gpu-box");
        assert_eq!(
            recraw.msg.get("message"),
            Some(&json!("plain engine line")),
            "raw text preserved under message"
        );
        assert_eq!(recraw.msg.get("instance_id"), Some(&json!("boot-1")));

        // A parsed but non-object doc degrades to the fallback shape.
        let mut arr = line.clone();
        arr.message = "[1, 2]".to_string();
        let recarr = line_to_record(&arr, "boot-1", "gpu-box");
        assert!(recarr.msg.is_object());
        assert_eq!(
            recarr.msg.get("message"),
            Some(&json!("[1, 2]")),
            "non-object doc preserved as the raw message text"
        );
    }

    // ── in-process integration tests (StubTamad scripted frames) ────

    /// Full ingest path against the in-process stub: `StreamInit`,
    /// normal lines (both taxonomies), an immediate duplicate seq (NO
    /// second row), a `level = -1` engine line (INFO + `level_known`
    /// false), and a new boot announced by a second `StreamInit`
    /// (NewInstance → its first lines are enqueued). Exactly the
    /// expected records — no more — land in the shared channel.
    #[tokio::test]
    async fn test_stream_ingest_frames_dedupe_and_enqueue() {
        let stub = stub_default();
        let addr = start_stub(stub.clone()).await;
        let url = format!("grpc://{addr}");
        let conn = test_grpc_conn("h1", "gpu-box", &url);
        let (hh, mut rx) = wired_tx();
        let dedupe = Arc::new(tokio::sync::Mutex::new(DedupState::new()));
        let (cancel_tx, cancel_rx) = empty_cancel();

        // Boot 1: init + a control line, an engine line, then the SAME
        // control seq (replaying) right after, then an unknown-level
        // engine line, then the next control seq.
        stub.stream_log_frames.lock().await.extend([
            init_frame("boot-1", &[("tamad", 0), ("model:m", 0)]),
            line_frame(1, 2, "tamad", r#"{"message":"tamad hello","target":"t"}"#),
            line_frame(1, 4, "model:m", r#"{"message":"engine boom","target":"m"}"#),
            line_frame(1, 2, "tamad", r#"{"message":"tamad hello","target":"t"}"#), // DUP
            line_frame(2, -1, "model:m", "plain engine line"),
            line_frame(2, 3, "tamad", r#"{"message":"warn line"}"#),
        ]);

        let task = spawn_stream_logs_ingest(
            "h1".to_string(),
            conn,
            Arc::clone(&hh),
            dedupe,
            "gpu-box".to_string(),
            cancel_rx,
            Duration::from_millis(20),
        );

        // Exactly 4 records (the six frames are: init + 4 fresh lines +
        // one replayed seq), in stream order, and nothing beyond them.
        let mut got: Vec<LogRecord> = Vec::new();
        while got.len() < 4 {
            got.push(
                tokio::time::timeout(Duration::from_secs(5), rx.recv())
                    .await
                    .expect("record within 5s")
                    .expect("receiver open"),
            );
        }
        if let Ok(r) = rx.try_recv() {
            panic!("unexpected extra record: {:?}", r.msg);
        }
        let ts_base = 1_700_000_000_000;
        // 1: control line.
        assert_eq!(got[0].source.as_str(), "tamad:gpu-box");
        assert_eq!(got[0].level, LogstoreLevel::INFO);
        assert_eq!(got[0].ts, ts_base + 100);
        let d0 = got[0].msg.as_object().unwrap();
        assert_eq!(d0.get("message"), Some(&json!("tamad hello")));
        assert_eq!(d0.get("target"), Some(&json!("t")));
        assert_eq!(d0.get("instance_id"), Some(&json!("boot-1")));
        assert_eq!(d0.get("host"), Some(&json!("gpu-box")));
        assert_eq!(d0.get("seq"), Some(&json!(1)));
        assert!(d0.get("level_known").is_none(), "known level: no key");
        // 2: engine line (ERROR).
        assert_eq!(got[1].source.as_str(), "tamad:gpu-box:model:m");
        assert_eq!(got[1].level, LogstoreLevel::ERROR);
        let d1 = got[1].msg.as_object().unwrap();
        assert_eq!(d1.get("seq"), Some(&json!(1)));
        assert_eq!(d1.get("instance_id"), Some(&json!("boot-1")));
        // 3: engine line, unknown level.
        assert_eq!(got[2].level, LogstoreLevel::INFO, "-1 maps to INFO");
        let d2 = got[2].msg.as_object().unwrap();
        assert_eq!(
            d2.get("level_known"),
            Some(&json!(false)),
            "doc level_known:false"
        );
        assert_eq!(
            d2.get("message"),
            Some(&json!("plain engine line")),
            "raw preserved"
        );
        assert_eq!(d2.get("seq"), Some(&json!(2)));
        // 4: next control line.
        assert_eq!(got[3].level, LogstoreLevel::WARN);
        assert_eq!(got[3].msg.get("seq"), Some(&json!(2)));
        assert_eq!(got[3].msg.get("message"), Some(&json!("warn line")));

        // New boot announced on the same live stream: its first lines
        // are enqueued (NewInstance + rule-2 Fresh on the 2nd).
        stub.stream_log_frames.lock().await.extend([
            init_frame("boot-2", &[("tamad", 0), ("model:m", 0)]),
            line_frame(1, 2, "model:m", r#"{"message":"engine re-boot"}"#),
            line_frame(2, 2, "model:m", r#"{"message":"engine again"}"#),
        ]);
        let mut boot2 = Vec::new();
        for _ in 0..2 {
            boot2.push(
                tokio::time::timeout(Duration::from_secs(5), rx.recv())
                    .await
                    .expect("boot-2 record within 5s")
                    .expect("receiver open"),
            );
        }
        assert_eq!(boot2[0].msg.get("instance_id"), Some(&json!("boot-2")));
        assert_eq!(boot2[0].msg.get("message"), Some(&json!("engine re-boot")));
        assert_eq!(boot2[1].msg.get("seq"), Some(&json!(2)));
        if let Ok(r) = rx.try_recv() {
            panic!("no third boot-2 row expected; got {:?}", r.msg);
        }

        // One dial for the whole stream's life; nothing extra queued.
        assert_eq!(
            stub.stream_log_calls.load(Ordering::SeqCst),
            1,
            "one dial, no spurious redials"
        );

        cancel_tx.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("task ends on cancel")
            .expect("task does not panic");
    }

    /// Reconnect ring replay: the stream drops and re-dials; the stub
    /// replays the SAME frames (same init, same lines); the already-seen
    /// seq's produce NO further rows, and the first genuinely-new line
    /// after the replay does. (Duplicate-seq count assertion across dials.)
    #[tokio::test]
    async fn test_stream_ingest_reconnect_replay_dedupes() {
        let stub = stub_default();
        let addr = start_stub(stub.clone()).await;
        let url = format!("grpc://{addr}");
        let conn = test_grpc_conn("h2", "gpu-box", &url);
        let (hh, mut rx) = wired_tx();
        let dedupe = Arc::new(tokio::sync::Mutex::new(DedupState::new()));
        let (_cancel_tx, cancel_rx) = empty_cancel();

        stub.stream_log_frames.lock().await.extend([
            init_frame("boot-1", &[("tamad", 0)]),
            line_frame(1, 2, "tamad", r#"{"message":"a"}"#),
            line_frame(2, 2, "tamad", r#"{"message":"b"}"#),
        ]);
        let task = spawn_stream_logs_ingest(
            "h2".to_string(),
            conn,
            Arc::clone(&hh),
            dedupe,
            "gpu-box".to_string(),
            cancel_rx,
            Duration::from_millis(20),
        );
        // Two fresh rows from the initial dial.
        let r1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("line a")
            .expect("receiver open");
        assert_eq!(r1.msg.get("message"), Some(&json!("a")));
        let r2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("line b")
            .expect("receiver open");
        assert_eq!(r2.msg.get("message"), Some(&json!("b")));
        if let Ok(r) = rx.try_recv() {
            panic!(
                "no row expected between dial 1 and the tamad restart: {:?}",
                r.msg
            );
        }

        // The tamad dies mid-stream: the stub's stream ends and the
        // ingest task redials; the stub replays the SAME frames from
        // the queue start (init + a + b) — all dups now.
        stub.down.send_replace(true);
        assert!(
            wait_for(|| async { stub.stream_log_calls.load(Ordering::SeqCst) >= 2 }).await,
            "ingest task redials after the stream ends"
        );

        // Live after the replay: the first genuinely-new line lands.
        stub.stream_log_frames
            .lock()
            .await
            .push(line_frame(3, 2, "tamad", r#"{"message":"c"}"#));
        let r3 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("line c after replay")
            .expect("receiver open");
        assert_eq!(r3.msg.get("message"), Some(&json!("c")));
        assert_eq!(r3.msg.get("seq"), Some(&json!(3)));
        if let Ok(r) = rx.try_recv() {
            panic!("replayed a/b must not be re-enqueued: {:?}", r.msg);
        }

        task.abort();
    }

    /// `UNIMPLEMENTED` on dial (old tamad) → the task logs once and
    /// STOPS: exactly one dial, no reconnect attempts after it, and the
    /// task handle completes.
    #[tokio::test]
    async fn test_stream_ingest_stops_on_unimplemented() {
        let mut stub = stub_default();
        stub.stream_log_refuse = true;
        stub.stream_log_frames
            .lock()
            .await
            .push(line_frame(1, 2, "tamad", "never"));
        let addr = start_stub(stub.clone()).await;
        let url = format!("grpc://{addr}");
        let conn = test_grpc_conn("h3", "gpu-box", &url);
        let (hh, _rx) = wired_tx();
        let dedupe = Arc::new(tokio::sync::Mutex::new(DedupState::new()));
        let (_cancel_tx, cancel_rx) = empty_cancel();

        let task = spawn_stream_logs_ingest(
            "h3".to_string(),
            conn,
            Arc::clone(&hh),
            dedupe,
            "gpu-box".to_string(),
            cancel_rx,
            Duration::from_millis(20),
        );

        // The task stops promptly on the UNIMPLEMENTED dial.
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("task stops on UNIMPLEMENTED")
            .expect("task does not panic");
        assert_eq!(
            stub.stream_log_calls.load(Ordering::SeqCst),
            1,
            "exactly one dial"
        );
        // No reconnect retry happens afterwards.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            stub.stream_log_calls.load(Ordering::SeqCst),
            1,
            "no retry after a terminal status"
        );
    }

    /// `log_tx` holder `None` (unwired) → the task holds off (no dial);
    /// once the boot setter lands, it dials and frames are delivered.
    #[tokio::test]
    async fn test_stream_ingest_waits_for_channel() {
        let stub = stub_default();
        let addr = start_stub(stub.clone()).await;
        let url = format!("grpc://{addr}");
        let conn = test_grpc_conn("h4", "gpu-box", &url);
        let holder = unwired_tx();
        let dedupe = Arc::new(tokio::sync::Mutex::new(DedupState::new()));
        let (_cancel_tx, cancel_rx) = empty_cancel();

        stub.stream_log_frames.lock().await.extend([
            init_frame("boot-1", &[("tamad", 0)]),
            line_frame(1, 2, "tamad", r#"{"message":"after wiring"}"#),
        ]);
        let task = spawn_stream_logs_ingest(
            "h4".to_string(),
            conn,
            Arc::clone(&holder),
            dedupe,
            "gpu-box".to_string(),
            cancel_rx,
            Duration::from_millis(20),
        );

        // Unwired: the task polls and does NOT dial.
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            stub.stream_log_calls.load(Ordering::SeqCst),
            0,
            "no dial while the channel is unwired"
        );

        // Wire the channel (boot-time setter) — ingest starts.
        let (tx, mut rx) = mpsc::channel::<LogRecord>(16);
        *holder.lock().unwrap() = Some(tx);
        let rec = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a record within 5s of wiring")
            .expect("receiver open");
        assert_eq!(rec.msg.get("message"), Some(&json!("after wiring")));
        assert_eq!(stub.stream_log_calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}
