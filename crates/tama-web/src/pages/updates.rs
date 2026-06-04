use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::alert_banner::{AlertBanner, AlertVariant};
use crate::components::job_log_panel::JobLogPanel;
use crate::components::list_card::ListCard;
use crate::components::self_update_section::SelfUpdateSection;
#[cfg(not(feature = "ssr"))]
use crate::utils::sse_stream;
use crate::utils::{extract_and_store_csrf_token, get_request, post_request};

fn short_sha(hash: &Option<String>) -> String {
    match hash {
        Some(h) => h.chars().take(8).collect(),
        None => "—".to_string(),
    }
}

/// Renders the expandable quant list for a model.
/// Called from within the Updates component view; captures signals via params.
/// Parsed quant detail from details_json: (quant_name, filename, current_hash, latest_hash, update_available)
type QuantRow = (Option<String>, String, Option<String>, Option<String>, bool);

fn render_quant_list(
    mid: String,
    quants: Vec<(String, Option<String>, Option<String>, bool)>,
    selections: RwSignal<std::collections::HashMap<String, std::collections::HashSet<String>>>,
    update_busy: RwSignal<Option<String>>,
    on_select_all: impl Fn() + 'static,
    on_update_selected: impl Fn(String) + 'static,
) -> impl IntoView {
    view! {
        <div class="quant-list" style="margin-top:0.5rem;padding-left:1.5rem;">
            {/* Select All button */}
            <div style="display:flex;gap:0.5rem;margin-bottom:0.5rem;">
                <button
                    class="btn btn-ghost btn-sm"
                    style="font-size:0.75rem;padding:0.125rem 0.5rem;"
                    on:click=move |_| on_select_all()
                >
                    "Select All"
                </button>
            </div>

            {/* Quant rows */}
            {quants.into_iter().map(|(quant_name, current_hash, latest_hash, update_available)| {
                let qn = quant_name.clone();
                let mid_for_sel = mid.clone();
                let qn_clone = qn.clone();
                let mid_clone = mid.clone();
                let qn_change = qn.clone();
                let mid_change = mid_for_sel.clone();
                let is_selected = move || {
                    selections.with(|map| map.get(&mid_clone)
                        .map(|set| set.contains(&qn_clone)).unwrap_or(false))
                };
                view! {
                    <label class="quant-item" style="display:flex;align-items:center;gap:0.5rem;padding:0.25rem 0;">
                        <input
                            type="checkbox"
                            prop:checked=is_selected
                            disabled={!update_available}
                            on:change=move |e| {
                                use wasm_bindgen::JsCast;
                                let checked = e.target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                    .map(|el| el.checked())
                                    .unwrap_or(false);
                                if checked {
                                    selections.update(|map| {
                                        map.entry(mid_change.clone())
                                            .or_insert_with(std::collections::HashSet::new)
                                            .insert(qn_change.clone());
                                    });
                                } else {
                                    selections.update(|map| {
                                        if let Some(set) = map.get_mut(&mid_change) {
                                            set.remove(&qn_change);
                                        }
                                    });
                                }
                            }
                        />
                        <span style="font-weight:500;">{quant_name}</span>
                        <span class="text-muted" style="font-size:0.75rem;">{short_sha(&current_hash)}</span>
                        <span style="color:#94a3b8;">"→"</span>
                        <span class="text-muted" style="font-size:0.75rem;">{short_sha(&latest_hash)}</span>
                        {if update_available {
                            view! { <span class="badge" style="background:#f59e0b;color:white;padding:0.125rem 0.375rem;border-radius:4px;font-size:0.625rem;">"Update"</span> }.into_any()
                        } else {
                            view! { <span class="badge" style="background:#22c55e;color:white;padding:0.125rem 0.375rem;border-radius:4px;font-size:0.625rem;">"Up to date"</span> }.into_any()
                        }}
                    </label>
                }.into_any()
            }).collect::<Vec<_>>()}

            {/* Update Selected button */}
            <button
                class="btn btn-primary btn-sm"
                style="margin-top:0.5rem;"
                disabled={
                    let mid_ref = mid.clone();
                    move || {
                        update_busy.with(|b| b.as_ref().map(|id| id == &mid_ref).unwrap_or(false))
                            || selections.with(|map| map.get(&mid_ref)
                                .map(|set| set.is_empty()).unwrap_or(true))
                    }
                }
                on:click=move |_| on_update_selected(mid.clone())
            >
                {let mid_ref = mid.clone(); move || if update_busy.with(|b| b.as_ref().map(|id| id == &mid_ref).unwrap_or(false)) { "Updating...".to_string() } else { "Update Selected".to_string() }}
            </button>
        </div>
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpdateCheckDto {
    pub item_type: String,
    pub item_id: String,
    #[serde(default)]
    pub variant: Option<String>,
    pub repo_id: Option<String>,
    pub display_name: Option<String>,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub status: String,
    pub error_message: Option<String>,
    pub checked_at: i64,
    pub details_json: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpdatesListResponse {
    pub backends: Vec<UpdateCheckDto>,
    pub models: Vec<UpdateCheckDto>,
}

/// Merge a DTO into the updates list, matching on (item_id, variant) for backends,
/// item_id for models. Replaces existing entry or appends if new.
#[cfg(not(feature = "ssr"))]
fn patch_list(list: &mut Vec<UpdateCheckDto>, dto: &UpdateCheckDto) {
    if let Some(existing) = list
        .iter_mut()
        .find(|i| i.item_id == dto.item_id && i.variant == dto.variant)
    {
        *existing = dto.clone();
    } else {
        list.push(dto.clone());
    }
}

#[cfg(not(feature = "ssr"))]
fn handle_update_event(
    event_type: &str,
    data: &serde_json::Value,
    updates: RwSignal<UpdatesListResponse>,
    last_checked: RwSignal<Option<i64>>,
    item_checking: RwSignal<std::collections::HashMap<String, bool>>,
    checking: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    outstanding: RwSignal<u32>,
) {
    let item_type: Option<String> = data
        .get("item_type")
        .and_then(|v| v.as_str().map(String::from));
    let item_id: Option<String> = data
        .get("item_id")
        .and_then(|v| v.as_str().map(String::from));
    let variant: Option<String> = data
        .get("variant")
        .and_then(|v| v.as_str().map(String::from));

    // Build checking key: "backend:name:variant" or "model:id"
    let checking_key = match (item_type.as_deref(), item_id.as_deref(), variant.as_deref()) {
        (Some("backend"), Some(id), Some(v)) => format!("backend:{}:{}", id, v),
        (Some("backend"), Some(id), None) => format!("backend:{}", id),
        (Some("model"), Some(id), _) => format!("model:{}", id),
        _ => format!(
            "{}:{}",
            item_type.as_deref().unwrap_or(""),
            item_id.as_deref().unwrap_or("")
        ),
    };

    match event_type {
        "CheckStarted" => {
            item_checking.update(|m| {
                m.insert(checking_key.clone(), true);
            });
            outstanding.update(|n| *n += 1);
        }
        "CheckCompleted" => {
            item_checking.update(|m| {
                m.remove(&checking_key);
            });
            outstanding.update(|n| {
                if *n > 0 {
                    *n -= 1;
                }
            });
            if outstanding.get() == 0 {
                checking.set(false);
            }
            // Patch the updates list
            if let Some(dto_value) = data.get("dto") {
                if let Ok(dto) = serde_json::from_value::<UpdateCheckDto>(dto_value.clone()) {
                    updates.update(|u| match item_type.as_deref() {
                        Some("backend") => patch_list(&mut u.backends, &dto),
                        Some("model") => patch_list(&mut u.models, &dto),
                        _ => {}
                    });
                    last_checked.set(Some(dto.checked_at));
                }
            }
        }
        "CheckError" => {
            item_checking.update(|m| {
                m.remove(&checking_key);
            });
            outstanding.update(|n| {
                if *n > 0 {
                    *n -= 1;
                }
            });
            if outstanding.get() == 0 {
                checking.set(false);
            }
            if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
                error.set(Some(format!(
                    "{}: {}",
                    item_id.as_deref().unwrap_or("item"),
                    err
                )));
            }
        }
        "CheckSkipped" => {
            checking.set(false);
            outstanding.set(0);
            item_checking.set(std::collections::HashMap::new());
            if let Some(reason) = data.get("reason").and_then(|v| v.as_str()) {
                error.set(Some(reason.to_string()));
            }
        }
        "Lagged" => {
            item_checking.set(std::collections::HashMap::new());
        }
        _ => {}
    }
}

#[component]
pub fn Updates() -> impl IntoView {
    let updates = RwSignal::new(UpdatesListResponse {
        backends: vec![],
        models: vec![],
    });
    let checking = RwSignal::new(false);
    let last_checked = RwSignal::new(Option::<i64>::None);
    let error = RwSignal::new(Option::<String>::None);
    let active_backend_job_id = RwSignal::new(Option::<String>::None);
    let backend_update_busy = RwSignal::new(false);

    // Tracks which models have their quant list expanded (model_id → bool)
    let model_expanded: RwSignal<std::collections::HashMap<String, bool>> =
        RwSignal::new(std::collections::HashMap::new());

    // Tracks selected quants per model (model_id → HashSet of quant keys)
    let model_selections: RwSignal<
        std::collections::HashMap<String, std::collections::HashSet<String>>,
    > = RwSignal::new(std::collections::HashMap::new());

    // Busy state for model update action (model_id → bool)
    let model_update_busy = RwSignal::new(Option::<String>::None);

    // Cancelled flag for SSE cleanup on unmount
    #[cfg(not(feature = "ssr"))]
    let cancelled = RwSignal::new(false);
    // DropGuard sets cancelled to true when component unmounts
    #[cfg(not(feature = "ssr"))]
    struct CancelledGuard(RwSignal<bool>);
    #[cfg(not(feature = "ssr"))]
    impl Drop for CancelledGuard {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    #[cfg(not(feature = "ssr"))]
    let _cancelled_guard = CancelledGuard(cancelled);

    // Per-item checking state: "backend:name:variant" → bool, "model:id" → bool
    #[allow(unused)]
    let item_checking: RwSignal<std::collections::HashMap<String, bool>> =
        RwSignal::new(std::collections::HashMap::new());

    // Outstanding checks counter for "Check Now" (unused in SSR mode)
    #[allow(unused_assignments)]
    let outstanding_checks = RwSignal::new(0u32);

    // Fetch on mount
    Effect::new(move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            match get_request("/tama/v1/updates").send().await {
                Ok(resp) if resp.ok() => {
                    // Store CSRF token from response header (fallback when cookie unavailable)
                    extract_and_store_csrf_token(&resp);
                    if let Ok(data) = resp.json::<UpdatesListResponse>().await {
                        updates.set(data.clone());
                        // Get last checked time from any record
                        let all_items: Vec<_> =
                            data.backends.iter().chain(data.models.iter()).collect();
                        last_checked.set(all_items.iter().map(|r| r.checked_at).max());
                    }
                }
                _ => error.set(Some("Failed to load updates".to_string())),
            }
        });
    });

    // SSE subscription for real-time update events
    #[cfg(not(feature = "ssr"))]
    Effect::new(move |_| {
        let updates = updates;
        let last_checked = last_checked;
        let item_checking = item_checking;
        let checking = checking;
        let error = error;
        let outstanding = outstanding_checks;
        wasm_bindgen_futures::spawn_local(async move {
            // Create ONE connection, subscribe to multiple named event types
            let conn = sse_stream::create("/tama/v1/updates/events".to_string(), cancelled, None);
            if conn.connect_once().await.is_err() {
                return;
            }

            let event_types = [
                "CheckStarted",
                "CheckCompleted",
                "CheckError",
                "CheckSkipped",
                "Lagged",
            ];
            for event_type in &event_types {
                // Clone signals for each spawned task
                let u = updates;
                let lc = last_checked;
                let ic = item_checking;
                let ch = checking;
                let er = error;
                let out = outstanding;
                let et = event_type.to_string();

                // Subscribe on the SAME connection (not a new one)
                match conn.subscribe(event_type) {
                    Ok(mut stream) => {
                        wasm_bindgen_futures::spawn_local(async move {
                            use futures_util::StreamExt;
                            while let Some(result) = stream.next().await {
                                if let Ok(event) = result {
                                    let data: serde_json::Value =
                                        serde_json::from_str(&event.data).unwrap_or_default();
                                    handle_update_event(&et, &data, u, lc, ic, ch, er, out);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        log::debug!("Failed to subscribe to {}: {}", event_type, e);
                    }
                }
            }
        });
    });

    let on_check_now = move |_| {
        checking.set(true);
        error.set(None);
        outstanding_checks.set(0);
        wasm_bindgen_futures::spawn_local(async move {
            match post_request("/tama/v1/updates/check").send().await {
                Ok(resp) if resp.ok() => {
                    // SSE events update cards progressively.
                    // Fallback: if no events arrive within 30s, poll once.
                    gloo_timers::future::TimeoutFuture::new(30000).await;
                    if checking.get() && outstanding_checks.get() == 0 {
                        if let Ok(resp2) = get_request("/tama/v1/updates").send().await {
                            if let Ok(data) = resp2.json::<UpdatesListResponse>().await {
                                updates.set(data);
                            }
                        }
                        checking.set(false);
                    }
                }
                _ => {
                    error.set(Some("Failed to trigger check".to_string()));
                    checking.set(false);
                }
            }
        });
    };

    let on_check_backend = move |(name, variant): (String, Option<String>)| {
        let key = match &variant {
            Some(v) => format!("backend:{}:{}", name, v),
            None => format!("backend:{}", name),
        };
        item_checking.update(|m| {
            m.insert(key.clone(), true);
        });
        let error_key = key.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!(
                "/tama/v1/updates/check/backend/{}",
                urlencoding::encode(&name)
            );
            match post_request(&url).send().await {
                Ok(resp) if !resp.ok() => {
                    let text = resp.text().await.unwrap_or_default();
                    error.update(|e| *e = Some(format!("Check failed: {}", text)));
                    item_checking.update(|m| {
                        m.remove(&error_key);
                    });
                }
                Err(err) => {
                    error.update(|e| *e = Some(format!("Check failed: {}", err)));
                    item_checking.update(|m| {
                        m.remove(&error_key);
                    });
                }
                _ => { /* success — SSE clears checking state */ }
            }
        });
    };

    let on_update_backend = move |(name, variant): (String, Option<String>)| {
        backend_update_busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let url = match variant {
                Some(v) => format!(
                    "/tama/v1/backends/{}/update?gpu_variant={}",
                    urlencoding::encode(&name),
                    urlencoding::encode(&v)
                ),
                None => format!("/tama/v1/backends/{}/update", urlencoding::encode(&name)),
            };
            if let Ok(resp) = post_request(&url).send().await {
                if resp.ok() {
                    if let Ok(data) = resp.json::<serde_json::Value>().await {
                        if let Some(job_id) = data["job_id"].as_str() {
                            active_backend_job_id.set(Some(job_id.to_string()));
                        }
                    }
                } else {
                    let text = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "unknown error".to_string());
                    error.set(Some(format!("Update failed: {}", text)));
                }
            }
        });
    };

    let on_backend_job_close = Callback::new(move |_| {
        active_backend_job_id.set(None);
        backend_update_busy.set(false);
        // Refresh the updates list after job completes
        wasm_bindgen_futures::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(500).await;
            if let Ok(resp) = get_request("/tama/v1/updates").send().await {
                if let Ok(data) = resp.json::<UpdatesListResponse>().await {
                    let all_items: Vec<_> =
                        data.backends.iter().chain(data.models.iter()).collect();
                    last_checked.set(all_items.iter().map(|r| r.checked_at).max());
                    updates.set(data);
                }
            }
        });
    });

    let on_check_model = move |id: String| {
        let key = format!("model:{}", id);
        item_checking.update(|m| {
            m.insert(key.clone(), true);
        });
        let error_key = key.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // The item_id in the DTO is the config_key (e.g., "model-123" or "owner--repo-name")
            let url = format!("/tama/v1/updates/check/model/{}", urlencoding::encode(&id));
            match post_request(&url).send().await {
                Ok(resp) if !resp.ok() => {
                    let text = resp.text().await.unwrap_or_default();
                    error.update(|e| *e = Some(format!("Check failed: {}", text)));
                    item_checking.update(|m| {
                        m.remove(&error_key);
                    });
                }
                Err(err) => {
                    error.update(|e| *e = Some(format!("Check failed: {}", err)));
                    item_checking.update(|m| {
                        m.remove(&error_key);
                    });
                }
                _ => { /* success — SSE clears checking state */ }
            }
        });
    };

    let on_toggle_expand = move |model_id: String| {
        model_expanded.update(|map| {
            map.entry(model_id).and_modify(|v| *v = !*v).or_insert(true);
        });
    };

    let on_update_selected = move |model_id: String| {
        // Read selections inside the async block (not before spawn — avoids unused capture)
        model_update_busy.set(Some(model_id.clone()));
        wasm_bindgen_futures::spawn_local(async move {
            let selected_quants: Vec<String> = model_selections
                .get()
                .get(&model_id)
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default();

            if selected_quants.is_empty() {
                model_update_busy.set(None);
                return;
            }

            let url = format!("/tama/v1/updates/apply/model/{}", model_id);
            match post_request(&url)
                .json(&serde_json::json!({ "quants": selected_quants }))
                .unwrap()
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    // Clear selections for this model immediately
                    model_selections.update(|map| {
                        map.remove(&model_id);
                    });
                    // Refresh list after delay
                    wasm_bindgen_futures::spawn_local(async move {
                        gloo_timers::future::TimeoutFuture::new(2000).await;
                        if let Ok(r) = get_request("/tama/v1/updates").send().await {
                            if let Ok(data) = r.json::<UpdatesListResponse>().await {
                                updates.set(data);
                            }
                        }
                    });
                }
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "unknown error".to_string());
                    if status == 409 {
                        error.set(Some(format!("Download already in progress: {}", text)));
                    } else if status == 422 {
                        error.set(Some(format!("Invalid quant keys: {}", text)));
                    } else {
                        error.set(Some(format!("Update failed: {}", text)));
                    }
                }
                Err(e) => error.set(Some(format!("Request failed: {}", e))),
            }
            model_update_busy.set(None);
        });
    };

    view! {
        <div class="page updates-page">
            <div class="page-header">
                <h1>"Updates Center"</h1>
                <div class="page-header-actions">
                    {move || last_checked.get().map(|ts| {
                        let date = chrono::DateTime::from_timestamp(ts, 0)
                            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_default();
                        view! { <span class="last-checked">"Last checked: " {date}</span> }
                    })}
                    <button
                        class="btn btn-primary"
                        disabled=move || checking.get()
                        on:click=on_check_now
                    >
                        {move || if checking.get() { "Checking..." } else { "Check Now" }}
                    </button>
                </div>
            </div>

            {move || error.get().map(|e| view! {
                <AlertBanner variant=AlertVariant::Error>{e}</AlertBanner>
            })}

            // Self-update section for the Tama application itself
            <section class="updates-section">
                <h2 class="section__title">"Application"</h2>
                <div class="updates-list">
                    <SelfUpdateSection />
                </div>
            </section>

            <section class="updates-section">
                <h2 class="section__title">"Backends"</h2>
                <div class="updates-list">
                    {move || {
                        let backends = updates.with(|u| u.backends.clone());
                        backends.into_iter().map(|b| {
                            let item_id = b.item_id.clone();
                            let variant_for_update = b.variant.clone();
                            let is_update_available = b.update_available;
                            view! {
                                <ListCard
                                    state=if is_update_available { Some(RwSignal::new(Some("update-available".to_string())).read_only()) } else { None }
                                   actions=Some(Box::new(move || {
                                        let variant_clone = variant_for_update.clone();
                                        let btn_key = match &variant_for_update {
                                            Some(v) => format!("backend:{}:{}", item_id, v),
                                            None => format!("backend:{}", item_id),
                                        };
                                        let is_checking = Memo::new(move |_| {
                                            item_checking.with(|m| m.get(&btn_key).copied().unwrap_or(false))
                                        });
                                        view! {
                                            {if is_update_available {
                                                let id = item_id.clone();
                                                let vc = variant_clone.clone();
                                                view! {
                                                    <button class="btn btn-secondary"
                                                        on:click=move |_| on_update_backend((id.clone(), vc.clone()))>
                                                        "Update"
                                                    </button>
                                                }.into_any()
                                            } else {
                                                view! { <span/> }.into_any()
                                            }}
                                            <button
                                                class="btn btn-ghost"
                                                disabled=is_checking
                                                on:click=move |_| on_check_backend((item_id.clone(), variant_clone.clone()))
                                            >
                                                {move || if is_checking.get() { "Checking..." } else { "Check" }}
                                            </button>
                                        }.into_any()
                                    }))
                                >
                                    <span class="update-item__name">{b.item_id.clone()}</span>
                                   {b.variant.as_ref().map(|v| {
                                         view! { <span class="update-item__variant">{v.clone()}</span> }
                                     })}
                                    <span class="update-item__version">
                                        {b.current_version.clone().unwrap_or_else(|| "—".to_string())}
                                    </span>
                                    {if b.update_available {
                                        let latest = b.latest_version.clone().unwrap_or_default();
                                        view! {
                                            <span class="update-badge">
                                                {format!(" → {}", latest)}
                                            </span>
                                        }.into_any()
                                    } else {
                                        view! { <span class="up-to-date-badge">{"✓ Up to date"}</span> }.into_any()
                                    }}
                                </ListCard>
                            }
                        }).collect::<Vec<_>>()
                    }}
                </div>
            </section>

            {/* Backend update progress panel */}
            {move || active_backend_job_id.get().map(|job_id| {
                view! {
                    <JobLogPanel job_id=job_id on_close=on_backend_job_close />
                }.into_any()
            })}

            <section class="updates-section">
                <h2 class="section__title">"Models"</h2>
                <div class="updates-list">
                    {move || {
                        let models = updates.with(|u| u.models.clone());
                        models.into_iter().map(|m| {
                            let model_id = m.item_id.clone();
                            let display_name = m.display_name
                                .clone()
                                .or_else(|| m.repo_id.clone())
                                .unwrap_or_else(|| m.item_id.clone());

                            // Parse quants from details_json (same pattern as get_updates in api/updates.rs)
                            // Use Option<String> for quant_name to preserve entries where it's null (e.g., new remote files)
                            let quants_with_updates: Vec<QuantRow> = m.details_json
                                .as_ref()
                                .and_then(|d| d.get("quants"))
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|q| {
                                            let quant_name = q["quant_name"].as_str().map(String::from);
                                            let filename = q["filename"].as_str()?.to_string();
                                            let current_hash = q["current_hash"].as_str().map(String::from);
                                            let latest_hash = q["latest_hash"].as_str().map(String::from);
                                            let update_available = q["update_available"].as_bool()?;
                                            Some((quant_name, filename, current_hash, latest_hash, update_available))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            // Clone model_id for use in nested closures (avoids FnOnce issue)
                            let mid_expand = model_id.clone();

                            // Owned copy for the select-all callback (use quant_name or fallback to filename)
                            let quants_for_select_owned: Vec<(String, bool)> = quants_with_updates
                                .iter()
                                .map(|(qn, filename, _, _, u)| {
                                    (
                                        qn.clone().unwrap_or_else(|| filename.clone()),
                                        *u,
                                    )
                                })
                                .collect();

                            let has_updates = quants_with_updates.iter().any(|(_, _, _, _, u)| *u);

                            // Clone for icon closure (needs 'static)
                            let mid_for_icon = mid_expand.clone();
                            let model_id_for_icon = model_id.clone();
                            // Clone for line2 closure
                            let mid_for_line2 = mid_expand.clone();
                            // Clone for actions closure
                            let m_item_id_for_actions = m.item_id.clone();

                            view! {
                                <ListCard
                                    state=if has_updates { Some(RwSignal::new(Some("update-available".to_string())).read_only()) } else { None }
                                    icon=Some(Box::new(move || view! {
                                        <span
                                            class="expand-toggle"
                                            style="cursor:pointer;margin-right:0.5rem;font-size:0.75rem;"
                                            on:click=move |_| on_toggle_expand(model_id_for_icon.clone())
                                        >
                                            {let mid_chev = mid_for_icon.clone(); move || {
                                                match model_expanded.get().get(&mid_chev) {
                                                    Some(&v) => if v { "▼".to_string() } else { "▶".to_string() },
                                                    None => "▶".to_string(),
                                                }
                                            }}
                                        </span>
                                    }.into_any()))
                                  actions=Some(Box::new(move || {
                                            let model_key = format!("model:{}", m_item_id_for_actions);
                                            let is_checking = Memo::new(move |_| {
                                                item_checking.with(|m| m.get(&model_key).copied().unwrap_or(false))
                                            });
                                            view! {
                                                <a href=format!("/tama/model/{}/edit", m_item_id_for_actions) class="btn btn-ghost btn-sm">
                                                    "Edit"
                                                </a>
                                                <button
                                                    class="btn btn-ghost btn-sm"
                                                    disabled=is_checking
                                                    on:click=move |_| on_check_model(m_item_id_for_actions.clone())
                                                >
                                                    {move || if is_checking.get() { "Checking..." } else { "Check" }}
                                                </button>
                                            }.into_any()
                                        }))
                                    line2=Some(Box::new(move || view! {
                                        {/* Expandable quant list */}
                                        {let mid_for_cond = mid_for_line2.clone();
                                         let expanded = model_expanded.with(|map| map.get(&mid_for_cond).copied().unwrap_or(false));
                                         if expanded {
                                            // Prepare owned data for the helper function
                                            let mid_sel = mid_for_line2.clone();
                                            let mid_select_all = mid_for_line2.clone();
                                            let quants_owned: Vec<(String, Option<String>, Option<String>, bool)> =
                                                quants_with_updates.iter().map(|(qn, filename, ch, lh, u)| {
                                                    (
                                                        qn.clone().unwrap_or_else(|| filename.clone()),
                                                        ch.clone(),
                                                        lh.clone(),
                                                        *u,
                                                    )
                                                }).collect();
                                            let on_select_all_cb = move || {
                                                model_selections.update(|map| {
                                                    let set: std::collections::HashSet<String> = quants_for_select_owned
                                                        .iter()
                                                        .filter(|(_, u)| *u)
                                                        .map(|(k, _)| k.clone())
                                                        .collect();
                                                    map.insert(mid_select_all.clone(), set);
                                                });
                                            };
                                            render_quant_list(
                                                mid_sel,
                                                quants_owned,
                                                model_selections,
                                                model_update_busy,
                                                on_select_all_cb,
                                                on_update_selected,
                                            ).into_any()
                                        } else {
                                            view! { <span/> }.into_any()
                                        }}

                                    }.into_any()))
                                >
                                    <span class="update-item__name">{display_name}</span>
                                    {m.current_version.as_ref().map(|v| {
                                        let ver = v[..8.min(v.len())].to_string();
                                        view! {
                                            <span class="update-item__version">
                                                {ver}
                                            </span>
                                        }
                                    })}
                                    {if has_updates {
                                        let latest = m.latest_version.as_ref().map(|v| &v[..8.min(v.len())]).unwrap_or("").to_string();
                                        view! {
                                            <span class="update-badge">
                                                {format!(" → {}", latest)}
                                            </span>
                                        }.into_any()
                                    } else {
                                        view! { <span class="up-to-date-badge">{"✓ Up to date"}</span> }.into_any()
                                    }}
                                </ListCard>
                            }.into_any()
                        }).collect::<Vec<_>>()
                    }}
                </div>
            </section>
        </div>
    }
}
