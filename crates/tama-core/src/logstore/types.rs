//! Log store types: level domain, source labels, query shape, and result rows.
//!
//! One JSON document per row with indexed label columns (the Loki model —
//! see ADR-0013, `docs/adr/0013-log-store-sqlite.md`). Implemented in
//! plan-195 Task 1 (`docs/plans/plan-195-structured-logging.md`).

use serde::{Deserialize, Serialize};

/// Log-level domain: `trace=0, debug=1, info=2, warn=3, error=4`.
///
/// Newtype over `u8`, serialised to JSON as a plain number. The store only
/// ever holds values in `0..=4`; unknown-level entries arriving over the
/// wire (level `-1`) are mapped to [`Self::INFO`] by the proto layer
/// (plan-195 task 6) — never stored as out-of-domain values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct LogstoreLevel(u8);

impl LogstoreLevel {
    /// `0` — most verbose level.
    pub const TRACE: Self = Self(0);
    /// `1`
    pub const DEBUG: Self = Self(1);
    /// `2`
    pub const INFO: Self = Self(2);
    /// `3`
    pub const WARN: Self = Self(3);
    /// `4` — highest severity.
    pub const ERROR: Self = Self(4);

    /// Numeric form (`0 = trace .. 4 = error`).
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    /// Parse the numeric form; `None` when the value exceeds `ERROR`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        if value <= 4 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Display name (`"trace"` .. `"error"`); `"unknown"` for out-of-domain
    /// values defensively (invariants make these unreachable in practice).
    pub const fn as_str(self) -> &'static str {
        match self.0 {
            0 => "trace",
            1 => "debug",
            2 => "info",
            3 => "warn",
            4 => "error",
            _ => "unknown",
        }
    }
}

/// Indexed source label of a log row.
///
/// Shapes:
/// - `proxy` — proxy-side lines
/// - `backend:<name>` — one per backend runtime
/// - `tamad:<host>` — per-host tamad control line
/// - `tamad:<host>:model:<model>` — per-model line
/// - `tamad:<host>:model:<model>:tail` — trailing (post-load) line
///
/// Query terms match **exact or delimiter-aware prefix**: `tamad:gpu-box`
/// matches `tamad:gpu-box` and `tamad:gpu-box:model:x` but NOT
/// `tamad:gpu-boxer` (the prefix clause appends `:`, not nothing).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Source(pub String);

impl Source {
    /// Proxy-side lines.
    pub fn proxy() -> Self {
        Self("proxy".into())
    }

    /// One per backend runtime.
    pub fn backend(name: impl AsRef<str>) -> Self {
        Self(format!("backend:{}", name.as_ref()))
    }

    /// Per-host tamad control line.
    pub fn tamad(host: impl AsRef<str>) -> Self {
        Self(format!("tamad:{}", host.as_ref()))
    }

    /// Per-model tamad line.
    pub fn tamad_model(host: impl AsRef<str>, model: impl AsRef<str>) -> Self {
        Self(format!("tamad:{}:model:{}", host.as_ref(), model.as_ref()))
    }

    /// Trailing (post-load) tamad line for a model.
    pub fn tamad_model_tail(host: impl AsRef<str>, model: impl AsRef<str>) -> Self {
        Self(format!(
            "tamad:{}:model:{}:tail",
            host.as_ref(),
            model.as_ref()
        ))
    }

    /// Parse a raw query term; `None` for empty / whitespace-only terms.
    pub fn parse(query_term: &str) -> Option<Self> {
        let term = query_term.trim();
        if term.is_empty() {
            None
        } else {
            Some(Self(term.to_string()))
        }
    }

    /// Label text as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Incoming log record (not yet stored; no row id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRecord {
    /// Unix milliseconds.
    pub ts: i64,
    /// Stored level (0..=4).
    pub level: LogstoreLevel,
    /// Indexed source label.
    pub source: Source,
    /// One JSON document per row (payload; not indexed as SQL columns).
    pub msg: serde_json::Value,
}

/// A stored log row as read back from the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// SQLite rowid — also the pagination cursor.
    pub id: i64,
    /// Unix milliseconds.
    pub ts: i64,
    /// Stored level (0..=4).
    pub level: LogstoreLevel,
    /// Indexed source label.
    pub source: Source,
    /// The stored JSON document.
    pub msg: serde_json::Value,
}

/// Row ordering for [`LogQuery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryOrder {
    /// Newest first (default; standard log viewer behaviour).
    #[default]
    Desc,
    /// Oldest first.
    Asc,
}

/// Read-query shape shared by every future log endpoint (plan-195 task 4+).
#[derive(Debug, Clone)]
pub struct LogQuery {
    /// `level >= min_level` when set; `None` = any level.
    pub min_level: Option<LogstoreLevel>,
    /// `None` = any source; otherwise matched exact OR as delimiter-aware
    /// prefix (see [`Source`]).
    pub source: Option<Source>,
    /// FTS5 `MATCH` on the msg document. Malformed FTS syntax (or a query
    /// matching nothing) transparently falls back to a `LIKE '%q%'` scan —
    /// search text must never surface a 500.
    pub q: Option<String>,
    /// Unix ms, inclusive (`ts >= since`).
    pub since: Option<i64>,
    /// Unix ms, exclusive (`ts < until`).
    pub until: Option<i64>,
    /// Page size: default 200, hard cap 1000 — clamped, never an error.
    pub limit: Option<i64>,
    /// Rowid cursor: `id < cursor` (Desc) / `id > cursor` (Asc).
    pub cursor: Option<i64>,
    /// Ordering (default [`QueryOrder::Desc`]).
    pub order: QueryOrder,
}

impl Default for LogQuery {
    fn default() -> Self {
        Self {
            min_level: None,
            source: None,
            q: None,
            since: None,
            until: None,
            limit: None,
            cursor: None,
            order: QueryOrder::Desc,
        }
    }
}

impl LogQuery {
    /// Effective page size: defaults to 200 and clamps to `1..=1000`.
    pub fn effective_limit(&self) -> i64 {
        self.limit.unwrap_or(200).clamp(1, 1000)
    }
}

/// Retention bounds for [`crate::logstore::db::LogStore::prune`].
///
/// All three are ceilings on what is kept: never delete a row that any
/// single bound still needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneBounds {
    /// Keep rows newer than `now - max_age_secs` (per-row `ts`).
    pub max_age_secs: i64,
    /// Keep at most this many rows (newest win).
    pub max_rows: i64,
    /// Keep at most this many estimated bytes of log text
    /// (`SUM(LENGTH(msg) + LENGTH(source))` of kept rows).
    pub max_bytes: i64,
}

/// Distinct source label with the latest ts observed for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// The source label.
    pub source: Source,
    /// Latest ts (unix ms) seen for this source.
    pub last_ts: i64,
}

/// Row count for one level (only levels with rows appear in results).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelCount {
    /// The level, numeric domain 0..=4.
    pub level: LogstoreLevel,
    /// Number of rows.
    pub count: i64,
}
