//! Structured log read API (`/tama/v1/logs*`) + legacy tail adapter
//! (plan-195 Task 4).
//!
//! ## Endpoints
//!
//! - `GET /tama/v1/logs` — query the structured log store (newest-first
//!   by default, `cursor` pagination, FTS5 `q` with LIKE fallback) or —
//!   for `?source=tamad:*` — the on-demand legacy tail for that single
//!   source (`level`/`q` are IGNORED on tail sources; the DTO's
//!   `legacy: true` lets the UI grey them out).
//! - `GET /tama/v1/logs/sources` — distinct sources + latest ts.
//! - `GET /tama/v1/logs/summary?since=` — per-level counts.
//! - `GET /tama/v1/logs/status` — writer status snapshot.
//! - `GET /tama/v1/logs/stream` — SSE: new rows above `after` per poll.
//! - `GET /tama/v1/logs/export` — CSV export (count-first, hard cap).
//! - `DELETE /tama/v1/logs` — delete all rows (CSRF-enforced route home).
//! - `GET /tama/v1/logs/events` — SSE: writer degraded/restored frames
//!   (frames are produced by the bridge task in `main.rs`).
//!
//! ## Search + FTS5
//!
//! `q` matches the WHOLE stored JSON document (the FTS5 table indexes
//! the document, so searching structural keys like `message` matches
//! most rows — expected and documented on the endpoint). Malformed FTS
//! syntax or a no-match token transparently fall back to a `LIKE`
//! substring scan inside `LogStore::query`/`count` — search text never
//! surfaces a 500.
//!
//! ## Concurrency
//!
//! [`LogStore`] is `Send` but NOT `Sync` (one `rusqlite::Connection`),
//! so state carries `Arc<Mutex<LogStore>>` and every handler runs its
//! store call in `spawn_blocking` (matches `backend_logs.rs`). Readers
//! block each other briefly — acceptable at viewer scale; the writer
//! holds its own separate connection.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Json, Response, Sse};
use futures_util::stream::Stream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use crate::logging::tail_lines;
use crate::logstore::{
    LogEntry, LogQuery, LogStore, LogStoreStatus, LogstoreLevel, QueryOrder, Source,
};
use crate::tamad::pool::TamadPool;

/// Hard cap of `GET /tama/v1/logs/export` rows (check RUN before
/// streaming; `413` when exceeded).
pub const EXPORT_CAP_ROWS: i64 = 50_000;
/// Hard cap of the `q` search term length in characters (`400` above).
pub const MAX_Q_CHARS: usize = 512;
/// Lines fetched per on-demand legacy tail.
pub const TAIL_LINES: usize = 200;
/// `log_stream_frames` poll interval.
pub const STREAM_POLL: Duration = Duration::from_secs(1);
/// Rows fetched per `log_stream_frames` poll.
pub const STREAM_PAGE: i64 = 200;
/// Default legacy tail cache TTL (concurrent UI polls reuse a fetch).
pub const TAIL_CACHE_TTL: Duration = Duration::from_secs(5);

// ── State ────────────────────────────────────────────────────────────────

/// Read-endpoint projection of the web state, applied to the router as
/// `Extension<LogsApiState>` alongside the existing `Extension<WebState>`.
///
/// The axum handlers live here (tama-core), but `WebState` is defined
/// in the `tama` crate (the web UI owns web-specific state — see
/// `web_types.rs`) and cannot be named from this module. `router.rs`
/// builds this state once from the `WebState` fields (all cheap `Arc`
/// clones) and applies it as a second `Extension` layer.
#[derive(Clone)]
pub struct LogsApiState {
    /// Second read-only log-store connection (the writer owns the
    /// store's single write connection; WAL allows one writer + N
    /// readers; `Mutex` because `LogStore` is `Send` but not `Sync`).
    /// `None` when the log runtime is not wired (tests): queries
    /// return an empty row set, the other endpoints `503`.
    pub log_read: Option<Arc<Mutex<LogStore>>>,
    /// On-demand legacy tail provider (`tamad:*` RPC tails + local
    /// `*.log` tails). `None` when not wired (tests): tail sources
    /// return an empty row set.
    pub log_tail: Option<Arc<dyn LogTailProvider>>,
    /// Writer status receiver for `/logs/status` (`borrow_and_update`).
    /// `None` → healthy zeros.
    pub log_status: Option<Arc<tokio::sync::watch::Receiver<LogStoreStatus>>>,
    /// Per-endpoint broadcast for `/logs/events` SSE (same pattern as
    /// `WebState.log_events_tx` / the update-tx mirror: the handler
    /// creates the channel on first connect; `main.rs`'s bridge task
    /// publishes degraded/restored frames onto it).
    pub log_events_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::broadcast::Sender<String>>>>,
}

// ── DTO ──────────────────────────────────────────────────────────────────

/// JSON wire row for every `/tama/v1/logs*` read endpoint.
///
/// `message` / `dropped` / `dropped_count` are flattened out of the
/// stored JSON document; `fields` is the document minus those known
/// keys plus `dropped_since_ts` / `level_known` — the document's
/// `target` stays INSIDE `fields` and is never used as the message.
///
/// id convention: store rows have their positive SQLite rowid. Legacy
/// tail rows have a synthetic `id = -(fetch_ts_ms * 1000 + line_ordinal)`
/// — always negative, unique within a fetch, ordered by line ordinal,
/// and never colliding with a real (positive) id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntryDto {
    pub id: i64,
    /// Unix milliseconds.
    pub ts: i64,
    /// Level name (`"info"` etc.; `"info"` with `level_known: false`
    /// on legacy tail rows).
    pub level: String,
    pub source: String,
    pub message: String,
    pub fields: Map<String, Value>,
    /// Drop-marker rows only (`dropped: true`, with `dropped_count`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_count: Option<i64>,
    /// Store rows always know their level; legacy tail rows are guess
    /// (`false`).
    pub level_known: bool,
    /// `Some(true)` on on-demand legacy tail rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy: Option<bool>,
}

/// Known document keys flattened into the DTO (the rest become `fields`).
const DOC_KNOWN_KEYS: &[&str] = &[
    "message",
    "dropped",
    "dropped_count",
    "dropped_since_ts",
    "level_known",
];

impl LogEntryDto {
    /// Flatten a stored row into its DTO shape.
    pub fn from_entry(e: &LogEntry) -> Self {
        let mut fields = Map::new();
        if let Some(obj) = e.msg.as_object() {
            for (k, v) in obj {
                if !DOC_KNOWN_KEYS.contains(&k.as_str()) {
                    fields.insert(k.clone(), v.clone());
                }
            }
        }
        Self {
            id: e.id,
            ts: e.ts,
            level: e.level.as_str().to_string(),
            source: e.source.as_str().to_string(),
            message: e
                .msg
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            fields,
            dropped: e.msg.get("dropped").and_then(|v| v.as_bool()),
            dropped_count: e.msg.get("dropped_count").and_then(|v| v.as_i64()),
            level_known: true,
            legacy: None,
        }
    }

    /// A legacy tail row: `fetch_ts` is the fetch time (unix ms, NOT a
    /// per-line timestamp — legacy sources are unstructured text),
    /// `ordinal` is the line index within the fetched tail.
    pub fn from_tail_line(source: &str, fetch_ts: i64, ordinal: usize, line: &str) -> Self {
        Self {
            id: -(fetch_ts * 1000 + ordinal as i64),
            ts: fetch_ts,
            level: LogstoreLevel::INFO.as_str().to_string(),
            source: source.to_string(),
            message: line.to_string(),
            fields: Map::new(),
            dropped: None,
            dropped_count: None,
            level_known: false,
            legacy: Some(true),
        }
    }
}

// ── Query params + validation ────────────────────────────────────────────

/// Parameter validation failures (`400` + `ValidationError`).
#[derive(Debug)]
pub enum LogParamError {
    InvalidLevel(String),
    InvalidOrder(String),
    /// `since` / `until` / `limit` / `cursor` / `after` are not integers.
    InvalidNumber(String),
    QTooLong,
    /// The `source` param was supplied more than once (v1 is
    /// single-source; multi-source selection is future work).
    RepeatedSource,
    UnsupportedFormat(String),
}

impl LogParamError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::InvalidLevel(v) => {
                format!("invalid level {v:?} (expected trace, debug, info, warn or error)")
            }
            Self::InvalidOrder(v) => format!("invalid order {v:?} (expected desc or asc)"),
            Self::InvalidNumber(v) => format!("invalid {v} (expected an integer)"),
            Self::QTooLong => format!("q is limited to {MAX_Q_CHARS} characters"),
            Self::RepeatedSource => "source may only be given once".to_string(),
            Self::UnsupportedFormat(v) => format!("unsupported format {v:?} (only csv)"),
        }
    }
}

/// URL-decode the request's query string into (key, value) pairs, in
/// order (repeated keys preserved — [`LogParamError::RepeatedSource`
/// detection relies on it).
fn query_pairs(req: &axum::http::Request<axum::body::Body>) -> Vec<(String, String)> {
    req.uri()
        .query()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn first_of<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn opt_i64(pairs: &[(String, String)], key: &str) -> Result<Option<i64>, LogParamError> {
    first_of(pairs, key)
        .map(|v| v.parse::<i64>())
        .transpose()
        .map_err(|_| LogParamError::InvalidNumber(key.to_string()))
}

/// Endpoint-level failures: parameter errors (`400`), store errors
/// (`500`, logged server-side), unwired runtime (`503`), and the export
/// cap (`413`).
#[derive(Debug)]
pub enum LogApiError {
    Param(LogParamError),
    Store,
    Unavailable,
    ExportCap,
}

impl From<LogParamError> for LogApiError {
    fn from(e: LogParamError) -> Self {
        Self::Param(e)
    }
}

impl LogApiError {
    pub(crate) fn response(&self) -> Response {
        match self {
            Self::Param(e) => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": { "message": e.message(), "type": "ValidationError" } })),
            )
                .into_response(),
            Self::Store => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": { "message": "log store query failed", "type": "ServerError" } })),
            )
                .into_response(),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": { "message": "log store is not available", "type": "ServiceUnavailableError" } })),
            )
                .into_response(),
            Self::ExportCap => (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!(
                    { "error": "export cap of 50000 rows exceeded — narrow the window" }
                )),
            )
                .into_response(),
        }
    }
}

impl IntoResponse for LogApiError {
    fn into_response(self) -> Response {
        self.response()
    }
}

impl std::fmt::Display for LogApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "log api error: {:?}", self)
    }
}

impl std::error::Error for LogApiError {}

fn parse_level(s: &str) -> Option<LogstoreLevel> {
    match s {
        "trace" => Some(LogstoreLevel::TRACE),
        "debug" => Some(LogstoreLevel::DEBUG),
        "info" => Some(LogstoreLevel::INFO),
        "warn" => Some(LogstoreLevel::WARN),
        "error" => Some(LogstoreLevel::ERROR),
        _ => None,
    }
}

fn parse_order(s: &str) -> Option<QueryOrder> {
    match s {
        "desc" => Some(QueryOrder::Desc),
        "asc" => Some(QueryOrder::Asc),
        _ => None,
    }
}

/// Shared validation for the level / source / q params across the read
/// endpoints: `level` enum, repeated `source` (400), `q` length cap.
/// An unrecognized / blank `source` term yields `None` (→ 200 with an
/// Validated core query filters (shared shape across endpoints).
type ValidatedLogFilters = (Option<LogstoreLevel>, Option<Source>, Option<String>);

/// empty row set — never a 400).
fn validate_common(
    level: Option<&str>,
    source: &Option<Vec<String>>,
    q: Option<&str>,
) -> Result<ValidatedLogFilters, LogParamError> {
    let level = match level {
        None => None,
        Some(v) => Some(parse_level(v).ok_or_else(|| LogParamError::InvalidLevel(v.to_string()))?),
    };
    let source = match source {
        None => None,
        Some(v) if v.len() > 1 => return Err(LogParamError::RepeatedSource),
        Some(v) => match v.first() {
            Some(s) if !s.trim().is_empty() => Source::parse(s),
            // Empty / whitespace-only source term: explicit filter that
            // matches nothing (cached lines are never stored as "").
            _ => Some(Source(String::new())),
        },
    };
    if let Some(q) = q {
        if q.chars().count() > MAX_Q_CHARS {
            return Err(LogParamError::QTooLong);
        }
    }
    Ok((level, source, q.map(|s| s.to_string())))
}

/// Query params for `GET /tama/v1/logs`, built manually from the raw
/// query string (repeated `source` detection needs the raw pairs).
#[derive(Debug, Default)]
pub struct LogQueryParams {
    pub level: Option<String>,
    /// All `source` values, in order (length > 1 → 400).
    pub source: Option<Vec<String>>,
    pub q: Option<String>,
    /// Unix ms, inclusive.
    pub since: Option<i64>,
    /// Unix ms, exclusive.
    pub until: Option<i64>,
    /// Page size (store clamps to `1..=1000`, default 200).
    pub limit: Option<i64>,
    /// Rowid cursor (`id < cursor` desc / `id > cursor` asc).
    pub cursor: Option<i64>,
    pub order: Option<String>,
}

impl LogQueryParams {
    pub(crate) fn from_request(
        req: &axum::http::Request<axum::body::Body>,
    ) -> Result<Self, LogParamError> {
        let pairs = query_pairs(req);
        let source: Vec<String> = pairs
            .iter()
            .filter(|(k, _)| k == "source")
            .map(|(_, v)| v.clone())
            .collect();
        Ok(Self {
            level: first_of(&pairs, "level").map(|s| s.to_string()),
            source: (!source.is_empty()).then_some(source),
            q: first_of(&pairs, "q").map(|s| s.to_string()),
            since: opt_i64(&pairs, "since")?,
            until: opt_i64(&pairs, "until")?,
            limit: opt_i64(&pairs, "limit")?,
            cursor: opt_i64(&pairs, "cursor")?,
            order: first_of(&pairs, "order").map(|s| s.to_string()),
        })
    }

    /// Validate + build the store query.
    pub(crate) fn validated(&self) -> Result<LogQuery, LogParamError> {
        let order = match self.order.as_deref() {
            None => QueryOrder::default(),
            Some(v) => parse_order(v).ok_or_else(|| LogParamError::InvalidOrder(v.to_string()))?,
        };
        let (level, source, q) =
            validate_common(self.level.as_deref(), &self.source, self.q.as_deref())?;
        Ok(LogQuery {
            min_level: level,
            source,
            q,
            since: self.since,
            until: self.until,
            limit: self.limit,
            cursor: self.cursor,
            order,
        })
    }
}

/// Query params for `GET /tama/v1/logs/stream` (query filters + `after`
/// rowid anchor: rows with `id > after`, polled every second).
#[derive(Debug, Default)]
pub struct LogStreamParams {
    pub level: Option<String>,
    pub source: Option<Vec<String>>,
    pub q: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// Anchor rowid (default 0 = from the beginning, page first).
    pub after: Option<i64>,
}

impl LogStreamParams {
    pub(crate) fn from_request(
        req: &axum::http::Request<axum::body::Body>,
    ) -> Result<Self, LogParamError> {
        let pairs = query_pairs(req);
        let source: Vec<String> = pairs
            .iter()
            .filter(|(k, _)| k == "source")
            .map(|(_, v)| v.clone())
            .collect();
        Ok(Self {
            level: first_of(&pairs, "level").map(|s| s.to_string()),
            source: (!source.is_empty()).then_some(source),
            q: first_of(&pairs, "q").map(|s| s.to_string()),
            since: opt_i64(&pairs, "since")?,
            until: opt_i64(&pairs, "until")?,
            after: opt_i64(&pairs, "after")?,
        })
    }

    pub(crate) fn validated(&self) -> Result<(LogQuery, i64), LogParamError> {
        let (level, source, q) =
            validate_common(self.level.as_deref(), &self.source, self.q.as_deref())?;
        Ok((
            LogQuery {
                min_level: level,
                source,
                q,
                since: self.since,
                until: self.until,
                ..LogQuery::default()
            },
            self.after.unwrap_or(0),
        ))
    }
}

/// Query params for `GET /tama/v1/logs/export` (query filters +
/// `format`, `csv` only).
#[derive(Debug, Default)]
pub struct ExportParams {
    pub level: Option<String>,
    pub source: Option<Vec<String>>,
    pub q: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub format: Option<String>,
}

impl ExportParams {
    pub(crate) fn from_request(
        req: &axum::http::Request<axum::body::Body>,
    ) -> Result<Self, LogParamError> {
        let pairs = query_pairs(req);
        let source: Vec<String> = pairs
            .iter()
            .filter(|(k, _)| k == "source")
            .map(|(_, v)| v.clone())
            .collect();
        Ok(Self {
            level: first_of(&pairs, "level").map(|s| s.to_string()),
            source: (!source.is_empty()).then_some(source),
            q: first_of(&pairs, "q").map(|s| s.to_string()),
            since: opt_i64(&pairs, "since")?,
            until: opt_i64(&pairs, "until")?,
            format: first_of(&pairs, "format").map(|s| s.to_string()),
        })
    }

    pub(crate) fn validated(&self) -> Result<LogQuery, LogParamError> {
        if let Some(f) = &self.format {
            if f != "csv" {
                return Err(LogParamError::UnsupportedFormat(f.clone()));
            }
        }
        let (level, source, q) =
            validate_common(self.level.as_deref(), &self.source, self.q.as_deref())?;
        Ok(LogQuery {
            min_level: level,
            source,
            q,
            since: self.since,
            until: self.until,
            ..LogQuery::default()
        })
    }
}

/// Query params for `GET /tama/v1/logs/summary`.
#[derive(Debug, Default, Deserialize)]
pub struct SummaryParams {
    /// Unix ms, inclusive (default 0 = all time).
    pub since: Option<i64>,
}

// ── Legacy tail adapter ─────────────────────────────────────────────────

/// The named on-demand legacy source (the `source` query term, verbatim
/// — e.g. `tamad:gpu-box:model:qwen3:8b`, `proxy`, `backend:llama_cpp_1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogTailSource {
    pub label: String,
}

impl LogTailSource {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }

    /// Tamad host / per-model control line (remote engine-log tail).
    pub fn is_tamad(&self) -> bool {
        let rest = self.label.strip_prefix("tamad:").unwrap_or("");
        !rest.is_empty()
    }
}

/// On-demand legacy row-tail provider.
///
/// Implementations return up to [`TAIL_LINES`] raw lines as
/// `(fetch_ts_ms, line)` tuples — the fetch time is captured ONCE per
/// tail (legacy sources are unstructured text with no reliable per-line
/// timestamps). Tamad tails are network-bound (bounded single-attempt
/// fetch, silent skip on timeout — a wedge must not hang the endpoint);
/// local `*.log` tails are blocking file reads — implementations expect
/// to be called from an async context that tolerates that.
///
/// Results are cached per source with a short TTL by
/// [`CachingTailProvider`] so concurrent UI polls reuse one fetch.
pub trait LogTailProvider: Send + Sync {
    /// Tail up to [`TAIL_LINES`] lines for `source` (as
    /// `(fetch_ts_ms, line)` tuples, see the type docs above).
    fn tail<'a>(
        &'a self,
        source: &'a LogTailSource,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(i64, String)>>>;
}

/// Cached fetch for a tail source: (fetch ts, raw lines).
type TailCacheEntry = (Instant, Vec<(i64, String)>);

/// TTL-cached wrapper around any [`LogTailProvider`] (consecutive polls
/// inside the window reuse the last fetch; TTL is injectable for tests).
pub struct CachingTailProvider {
    inner: Arc<dyn LogTailProvider>,
    ttl: Duration,
    cache: Arc<RwLock<HashMap<String, TailCacheEntry>>>,
}

impl CachingTailProvider {
    /// Default [`TAIL_CACHE_TTL`] (5 s).
    pub fn new(inner: Arc<dyn LogTailProvider>) -> Self {
        Self::with_ttl(inner, TAIL_CACHE_TTL)
    }

    pub fn with_ttl(inner: Arc<dyn LogTailProvider>, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// The wrapped provider (exposed for operator introspection).
    pub fn inner(&self) -> &Arc<dyn LogTailProvider> {
        &self.inner
    }
}

impl LogTailProvider for CachingTailProvider {
    fn tail<'a>(
        &'a self,
        source: &'a LogTailSource,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(i64, String)>>> {
        Box::pin(async move {
            {
                let Ok(cache) = self.cache.read() else {
                    // Poisoned cache: bypass it (a stale snapshot is always
                    // available from the underlying provider).
                    return self.inner.tail(source).await;
                };
                if let Some((at, rows)) = cache.get(&source.label) {
                    if at.elapsed() < self.ttl {
                        return Ok(rows.clone());
                    }
                }
            }
            let rows = self.inner.tail(source).await?;
            if let Ok(mut cache) = self.cache.write() {
                cache.insert(source.label.clone(), (Instant::now(), rows.clone()));
            }
            Ok(rows)
        })
    }
}

/// Legacy-source tail adapter (plan-195 Task 4):
///
/// - `tamad:<host>[:model:<model>[:tail]]` — engine tail over the
///   tamad `Logs` RPC (reuses `backend_logs::tail_one_tamad_source`);
/// - `proxy` — `tama.log` in the resolved logs dir;
/// - `backend:<x>` — `<x>.log`; anything else — `<label>.log`.
///
/// File names map to the local `*.log` files in the resolved logs dir. The fetch timestamp is captured at fetch time for every line
/// of the tail.
#[derive(Clone)]
pub struct TamadTailProvider {
    pub pool: Arc<TamadPool>,
    /// Local logs directory (`None` → local tails are empty).
    pub logs_dir: Option<PathBuf>,
}

impl TamadTailProvider {
    pub fn new(pool: Arc<TamadPool>, logs_dir: Option<PathBuf>) -> Self {
        Self { pool, logs_dir }
    }
}

impl LogTailProvider for TamadTailProvider {
    fn tail<'a>(
        &'a self,
        source: &'a LogTailSource,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(i64, String)>>> {
        Box::pin(async move {
            let lines: Vec<String> = if source.is_tamad() {
                let rest = source.label.strip_prefix("tamad:").unwrap_or("");
                // "tamad:<host>" | "tamad:<host>:model:<model>" | "…:tail"
                let (host, model): (&str, Option<&str>) = match rest.split_once(":model:") {
                    Some((h, m)) => (h, Some(m.strip_suffix(":tail").unwrap_or(m))),
                    None => (rest, None),
                };
                if host.is_empty() {
                    Vec::new()
                } else {
                    super::backend_logs::tail_one_tamad_source(&self.pool, host, model, TAIL_LINES)
                        .await
                }
            } else {
                let Some(dir) = &self.logs_dir else {
                    return Ok(Vec::new());
                };
                let file_name = if source.label == "proxy" {
                    "tama.log".to_string()
                } else if let Some(name) = source.label.strip_prefix("backend:") {
                    format!("{name}.log")
                } else {
                    format!("{}.log", source.label)
                };
                let path = dir.join(&file_name);
                let out = tokio::task::spawn_blocking(move || tail_lines(&path, TAIL_LINES))
                    .await
                    .unwrap_or_else(|e| Err(anyhow::anyhow!("tail task panicked: {e}")))
                    .unwrap_or_default();
                out
            };

            let ts = now_unix_ms();
            Ok(lines.into_iter().map(|l| (ts, l)).collect())
        })
    }
}

/// Current unix time in milliseconds.
pub(crate) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Store access ────────────────────────────────────────────────────────

/// Run `f` on the mutex-guarded read store in `spawn_blocking`.
/// `Ok(None)` = store not wired; `Ok(Ok(value))` / `Ok(Err(e))` = call
/// outcome; the inner shutdown panic case becomes an error.
async fn with_store<R: Send + 'static>(
    store: &Option<Arc<Mutex<LogStore>>>,
    f: impl FnOnce(&LogStore) -> anyhow::Result<R> + Send + 'static,
) -> Option<Result<R, anyhow::Error>> {
    let Some(store) = store else {
        return None;
    };
    let store = store.clone();
    match tokio::task::spawn_blocking(move || {
        let guard = store.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    })
    .await
    {
        Ok(r) => Some(r),
        Err(e) => Some(Err(anyhow::anyhow!("log store task panicked: {e}"))),
    }
}

// ── Query core ──────────────────────────────────────────────────────────

/// Core of `GET /tama/v1/logs` (apart from the axum plumbing): param
/// validation, source dispatch (store rows vs legacy tail rows), and
/// DTO mapping. Exported for handler tests.
pub async fn run_log_query(
    state: &LogsApiState,
    p: &LogQueryParams,
) -> Result<(Vec<LogEntryDto>, Option<i64>), LogApiError> {
    // v1 is single-source: repeated `source` params are a validation
    // error (multi-source selection is future work).
    let qu = p.validated()?;
    let source_label: Option<String> = qu.source.as_ref().map(|s| s.as_str().to_string());

    let (rows, next): (Vec<crate::logstore::LogEntry>, Option<i64>) =
        match with_store(&state.log_read, move |s| s.query(&qu)).await {
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                tracing::warn!(error = %e, "structured log query failed");
                return Err(LogApiError::Store);
            }
            // Store not wired (tests): 200 with an empty row set.
            None => (Vec::new(), None),
        };
    let mut entries: Vec<LogEntryDto> = rows
        .into_iter()
        .map(|e| LogEntryDto::from_entry(&e))
        .collect();

    // Tamad sources additionally expose the on-demand engine-log tail
    // (legacy rows, flagged `legacy: true`) — appended after the
    // structured rows when a tail provider is wired and the store had
    // nothing recorded yet (engine logs before the structured bridge
    // started, or hosts whose lines are only observable via tail).
    if let Some(label) = source_label {
        if LogTailSource::new(&label).is_tamad() && entries.is_empty() {
            if let Some(provider) = &state.log_tail {
                entries = provider
                    .tail(&LogTailSource::new(&label))
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(i, (ts, line))| LogEntryDto::from_tail_line(&label, ts, i, &line))
                    .collect();
            }
        }
    }

    Ok((entries, next))
}

// ── Handlers ────────────────────────────────────────────────────────────

/// `GET /tama/v1/logs` — query the structured log store.
///
/// `200 {"entries": [...], "next_cursor": null|int}` | `400` (invalid
/// `level` / `order` / `q` length / repeated `source`).
pub async fn handle_log_query(
    Extension(state): Extension<LogsApiState>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    let params = match LogQueryParams::from_request(&req) {
        Ok(p) => p,
        Err(e) => return LogApiError::from(e).response(),
    };
    match run_log_query(&state, &params).await {
        Ok((entries, next_cursor)) => {
            Json(json!({ "entries": entries, "next_cursor": next_cursor })).into_response()
        }
        Err(e) => e.response(),
    }
}

/// `GET /tama/v1/logs/sources` — distinct sources with latest ts.
///
/// Legacy host tails do NOT appear here (they are on-demand, not rows).
pub async fn handle_log_sources(Extension(state): Extension<LogsApiState>) -> Response {
    let r = with_store(&state.log_read, |s| s.distinct_sources()).await;
    match r {
        Some(Ok(sources)) => Json(json!({
            "sources": sources
                .iter()
                .map(|x| json!({ "source": x.source.as_str(), "last_ts": x.last_ts }))
                .collect::<Vec<_>>()
        }))
        .into_response(),
        Some(Err(e)) => {
            tracing::warn!(error = %e, "distinct_sources failed");
            LogApiError::Store.response()
        }
        None => LogApiError::Unavailable.response(),
    }
}

/// `GET /tama/v1/logs/summary?since=` — per-level counts (`ts >= since`).
pub async fn handle_log_summary(
    Extension(state): Extension<LogsApiState>,
    Query(params): Query<SummaryParams>,
) -> Response {
    let since = params.since.unwrap_or(0);
    let r = with_store(&state.log_read, move |s| s.level_counts_since(since)).await;
    match r {
        Some(Ok(counts)) => {
            let mut c = json!({ "debug": 0, "info": 0, "warn": 0, "error": 0 });
            let mut total = 0i64;
            for x in counts {
                total += x.count;
                // trace rows count toward `total` only (no `trace` key
                // in the response shape — documented).
                let key = match x.level.as_str() {
                    "debug" | "info" | "warn" | "error" => x.level.as_str(),
                    _ => "",
                };
                if !key.is_empty() {
                    if let Some(map) = c.as_object_mut() {
                        map.insert(key.to_string(), json!(x.count));
                    }
                }
            }
            if let Some(map) = c.as_object_mut() {
                map.insert("total".to_string(), json!(total));
            }
            Json(json!({ "counts": c })).into_response()
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "level_counts_since failed");
            LogApiError::Store.response()
        }
        None => LogApiError::Unavailable.response(),
    }
}

/// `GET /tama/v1/logs/status` — writer status snapshot
/// (`LogStoreStatus` as JSON); `None` receiver → healthy zeros.
pub async fn handle_log_status(Extension(state): Extension<LogsApiState>) -> Response {
    let st = if let Some(rx) = state.log_status.as_ref() {
        let mut rx_clone = (**rx).clone();
        let guard = rx_clone.borrow_and_update();
        *guard
    } else {
        LogStoreStatus::ok()
    };
    Json(json!({
        "degraded": st.degraded,
        "degraded_since": st.degraded_since,
        "channel_len": st.channel_len,
        "ring_len": st.ring_len,
        "dropped_count": st.dropped_count,
        "retries_seen": st.retries_seen,
        "last_prune_deleted": st.last_prune_deleted,
    }))
    .into_response()
}

/// `GET /tama/v1/logs/stream?…filters…&after=<id>` — SSE (see module
/// doc). Each event `event: entry` carries one [`LogEntryDto`]; empty
/// ticks emit `event: keepalive` (project SSE keep-alive convention).
pub async fn handle_log_stream(
    Extension(state): Extension<LogsApiState>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    let params = match LogStreamParams::from_request(&req) {
        Ok(p) => p,
        Err(e) => return LogApiError::from(e).response(),
    };
    let (filters, after) = match params.validated() {
        Ok(v) => v,
        Err(e) => return LogApiError::from(e).response(),
    };
    let Some(store) = state.log_read.clone() else {
        return LogApiError::Unavailable.response();
    };
    // The token stops the poll loop (channel closed, ctrl, tests). The
    // stream also stops when the Sse body stops polling it (client
    // disconnect).
    let cancel = CancellationToken::new();
    let frames = log_stream_frames(store, after, filters, cancel);
    let stream = frames.map(|f| Ok::<Event, axum::Error>(frame_to_sse_event(f)));
    Sse::new(stream).into_response()
}

/// `GET /tama/v1/logs/export?…filters…&format=csv` — CSV export with the
/// header `id,ts,level,source,message`. COUNT-FIRST: `413` before
/// streaming when the window exceeds [`EXPORT_CAP_ROWS`].
pub async fn handle_log_export(
    Extension(state): Extension<LogsApiState>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<Response, LogApiError> {
    let params = ExportParams::from_request(&req)?;
    let qu = params.validated()?;
    let Some(store) = state.log_read.clone() else {
        return Err(LogApiError::Unavailable);
    };

    // Count FIRST under the same filters (before streaming anything).
    let store_count = store.clone();
    let count_qu = qu.clone();
    let count = tokio::task::spawn_blocking(move || {
        let guard = store_count.lock().unwrap_or_else(|e| e.into_inner());
        guard.count(&count_qu)
    })
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "export count task panicked");
        LogApiError::Store
    })?;
    let count = count.map_err(|e| {
        tracing::warn!(error = %e, "export count failed");
        LogApiError::Store
    })?;
    if count > EXPORT_CAP_ROWS {
        return Err(LogApiError::ExportCap);
    }

    // Cursor walk (store page cap 1000) — `count` bounds the total.
    let mut qu = qu;
    qu.order = QueryOrder::Desc;
    let mut entries: Vec<LogEntry> = Vec::with_capacity(count as usize);
    let mut next: Option<i64> = None;
    loop {
        qu.cursor = next;
        let store_page = store.clone();
        let log_qu = qu.clone();
        let page = tokio::task::spawn_blocking(move || {
            let guard = store_page.lock().unwrap_or_else(|e| e.into_inner());
            guard.query(&log_qu)
        })
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "export task panicked");
            LogApiError::Store
        })?
        .map_err(|e| {
            tracing::warn!(error = %e, "export page failed");
            LogApiError::Store
        })?;
        entries.extend(page.0);
        let nc = page.1;
        if nc.is_none() {
            break;
        }
        next = nc;
    }

    let mut body = String::from("id,ts,level,source,message\n");
    for e in &entries {
        let dto = LogEntryDto::from_entry(e);
        body.push_str(&format!(
            "{},{},{},{},{}\n",
            e.id,
            e.ts,
            csv_field(&dto.level),
            csv_field(&dto.source),
            csv_field(&dto.message)
        ));
    }
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"tama-logs.csv\"",
            ),
        ],
        body,
    )
        .into_response())
}

/// `DELETE /tama/v1/logs` — delete every row (`LogStore::delete_all`
/// chunks + incremental vacuum). CSRF-enforced route home: the
/// sub-router in `router.rs`.
pub async fn handle_delete_logs(Extension(state): Extension<LogsApiState>) -> Response {
    let r = with_store(&state.log_read, |s| s.delete_all()).await;
    match r {
        Some(Ok(deleted)) => {
            // compacted: `delete_all` already ran the incremental
            // vacuum (reclaim is not a separate step).
            (
                StatusCode::ACCEPTED,
                Json(json!({ "deleted": deleted, "compacted": true })),
            )
                .into_response()
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "delete_all failed");
            LogApiError::Store.response()
        }
        None => LogApiError::Unavailable.response(),
    }
}

/// `GET /tama/v1/logs/events` — SSE of the writer's degraded / restored
/// frames (self-describing JSON, per `docs/api/sse.md`). Frames are
/// produced by the bridge task in `main.rs`; the handler creates the
/// per-endpoint broadcast on first connect (same pattern as
/// `update_tx`).
pub async fn handle_log_events_sse(Extension(state): Extension<LogsApiState>) -> Response {
    let (sender, rx) = {
        let mut guard = state.log_events_tx.lock().await;
        match guard.as_ref() {
            Some(tx) => (tx.clone(), tx.subscribe()),
            None => {
                let (tx, _seed) = tokio::sync::broadcast::channel(256);
                let rx = tx.subscribe();
                *guard = Some(tx.clone());
                (tx, rx)
            }
        }
    };
    // Holding a clone of the sender here (beyond the mutex instance) is
    // deliberate: closing it is purely client-driven (each Sse body
    // holds a receiver).
    let _oc = sender;
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(frame) => Some((
                Ok::<Event, axum::Error>(Event::default().event("log_store").data(frame)),
                rx,
            )),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => Some((
                Ok::<Event, axum::Error>(
                    Event::default()
                        .event("Lagged")
                        .json_data(json!({ "Lagged": n }))
                        .expect("json_data on valid JSON cannot fail"),
                ),
                rx,
            )),
            Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// ── SSE frames ──────────────────────────────────────────────────────────

/// One new log row as an SSE frame. Frame shape is EXACTLY
/// `event: <name>\ndata: <compact single-line JSON>\n\n`.
pub fn entry_frame(dto: &LogEntryDto) -> String {
    format!(
        "event: entry\ndata: {}\n\n",
        serde_json::to_string(dto).expect("LogEntryDto always serializes")
    )
}

/// Empty-tick keep-alive frame (project SSE convention).
pub fn keepalive_frame() -> String {
    "event: keepalive\ndata: {\"keepalive\": true}\n\n".to_string()
}

/// Split a raw frame into its `(event name, data payload)` parts for the
/// axum `Event`.
///
/// The data payload is returned **bare** (no `data: ` prefix) — axum
/// re-emits it as `data: <payload>` on the wire, and the browser's
/// EventSource strips exactly one `data: `. Keeping the frame's own
/// prefix here produced `data: data: {…}` on the wire and a silent
/// client-side parse failure of every live row (plan-195 Task 5).
pub fn sse_event_parts(frame: &str) -> (String, String) {
    let (first, rest) = frame.split_once('\n').unwrap_or(("", ""));
    let name = first
        .strip_prefix("event: ")
        .unwrap_or("message")
        .to_string();
    let data = rest
        .trim_start_matches("data: ")
        .trim_end_matches('\n')
        .trim_end();
    (name, data.to_string())
}

/// Turn one [`entry_frame`] / [`keepalive_frame`] output into an axum SSE
/// `Event` (frame shape `event: <name>\ndata: <json>\n\n` — the `data`
/// payload is compact single-line JSON, re-prefixed by axum).
pub fn frame_to_sse_event(frame: String) -> Event {
    let (name, payload) = sse_event_parts(&frame);
    let event = Event::default().event(name);
    if payload.is_empty() {
        event
    } else {
        event.data(payload)
    }
}

/// SSE poll loop (extracted for unit testing): every [`STREAM_POLL`]
/// tick runs `query(order: Asc, cursor: after, limit: 200)` on the
/// store; a found batch is emitted in id ascending order and the max
/// id becomes the new `after`; empty ticks emit a keepalive frame.
/// `cancel` stops the stream (the Sse body dropping also stops it).
pub fn log_stream_frames(
    store: Arc<Mutex<LogStore>>,
    after: i64,
    filters: LogQuery,
    cancel: CancellationToken,
) -> impl Stream<Item = String> + Send {
    futures_util::stream::unfold(
        StreamLoop {
            store,
            filters,
            after,
            cancel,
            pending: VecDeque::new(),
        },
        |mut st| async move {
            loop {
                if let Some(frame) = st.pending.pop_front() {
                    return Some((frame, st));
                }
                tokio::select! {
                    _ = st.cancel.cancelled() => return None,
                    _ = tokio::time::sleep(STREAM_POLL) => {}
                }
                let mut qu = st.filters.clone();
                qu.order = QueryOrder::Asc;
                qu.cursor = Some(st.after);
                qu.limit = Some(STREAM_PAGE);
                let store = st.store.clone();
                let res = tokio::task::spawn_blocking(move || {
                    let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                    guard.query(&qu)
                })
                .await;
                let Ok(Ok((entries, _))) = res else {
                    // Store hiccup: keep the stream alive, skip the tick.
                    tracing::warn!("log stream poll failed");
                    return Some((keepalive_frame(), st));
                };
                if entries.is_empty() {
                    return Some((keepalive_frame(), st));
                }
                // Ascending: the last entry carries the new anchor.
                st.after = entries.last().map(|e| e.id).unwrap_or(st.after);
                for e in entries {
                    st.pending
                        .push_back(entry_frame(&LogEntryDto::from_entry(&e)));
                }
                // Loop → immediately pop the first pending frame.
            }
        },
    )
}

struct StreamLoop {
    store: Arc<Mutex<LogStore>>,
    filters: LogQuery,
    after: i64,
    cancel: CancellationToken,
    pending: VecDeque<String>,
}

// ── CSV ─────────────────────────────────────────────────────────────────

/// RFC 4180: quote a field containing a comma, double-quote, CR or LF;
/// double any embedded quotes.
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode as HttpStatus};
    use axum::routing::get;
    use futures_util::StreamExt;
    use serde_json::json as vjson;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    fn rec(ts: i64, level: LogstoreLevel, source: &str, msg: Value) -> crate::logstore::LogRecord {
        crate::logstore::LogRecord {
            ts,
            level,
            source: Source::parse(source).expect("valid source in test"),
            msg,
        }
    }

    /// The canonical seeded store used by the handler-level tests:
    /// ids 1..8, ts 100..800.
    fn seed_store() -> LogStore {
        let s = LogStore::in_memory().expect("in-memory store");
        s.insert_batch(&[
            rec(
                100,
                LogstoreLevel::INFO,
                "proxy",
                vjson!({"message": "boot complete"}),
            ),
            rec(
                200,
                LogstoreLevel::INFO,
                "proxy",
                vjson!({"message": "hot reload ok", "run_id": 7}),
            ),
            rec(
                300,
                LogstoreLevel::WARN,
                "backend:llama-cpp",
                vjson!({"message": "slow kv"}),
            ),
            rec(
                400,
                LogstoreLevel::ERROR,
                "proxy",
                vjson!({"message": "backend down", "target": "tama_core::proxy::lifecycle"}),
            ),
            rec(
                500,
                LogstoreLevel::WARN,
                "proxy",
                vjson!({
                    "message": "log store: dropped 3 events since 2026-01-01T00:00:00Z",
                    "dropped": true,
                    "dropped_count": 3,
                    "dropped_since_ts": "2026-01-01T00:00:00Z"
                }),
            ),
            rec(
                600,
                LogstoreLevel::INFO,
                "tamad:gpu-box",
                vjson!({"message": "host live"}),
            ),
            rec(
                700,
                LogstoreLevel::INFO,
                "tamad:gpu-box:model:qwen3:8b",
                vjson!({"message": "model ready", "model": "qwen3:8b"}),
            ),
            rec(
                800,
                LogstoreLevel::INFO,
                "tamad:gpu-boxer",
                vjson!({"message": "other host"}),
            ),
        ])
        .expect("seed");
        s
    }

    fn state_with(store: LogStore) -> LogsApiState {
        LogsApiState {
            log_read: Some(Arc::new(Mutex::new(store))),
            log_tail: None,
            log_status: None,
            log_events_tx: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    fn router(state: LogsApiState) -> axum::Router {
        axum::Router::new()
            .route("/logs", get(handle_log_query).delete(handle_delete_logs))
            .route("/logs/sources", get(handle_log_sources))
            .route("/logs/summary", get(handle_log_summary))
            .route("/logs/status", get(handle_log_status))
            .route("/logs/stream", get(handle_log_stream))
            .route("/logs/export", get(handle_log_export))
            .route("/logs/events", get(handle_log_events_sse))
            .layer(Extension(state))
    }

    async fn oneshot(state: LogsApiState, method: &str, uri: &str) -> (HttpStatus, Vec<u8>) {
        let app = router(state);
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.expect("request completes");
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body readable");
        (status, bytes.to_vec())
    }

    fn json_body(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("valid JSON body")
    }

    fn entry_ids(body: &Value) -> Vec<i64> {
        body["entries"]
            .as_array()
            .expect("entries array")
            .iter()
            .map(|e| e["id"].as_i64().expect("id"))
            .collect()
    }

    // ── query: filters ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_query_no_filters_newest_first() {
        let (status, bytes) = oneshot(state_with(seed_store()), "GET", "/logs").await;
        assert_eq!(status, HttpStatus::OK);
        let body = json_body(&bytes);
        assert_eq!(entry_ids(&body), vec![8, 7, 6, 5, 4, 3, 2, 1]);
        assert!(body["next_cursor"].is_null(), "small result: no next");
    }

    #[tokio::test]
    async fn test_query_min_level_excludes_info() {
        let (status, bytes) = oneshot(state_with(seed_store()), "GET", "/logs?level=warn").await;
        assert_eq!(status, HttpStatus::OK);
        assert_eq!(entry_ids(&json_body(&bytes)), vec![5, 4, 3]);
    }

    /// `source=tamad:gpu-box` matches the host row AND its `:model:`
    /// prefix child, but NOT the over-matching label `tamad:gpu-boxer`.
    #[tokio::test]
    async fn test_query_source_exact_prefix_no_overmatch() {
        let (status, bytes) = oneshot(
            state_with(seed_store()),
            "GET",
            "/logs?source=tamad%3Agpu-box",
        )
        .await;
        assert_eq!(status, HttpStatus::OK);
        assert_eq!(entry_ids(&json_body(&bytes)), vec![7, 6]);

        let (_, bytes) = oneshot(state_with(seed_store()), "GET", "/logs?source=proxy").await;
        assert_eq!(entry_ids(&json_body(&bytes)), vec![5, 4, 2, 1]);
    }

    /// Unrecognized (or empty) `source` → 200 with EMPTY entries, never 400.
    #[tokio::test]
    async fn test_query_unrecognized_source_is_empty_200() {
        let (status, bytes) = oneshot(
            state_with(seed_store()),
            "GET",
            "/logs?source=no-such-source",
        )
        .await;
        assert_eq!(status, HttpStatus::OK);
        let body = json_body(&bytes);
        assert!(body["entries"].as_array().unwrap().is_empty());
        assert!(body["next_cursor"].is_null());

        let (status, bytes) = oneshot(state_with(seed_store()), "GET", "/logs?source=").await;
        assert_eq!(status, HttpStatus::OK);
        assert!(json_body(&bytes)["entries"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_query_fts_hit() {
        let (status, bytes) = oneshot(state_with(seed_store()), "GET", "/logs?q=hot").await;
        assert_eq!(status, HttpStatus::OK);
        assert_eq!(entry_ids(&json_body(&bytes)), vec![2]);
    }

    /// A token no FTS row contains → like fallback, still 200.
    #[tokio::test]
    async fn test_query_fts_zero_rows_falls_back_to_like() {
        // "complet" is a PREFIX inside the FTS token "complete" — FTS5
        // has no prefix match → zero FTS rows → LIKE '%complet%'.
        let (status, bytes) = oneshot(state_with(seed_store()), "GET", "/logs?q=complet").await;
        assert_eq!(status, HttpStatus::OK);
        assert_eq!(entry_ids(&json_body(&bytes)), vec![1]);
    }

    /// Malformed FTS syntax → like fallback, still 200 (never 500 on
    /// search text).
    #[tokio::test]
    async fn test_query_malformed_fts_falls_back() {
        let (status, bytes) =
            oneshot(state_with(seed_store()), "GET", "/logs?q=%22not%20fts%20(").await;
        assert_eq!(status, HttpStatus::OK, "fallback must never 500");
        assert!(json_body(&bytes)["entries"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_query_since_until_window() {
        let (status, bytes) =
            oneshot(state_with(seed_store()), "GET", "/logs?since=350&until=550").await;
        assert_eq!(status, HttpStatus::OK);
        // ts >= 350 (id 3 at 300 excluded) and ts < 550 (id 6 at 600
        // excluded) → ids 4 (400) and 5 (500).
        assert_eq!(entry_ids(&json_body(&bytes)), vec![5, 4]);
    }

    /// Cursor walk (desc and asc) covers every row exactly once.
    #[tokio::test]
    async fn test_query_cursor_walk_desc_and_asc() {
        let mut cursor = None;
        let mut seen_desc = Vec::new();
        loop {
            let uri = format!(
                "/logs?limit=2{}",
                cursor.map(|c| format!("&cursor={c}")).unwrap_or_default()
            );
            let (_, bytes) = oneshot(state_with(seed_store()), "GET", &uri).await;
            let body = json_body(&bytes);
            for id in &entry_ids(&body) {
                seen_desc.push(*id);
            }
            let next = body["next_cursor"].as_i64();
            if next.is_none() {
                break;
            }
            cursor = next;
        }
        assert_eq!(seen_desc, vec![8, 7, 6, 5, 4, 3, 2, 1]);

        let mut cursor = None;
        let mut seen_asc = Vec::new();
        loop {
            let uri = format!(
                "/logs?limit=2&order=asc{}",
                cursor.map(|c| format!("&cursor={c}")).unwrap_or_default()
            );
            let (_, bytes) = oneshot(state_with(seed_store()), "GET", &uri).await;
            let body = json_body(&bytes);
            for id in &entry_ids(&body) {
                seen_asc.push(*id);
            }
            let next = body["next_cursor"].as_i64();
            if next.is_none() {
                break;
            }
            cursor = next;
        }
        assert_eq!(seen_asc, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[tokio::test]
    async fn test_query_400_validation_rules() {
        // q > 512 chars
        let long = "x".repeat(513);
        let (status, bytes) =
            oneshot(state_with(seed_store()), "GET", &format!("/logs?q={long}")).await;
        assert_eq!(status, HttpStatus::BAD_REQUEST);
        assert_eq!(json_body(&bytes)["error"]["type"], "ValidationError");

        // invalid level
        let (status, _) = oneshot(state_with(seed_store()), "GET", "/logs?level=banana").await;
        assert_eq!(status, HttpStatus::BAD_REQUEST);

        // invalid order
        let (status, _) = oneshot(state_with(seed_store()), "GET", "/logs?order=banana").await;
        assert_eq!(status, HttpStatus::BAD_REQUEST);

        // repeated source
        let (status, bytes) =
            oneshot(state_with(seed_store()), "GET", "/logs?source=a&source=b").await;
        assert_eq!(status, HttpStatus::BAD_REQUEST);
        assert_eq!(
            json_body(&bytes)["error"]["message"].as_str(),
            Some("source may only be given once")
        );
    }

    /// DTO shape: message flattening, keep-target-in-fields, drop
    /// marker drop / drop count keys, level name.
    #[tokio::test]
    async fn test_query_dto_shape() {
        let (_, bytes) = oneshot(state_with(seed_store()), "GET", "/logs").await;
        let body = json_body(&bytes);
        let by_id = body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["id"].as_i64().unwrap(), e))
            .collect::<HashMap<i64, &Value>>();

        // id 4: error level name, target kept INSIDE fields.
        let e4 = by_id[&4];
        assert_eq!(e4["level"], "error");
        assert_eq!(e4["level_known"], true);
        assert_eq!(e4["message"], "backend down");
        assert_eq!(e4["fields"]["target"], "tama_core::proxy::lifecycle");

        // id 2: structured key "run_id" lands in fields.
        assert_eq!(by_id[&2]["fields"]["run_id"], 7);

        // id 5: drop marker — droppped keys flattened, NOT in fields.
        let e5 = by_id[&5];
        assert_eq!(e5["dropped"], true);
        assert_eq!(e5["dropped_count"], 3);
        assert!(
            e5["fields"].get("dropped_since_ts").is_none(),
            "dropped_since_ts is a known key: not in fields"
        );
        assert!(e5["fields"].get("message").is_none());
        assert_eq!(
            e5["message"],
            "log store: dropped 3 events since 2026-01-01T00:00:00Z"
        );

        // non-drop rows: no dropped key on the wire.
        assert!(by_id[&2].get("dropped").is_none());
    }

    // ── sources + summary + status ──────────────────────────────────────

    #[tokio::test]
    async fn test_sources_shape() {
        let (status, bytes) = oneshot(state_with(seed_store()), "GET", "/logs/sources").await;
        assert_eq!(status, HttpStatus::OK);
        let body = json_body(&bytes);
        let map = body["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["source"].as_str().unwrap().to_string(),
                    s["last_ts"].as_i64().unwrap(),
                )
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(
            map,
            [
                ("proxy".to_string(), 500),
                ("backend:llama-cpp".to_string(), 300),
                ("tamad:gpu-box".to_string(), 600),
                ("tamad:gpu-box:model:qwen3:8b".to_string(), 700),
                ("tamad:gpu-boxer".to_string(), 800),
            ]
            .into_iter()
            .collect::<HashMap<_, _>>(),
            "set of (source, last_ts) must be exact"
        );
    }

    #[tokio::test]
    async fn test_summary_counts_full_and_windowed() {
        let (status, bytes) = oneshot(state_with(seed_store()), "GET", "/logs/summary").await;
        assert_eq!(status, HttpStatus::OK);
        assert_eq!(
            json_body(&bytes)["counts"],
            vjson!({ "debug": 0, "info": 5, "warn": 2, "error": 1, "total": 8 })
        );

        let (_, bytes) = oneshot(state_with(seed_store()), "GET", "/logs/summary?since=350").await;
        assert_eq!(
            json_body(&bytes)["counts"],
            vjson!({ "debug": 0, "info": 3, "warn": 1, "error": 1, "total": 5 })
        );
    }

    fn state_with_status(rx: tokio::sync::watch::Receiver<LogStoreStatus>) -> LogsApiState {
        let mut st = state_with(seed_store());
        st.log_status = Some(Arc::new(rx));
        st
    }

    #[tokio::test]
    async fn test_status_from_receiver() {
        let (tx, rx) = tokio::sync::watch::channel(LogStoreStatus::ok());
        tx.send_modify(|s| {
            s.degraded = true;
            s.degraded_since = Some(123);
            s.channel_len = 4;
            s.ring_len = 2;
            s.dropped_count = 9;
            s.retries_seen = 1;
            s.last_prune_deleted = Some(30);
        });
        let (status, bytes) = oneshot(state_with_status(rx), "GET", "/logs/status").await;
        assert_eq!(status, HttpStatus::OK);
        assert_eq!(json_body(&bytes)["degraded"], true);
        assert_eq!(json_body(&bytes)["degraded_since"], 123);
        assert_eq!(json_body(&bytes)["channel_len"], 4);
        assert_eq!(json_body(&bytes)["ring_len"], 2);
        assert_eq!(json_body(&bytes)["dropped_count"], 9);
        assert_eq!(json_body(&bytes)["retries_seen"], 1);
        assert_eq!(json_body(&bytes)["last_prune_deleted"], 30);
    }

    /// Unwired status receiver → healthy zeros (not 503).
    #[tokio::test]
    async fn test_status_none_receiver_is_zeros() {
        let (status, bytes) = oneshot(state_with(seed_store()), "GET", "/logs/status").await;
        assert_eq!(status, HttpStatus::OK);
        assert_eq!(json_body(&bytes)["degraded"], false);
        assert_eq!(json_body(&bytes)["channel_len"], 0);
        assert_eq!(json_body(&bytes)["dropped_count"], 0);
        assert_eq!(
            json_body(&bytes)["last_prune_deleted"],
            serde_json::Value::Null
        );
    }

    // ── store-not-wired behaviour ────────────────────────────────────────

    #[tokio::test]
    async fn test_unwired_store_query_is_empty_sources_503() {
        let unwired = LogsApiState {
            log_read: None,
            log_tail: None,
            log_status: None,
            log_events_tx: Arc::new(tokio::sync::Mutex::new(None)),
        };
        // query → 200 with an empty row set
        let (status, bytes) = oneshot(unwired.clone(), "GET", "/logs").await;
        assert_eq!(status, HttpStatus::OK);
        assert!(json_body(&bytes)["entries"].as_array().unwrap().is_empty());

        // sources / summary → 503
        let (status, bytes) = oneshot(unwired.clone(), "GET", "/logs/sources").await;
        assert_eq!(status, HttpStatus::SERVICE_UNAVAILABLE);
        assert_eq!(
            json_body(&bytes)["error"]["type"],
            "ServiceUnavailableError"
        );
        let (status, _) = oneshot(unwired, "GET", "/logs/summary").await;
        assert_eq!(status, HttpStatus::SERVICE_UNAVAILABLE);
    }

    // ── stream (generator level) ─────────────────────────────────────────

    fn parse_frame(frame: &str) -> (String, Value) {
        let (first, rest) = frame.split_once('\n').expect("frame has lines");
        let name = first
            .strip_prefix("event: ")
            .expect("event name line")
            .to_string();
        let data = rest.trim_start_matches("data: ").trim_end_matches('\n');
        (
            name,
            serde_json::from_str(data).expect("frame data is JSON"),
        )
    }

    /// Pre-populated ids 1..5, `after=0`: first ticks emit ascending
    /// entries, empty ticks emit keepalive, cancel stops the stream.
    #[tokio::test]
    async fn test_log_stream_frames_entries_keepalive_cancel() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("stream.db");
        {
            let s = LogStore::open(&path).expect("seed conn");
            let rows: Vec<_> = (1..=5)
                .map(|i| {
                    rec(
                        i * 100,
                        LogstoreLevel::INFO,
                        "proxy",
                        vjson!({"message": format!("m{i}") }),
                    )
                })
                .collect();
            s.insert_batch(&rows).expect("seed");
        }
        let store = Arc::new(Mutex::new(LogStore::open(&path).expect("stream conn")));
        let cancel = CancellationToken::new();
        let mut frames = Box::pin(log_stream_frames(
            store,
            0,
            LogQuery::default(),
            cancel.clone(),
        ));

        // First tick: one entry frame per row, ids ASCENDING.
        let mut ids = Vec::new();
        for _ in 0..5 {
            let frame = tokio::time::timeout(Duration::from_secs(3), frames.next())
                .await
                .expect("frame within 3s")
                .expect("stream open");
            assert!(
                frame.starts_with("event: entry\n"),
                "entry frame: {frame:?}"
            );
            let (name, data) = parse_frame(&frame);
            assert_eq!(name, "entry");
            ids.push(data["id"].as_i64().expect("id"));
        }
        assert_eq!(
            ids,
            vec![1, 2, 3, 4, 5],
            "one ascending entry frame per row"
        );

        // Second tick: nothing new → keepalive.
        let frame = tokio::time::timeout(Duration::from_secs(3), frames.next())
            .await
            .expect("second tick within 3s")
            .expect("stream open");
        assert!(
            frame.starts_with("event: keepalive"),
            "empty tick → keepalive: {frame:?}"
        );

        // Cancel stops the stream.
        cancel.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), frames.next())
                .await
                .expect("stop within 2s")
                .is_none(),
            "cancel must end the stream"
        );
    }

    /// A row INSERTED after the stream starts arrives on a later tick.
    #[tokio::test]
    async fn test_log_stream_frames_picks_up_new_rows() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir.path().join("stream2.db");
        {
            let s = LogStore::open(&path).expect("seed conn");
            s.insert_batch(&[rec(
                100,
                LogstoreLevel::INFO,
                "proxy",
                vjson!({"message": "first"}),
            )])
            .expect("seed");
        }
        let writer_conn = Arc::new(Mutex::new(LogStore::open(&path).expect("writer conn")));
        let store = Arc::new(Mutex::new(LogStore::open(&path).expect("stream conn")));
        let cancel = CancellationToken::new();
        let mut frames = Box::pin(log_stream_frames(
            store,
            0,
            LogQuery::default(),
            cancel.clone(),
        ));

        // Consume the first-tick row (id 1), then let the writer insert.
        let frame = tokio::time::timeout(Duration::from_secs(3), frames.next())
            .await
            .expect("first tick")
            .expect("open");
        let (_, data) = parse_frame(&frame);
        assert_eq!(data["id"].as_i64(), Some(1));

        let writer = writer_conn.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1300)).await;
            writer
                .lock()
                .unwrap()
                .insert_batch(&[
                    rec(
                        200,
                        LogstoreLevel::INFO,
                        "proxy",
                        vjson!({"message": "second"}),
                    ),
                    rec(
                        300,
                        LogstoreLevel::INFO,
                        "proxy",
                        vjson!({"message": "third"}),
                    ),
                ])
                .expect("writer insert");
        });

        // Within ~3 s the new rows (ids 2, 3) must arrive in order.
        let mut got = Vec::new();
        while got.len() < 2 {
            let frame = tokio::time::timeout(Duration::from_secs(5), frames.next())
                .await
                .expect("tick within 5s")
                .expect("open");
            if let Some(entry) = frame.strip_prefix("event: entry\n") {
                let data: Value =
                    serde_json::from_str(entry.trim_start_matches("data: ").trim_end_matches('\n'))
                        .expect("entry frame data is JSON");
                got.push(data["id"].as_i64().expect("id"));
            }
        }
        assert_eq!(got, vec![2, 3], "new rows above the anchor, ascending");
        cancel.cancel();
    }

    // ── export ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_export_csv_shape_and_quoting() {
        let store = LogStore::in_memory().expect("store");
        store
            .insert_batch(&[
                rec(
                    100,
                    LogstoreLevel::INFO,
                    "proxy",
                    vjson!({"message": "plain"}),
                ),
                rec(
                    200,
                    LogstoreLevel::WARN,
                    "backend:x",
                    vjson!({"message": "he said \"hi, there\""}),
                ),
            ])
            .expect("seed");
        let (status, bytes) = oneshot(state_with(store), "GET", "/logs/export").await;
        assert_eq!(status, HttpStatus::OK);
        let body = String::from_utf8_lossy(&bytes).to_string();
        let lines: Vec<&str> = body.trim_end().split('\n').collect();
        assert_eq!(lines[0], "id,ts,level,source,message");
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[1],
            "2,200,warn,backend:x,\"he said \"\"hi, there\"\"\""
        );
        assert_eq!(lines[2], "1,100,info,proxy,plain");
    }

    #[tokio::test]
    async fn test_export_rejects_non_csv_format() {
        let (status, bytes) =
            oneshot(state_with(seed_store()), "GET", "/logs/export?format=json").await;
        assert_eq!(status, HttpStatus::BAD_REQUEST);
        assert_eq!(json_body(&bytes)["error"]["type"], "ValidationError");
    }

    /// COUNT-FIRST + cap: 50_001 seeded rows → 413 BEFORE any row is
    /// streamed (end-to-end).
    #[tokio::test]
    async fn test_export_cap_413_before_streaming() {
        let s = LogStore::in_memory().expect("store");
        let total = EXPORT_CAP_ROWS + 1;
        let rows: Vec<_> = (1..=total)
            .map(|i| rec(i, LogstoreLevel::INFO, "proxy", vjson!({"message": "n"})))
            .collect();
        s.insert_batch(&rows).expect("bulk seed");

        let (status, bytes) = oneshot(state_with(s), "GET", "/logs/export").await;
        assert_eq!(status, HttpStatus::PAYLOAD_TOO_LARGE);
        assert_eq!(
            json_body(&bytes)["error"].as_str(),
            Some("export cap of 50000 rows exceeded — narrow the window")
        );
    }

    // ── delete ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_logs_all_and_idempotent() {
        let state = state_with(seed_store());
        let (status, bytes) = oneshot(state.clone(), "DELETE", "/logs").await;
        assert_eq!(status, HttpStatus::ACCEPTED);
        assert_eq!(
            json_body(&bytes),
            vjson!({ "deleted": 8, "compacted": true })
        );

        // Same state (same store): nothing left to delete.
        let (status, bytes) = oneshot(state, "DELETE", "/logs").await;
        assert_eq!(status, HttpStatus::ACCEPTED);
        assert_eq!(json_body(&bytes)["deleted"], 0);
    }

    // ── legacy tail ────────────────────────────────────────────────────

    /// A fake provider: records call count, returns N pre-scripted
    /// rows for any source with a single fetch timestamp.
    struct FakeTail {
        lines: Arc<Vec<String>>,
        calls: Arc<AtomicUsize>,
    }

    impl LogTailProvider for FakeTail {
        fn tail<'a>(
            &'a self,
            _source: &'a LogTailSource,
        ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(i64, String)>>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let ts = now_unix_ms();
                Ok(self.lines.iter().map(|l| (ts, l.clone())).collect())
            })
        }
    }

    fn state_with_tail(provider: Arc<dyn LogTailProvider>) -> LogsApiState {
        // Empty store: the tail rows are the only content for the
        // tamad source in this test.
        let mut st = state_with(LogStore::in_memory().expect("empty store"));
        st.log_tail = Some(provider);
        st
    }

    #[tokio::test]
    async fn test_query_legacy_tail_rows() {
        let provider: Arc<dyn LogTailProvider> = Arc::new(FakeTail {
            lines: Arc::new(vec![
                "engine warmup".to_string(),
                "weights loaded".to_string(),
                "first token in 12ms".to_string(),
            ]),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let (status, bytes) = oneshot(
            state_with_tail(provider),
            "GET",
            "/logs?source=tamad%3Agpu-box%3Amodel%3Aqwen3",
        )
        .await;
        assert_eq!(status, HttpStatus::OK);
        let body = json_body(&bytes);
        let entries = body["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        for e in entries {
            assert_eq!(e["legacy"], true);
            assert_eq!(e["level"], "info");
            assert_eq!(e["level_known"], false);
            assert!(e["id"].as_i64().unwrap() < 0, "tail id is negative");
            assert!(e["fields"].as_object().unwrap().is_empty());
        }
        assert_eq!(entries[0]["message"].as_str(), Some("engine warmup"));
        assert_eq!(entries[2]["message"].as_str(), Some("first token in 12ms"));
        assert!(body["next_cursor"].is_null());
    }

    /// TTL cache: the 2nd poll inside the window reuses the fetch (one
    /// underlying call, same fetch_ts); after expiry a re-fetch.
    #[tokio::test]
    async fn test_tail_cache_ttl() {
        let calls = Arc::new(AtomicUsize::new(0));
        let inner: Arc<dyn LogTailProvider> = Arc::new(FakeTail {
            lines: Arc::new(vec!["a".to_string(), "b".to_string()]),
            calls: calls.clone(),
        });
        let cache = CachingTailProvider::with_ttl(inner, Duration::from_millis(10));
        let src = LogTailSource::new("tamad:host:model:x");

        let first = cache.tail(&src).await.expect("tail 1");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let second = cache.tail(&src).await.expect("tail 2");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "within-TTL polls must reuse the fetch"
        );
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.0, b.0, "same fetch ts within the window");
        }

        tokio::time::sleep(Duration::from_millis(25)).await;
        let third = cache.tail(&src).await.expect("tail 3");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "past-TTL polls must re-fetch"
        );
        assert!(
            third.first().map(|(ts, _)| *ts).unwrap() >= first.first().map(|(ts, _)| *ts).unwrap(),
            "re-fetch ts is not before the first"
        );
    }

    // ── csv + frames ────────────────────────────────────────────────────

    #[test]
    fn test_csv_field_quoting() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("a\nb"), "\"a\nb\"");
        assert_eq!(csv_field("a\rb"), "\"a\rb\"");
    }

    #[test]
    fn test_frame_shapes_and_round_trip() {
        let dto = LogEntryDto::from_tail_line("proxy", 1_000, 0, "hello");
        let frame = entry_frame(&dto);
        assert!(frame.starts_with("event: entry\n"));
        let (name, data) = parse_frame(&frame);
        assert_eq!(name, "entry");
        assert_eq!(data["message"], "hello");

        let keepalive = keepalive_frame();
        assert!(keepalive.starts_with("event: keepalive\n"));
    }

    /// The wire payload of an `event: entry` frame must be **bare** compact
    /// JSON: the browser's EventSource strips exactly one `data: ` prefix,
    /// so `frame_to_sse_event` (which hands its `data` payload to axum,
    /// which re-emits `data: <payload>`) must NOT keep the frame's own
    /// `data: ` prefix — a double prefix made every event fail to parse
    /// client-side (data arrived as `data: {…}`, plan-195 Task 5 regression).
    #[test]
    fn test_frame_to_sse_event_emits_bare_json_payload() {
        let dto =
            LogEntryDto::from_tail_line("proxy", 1_000, 0, "data must not be double-prefixed");
        let frame = entry_frame(&dto);
        let (name, payload) = sse_event_parts(&frame);
        assert_eq!(name, "entry");
        assert!(
            !payload.starts_with("data: "),
            "payload is double-prefixed: {payload:?}"
        );
        let v: serde_json::Value =
            serde_json::from_str(&payload).expect("payload must be bare JSON");
        assert_eq!(v["message"], "data must not be double-prefixed");

        // keepalive carries its flag as bare JSON too.
        let (kname, kpayload) = sse_event_parts(&keepalive_frame());
        assert_eq!(kname, "keepalive");
        let kv: serde_json::Value =
            serde_json::from_str(&kpayload).expect("keepalive payload must be bare JSON");
        assert_eq!(kv["keepalive"], true);
    }

    /// `from_tail_line` id convention: negative, unique + ordered by line
    /// ordinal, never colliding with positive store ids.
    #[test]
    fn test_tail_id_convention() {
        let a = LogEntryDto::from_tail_line("proxy", 5, 0, "x");
        let b = LogEntryDto::from_tail_line("proxy", 5, 1, "y");
        assert!(a.id < 0);
        assert!(b.id < 0);
        assert_eq!(a.id, -5000);
        assert_eq!(b.id, -(5 * 1000 + 1));
        assert!(a.id > b.id, "smaller ordinal → larger (less negative) id");
    }
}
