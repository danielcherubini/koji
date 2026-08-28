//! Structured log push — tamad half of plan-195 task 6 (stage 2a).
//!
//! The tamad captures two feeds:
//!
//! 1. **its own tracing output** — captured by [`layer::PushLogLayer`]
//!    (a `tracing` `Layer` with the same field-visit JSON encoding as the
//!    proxy-side layer, plan-195 task 2), mapped to source `"tamad"` with
//!    its real level;
//! 2. **per-model engine container logs** — the [`tails`] supervisor
//!    polls the process table at 1 s (it exposes no watch channel) and
//!    runs one `docker logs -f -t` child per container-backed model,
//!    mapped to source `"model:<model_name>"` with `level = -1`
//!    (the proxy maps that to level 2 + `level_known: false` in task 7).
//!
//! Both feeds `try_send` into ONE bounded mpsc ([`EVENT_CHANNEL_CAP`]);
//! when the peer is not known to support push (old proxy ⇒ the
//! register flag absent ⇒ `false`) the channel simply cycles — bounded
//! memory, try-drop-newest, no spurious drop markers. When the gate
//! is on, [`runtime::LogPushRuntime`] numbers every line with a
//! per-source `seq` (all `0` per boot) and writes it through the
//! bounded buffers in [`ring::LogRing`] (drop-oldest + in-stream
//! synthetic `dropped` markers).
//!
//! ## Direction — the tamad is the gRPC *server*
//!
//! The proxy (task 7's per-online-tamad ingest task — the same shape
//! as `run_stream_task` in `tama_core::tamad::pool`, which such-has the
//! proxy dial the tamad) opens `stream_logs`; the tamad's handler
//! (after `check_auth`) then streams `StreamInit{instance_id,
//! start_seq_by_source}` first, the replay ring oldest→newest, then
//! live entries, until the client disconnects. The tamad dials
//! NOTHING: no tonic client, no endpoint, no reconnect loop on this
//! side. The registration handshake flag `supports_stream_logs`
//! (absent → `false` from old proxies) is the "peer supports v2" gate
//! the runtime carries: old proxy + new tamad → nothing changes on the
//! wire.
//!

pub mod layer;
pub mod ring;
pub mod runtime;
pub mod tails;

/// One captured log event, before `seq` assignment ([`ring::PushRing`]).
///
/// `message` is already a compact JSON document:
/// `{"message": <text>, "target": <module>, ...fields}` for the tamad
/// feed, `{"message": <line>}` for engine-tail lines.
#[derive(Debug, Clone)]
pub struct PushEvent {
    /// Capture time, unix milliseconds (tamad's clock).
    pub ts: i64,
    /// `0..4` = TRACE..ERROR, `-1` = unknown (engine container line).
    pub level: i32,
    /// `"tamad"` or `"model:<model_name>"` (see [`model_source`]).
    pub source: String,
    /// The JSON document (see above).
    pub message: String,
}

impl PushEvent {
    /// Consume this event and frame it as the wire line for `seq`.
    pub fn into_line(self, seq: i64) -> tama_core::tamad::tamad_service::LoggedLine {
        tama_core::tamad::tamad_service::LoggedLine {
            ts: self.ts,
            level: self.level,
            source: self.source,
            message: self.message,
            seq,
            dropped: false,
            dropped_count: 0,
            dropped_since_ts: 0,
        }
    }
}

/// Source id the tamad stamps on its own tracing events.
pub const TAMAD_SOURCE: &str = "tamad";

/// Source id for an engine container's tail.
pub fn model_source(model_name: &str) -> String {
    format!("model:{model_name}")
}

/// Bounded transport between producers (tracing layer, container tails)
/// and the push runtime. Capacity matches the proxy-side log channel
/// (plan-195 task 2).
pub const EVENT_CHANNEL_CAP: usize = 1024;

/// Current unix time in milliseconds (tamad's clock).
pub fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
