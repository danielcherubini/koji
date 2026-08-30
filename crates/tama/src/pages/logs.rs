//! `/tama/logs` page (plan-195 task 5).
//!
//! Replaces the old polling file-tail UI. All data comes from the queryable
//! read API (`GET /tama/v1/logs*`, spec: `docs/api/logs.md`):
//!
//! - **Initial page + refetch** on every filter change: source (picker),
//!   level chips (minimum level → `level=`), time presets (`since`),
//!   full-text search (`q`).
//! - **Live tail** via server-sent `entry` events on
//!   `/tama/v1/logs/stream?…&after=<max id seen>`; newest-appended
//!   into a capped in-buffer row list (2 000, drop-oldest from the head
//!   with a "…N older trimmed" line); scrolling up (away from the
//!   bottom) pauses, "jump to latest" scrolls to the bottom, resumes
//!   and flushes the held rows.
//! - **Writer-health banner** seeded by a one-shot `GET /logs/status` and
//!   live via the `log_store` self-describing frames on
//!   `GET /tama/v1/logs/events`; dismissible per browser session.
//!
//! The pure helpers (URL codec, window math, row model, buffer trim) live
//! in [`crate::utils::log_page`] and are unit-tested in the native build.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashSet;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::utils::log_page::{self as lp, LevelFilter, LogEntryRow, TimeWindow, MAX_BUFFER_ROWS};
use crate::utils::{get_request, handle_response};

/// Hard cap of the initial page fetch (the API clamps to 1000 anyway).
const PAGE_LIMIT: u32 = 1_000;
/// Source selected when the URL carries none.
const DEFAULT_SOURCE: &str = "proxy";
/// How far (px) you may be scrolled away from the BOTTOM before
/// live follow pauses.
const SCROLL_FOLLOW_TOLERANCE_PX: i32 = 4;
/// Search-box debounce (ms).
const SEARCH_DEBOUNCE_MS: u32 = 400;
/// sessionStorage key for the banner dismissal (per browser session).
const BANNER_DISMISS_KEY: &str = "tama_logs_banner_dismissed";

// ── Wire DTOs (mirror `docs/api/logs.md`) ─────────────────────────────────

/// One wire row from `GET /tama/v1/logs` (and stream `entry` payloads).
#[derive(Debug, Clone, Deserialize)]
struct LogEntryDto {
    id: i64,
    ts: i64,
    /// Normalized level from the store — an enum field, rendered as-is and
    /// mapped to a badge class. NEVER parsed out of the message text.
    level: String,
    source: String,
    message: String,
    fields: Map<String, Value>,
    /// Drop-marker rows only (`dropped: true` + the row count dropped).
    #[serde(default)]
    dropped: Option<bool>,
    #[serde(default)]
    dropped_count: Option<i64>,
    /// `Some(true)` on on-demand legacy tail rows.
    #[serde(default)]
    legacy: Option<bool>,
}

/// Body of `GET /tama/v1/logs`.
#[derive(Debug, Deserialize)]
struct ListResponse {
    entries: Vec<LogEntryDto>,
}

/// Body of `GET /tama/v1/logs/sources`.
#[derive(Debug, Deserialize)]
struct SourcesResponse {
    sources: Vec<SourceInfo>,
}

/// One entry of the sources list.
#[derive(Debug, Clone, Deserialize)]
struct SourceInfo {
    source: String,
}

/// Per-level counts; the four level keys are zero-filled, `total` counts
/// every row in the window (including `trace` rows).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SummaryCounts {
    debug: i64,
    info: i64,
    warn: i64,
    error: i64,
    total: i64,
}

impl SummaryCounts {
    fn from_map(m: &Value) -> Self {
        let pick = |k: &str| m.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        Self {
            debug: pick("debug"),
            info: pick("info"),
            warn: pick("warn"),
            error: pick("error"),
            total: pick("total"),
        }
    }
}

/// Writer-health snapshot from `GET /tama/v1/logs/status`.
#[derive(Debug, Clone, Copy, Default)]
struct WriterStatus {
    degraded: bool,
    degraded_since: Option<i64>,
}

/// One rendered row (pure DTO → row mapping, unit-tested separately).
fn dto_to_row(dto: &LogEntryDto) -> LogEntryRow {
    let fields: Vec<(String, Value)> = dto
        .fields
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    lp::row_from_parts(
        dto.id,
        dto.ts,
        &dto.level,
        &dto.source,
        &dto.message,
        &fields,
        dto.dropped,
        dto.dropped_count,
        dto.legacy,
    )
}

/// Wall-clock unix ms. `SystemTime::now()` PANICS at runtime on
/// wasm32-unknown-unknown — the JS clock is the only sane unix source.
fn now_wall_ms() -> i64 {
    js_sys::Date::now() as i64
}

/// Unix ms → local `HH:MM:SS` (browser-local clock).
fn fmt_clock(ms: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    format!(
        "{:02}:{:02}:{:02}",
        d.get_hours(),
        d.get_minutes(),
        d.get_seconds()
    )
}

/// Read the banner-dismissal episode from sessionStorage (None if unset).
fn read_banner_dismissed() -> Option<i64> {
    web_sys::window()
        .and_then(|w| w.session_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(BANNER_DISMISS_KEY).ok())
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
}

fn persist_banner_dismissed(value: i64) {
    if let Some(storage) = web_sys::window()
        .and_then(|w| w.session_storage().ok())
        .flatten()
    {
        let _ = storage.set_item(BANNER_DISMISS_KEY, &value.to_string());
    }
}

// ── Page state ─────────────────────────────────────────────────────────────

/// The filter controls, all URL-synced (`?source=&level=&window=&q=`).
#[derive(Debug, Clone)]
struct PageFilters {
    source: String,
    level: LevelFilter,
    window: TimeWindow,
    q: Option<String>,
}

impl PageFilters {
    fn from_query(q: lp::LogPageQuery) -> Self {
        Self {
            source: q.source.unwrap_or_else(|| DEFAULT_SOURCE.to_string()),
            level: q.level,
            window: q.window,
            q: q.q,
        }
    }

    fn to_query(&self) -> lp::LogPageQuery {
        lp::LogPageQuery {
            source: Some(self.source.clone()),
            level: self.level,
            window: self.window,
            q: self.q.clone(),
        }
    }
}

/// Append `incoming` rows (chronological, oldest→newest) onto `rows`, or hold them in
/// `pending` while paused; dedupes by id and trims the oldest head past
/// `MAX_BUFFER_ROWS`.
fn ingest_rows(
    rows: RwSignal<Vec<LogEntryRow>>,
    pending: RwSignal<Vec<LogEntryRow>>,
    live: ReadSignal<bool>,
    trimmed_total: RwSignal<usize>,
    max_id: RwSignal<i64>,
    incoming: Vec<LogEntryRow>,
) {
    if incoming.is_empty() {
        return;
    }
    for r in &incoming {
        max_id.update(|m| *m = (*m).max(r.id));
    }
    let mut seen: HashSet<i64> = HashSet::with_capacity(rows.get_untracked().len() + 16);
    for r in rows.get_untracked() {
        seen.insert(r.id);
    }
    for r in pending.get_untracked() {
        seen.insert(r.id);
    }
    let fresh = lp::only_new(&incoming, |r| r.id, &seen);
    if live.get_untracked() {
        let current_len = rows.get_untracked().len();
        let drop = lp::buffer_trim(current_len, fresh.len(), MAX_BUFFER_ROWS);
        let mut next = rows.get_untracked();
        next.extend(fresh); // chronological: oldest first
        if drop > 0 {
            next.drain(0..drop); // the excess is the oldest head
            trimmed_total.update(|t| *t = t.saturating_add(drop));
        }
        rows.set(next);
    } else {
        pending.update(|v| v.extend(fresh));
    }
}

/// Pin `#id`'s scroll position to the absolute bottom (newest rows live
/// there: the page renders oldest→newest). No-op when the element is
/// absent (page not mounted yet) — the caller already decided to follow.
fn force_scroll_bottom(id: &str) {
    if let Some(window) =
        web_sys::window().and_then(|w| w.document().and_then(|d| d.get_element_by_id(id)))
    {
        if let Ok(el) = window.dyn_into::<web_sys::HtmlElement>() {
            el.set_scroll_top(el.scroll_height());
        }
    }
}

#[component]
pub fn Logs() -> impl IntoView {
    // ── URL state (read once at mount; synced back on every change) ──
    let query = use_query_map();
    let initial =
        lp::LogPageQuery::from_query_string(query.get().to_query_string().trim_start_matches('?'));
    let filters = RwSignal::new(PageFilters::from_query(initial));
    let rows = RwSignal::new(Vec::<LogEntryRow>::new());
    let pending = RwSignal::new(Vec::<LogEntryRow>::new());
    let live = RwSignal::new(true);
    let sources = RwSignal::new(vec![DEFAULT_SOURCE.to_string()]);
    let summary = RwSignal::new(Option::<SummaryCounts>::None);
    let status = RwSignal::new(Option::<WriterStatus>::None);
    let banner_dismissed = RwSignal::new(Option::<i64>::None);
    let loading = RwSignal::new(true);
    let page_error = RwSignal::new(Option::<String>::None);
    let trimmed_total = RwSignal::new(0usize);
    // Generation counter: the filter effect bumps it (0 → 1 on mount, +1
    // per change); the stream effect re-creates its EventSource on each
    // transition (the old connection's on_cleanup closes it).
    let stream_gen = RwSignal::new(0u64);
    let max_id = RwSignal::new(0i64);
    // Coalescing queue for SSE entry events (one message = one row; we
    // batch them so one burst = one render, not one render per row).
    let stream_queue = RwSignal::new(Vec::<LogEntryRow>::new());
    let stream_flushing = RwSignal::new(false);
    let server_now = RwSignal::new(0i64);
    let search_pending = RwSignal::new(Option::<String>::None);
    let search_value = RwSignal::new(String::new());

    // Apply a search value (trims; empty clears the filter).
    // Duplicated as `Fn(String)` closures where needed (closures are not
    // Clone).
    let (apply_search_a, apply_search_b) = {
        let f1 = filters;
        let f2 = filters;
        (
            move |v: String| {
                let v = v.trim().to_string();
                f1.update(|f| f.q = if v.is_empty() { None } else { Some(v) });
            },
            move |v: String| {
                let v = v.trim().to_string();
                f2.update(|f| f.q = if v.is_empty() { None } else { Some(v) });
            },
        )
    };

    // ── Mount effect: clock, sources list, status, store-event SSE ──
    Effect::new(move |_| {
        server_now.set(now_wall_ms());
        banner_dismissed.set(read_banner_dismissed());

        // Sources list for the picker; `proxy` always stays first.
        let sources_c = sources;
        spawn_local(async move {
            if let Ok(resp) = get_request("/tama/v1/logs/sources").send().await {
                if handle_response(&resp) {
                    let _ = resp.text().await;
                    return;
                }
                if (200..300).contains(&resp.status()) {
                    if let Ok(text) = resp.text().await {
                        if let Ok(data) = serde_json::from_str::<SourcesResponse>(&text) {
                            let mut list = vec![DEFAULT_SOURCE.to_string()];
                            for s in data.sources {
                                if !list.contains(&s.source) {
                                    list.push(s.source);
                                }
                            }
                            sources_c.set(list);
                        }
                    }
                }
            }
        });

        // One-shot writer-health snapshot (the SSE covers transitions).
        let status_c = status;
        spawn_local(async move {
            if let Ok(resp) = get_request("/tama/v1/logs/status").send().await {
                if handle_response(&resp) {
                    let _ = resp.text().await;
                    return;
                }
                if (200..300).contains(&resp.status()) {
                    if let Ok(text) = resp.text().await {
                        if let Ok(v) = serde_json::from_str::<Value>(&text) {
                            status_c.set(Some(WriterStatus {
                                degraded: v
                                    .get("degraded")
                                    .and_then(|x| x.as_bool())
                                    .unwrap_or(false),
                                degraded_since: v.get("degraded_since").and_then(|x| x.as_i64()),
                            }));
                        }
                    }
                }
            }
        });

        // Store-event SSE: self-describing frames carry
        // `log_store_degraded` / `log_store_restored`. The browser
        // auto-reconnects transient drops.
        if let Ok(es) = web_sys::EventSource::new("/tama/v1/logs/events") {
            let status_c = status;
            let handler = wasm_bindgen::closure::Closure::<dyn Fn(web_sys::MessageEvent)>::new(
                move |e: web_sys::MessageEvent| {
                    let Some(data) = e.data().as_string() else {
                        return;
                    };
                    let Ok(v) = serde_json::from_str::<Value>(&data) else {
                        return;
                    };
                    match v.get("event").and_then(|x| x.as_str()).unwrap_or_default() {
                        "log_store_degraded" => status_c.set(Some(WriterStatus {
                            degraded: true,
                            degraded_since: v.get("since").and_then(|x| x.as_i64()),
                        })),
                        "log_store_restored" => status_c.set(Some(WriterStatus {
                            degraded: false,
                            degraded_since: None,
                        })),
                        _ => {}
                    }
                },
            );
            let _ =
                es.add_event_listener_with_callback("log_store", handler.as_ref().unchecked_ref());
            handler.forget();
            on_cleanup(move || es.close());
        }
    });

    // ── Filter effect: URL sync, page+summary refetch, stream bump ──
    Effect::new(move |_| {
        let f = filters.get();
        let q = f.to_query();

        // Keep ?source=&level=&window=&q= bookmarkable (history push).
        if let Some(window) = web_sys::window() {
            if let Ok(href) = window.location().href() {
                if let Ok(mut url) = url::Url::parse(&href) {
                    if let Ok(history) = window.history() {
                        url.set_query(Some(&q.to_query_string()));
                        let new_href = url.to_string();
                        let state = wasm_bindgen::JsValue::from(js_sys::Object::new());
                        let _ = history.push_state_with_url(&state, "", Some(&new_href));
                    }
                }
            }
        }

        loading.set(true);
        page_error.set(None);
        trimmed_total.set(0usize);
        search_pending.set(None);
        pending.set(vec![]);
        // Cloned so the async task can bump the stream generation after it
        // has anchored `max_id` (see re-key inside the task).
        let gen_c = stream_gen;
        spawn_local(async move {
            // Initial page: filtered, oldest first (oldest at the top
            // of the buffer / bottom-pinned view lands on the newest).
            let qstring = q.api_query(server_now.get_untracked());
            let page_url = format!("/tama/v1/logs?{qstring}&limit={PAGE_LIMIT}&order=asc");
            match get_request(&page_url).send().await {
                Ok(resp) => {
                    if handle_response(&resp) {
                        let _ = resp.text().await;
                        return;
                    }
                    let status = resp.status();
                    if (200..300).contains(&status) {
                        match resp.text().await {
                            Ok(text) => match serde_json::from_str::<ListResponse>(&text) {
                                Ok(data) => {
                                    let total = data.entries.len();
                                    let mut mapped =
                                        data.entries.iter().map(dto_to_row).collect::<Vec<_>>();
                                    let mut hi = 0i64;
                                    for r in &mapped {
                                        hi = hi.max(r.id);
                                    }
                                    if hi > max_id.get() {
                                        max_id.set(hi);
                                    }
                                    let dropped_by_cap = total.saturating_sub(MAX_BUFFER_ROWS);
                                    mapped.truncate(MAX_BUFFER_ROWS);
                                    if dropped_by_cap > 0 {
                                        trimmed_total.set(dropped_by_cap);
                                    }
                                    rows.set(mapped);

                                    // Land the viewport on the newest rows (the
                                    // buffer's tail / bottom); re-assert once Leptos
                                    // has rendered the batch (the immediate read can
                                    // predate the layout).
                                    force_scroll_bottom("log-entries");
                                    let landing_live = live;
                                    wasm_bindgen_futures::spawn_local(async move {
                                        gloo_timers::future::TimeoutFuture::new(80).await;
                                        if landing_live.get_untracked() {
                                            force_scroll_bottom("log-entries");
                                        }
                                    });
                                }
                                Err(e) => {
                                    page_error.set(Some(format!("Parse error: {e}")));
                                }
                            },
                            Err(e) => {
                                page_error.set(Some(format!("Failed to read body: {e}")));
                            }
                        }
                        loading.set(false);
                    } else if status == 503 {
                        page_error.set(Some(
                            "Log store not wired (degraded runtime) — read API unavailable"
                                .to_string(),
                        ));
                        loading.set(false);
                    } else {
                        page_error.set(Some(format!("HTTP {status} from the log store")));
                        loading.set(false);
                    }
                }
                Err(e) => {
                    page_error.set(Some(format!("Failed to load logs: {e}")));
                    loading.set(false);
                }
            }

            // Count eyebrow for the current window (honours `since`).
            let since_part = q
                .window
                .since_ms(server_now.get_untracked())
                .map(|s| format!("?since={s}"))
                .unwrap_or_default();
            if let Ok(resp) = get_request(&format!("/tama/v1/logs/summary{since_part}"))
                .send()
                .await
            {
                if !handle_response(&resp) && (200..300).contains(&resp.status()) {
                    let mut got_summary = false;
                    if let Ok(text) = resp.text().await {
                        if let Ok(body) = serde_json::from_str::<Value>(&text) {
                            if let Some(counts) = body.get("counts") {
                                summary.set(Some(SummaryCounts::from_map(counts)));
                                got_summary = true;
                            }
                        }
                    }
                    if !got_summary {
                        summary.set(None);
                    }
                }
            }

            // Re-key the live tail now that the page fetch above has anchored
            // `max_id` (the stream effect reads it lazily per generation).
            // Bumping at filter-effect time would (re)create the EventSource
            // with the previous anchor (0 on first paint), replaying every
            // stored row in the window at once — a render storm that wedged
            // the page under the dev debug-level flood.
            gen_c.update(|g| *g += 1);
        });
    });

    // ── Live-tail effect: ONE EventSource per generation ──────────────
    Effect::new(move |_| {
        let gen = stream_gen.get();
        if gen == 0 {
            return;
        }
        let max = max_id.get_untracked();
        let q = filters.get_untracked().to_query();
        let qstring = q.api_query(server_now.get_untracked());
        let path = format!("/tama/v1/logs/stream?{qstring}&after={max}");
        let Ok(es) = web_sys::EventSource::new(&path) else {
            return;
        };
        let handler = wasm_bindgen::closure::Closure::<dyn Fn(web_sys::MessageEvent)>::new(
            move |e: web_sys::MessageEvent| {
                let Some(data) = e.data().as_string() else {
                    return;
                };
                let Ok(dto) = serde_json::from_str::<LogEntryDto>(&data) else {
                    return;
                };
                stream_queue.update(|q| q.push(dto_to_row(&dto)));
                if stream_flushing.get_untracked() {
                    return;
                }
                stream_flushing.set(true);
                let batch_rows = rows;
                let batch_pending = pending;
                let batch_live = live;
                let batch_trimmed = trimmed_total;
                let batch_max_id = max_id;
                let queue_q = stream_queue;
                let flushing_f = stream_flushing;
                spawn_local(async move {
                    gloo_timers::future::TimeoutFuture::new(75).await;
                    let batch = queue_q.get_untracked();
                    queue_q.set(Vec::new());
                    flushing_f.set(false);
                    if !batch.is_empty() {
                        ingest_rows(
                            batch_rows,
                            batch_pending,
                            batch_live.read_only(),
                            batch_trimmed,
                            batch_max_id,
                            batch,
                        );
                        // Keep the bottom pinned while following: re-assert the
                        // bottom edge once the appended rows are laid out.
                        if batch_live.get() {
                            wasm_bindgen_futures::spawn_local(async move {
                                gloo_timers::future::TimeoutFuture::new(80).await;
                                if batch_live.get() {
                                    force_scroll_bottom("log-entries");
                                }
                            });
                        }
                    }
                });
            },
        );
        let _ = es.add_event_listener_with_callback("entry", handler.as_ref().unchecked_ref());
        handler.forget();
        on_cleanup(move || es.close());
    });

    // ── Search debounce (Enter applies immediately) ───────────────────
    let apply_search_b_c = apply_search_b;
    let search_pending_c = search_pending;
    Effect::new(move |_| {
        let Some(v) = search_pending_c.get() else {
            return;
        };
        let v_c = v.clone();
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(SEARCH_DEBOUNCE_MS).await;
            if search_pending_c.get_untracked() != Some(v_c.clone()) {
                return; // a newer keystroke supersedes this one
            }
            apply_search_b_c(v_c);
            search_pending_c.set(None);
        });
    });

    // ── View ───────────────────────────────────────────────────────────
    view! {
        <div class="page-header">
            <h1>"Log Viewer"</h1>
            <div class="log-filters">
                // Source picker: `proxy` + rows from `GET /logs/sources`
                // (+ the deep-linked source, which may predate its first row).
                <select
                    class="form-select form-select-sm"
                    on:change=move |e: web_sys::Event| {
                    let Some(el) = e
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
                    else {
                        return;
                    };
                    filters.update(|f| f.source = el.value());
                }
                >
                    {move || {
                        let current = filters.with(|f| f.source.clone());
                        let mut list = sources.get();
                        if !list.contains(&current) {
                            list.push(current);
                        }
                        list.into_iter()
                            .map(|s| {
                                let v1 = s.clone();
                                let v2 = s.clone();
                                view! {
                                    <option
                                        value=move || v1.clone()
                                        selected=move || filters.with(|f| f.source.clone()) == v2
                                    >
                                        {s}
                                    </option>
                                }
                            })
                            .collect::<Vec<_>>()
                    }}
                </select>

                // Level chips (minimum level → `level=` API param).
                <div class="log-chips">
                    {lp::LevelFilter::CHIPS.iter().map(|(label, lv)| {
                        let label_c = *label;
                        let lv_c = *lv;
                        view! {
                            <button
                                type="button"
                                class=move || {
                                    if filters.with(|f| f.level) == lv_c {
                                        "log-chip log-chip--active"
                                    } else {
                                        "log-chip"
                                    }
                                }
                                on:click=move |_| {
                                    filters.update(|f| f.level = lv_c);
                                }
                            >
                                {label_c}
                            </button>
                        }
                    }).collect::<Vec<_>>()}
                </div>

                // Time presets (bounded by `since`, omitted for `all`).
                <div class="log-chips">
                    {[(
                        "15m",
                        TimeWindow::FifteenMin,
                    ),
                    ("1h", TimeWindow::Hour),
                    ("24h", TimeWindow::Day),
                    ("all", TimeWindow::All)]
                    .into_iter()
                    .map(|(label, w)| {
                        let w_c = w;
                        view! {
                            <button
                                type="button"
                                class=move || {
                                    if filters.with(|f| f.window) == w_c {
                                        "log-chip log-chip--active"
                                    } else {
                                        "log-chip"
                                    }
                                }
                                on:click=move |_| {
                                    filters.update(|f| f.window = w_c);
                                }
                            >
                                {label}
                            </button>
                        }
                    }).collect::<Vec<_>>()}
                </div>

                // Search box — FTS over the WHOLE stored document.
                <input
                    class="form-input form-input-sm log-search"
                    type="search"
                    placeholder="Search logs…"
                    value=search_value
                    on:input=move |e: web_sys::Event| {
                        let v = e
                            .target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|i| i.value())
                            .unwrap_or_default();
                        search_value.set(v.clone());
                        search_pending.set(Some(v));
                    }
                    on:keydown=move |e: web_sys::KeyboardEvent| {
                        if e.key() == "Enter" {
                            search_pending.set(None);
                            apply_search_a(search_value.get());
                        }
                    }
                />
            </div>
        </div>

        // Degraded-store banner — sticky, dismissible per session.
        {move || {
            let Some(st) = status.get() else {
                return view! { <div></div> }.into_any();
            };
            if !st.degraded {
                return view! { <div></div> }.into_any();
            }
            let Some(since) = st.degraded_since else {
                return view! { <div></div> }.into_any();
            };
            if banner_dismissed.get() == Some(since) {
                return view! { <div></div> }.into_any();
            }
            let since_c = since;
            view! {
                <div class="log-banner log-banner---degraded">
                    <span class="log-banner__icon">"⚠"</span>
                    <span>{format!("log store degraded since {} — storing warn+ only", fmt_clock(since_c))}</span>
                    <button
                        class="log-banner__dismiss"
                        title="Dismiss"
                        on:click=move |_| {
                            persist_banner_dismissed(since_c);
                            banner_dismissed.set(Some(since_c));
                        }
                    >
                        "✕"
                    </button>
                </div>
            }
            .into_any()
        }}

        // Status line: count eyebrow + live pill + CSV export.
        <div class="log-statusline">
            <span class="log-eyebrow">
                {move || match summary.get() {
                    Some(c) => format!(
                        "{} rows this window · {} debug · {} info · {} warn · {} error",
                        c.total, c.debug, c.info, c.warn, c.error
                    ),
                    None => "—".to_string(),
                }}
            </span>
            <span
                class=move || {
                    if live.get() {
                        "log-live-pill log-live-pill---on"
                    } else {
                        "log-live-pill log-live-pill---paused"
                    }
                }
            >
                {move || {
                    if live.get() {
                        "● live".to_string()
                    } else {
                        format!("⏸ holding {} new", pending.get().len())
                    }
                }}
            </span>
            <a
                class="btn btn-secondary btn-sm"
                target="_blank"
                href=move || {
                    let qstring = filters
                        .get()
                        .to_query()
                        .api_query(server_now.get());
                    format!("/tama/v1/logs/export?{qstring}&format=csv")
                }
            >
                "⬇ Export CSV"
            </a>
        </div>

        // Entries — oldest first (top); NEW rows APPEND at the bottom.
        <div
            class="log-entries"
            id="log-entries"
            on:scroll=move |e: web_sys::Event| {
                let target = e.target();
                let Some(el) = target.and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
                else {
                    return;
                };
                // Follow = at the bottom (newest rows live there).
                // (web-sys 0.3 exposes no `scrollBottom`; distance to the
                // bottom edge = scrollHeight - clientHeight - scrollTop.)
                let distance_to_bottom =
                    el.scroll_height() - el.client_height() - el.scroll_top();
                live.set(distance_to_bottom <= SCROLL_FOLLOW_TOLERANCE_PX);
            }
        >
            // Jump-to-latest while paused (flushes held rows + resumes).
            {move || {
                if live.get() {
                    view! { <div></div> }.into_any()
                } else {
                    view! {
                        <button
                            type="button"
                            class="log-jump"
                            on:click=move |_| {
                                let held = pending.get();
                                pending.set(vec![]);
                                live.set(true);
                                if !held.is_empty() {
                                    ingest_rows(
                                        rows,
                                        pending,
                                        live.read_only(),
                                        trimmed_total,
                                        max_id,
                                        held,
                                    );
                                }
                                force_scroll_bottom("log-entries");
                                // Re-assert after Leptos flushes the appended
                                // rows (the flush grows the content below the
                                // viewport, shifting the scroll offset — one
                                // scroll event later the live check sees us
                                // bottom-locked again).
                                wasm_bindgen_futures::spawn_local(async move {
                                    gloo_timers::future::TimeoutFuture::new(80).await;
                                    if live.get() {
                                        force_scroll_bottom("log-entries");
                                    }
                                });
                            }
                        >
                            {format!("↓ Jump to latest ({} new)", pending.get().len())}
                        </button>
                    }
                    .into_any()
                }
            }}
            // Buffer-trim notice: trimmed rows are OLDER than the buffered
            // window, and the oldest buffered rows sit at the top — so the
            // notice renders at the top of the scroll area.
            {move || {
                let t = trimmed_total.get();
                if t > 0 {
                    view! {
                        <div class="log-trimmed">{format!("…{t} older rows trimmed")}</div>
                    }
                    .into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}
            {move || {
                if loading.get() && rows.get().is_empty() {
                    return view! {
                        <div class="spinner-container">
                            <span class="spinner"></span>
                            <span class="text-muted">"Loading logs…"</span>
                        </div>
                    }
                    .into_any();
                }
                if let Some(e) = page_error.get() {
                    return view! {
                        <div class="alert alert--warning">
                            <span class="alert__icon">"⚠"</span>
                            <span>{e}</span>
                        </div>
                    }
                    .into_any();
                }
                if rows.get().is_empty() {
                    return view! {
                        <div class="alert alert--info">
                            <span class="alert__icon">"ℹ"</span>
                            <span>"No log lines match the current filter."</span>
                        </div>
                    }
                    .into_any();
                }
                view! {
                    <div class="log-row-list">
                        {
                            rows.get()
                                .into_iter()
                                .map(|r| {
                            let ts = r.ts;
                            view! {
                                <div class="log-row" id=format!("lr{}", r.id)>
                                <span class="log-row__time">{fmt_clock(ts)}</span>
                                    <span
                                        class=format!("log-row__level {}", r.level_class)
                                    >
                                        {r.level.clone()}
                                    </span>
                                    <span class="log-row__source" title=r.source.clone()>
                                        {r.source.clone()}
                                    </span>
                                    <span class="log-row__msg">{r.message.clone()}</span>
                            {if r.dropped {
                                view! {
                                    <span class="log-row__tag log-row__tag---dropped">
                                        {format!(
                                            "dropped{}",
                                            r.dropped_count
                                                .map(|c| format!(" ×{c}"))
                                                .unwrap_or_default()
                                        )}
                                    </span>
                                }
                                .into_any()
                            } else {
                                view! { <div></div> }.into_any()
                            }}
                            {if r.legacy {
                                view! { <span class="log-row__tag">"legacy tail"</span> }
                                    .into_any()
                            } else {
                                view! { <div></div> }.into_any()
                            }}
                            {if !r.fields.is_empty() {
                                view! {
                                    <details class="log-row__fields">
                                        <summary class="log-row__fields--toggle">
                                            {format!("+{} field{}", r.fields.len(), if r.fields.len() > 1 { "s" } else { "" })}
                                        </summary>
                                        <dl>
                                            {r.fields.iter().map(|(k, v)| {
                                                let k_c = k.clone();
                                                let v_c = v.clone();
                                                view! {
                                                    <div class="log-row__field">
                                                        <dt>{k_c}</dt>
                                                        <dd>{v_c}</dd>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </dl>
                                    </details>
                                }
                                .into_any()
                            } else {
                                view! { <div></div> }.into_any()
                            }}
                        </div>
                    }
                            }).collect::<Vec<_>>()
                        }
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}
