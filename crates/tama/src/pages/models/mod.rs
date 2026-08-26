mod aliases_tab;
mod providers_tab;
mod tab;

mod filters;
use filters::{
    apply_pipeline, group_survivors, parse_group_by, parse_sort_by, GroupBy, SortBy, ViewFilter,
};

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use self::aliases_tab::AliasesTab;
use self::providers_tab::ProvidersTab;
use self::tab::{Tab, TabPills};
use crate::components::alert_banner::{AlertBanner, AlertVariant};
use crate::components::modal::Modal;
use crate::components::model_card::{ModelCard, ModelPips};
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
use crate::core_mirrors::ModelState;
use crate::utils::{
    get_request, handle_response, post_request, rw_signal_to_signal, target_value,
    CheckAllModelsApiResponse,
};

// ── Data structs ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelEntry {
    id: i64,
    backend: String,
    model: Option<String>,
    quant: Option<String>,
    enabled: bool,
    /// Lifecycle state: idle, loading, ready, unloading, failed.
    #[serde(default)]
    state: ModelState,
    #[serde(default)]
    api_name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    gpu_device: Option<String>,
    #[serde(default)]
    gpu_variant: Option<String>,
    #[serde(default)]
    hf_architecture_type: Option<String>,
    #[serde(default)]
    hf_base_model: Option<String>,
    #[serde(default)]
    hf_format: Option<String>,
    #[serde(default)]
    context_length: Option<u32>,
    #[serde(default)]
    cache_type_k: Option<String>,
    #[serde(default)]
    cache_type_v: Option<String>,
    #[serde(default)]
    spec_types: Vec<String>,
    /// Name of the backend log file stem to open in /tama/logs?source=...
    #[serde(default)]
    log_source: Option<String>,
    /// vLLM-specific config (quantization, kv_cache_dtype, max_model_len, etc.)
    #[serde(default)]
    vllm: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsResponse {
    models: Vec<ModelEntry>,
}

// `tama`'s default feature is `ssr`; BOTH CI clippy gates compile the
// native (ssr) build where the scheduler below is cfg'd out — so these
// consts must be cfg-gated too, or `dead_code` -D-warnings fails the gate.
#[cfg(not(feature = "ssr"))]
/// Fixed interval of the polling scheduler (ms). One interval, dual
/// condition — no dynamic rescheduling.
const FAST_TICK_MS: u64 = 1_500;
#[cfg(not(feature = "ssr"))]
/// Steady-state heartbeat between refetches (ms) when nothing is
/// transitional.
const HEARTBEAT_MS: u64 = 8_000;

/// Current wall-clock time in epoch milliseconds. Saturates to 0 on
/// systems whose clock predates the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// Scheduler decision: refetch iff a model is in a transitional state,
/// the initial fetch has never completed (`last_fetch_ms == 0`), or the
/// last successful fetch is at least `heartbeat_ms` old.
#[cfg(any(not(feature = "ssr"), test))]
fn should_refetch(transitional: bool, last_fetch_ms: u64, now_ms: u64, heartbeat_ms: u64) -> bool {
    transitional || last_fetch_ms == 0 || now_ms.saturating_sub(last_fetch_ms) >= heartbeat_ms
}

// ── Helper functions ─────────────────────────────────────────────────────────

/// Read a key from browser localStorage. `None` during SSR or when the
/// browser/window is unavailable.
fn read_stored(key: &str) -> Option<String> {
    let storage = web_sys::window()?.local_storage().ok()??;
    storage.get_item(key).ok().flatten()
}

/// Write a key to browser localStorage, ignoring failures (SSR, private
/// browsing modes).
fn write_stored(key: &str, value: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(key, value);
    }
}

/// Returns the preferred display name for a model, preferring `display_name`,
/// then `api_name`, falling back to the model `id` otherwise.
fn model_display_name(m: &ModelEntry) -> String {
    m.display_name
        .clone()
        .or(m.api_name.clone())
        .unwrap_or_else(|| m.id.to_string())
}

/// Human-readable GPU label for display.
/// "CUDA1" → "GPU 1", "ROCm0" → "GPU 0", "GPU" → "GPU", None → "No GPU"
fn gpu_group_label(gpu_device: &Option<String>) -> String {
    fn extract_gpu_index(device: &str) -> Option<u32> {
        let mut digits = String::new();
        for c in device.chars().rev() {
            if c.is_ascii_digit() {
                digits.push(c);
            } else {
                break;
            }
        }
        if digits.is_empty() {
            None
        } else {
            let num_str = digits.chars().rev().collect::<String>();
            num_str.parse::<u32>().ok()
        }
    }
    match gpu_device {
        Some(device) => {
            if let Some(index) = extract_gpu_index(device) {
                format!("GPU {}", index)
            } else {
                device.clone()
            }
        }
        None => "No GPU".to_string(),
    }
}

// ── Helper functions for vLLM config fallbacks ─────────────────────────────

/// Extract a string value from a JSON value.
fn json_str(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(String::from)
}

/// Extract a u32 value from a JSON value.
fn json_u32(v: &serde_json::Value) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok())
}

/// Resolve effective context length: top-level `context_length` or vLLM `max_model_len`.
fn resolve_context_length(m: &ModelEntry) -> Option<u32> {
    m.context_length
        .or_else(|| m.vllm.get("max_model_len").and_then(json_u32))
}

/// Resolve effective quant: top-level `quant` or vLLM `quantization`.
fn resolve_quant(m: &ModelEntry) -> Option<String> {
    m.quant
        .clone()
        .or_else(|| m.vllm.get("quantization").and_then(json_str))
}

/// Resolve effective KV cache dtype: top-level `cache_type_k` or vLLM `kv_cache_dtype`.
fn resolve_cache_k(m: &ModelEntry) -> Option<String> {
    m.cache_type_k
        .clone()
        .or_else(|| m.vllm.get("kv_cache_dtype").and_then(json_str))
}

/// Resolve effective KV cache dtype: top-level `cache_type_v` or vLLM `kv_cache_dtype`.
fn resolve_cache_v(m: &ModelEntry) -> Option<String> {
    m.cache_type_v
        .clone()
        .or_else(|| m.vllm.get("kv_cache_dtype").and_then(json_str))
}

// ── Component ────────────────────────────────────────────────────────────────

#[component]
pub fn Models() -> impl IntoView {
    // Refresh trigger signal — increment to force a refetch
    let refresh = RwSignal::new(0u32);
    let pull_modal_open = RwSignal::new(false);

    // Active tab navigation
    let active_tab = RwSignal::new(Tab::Models);

    // Global "Check all for updates" status
    let check_all_busy = RwSignal::new(false);
    let check_all_status = RwSignal::new(Option::<(bool, String)>::None);

    // Epoch ms of the last successful fetch (0 = none yet).
    let last_fetch_ms = RwSignal::new(0u64);
    // True while any model in the last fetch was Starting or Unloading.
    let transitional = RwSignal::new(false);
    // True while a fetch request is in flight (initial value `true`: the
    // first fetch is pending on mount). The scheduler skips ticks while
    // this is set.
    let fetching = RwSignal::new(true);

    // Filter toolbar signals. Search and the state pill are session state
    // (NOT persisted); sort/group-by persist across reloads under their
    // stored keys (#136 parity).
    let search_query = RwSignal::new(String::new());
    let view_filter = RwSignal::new(ViewFilter::All);
    let sort_by = RwSignal::new(
        read_stored("tama-models-sort-by")
            .as_deref()
            .map(parse_sort_by)
            .unwrap_or_default(),
    );
    let group_by = RwSignal::new(
        read_stored("tama-models-group-by")
            .as_deref()
            .and_then(parse_group_by),
    );
    // Persist sort/group whenever they change (also on first run).
    let sort_by_persist = sort_by;
    let _persist_sort = Effect::new(move || {
        let key = match sort_by_persist.get() {
            SortBy::Name => "name",
            SortBy::Status => "status",
            SortBy::Gpu => "gpu",
            SortBy::Family => "family",
            SortBy::Vendor => "vendor",
        };
        write_stored("tama-models-sort-by", key);
    });
    let group_by_persist = group_by;
    let _persist_group = Effect::new(move || {
        let key = match group_by_persist.get() {
            None => "",
            Some(GroupBy::Gpu) => "gpu",
            Some(GroupBy::Family) => "family",
            Some(GroupBy::Vendor) => "vendor",
            Some(GroupBy::Status) => "status",
        };
        write_stored("tama-models-group-by", key);
    });

    let models = LocalResource::new(move || async move {
        let _ = refresh.get(); // track the signal
        fetching.set(true);
        let parsed = async {
            let resp = get_request("/tama/v1/models").send().await.ok()?;
            // POLARITY: `handle_response` returns TRUE when a 401 redirect
            // was triggered (caller must bail) and FALSE for a valid
            // response.
            if handle_response(&resp) {
                return None;
            }
            resp.json::<ModelsResponse>().await.ok()
        }
        .await;
        fetching.set(false);
        if let Some(p) = &parsed {
            last_fetch_ms.set(now_ms());
            transitional.set(
                p.models
                    .iter()
                    .any(|m| matches!(m.state, ModelState::Starting | ModelState::Unloading)),
            );
        }
        parsed
    });

    // Adaptive polling: ~1.5s ticks while any model is transitional, an 8s
    // steady-state heartbeat otherwise. The scheduler is wasm-only — under
    // `--features ssr` the whole frontend runtime is compiled out.
    #[cfg(not(feature = "ssr"))]
    {
        let refresh_i = refresh;
        let last_fetch_ms_i = last_fetch_ms;
        let transitional_i = transitional;
        let fetching_i = fetching;
        let interval = gloo_timers::callback::Interval::new(FAST_TICK_MS as u32, move || {
            // Skip while the tab is hidden. On return the NEXT tick
            // (within one fast tick ≈1.5s) does the catch-up refetch.
            let hidden = web_sys::window()
                .and_then(|w| w.document())
                .map(|d| d.hidden())
                .unwrap_or(false);
            if hidden {
                return;
            }
            // Skip while a fetch is in flight (prevents request
            // overlap; covers the initial load too).
            if fetching_i.get() {
                return;
            }
            if should_refetch(
                transitional_i.get(),
                last_fetch_ms_i.get(),
                now_ms(),
                HEARTBEAT_MS,
            ) {
                refresh_i.update(|n| *n += 1);
            }
        });
        on_cleanup(move || interval.cancel());
    }

    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            transitional.set(true); // optimistic: refetch warranted from next tick
            if let Ok(resp) = post_request(&format!("/tama/v1/models/{}/load", id))
                .send()
                .await
            {
                let _ = handle_response(&resp);
            }
            refresh.update(|n| *n += 1);
            transitional.set(true); // optimistic: refetch warranted from next tick
        }
    });

    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            transitional.set(true); // optimistic: refetch warranted from next tick
            if let Ok(resp) = post_request(&format!("/tama/v1/models/{}/unload", id))
                .send()
                .await
            {
                let _ = handle_response(&resp);
            }
            refresh.update(|n| *n += 1);
            transitional.set(true); // optimistic: refetch warranted from next tick
        }
    });

    let cancel_busy = RwSignal::new(false);
    let cancel_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            transitional.set(true); // optimistic: refetch warranted from next tick
            cancel_busy.set(true);
            if let Ok(resp) = post_request(&format!("/tama/v1/models/{}/cancel", id))
                .send()
                .await
            {
                let _ = handle_response(&resp);
            }
            refresh.update(|n| *n += 1);
            transitional.set(true); // optimistic: refetch warranted from next tick
            cancel_busy.set(false);
        }
    });

    // Fire POST /api/models/:id/refresh for every model sequentially. Safe to
    // run without progress streaming because refresh is a pair of small HTTP
    // calls per model (no downloads, no hashing).
    let check_all_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            transitional.set(true); // optimistic: refetch warranted from next tick
            check_all_busy.set(true);
            check_all_status.set(None);
            // Fetch the list directly from the backend that exposes `id`s with
            // DB metadata so we iterate over the same set the editor operates on.
            let resp = match get_request("/tama/v1/models").send().await {
                Ok(r) => {
                    // NOTE: 401 redirects to /login, which tears down the entire app.
                    // Skipping check_all_busy.set(false) is safe — the component unmounts.
                    if handle_response(&r) {
                        return;
                    }
                    r
                }
                Err(e) => {
                    check_all_status.set(Some((false, format!("Failed to list models: {}", e))));
                    check_all_busy.set(false);
                    return;
                }
            };
            // Surface non-2xx HTTP responses instead of silently falling
            // through to an empty list, which would report "Refreshed 0/0
            // models successfully" on a real server error.
            if !resp.ok() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                check_all_status.set(Some((
                    false,
                    format!("Failed to list models: HTTP {} {}", status, body),
                )));
                check_all_busy.set(false);
                return;
            }
            let list = match resp.json::<CheckAllModelsApiResponse>().await {
                Ok(v) => v,
                Err(e) => {
                    check_all_status
                        .set(Some((false, format!("Failed to parse models list: {}", e))));
                    check_all_busy.set(false);
                    return;
                }
            };

            let ids: Vec<i64> = list.models.iter().map(|m| m.id).collect();

            let total = ids.len();
            let mut ok_count = 0usize;
            let mut failed = Vec::<String>::new();
            for id in ids {
                // Integer IDs don't need URL encoding, but we use format! for
                // consistency with the string-based API in models.rs.
                let url = format!("/tama/v1/models/{}/refresh", id);
                match post_request(&url).send().await {
                    Ok(r) => {
                        // NOTE: 401 redirects to /login, which tears down the entire app.
                        // Skipping check_all_busy.set(false) is safe — the component unmounts.
                        if handle_response(&r) {
                            return;
                        }
                        if r.status() == 200 {
                            ok_count += 1;
                        } else {
                            let text = r.text().await.unwrap_or_default();
                            failed.push(format!("{}: {}", id, text));
                        }
                    }
                    Err(e) => failed.push(format!("{}: {}", id, e)),
                }
            }

            if failed.is_empty() {
                check_all_status.set(Some((
                    true,
                    format!("Refreshed {}/{} models successfully.", ok_count, total),
                )));
            } else {
                check_all_status.set(Some((
                    false,
                    format!(
                        "Refreshed {}/{} models. Failures: {}",
                        ok_count,
                        total,
                        failed.join("; ")
                    ),
                )));
            }
            check_all_busy.set(false);
            refresh.update(|n| *n += 1);
            transitional.set(true); // optimistic: refetch warranted from next tick
        });

    view! {
        {move || match active_tab.get() {
            Tab::Models => view! {
                <div class="page-header">
                    <h1>"Models"</h1>
                    <div class="page-header-actions">
                        <button
                            class="btn btn-secondary"
                            prop:disabled=move || check_all_busy.get()
                            on:click=move |_| { check_all_action.dispatch(()); }
                            title="Check HuggingFace for updated metadata on every model"
                        >
                            {move || if check_all_busy.get() { "Checking..." } else { "Check all for updates" }}
                        </button>
                        <button class="btn btn-primary" on:click=move |_| pull_modal_open.set(true)>
                            "Pull Model"
                        </button>
                    </div>
                </div>
                <TabPills active_tab=active_tab />
                {move || check_all_status.get().map(|(ok, msg)| {
                    let variant = if ok { AlertVariant::Success } else { AlertVariant::Error };
                    view! { <AlertBanner variant=variant>{msg}</AlertBanner> }
                })}

                <div class="models-toolbar models-filter-bar">
                    <input
                        type="search"
                        class="form-input filter-search"
                        placeholder="Filter models (name, repo, quant)…"
                        value=move || search_query.get()
                        on:input=move |ev| search_query.set(target_value(&ev))
                    />
                    <button
                        class=move || {
                            format!("state-pill{}", if view_filter.get() == ViewFilter::All { " state-pill--active" } else { "" })
                        }
                        on:click=move |_| view_filter.set(ViewFilter::All)
                        title="Show all models"
                    >
                        "All"
                    </button>
                    <button
                        class=move || {
                            format!("state-pill{}", if view_filter.get() == ViewFilter::Loaded { " state-pill--active" } else { "" })
                        }
                        on:click=move |_| view_filter.set(ViewFilter::Loaded)
                        title="Only models whose backend is ready"
                    >
                        "Loaded"
                    </button>
                    <button
                        class=move || {
                            format!("state-pill{}", if view_filter.get() == ViewFilter::Idle { " state-pill--active" } else { "" })
                        }
                        on:click=move |_| view_filter.set(ViewFilter::Idle)
                        title="Only idle (nothing loaded) backends"
                    >
                        "Idle"
                    </button>
                    <button
                        class=move || {
                            format!("state-pill{}", if view_filter.get() == ViewFilter::Failed { " state-pill--active" } else { "" })
                        }
                        on:click=move |_| view_filter.set(ViewFilter::Failed)
                        title="Only backends that failed to load"
                    >
                        "Failed"
                    </button>
                    <button
                        class=move || {
                            format!("state-pill{}", if view_filter.get() == ViewFilter::Disabled { " state-pill--active" } else { "" })
                        }
                        on:click=move |_| view_filter.set(ViewFilter::Disabled)
                        title="Only disabled models (regardless of lifecycle state)"
                    >
                        "Disabled"
                    </button>
                    <div class="filter-controls">
                        <select
                            class="filter-select"
                            prop:value=move || {
                                match sort_by.get() {
                                    SortBy::Name => "name",
                                    SortBy::Status => "status",
                                    SortBy::Gpu => "gpu",
                                    SortBy::Family => "family",
                                    SortBy::Vendor => "vendor",
                                }
                            }
                            on:change=move |ev| sort_by.set(parse_sort_by(&target_value(&ev)))
                            title="Sort models by"
                        >
                            <option value="name">"Name"</option>
                            <option value="status">"Status"</option>
                            <option value="gpu">"GPU"</option>
                            <option value="family">"Family"</option>
                            <option value="vendor">"Vendor"</option>
                        </select>
                        <select
                            class="filter-select"
                            prop:value=move || {
                                match group_by.get() {
                                    None => "",
                                    Some(GroupBy::Gpu) => "gpu",
                                    Some(GroupBy::Family) => "family",
                                    Some(GroupBy::Vendor) => "vendor",
                                    Some(GroupBy::Status) => "status",
                                }
                            }
                            on:change=move |ev| group_by.set(parse_group_by(&target_value(&ev)))
                            title="Group models by"
                        >
                            <option value="">"None"</option>
                            <option value="gpu">"GPU"</option>
                            <option value="family">"Family"</option>
                            <option value="vendor">"Vendor"</option>
                            <option value="status">"Status"</option>
                        </select>
                    </div>
                </div>

                <Suspense fallback=|| view! {
                    <div class="card card--centered">
                        <span class="spinner">"Loading models..."</span>
                    </div>
                }>
                    {move || {
                        models.get().map(|guard| {
                            let result = guard.take();
                            match result {
                                Some(data) if data.models.is_empty() => {
                                    view! {
                                        <div class="card card--centered">
                                            <p class="text-muted">"No models configured yet."</p>
                                            <button class="btn btn-primary mt-2" on:click=move |_| pull_modal_open.set(true)>
                                                "Pull a Model"
                                            </button>
                                        </div>
                                    }.into_any()
                                }
                                Some(data) => {
                                    // Client-side pipeline (search → pill → sort → group-by)
                                    // over the already-fetched list — never triggers
                                    // a refetch.
                                    let all = &data.models;
                                    let visible = apply_pipeline(
                                        all,
                                        &search_query.get(),
                                        view_filter.get(),
                                        sort_by.get(),
                                    );
                                    let render_models = |entries: Vec<ModelEntry>| {
                                        entries.into_iter().map(|m| {
                                            let on_load_cb = Callback::new(move |id: String| {
                                                load_action.dispatch(id);
                                            });
                                            let on_unload_cb = Callback::new(move |id: String| {
                                                unload_action.dispatch(id);
                                            });
                                            let on_cancel_cb = Callback::new(move |id: String| {
                                                cancel_action.dispatch(id);
                                            });
                                            let effective_quant = resolve_quant(&m);
                                            let effective_ctx = resolve_context_length(&m);
                                            let effective_cache_k = resolve_cache_k(&m);
                                            let effective_cache_v = resolve_cache_v(&m);
                                            view! {
                                                <ModelCard
                                                    id=m.id.to_string()
                                                    db_id=Some(m.id)
                                                    display_name=model_display_name(&m)
                                                    quant=effective_quant
                                                    context_length=effective_ctx
                                                    pips=ModelPips {
                                                        gpu_variant: m.gpu_variant.clone(),
                                                        gpu_label: Some(gpu_group_label(&m.gpu_device)),
                                                        cache_type_k: effective_cache_k,
                                                        cache_type_v: effective_cache_v,
                                                        spec_types: m.spec_types.clone(),
                                                    }
                                                    backend=m.backend.clone()
                                                    log_source=m.log_source.clone()
                                                    state=m.state.clone()
                                                    enabled=Some(m.enabled)
                                                    hf_architecture_type=m.hf_architecture_type.clone()
                                                    hf_base_model=m.hf_base_model.clone()
                                                    hf_format=m.hf_format.clone()
                                                    on_load=on_load_cb
                                                    on_unload=on_unload_cb
                                                    on_cancel=on_cancel_cb
                                                    cancel_busy=cancel_busy
                                                />
                                            }
                                            .into_any()
                                        }).collect::<Vec<AnyView>>()
                                    };
                                    if !all.is_empty() && visible.is_empty() {
                                        view! {
                                            <div class="card card--centered">
                                                <p class="text-muted">"No models match your filters."</p>
                                                <button class="btn btn-secondary mt-2" on:click=move |_| {
                                                    search_query.set(String::new());
                                                    view_filter.set(ViewFilter::All);
                                                }>
                                                    "Clear filters"
                                                </button>
                                            </div>
                                        }.into_any()
                                    } else if group_by.get().is_none() {
                                        view! {
                                            <div class="models-list">{render_models(visible)}</div>
                                        }.into_any()
                                    } else {
                                        let group_cards: Vec<AnyView> =
                                            group_survivors(&visible, group_by.get().unwrap())
                                                .into_iter()
                                                .map(|(label, bucket)| {
                                                    view! {
                                                        <div class="model-group">
                                                            <div class="group-header">
                                                                <span>{label}</span>
                                                                <span class="group-count">{bucket.len()}</span>
                                                            </div>
                                                            <div class="models-list">{render_models(bucket)}</div>
                                                        </div>
                                                    }
                                                    .into_any()
                                                })
                                                .collect();
                                        view! {
                                            {group_cards}
                                        }
                                        .into_any()
                                    }
                                }
                                None => view! {
                                    <div class="card">
                                        <p class="text-error">"Failed to load models."</p>
                                    </div>
                                }.into_any(),
                            }
                        })
                    }}
                </Suspense>
                <Modal
                    open=rw_signal_to_signal(pull_modal_open)
                    on_close=Callback::new(move |_| pull_modal_open.set(false))
                    title="Pull Model".to_string()
                >
                    <PullQuantWizard
                        initial_repo=Signal::derive(String::new)
                        is_open=rw_signal_to_signal(pull_modal_open)
                        on_complete=Callback::new(move |_completed: Vec<CompletedQuant>| {
                            pull_modal_open.set(false);
                            refresh.update(|n| *n += 1);
                        })
                        on_close=Callback::new(move |_| pull_modal_open.set(false))
                    />
                </Modal>
            }.into_any(),
            Tab::Aliases => view! {
                <AliasesTab active_tab=active_tab />
            }.into_any(),
            Tab::Providers => view! {
                <ProvidersTab active_tab=active_tab />
            }.into_any(),
        }}
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_group_label_cuda() {
        assert_eq!(gpu_group_label(&Some("CUDA0".to_string())), "GPU 0");
    }

    #[test]
    fn test_gpu_group_label_rocm() {
        assert_eq!(gpu_group_label(&Some("ROCm1".to_string())), "GPU 1");
    }

    #[test]
    fn test_gpu_group_label_none() {
        assert_eq!(gpu_group_label(&None), "No GPU");
    }

    #[test]
    fn test_gpu_group_label_no_number() {
        assert_eq!(gpu_group_label(&Some("GPU".to_string())), "GPU");
    }

    #[test]
    fn test_should_refetch_when_transitional() {
        assert!(should_refetch(true, 1_000, 1_001, 8_000)); // 1ms since fetch, still refetch
    }

    #[test]
    fn test_should_refetch_after_heartbeat() {
        assert!(should_refetch(false, 1_000, 9_000, 8_000)); // 8000ms elapsed
    }

    #[test]
    fn test_no_refetch_before_heartbeat_and_not_transitional() {
        assert!(!should_refetch(false, 1_000, 5_000, 8_000));
    }

    #[test]
    fn test_should_refetch_never_overshoots_on_clock_jitter() {
        assert!(!should_refetch(false, 5_000, 1_000, 8_000)); // now < last_fetch
        assert!(should_refetch(false, 0, 1, 8_000)); // last_fetch 0 = never fetched
    }
}
