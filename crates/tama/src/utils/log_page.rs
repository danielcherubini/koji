//! Pure helpers for the `/tama/logs` page (plan-195 task 5).
//!
//! These helpers are deliberately free of any web / async dependency so
//! they compile and are unit-tested in the SSR (native) build as well as
//! the CSR (wasm) build. The page component
//! (`crate::pages::logs`) is the only part that touches the browser.

use std::collections::HashSet;

/// Hard cap for the in-buffer log rows (drop the oldest past this).
pub const MAX_BUFFER_ROWS: usize = 2_000;
/// `GET /tama/v1/logs? q=` is a 400 beyond this — cap the search box locally.
pub const MAX_QUERY_LEN: usize = 512;

/// Minimum-level filter. The chips are `all`, `debug+`, `info+`, `warn+`,
/// `error` — mapped to the API's minimum-level `level` param.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LevelFilter {
    #[default]
    All,
    Debug,
    Info,
    Warn,
    Error,
}

impl LevelFilter {
    /// Chip labels in display order (label, filter).
    pub const CHIPS: [(&'static str, LevelFilter); 5] = [
        ("all", LevelFilter::All),
        ("debug+", LevelFilter::Debug),
        ("info+", LevelFilter::Info),
        ("warn+", LevelFilter::Warn),
        ("error", LevelFilter::Error),
    ];

    /// Parse an API minimum-level name. `trace` and unknowns are rejected
    /// (chips never produce `trace`; the URL codec falls back to `All`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    /// The API `level` param value (minimum level) — `None` omits it.
    pub fn api_level(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Debug => Some("debug"),
            Self::Info => Some("info"),
            Self::Warn => Some("warn"),
            Self::Error => Some("error"),
        }
    }

    /// Resolve a chip label (from the URL or a click) to its API level.
    pub fn api_level_from_chip(label: &str) -> Option<&'static str> {
        Self::CHIPS
            .iter()
            .find(|(l, _)| *l == label)
            .and_then(|(_, f)| f.api_level())
    }
}

/// Time presets. `since = now - preset` is sent as unix ms; `all` omits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeWindow {
    FifteenMin,
    #[default]
    Hour,
    Day,
    All,
}

impl TimeWindow {
    /// Preset token as carried in the `window=` URL param (and shown on the UI).
    pub fn param(self) -> &'static str {
        match self {
            Self::FifteenMin => "15m",
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::All => "all",
        }
    }

    /// Parse a `window=` token; unknowns/empty are rejected (the caller
    /// falls back to the default, `1h`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "15m" => Some(Self::FifteenMin),
            "1h" => Some(Self::Hour),
            "24h" => Some(Self::Day),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// The `since` unix-ms cutoff for `now_ms`, or `None` for `all`.
    pub fn since_ms(self, now_ms: i64) -> Option<i64> {
        Some(match self {
            Self::FifteenMin => now_ms - 15 * 60 * 1_000,
            Self::Hour => now_ms - 60 * 60 * 1_000,
            Self::Day => now_ms - 24 * 60 * 60 * 1_000,
            Self::All => return None,
        })
    }
}

/// Bookmarkable filter state for the log page, URL-synced on the page as
/// `?source=&level=&window=&q=`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogPageQuery {
    pub source: Option<String>,
    pub level: LevelFilter,
    pub window: TimeWindow,
    pub q: Option<String>,
}

impl LogPageQuery {
    /// Encode to a URL query string. `source` is always emitted (it is the
    /// page's identity); `level` / `q` are omitted when empty; `window` is
    /// always emitted (it is the visible preset). Values are percent-encoded.
    pub fn to_query_string(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(source) = &self.source {
            parts.push(format!("source={}", urlencoding::encode(source)));
        }
        if let Some(level) = self.level.api_level() {
            parts.push(format!("level={level}"));
        }
        parts.push(format!("window={}", self.window.param()));
        if let Some(q) = &self.q {
            parts.push(format!("q={}", urlencoding::encode(q)));
        }
        parts.join("&")
    }

    /// Parse a URL query string. Unknown/empty `level` falls back to
    /// `All`, `window` to `1h`, and `q` is capped to `MAX_QUERY_LEN`
    /// instead of being rejected (the API 400s past it).
    pub fn from_query_string(qs: &str) -> Self {
        let mut source: Option<String> = None;
        let mut level = LevelFilter::default();
        let mut window = TimeWindow::default();
        let mut q: Option<String> = None;
        for pair in qs.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => continue,
            };
            match key {
                "source" => {
                    let s = percent_decode(value);
                    if !s.is_empty() {
                        source = Some(s);
                    }
                }
                "level" => {
                    if let Some(parsed) = LevelFilter::parse(&percent_decode(value)) {
                        level = parsed;
                    }
                }
                "window" => {
                    if let Some(parsed) = TimeWindow::parse(&percent_decode(value)) {
                        window = parsed;
                    }
                }
                "q" => {
                    let v = percent_decode(value);
                    if !v.is_empty() {
                        q = Some(v.chars().take(MAX_QUERY_LEN).collect());
                    }
                }
                _ => {}
            }
        }
        Self {
            source,
            level,
            window,
            q,
        }
    }

    /// The query string for the read / export endpoints (`/tama/v1/logs`
    /// or `/tama/v1/logs/export`). Emits the API's `level` (minimum level)
    /// and `since` for bounded windows. Param order: source, level, since, q.
    pub fn api_query(&self, now_ms: i64) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(source) = &self.source {
            parts.push(format!("source={}", urlencoding::encode(source)));
        }
        if let Some(level) = self.level.api_level() {
            parts.push(format!("level={level}"));
        }
        if let Some(since) = self.window.since_ms(now_ms) {
            parts.push(format!("since={since}"));
        }
        if let Some(q) = &self.q {
            parts.push(format!("q={}", urlencoding::encode(q)));
        }
        parts.join("&")
    }
}

/// Pure percent-decode for a single already-extracted query value — keeps
/// the codec std + `urlencoding` only so it stays native/wasm portable.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// ASCII hex digit value (0-15), or `None` for non-hex bytes.
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Map a store/DTO level token to the level-badge CSS class. The level is
/// a normalized enum field from the store (never substring-scanned out of
/// the message); unknown/`trace` fall back to the info class.
pub fn level_badge_class(level: &str) -> &'static str {
    match level {
        "error" => "log-row__level---error",
        "warn" => "log-row__level---warn",
        "debug" => "log-row__level---debug",
        _ => "log-row__level---info",
    }
}

/// Render a DTO `fields` value for the expandable `<dl>` — strings render
/// as plain text, every other JSON value as compact JSON (no external
/// JSON viewer, no client-side re-parse beyond the serde DTO).
pub fn field_display(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// One rendered row for the log viewer, derived purely from the DTO so it
/// is independently unit-testable.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntryRow {
    pub id: i64,
    pub ts: i64,
    pub level: String,
    pub source: String,
    pub message: String,
    /// `fields` (any unknown doc keys) formatted for the `<dl>`.
    pub fields: Vec<(String, String)>,
    pub dropped: bool,
    pub dropped_count: Option<i64>,
    pub legacy: bool,
    pub level_class: &'static str,
}

/// Flatten a `LogEntryDto` (imported by value so the page's DTO never
/// leaks into the helper's public types) into a row model.
#[allow(clippy::too_many_arguments)] // one arg per DTO field — full-flatten is the point
pub fn row_from_parts(
    id: i64,
    ts: i64,
    level: &str,
    source: &str,
    message: &str,
    fields: &[(String, serde_json::Value)],
    dropped: Option<bool>,
    dropped_count: Option<i64>,
    legacy: Option<bool>,
) -> LogEntryRow {
    LogEntryRow {
        id,
        ts,
        level: level.to_string(),
        source: source.to_string(),
        message: message.to_string(),
        fields: fields
            .iter()
            .map(|(k, v)| (k.clone(), field_display(v)))
            .collect(),
        dropped: dropped.unwrap_or(false),
        dropped_count,
        legacy: legacy.unwrap_or(false),
        level_class: level_badge_class(level),
    }
}

/// How many rows to drop from the OLDEST end once `incoming` rows are
/// added to `buffer_len` while staying `<= max`. `0` under cap.
/// (Which end is the oldest is decided by the caller: the page keeps
/// the buffer chronological, oldest at the head.)
pub fn buffer_trim(buffer_len: usize, incoming: usize, max: usize) -> usize {
    (buffer_len.saturating_add(incoming)).saturating_sub(max)
}

/// Keep only the rows whose id is NOT already in `seen` (reconnect
/// re-delivery must not duplicate), preserving `rows` order.
pub fn only_new<T>(rows: &[T], id: impl Fn(&T) -> i64, seen: &HashSet<i64>) -> Vec<T>
where
    T: Clone,
{
    rows.iter()
        .filter(|r| !seen.contains(&id(r)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Level chip / param round-trips: every chip maps to an API level
    /// (except `all`), and parsing the API level name gives the filter back.
    #[test]
    fn test_level_filter_parse_roundtrip() {
        assert_eq!(LevelFilter::parse("debug"), Some(LevelFilter::Debug));
        assert_eq!(LevelFilter::parse("info"), Some(LevelFilter::Info));
        assert_eq!(LevelFilter::parse("warn"), Some(LevelFilter::Warn));
        assert_eq!(LevelFilter::parse("error"), Some(LevelFilter::Error));
        assert_eq!(LevelFilter::parse("trace"), None);
        assert_eq!(LevelFilter::parse("loud"), None);
        assert_eq!(LevelFilter::parse(""), None);

        assert_eq!(LevelFilter::All.api_level(), None);
        assert_eq!(LevelFilter::Debug.api_level(), Some("debug"));
        assert_eq!(LevelFilter::Info.api_level(), Some("info"));
        assert_eq!(LevelFilter::Warn.api_level(), Some("warn"));
        assert_eq!(LevelFilter::Error.api_level(), Some("error"));

        for (label, level) in LevelFilter::CHIPS {
            assert!(label.is_ascii(), "chip labels are ascii: {label}");
            match level {
                LevelFilter::All => assert_eq!(LevelFilter::api_level_from_chip(label), None),
                other => {
                    assert_eq!(
                        LevelFilter::api_level_from_chip(label),
                        other.api_level(),
                        "chip {label}"
                    );
                }
            }
        }
        assert_eq!(LevelFilter::api_level_from_chip("bogus"), None);
    }

    /// Time preset → `since` offset given an injectable `now_ms`.
    #[test]
    fn test_window_since_offsets() {
        let now = 1_700_000_000_000_i64;
        assert_eq!(TimeWindow::FifteenMin.since_ms(now), Some(now - 900_000));
        assert_eq!(TimeWindow::Hour.since_ms(now), Some(now - 3_600_000));
        assert_eq!(TimeWindow::Day.since_ms(now), Some(now - 86_400_000));
        assert_eq!(TimeWindow::All.since_ms(now), None);

        assert_eq!(TimeWindow::parse("15m"), Some(TimeWindow::FifteenMin));
        assert_eq!(TimeWindow::parse("1h"), Some(TimeWindow::Hour));
        assert_eq!(TimeWindow::parse("24h"), Some(TimeWindow::Day));
        assert_eq!(TimeWindow::parse("all"), Some(TimeWindow::All));
        assert_eq!(TimeWindow::parse("2d"), None);
        assert_eq!(TimeWindow::parse(""), None);

        assert_eq!(TimeWindow::default(), TimeWindow::Hour);
        assert_eq!(TimeWindow::Hour.param(), "1h");
        assert_eq!(TimeWindow::All.param(), "all");
    }

    /// Page-URL query-string codec round-trips, including URL-encoded
    /// source labels (`:` and `/` are legal inside a `tamad:` label) and
    /// search terms with spaces / unicode.
    #[test]
    fn test_query_codec_roundtrip() {
        let q = LogPageQuery {
            source: Some("tamad:gpu-box:model:qwen--qwen3.8-27b-fp8".to_string()),
            level: LevelFilter::Warn,
            window: TimeWindow::FifteenMin,
            q: Some("model loaded in 200ms".to_string()),
        };
        let encoded = q.to_query_string();
        let decoded = LogPageQuery::from_query_string(&encoded);
        assert_eq!(decoded, q, "round trip: {encoded}");

        // Optional parts are omitted when empty (source always present).
        let bare = LogPageQuery {
            source: Some("proxy".to_string()),
            level: LevelFilter::All,
            window: TimeWindow::Hour,
            q: None,
        };
        let bare_encoded = bare.to_query_string();
        assert!(
            !bare_encoded.contains("level="),
            "level omitted: {bare_encoded}"
        );
        assert!(!bare_encoded.contains("q="), "q omitted: {bare_encoded}");
        assert!(bare_encoded.starts_with("source=proxy"), "{bare_encoded}");
        assert_eq!(LogPageQuery::from_query_string(&bare_encoded), bare);
    }

    /// Unknown level / window tokens fall back to defaults; a `q` longer
    /// than the API's 512-char cap is capped (never 400'd client-side).
    #[test]
    fn test_query_codec_fault_tolerance() {
        let decoded = LogPageQuery::from_query_string("level=bogus&window=weird&q=x");
        assert_eq!(decoded.level, LevelFilter::All);
        assert_eq!(decoded.window, TimeWindow::Hour);
        assert_eq!(decoded.q.as_deref(), Some("x"));

        let big_q = vec!['a'; MAX_QUERY_LEN + 100];
        let odd = LogPageQuery {
            source: None,
            level: LevelFilter::All,
            window: TimeWindow::All,
            q: Some(big_q.iter().collect::<String>()),
        };
        let decoded_oversized = LogPageQuery::from_query_string(&odd.to_query_string());
        assert_eq!(
            decoded_oversized.q.as_ref().unwrap().len(),
            MAX_QUERY_LEN,
            "oversized q is capped, not rejected"
        );
    }

    /// The API query string carries `level`/`since` (not the chip / window
    /// names) and omits absent parts — proving the server receives the
    /// budgeted parameters.
    #[test]
    fn test_api_query_string() {
        let q = LogPageQuery {
            source: Some("tamad:gpu-box".to_string()),
            level: LevelFilter::Error,
            window: TimeWindow::All,
            q: None,
        };
        assert_eq!(
            q.api_query(1_700_000_000_000),
            "source=tamad%3Agpu-box&level=error"
        );

        let q2 = LogPageQuery {
            source: Some("proxy".to_string()),
            level: LevelFilter::All,
            window: TimeWindow::Hour,
            q: Some("disk full".to_string()),
        };
        assert_eq!(
            q2.api_query(1_700_000_000_000),
            "source=proxy&since=1699996400000&q=disk%20full"
        );
    }

    /// Level → CSS class comes from the ENUM field only (no substring
    /// scan of the message); unknown strings fall back to the info class.
    #[test]
    fn test_level_badge_class() {
        assert_eq!(level_badge_class("error"), "log-row__level---error");
        assert_eq!(level_badge_class("warn"), "log-row__level---warn");
        assert_eq!(level_badge_class("info"), "log-row__level---info");
        assert_eq!(level_badge_class("debug"), "log-row__level---debug");
        assert_eq!(level_badge_class("INFO"), "log-row__level---info");
        assert_eq!(level_badge_class(""), "log-row__level---info");
    }

    /// DTO `fields` values render as plain strings, everything else as
    /// compact JSON (no external JSON viewer, no client-side re-parsing).
    #[test]
    fn test_field_display() {
        assert_eq!(
            field_display(&serde_json::Value::String("cuda:0".into())),
            "cuda:0"
        );
        assert_eq!(field_display(&serde_json::json!(42)), "42");
        assert_eq!(field_display(&serde_json::json!({"a": 1})), "{\"a\":1}");
        assert_eq!(field_display(&serde_json::Value::Null), "null");
    }

    /// Scroll-buffer trim: nothing dropped under the cap; the exact
    /// excess is dropped from the (oldest) tail when over it.
    #[test]
    fn test_buffer_trim() {
        assert_eq!(buffer_trim(0, 10, MAX_BUFFER_ROWS), 0);
        assert_eq!(buffer_trim(1_999, 1, MAX_BUFFER_ROWS), 0);
        assert_eq!(buffer_trim(1_999, 2, MAX_BUFFER_ROWS), 1);
        assert_eq!(buffer_trim(2_000, 500, MAX_BUFFER_ROWS), 500);
        assert_eq!(buffer_trim(2_500, 100, MAX_BUFFER_ROWS), 600);
    }

    /// Incoming stream rows are filtered to ids not already buffered
    /// (reconnect re-delivery must not duplicate rows).
    #[test]
    fn test_only_new() {
        use std::collections::HashSet;
        let seen = HashSet::from([10, 12, 15]);
        let rows: Vec<(i64, &str)> = vec![(15, "a"), (16, "b"), (12, "c"), (17, "d"), (13, "e")];
        let fresh = only_new(&rows, |r| r.0, &seen);
        assert_eq!(
            fresh.iter().map(|r| r.0).collect::<Vec<_>>(),
            vec![16, 17, 13],
            "new ids keep buffer order, known ids dropped"
        );
    }

    /// DTO → row-model flattening, including `dropped` / `legacy` flags and
    /// the level-class derivation from the ENUM field.
    #[test]
    fn test_row_from_parts() {
        let fields = vec![
            ("target".to_string(), serde_json::json!("tama_core::model")),
            ("gpu".to_string(), serde_json::json!({ "id": 0 })),
            ("count".to_string(), serde_json::json!(3)),
        ];
        let row = row_from_parts(
            42,
            1_700_000_123,
            "warn",
            "tamad:gpu-box:model:x",
            "retrying write",
            &fields,
            Some(true),
            Some(7),
            Some(true),
        );
        assert_eq!(row.id, 42);
        assert_eq!(row.ts, 1_700_000_123);
        assert_eq!(row.level, "warn");
        assert_eq!(row.level_class, "log-row__level---warn");
        assert_eq!(row.source, "tamad:gpu-box:model:x");
        assert_eq!(row.message, "retrying write");
        assert_eq!(
            row.fields,
            vec![
                ("target".to_string(), "tama_core::model".to_string()),
                ("gpu".to_string(), "{\"id\":0}".to_string()),
                ("count".to_string(), "3".to_string()),
            ]
        );
        assert!(row.dropped, "dropped flag carried through");
        assert_eq!(row.dropped_count, Some(7));
        assert!(row.legacy, "legacy flag carried through");

        // Normal rows: flags absent → false.
        let clean = row_from_parts(43, 1, "info", "proxy", "ok", &[], None, None, None);
        assert!(!clean.dropped);
        assert!(!clean.legacy);
        assert_eq!(clean.dropped_count, None);
        assert_eq!(clean.level_class, "log-row__level---info");
    }
}
