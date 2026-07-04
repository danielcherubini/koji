use std::collections::BTreeMap;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::window;

use crate::components::alert_banner::{AlertBanner, AlertVariant};
use crate::components::gpu_device_card::{
    device_display_label, model_for_device, model_gpu_label, GpuDeviceCard,
};
use crate::components::modal::Modal;
use crate::components::model_card::{ModelCard, ModelPips};
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
use crate::components::BarChart;
use crate::utils::{post_request, rw_signal_to_signal};

mod metrics;
pub use metrics::*;

// ── Sort/Group enums ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SortBy {
    #[default]
    Name,
    Gpu,
    Family,
    Vendor,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupBy {
    Gpu,
    Family,
    Vendor,
    Status,
}

// ── localStorage keys ────────────────────────────────────────────────────────

const SORT_KEY: &str = "tama-models-sort-by";
const GROUP_KEY: &str = "tama-models-group-by";

// ── Sort/Group helpers (adapted for ModelStatus) ─────────────────────────────

/// Extract trailing numeric index from a GPU device string (e.g. "CUDA10" → 10).
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

/// Extract vendor from a ModelStatus using a chain of fallbacks.
fn extract_vendor_model_status(m: &ModelStatus) -> String {
    for (field, separator) in &[
        (&m.display_name, ':'),
        (&m.api_name, ':'),
        (&m.hf_base_model, '/'),
    ] {
        if let Some(name) = field {
            if let Some(vendor) = name.split(*separator).next() {
                let vendor = vendor.trim();
                if !vendor.is_empty() {
                    return vendor.to_string();
                }
            }
        }
    }
    "other".to_string()
}

/// Returns `(priority, index)` for GPU sorting.
fn extract_gpu_sort_key_model_status(gpu_device: &Option<String>) -> (u32, u32) {
    match gpu_device {
        Some(device) => {
            let index = extract_gpu_index(device).unwrap_or(0);
            (0, index)
        }
        None => (1, 0),
    }
}

/// Human-readable GPU label for grouping.
fn gpu_group_label_model_status(gpu_device: &Option<String>) -> String {
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

/// Capitalizes the first letter of a string.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().next().unwrap_or(c).to_string() + chars.as_str(),
    }
}

/// Returns a comparable string for sorting (all non-GPU sorts).
fn extract_sort_key_model_status(m: &ModelStatus, sort_by: SortBy) -> String {
    match sort_by {
        SortBy::Name => metrics::model_display_name(m),
        SortBy::Family => m.hf_architecture_type.clone().unwrap_or_default(),
        SortBy::Vendor => extract_vendor_model_status(m),
        SortBy::Status => m.state.clone(),
        SortBy::Gpu => String::new(), // GPU sort handled separately in sort_models_status
    }
}

/// Returns the grouping key for a model.
fn extract_group_key_model_status(m: &ModelStatus, group_by: GroupBy) -> String {
    match group_by {
        GroupBy::Gpu => gpu_group_label_model_status(&m.gpu_device),
        GroupBy::Family => m
            .hf_architecture_type
            .clone()
            .unwrap_or_else(|| String::from("Unknown")),
        GroupBy::Vendor => extract_vendor_model_status(m),
        GroupBy::Status => match m.state.as_str() {
            "ready" => "Loaded",
            "loading" => "Loading",
            "unloading" => "Unloading",
            "failed" => "Failed",
            _ => "Idle",
        }
        .to_string(),
    }
}

/// Returns display order for group headers.
fn group_display_order(group_by: GroupBy, key: &str) -> u32 {
    match group_by {
        GroupBy::Gpu => {
            if key == "No GPU" {
                return u32::MAX;
            }
            extract_gpu_index(key).unwrap_or(0)
        }
        _ => 0,
    }
}

/// Sort models in place by the given sort criterion.
fn sort_models_status(models: &mut [ModelStatus], sort_by: SortBy) {
    match sort_by {
        SortBy::Gpu => {
            models.sort_by(|a, b| {
                let ka = extract_gpu_sort_key_model_status(&a.gpu_device);
                let kb = extract_gpu_sort_key_model_status(&b.gpu_device);
                ka.cmp(&kb)
            });
        }
        _ => {
            models.sort_by_key(|a| extract_sort_key_model_status(a, sort_by));
        }
    }
}

/// Parse a string into a SortBy enum.
fn parse_sort_by(s: &str) -> SortBy {
    match s {
        "gpu" => SortBy::Gpu,
        "family" => SortBy::Family,
        "vendor" => SortBy::Vendor,
        "status" => SortBy::Status,
        _ => SortBy::Name,
    }
}

/// Parse a string into an Option<GroupBy> enum.
fn parse_group_by(s: &str) -> Option<GroupBy> {
    match s {
        "gpu" => Some(GroupBy::Gpu),
        "family" => Some(GroupBy::Family),
        "vendor" => Some(GroupBy::Vendor),
        "status" => Some(GroupBy::Status),
        _ => None,
    }
}

/// Read a value from localStorage.
fn read_local_storage(key: &str) -> Option<String> {
    window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|ls| ls.get(key).ok())
        .flatten()
}

/// Write a value to localStorage.
fn write_local_storage(key: &str, value: &str) {
    if let Some(ls) = window().and_then(|w| w.local_storage().ok()).flatten() {
        let _ = ls.set(key, value);
    }
}

#[cfg(test)]
mod tests;

#[component]
pub fn Dashboard() -> impl IntoView {
    let history = RwSignal::new(Vec::<MetricHistoryPoint>::new());
    let current = RwSignal::new(MetricCurrent::default());
    let fetch_failed = RwSignal::new(false);
    // Incrementing this signal re-runs the Effect that opens the EventSource.
    let connect_trigger = RwSignal::new(0u32);

    // Open (or re-open) an EventSource each time connect_trigger changes.
    Effect::new(move |_| {
        let _ = connect_trigger.get(); // track signal

        let es = match web_sys::EventSource::new("/tama/v1/system/metrics/stream") {
            Ok(es) => es,
            Err(_) => {
                fetch_failed.set(true);
                return;
            }
        };

        // Handler for "snapshot" events — updates the history buffer (for sparklines)
        // and the current state (for GPU cards, model list, inference stats).
        let on_snapshot =
            Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    if let Ok(snapshot) = serde_json::from_str::<MetricsSnapshot>(&data_str) {
                        fetch_failed.set(false);
                        history.set(snapshot.history);
                        current.set(snapshot.current);
                    }
                }
            });
        let _ =
            es.add_event_listener_with_callback("snapshot", on_snapshot.as_ref().unchecked_ref());
        on_snapshot.forget();

        // Error handler — flag for the empty-history retry UI.
        let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
            fetch_failed.set(true);
        });
        es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        // Close the EventSource when the effect re-runs or the component unmounts.
        on_cleanup(move || {
            es.close();
        });
    });

    // Manual retry: close and re-open the EventSource.
    let manual_refresh = move |_| {
        fetch_failed.set(false);
        connect_trigger.update(|n| *n += 1);
    };

    let restart: Action<(), (), LocalStorage> = Action::new_unsync(|_: &()| async move {
        let _ = post_request("/tama/v1/system/restart").send().await;
    });

    // Per-model load/unload actions wired to the same REST endpoints used by
    // the `/models` page. Both actions are unsync because `gloo_net::Request`
    // returns `!Send` futures in the WASM target.
    //
    // We use a manual "busy" signal instead of relying on Action::pending()
    // because in some WASM error scenarios (e.g. proxy returns 500 with no
    // backend configured), the pending flag can get stuck and never reset,
    // leaving buttons permanently disabled with "Loading…" text.
    let load_busy = RwSignal::new(false);
    let unload_busy = RwSignal::new(false);
    let cancel_busy = RwSignal::new(false);

    // Pull Model modal
    let pull_modal_open = RwSignal::new(false);

    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            load_busy.set(true);
            // Ignore errors — the SSE stream will push updated model state.
            // Even if the request fails (e.g. no backend configured), we set
            // load_busy to false below so the button becomes clickable again.
            let _ = post_request(&format!("/tama/v1/models/{}/load", id))
                .send()
                .await;
            load_busy.set(false);
        }
    });
    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            unload_busy.set(true);
            // Same as load — ignore errors, SSE will push the updated state.
            let _ = post_request(&format!("/tama/v1/models/{}/unload", id))
                .send()
                .await;
            unload_busy.set(false);
        }
    });
    let cancel_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            cancel_busy.set(true);
            // Ignore errors — SSE will push updated model state.
            let _ = post_request(&format!("/tama/v1/models/{}/cancel", id))
                .send()
                .await;
            cancel_busy.set(false);
        }
    });

    // Sort/group state with localStorage persistence
    let sort_by = RwSignal::new({
        let stored = read_local_storage(SORT_KEY);
        stored.as_deref().map(parse_sort_by).unwrap_or(SortBy::Name)
    });
    let group_by = RwSignal::new({
        let stored = read_local_storage(GROUP_KEY);
        stored.as_deref().map(parse_group_by).unwrap_or(None)
    });

    // Persist sort preference
    Effect::new(move || {
        let val = sort_by.get();
        let key_str = match val {
            SortBy::Name => "name",
            SortBy::Gpu => "gpu",
            SortBy::Family => "family",
            SortBy::Vendor => "vendor",
            SortBy::Status => "status",
        };
        write_local_storage(SORT_KEY, key_str);
    });

    // Persist group preference
    Effect::new(move || {
        let val = group_by.get();
        let key_str = match val {
            Some(GroupBy::Gpu) => "gpu",
            Some(GroupBy::Family) => "family",
            Some(GroupBy::Vendor) => "vendor",
            Some(GroupBy::Status) => "status",
            None => "none",
        };
        write_local_storage(GROUP_KEY, key_str);
    });

    view! {
        <div class="page-header">
            <h1>"Dashboard"</h1>
            <div class="page-header-actions">
                // Existing status badge + Restart (inside conditional, only shown after SSE data arrives)
                {move || {
                    history.get().last().cloned().map(|_h| {
                        let badge_class = if fetch_failed.get() { "badge badge-danger" } else { "badge badge-success" };
                        let badge_text = if fetch_failed.get() { "error" } else { "ok" };
                        view! {
                            <div class="flex-between gap-1">
                                <span class={badge_class}>{badge_text}</span>
                                <button class="btn btn-secondary" on:click=move |_| { restart.dispatch(()); }>
                                    "Restart"
                                </button>
                            </div>
                        }
                    })
                }}
                // New buttons (always visible, outside conditional)
                <button class="btn btn-secondary" on:click=move |_| pull_modal_open.set(true)>"Pull Model"</button>

            </div>
        </div>



        {move || {
            let buf = history.get();
            if fetch_failed.get() && buf.is_empty() {
                // Network error, no data yet — show error with retry button
                return view! {
                    <div class="card">
                        <AlertBanner variant=AlertVariant::Error>"Failed to load metrics stream. Is Tama running?"</AlertBanner>
                        <button class="btn btn-secondary btn-sm mt-2" on:click=manual_refresh>"Retry"</button>
                    </div>
                }.into_any();
            }

            // Extract data for sparkline charts
            let cpu_data: Vec<f32> = buf.iter().map(|s| s.cpu_usage_pct).collect();
            let mem_data: Vec<f32> = buf.iter().map(|s| s.ram_used_mib as f32).collect();
            let timestamps: Vec<i64> = buf.iter().map(|s| s.ts_unix_ms).collect();
            let mem_max = buf.last().map(|h| h.ram_total_mib as f32).unwrap_or(1.0);


            // Network data extraction
            let net_download_data: Vec<f32> = buf.iter().map(|s| s.network.as_ref().map(|n| n.download_mibps as f32).unwrap_or(0.0)).collect();
            let net_upload_data: Vec<f32> = buf.iter().map(|s| s.network.as_ref().map(|n| n.upload_mibps as f32).unwrap_or(0.0)).collect();
            // Network has no natural ceiling — pass max_value=0 so BarChart
            // auto-scales to a stable nice number (see BarChart::nice_max).
            // A dynamic max computed from live data would rescale every 2s.

            let all_models: Vec<ModelStatus> = current.get().models.clone();
            let gpus_for_labels = current.get().gpus.clone();

            view! {
                <div class="grid-stats">
                    // CPU card
                    <div class="stat-card">
                        <div class="card-header">"CPU Usage"</div>
                        {match buf.last() {
                            Some(h) => view! {
                                <div class="card-value">{format!("{:.1}%", h.cpu_usage_pct)}</div>
                                <div class="card-secondary">"of 100%"</div>
                            }.into_any(),
                            None => view! {
                                <div class="card-value-empty">"—"</div>
                            }.into_any(),
                        }}
                        <div class="sparkline-container">
                            <BarChart
                                data=cpu_data
                                max_value=100.0
                                color="var(--accent-green)".to_string()
                                height=60.0
                                timestamps=timestamps.clone()
                                unit_label="%".to_string()
                            />
                        </div>
                    </div>

                    // Memory card
                    <div class="stat-card">
                        <div class="card-header">"Memory"</div>
                        {match buf.last() {
                            Some(h) => view! {
                                <div class="card-value">{format_number(h.ram_used_mib)}</div>
                                <div class="card-secondary">{format!("of {} MiB", format_number(h.ram_total_mib))}</div>
                            }.into_any(),
                            None => view! {
                                <div class="card-value-empty">"—"</div>
                            }.into_any(),
                        }}
                        <div class="sparkline-container">
                            <BarChart
                                data=mem_data
                                max_value=mem_max
                                color="var(--accent-blue)".to_string()
                                height=60.0
                                timestamps=timestamps.clone()
                                unit_label="MiB".to_string()
                            />
                        </div>
                    </div>

                    // Network card
                    {match buf.last().and_then(|h| h.network.as_ref()) {
                        Some(net) => view! {
                            <div class="stat-card">
                                <div class="card-header">"Network"</div>
                                <div class="network-rates">
                                    <span class="network-rate network-rate-down">{format!("↓ {:.1} MiB/s", net.download_mibps)}</span>
                                    <span class="network-rate network-rate-up">{format!("↑ {:.1} MiB/s", net.upload_mibps)}</span>
                                </div>
                                <div class="sparkline-container">
                                    <BarChart
                                        data=net_download_data
                                        data2=net_upload_data
                                        max_value=0.0
                                        color="var(--accent-blue)".to_string()
                                        color2="var(--accent-green)".to_string()
                                        height=60.0
                                        timestamps=timestamps.clone()
                                        unit_label="MiB/s".to_string()
                                    />
                                </div>
                            </div>
                        }.into_any(),
                        None => view! {
                            <div class="stat-card">
                                <div class="card-header">"Network"</div>
                                <div class="card-value-empty">"—"</div>
                            </div>
                        }.into_any(),
                    }}
                </div>

                // GPU Devices section — only rendered if any GPU data is present
                // Hidden when no GPUs are detected (laptops, CPU-only servers).
                {move || {
                    let cur = current.get();
                    if !cur.gpus.is_empty() {
                            let loaded_models = cur.models.clone();
                            let gpus = cur.gpus.clone();
                            view! {
                                <section class="dashboard-gpus">
                                    <div class="page-header">
                                        <h2>"GPU Cluster Nodes"</h2>
                                        <span class="text-muted">{format!("{} device(s)", gpus.len())}</span>
                                    </div>
                                    <div class="gpu-device-grid">
                                        {gpus.into_iter().enumerate().map(|(idx, gpu)| {
                                            let label = device_display_label(idx);
                                            let models = loaded_models.clone();
                                            let loaded_for_gpu = model_for_device(&models, &gpu.device_id);
                                            let gpu_prompt_tps = loaded_for_gpu.and_then(|m| m.prompt_tps);
                                            let gpu_tps = loaded_for_gpu.and_then(|m| m.tps);
                                            view! {
                                                <GpuDeviceCard
                                                    device=gpu
                                                    display_label=label
                                                    loaded_models=models
                                                    prompt_tps=gpu_prompt_tps
                                                    tps=gpu_tps
                                                />
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </section>
                            }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}


                // Models section
                <section class="dashboard-models">
                    <div class="page-header">
                        <h2>"Models"</h2>
                        <div class="models-toolbar">
                            <select
                                class="btn btn-secondary btn-sm"
                                on:change=move |e| {
                                    let val = e.target()
                                        .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
                                        .map(|s| s.value())
                                        .unwrap_or_default();
                                    sort_by.set(parse_sort_by(&val));
                                }
                            >
                                <option value="name" selected=move || sort_by.get() == SortBy::Name>"Name"</option>
                                <option value="gpu" selected=move || sort_by.get() == SortBy::Gpu>"GPU"</option>
                                <option value="family" selected=move || sort_by.get() == SortBy::Family>"Family"</option>
                                <option value="vendor" selected=move || sort_by.get() == SortBy::Vendor>"Vendor"</option>
                                <option value="status" selected=move || sort_by.get() == SortBy::Status>"Status"</option>
                            </select>
                            <select
                                class="btn btn-secondary btn-sm"
                                on:change=move |e| {
                                    let val = e.target()
                                        .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
                                        .map(|s| s.value())
                                        .unwrap_or_default();
                                    group_by.set(parse_group_by(&val));
                                }
                            >
                                <option value="none" selected=move || group_by.get().is_none()>"None"</option>
                                <option value="gpu" selected=move || group_by.get() == Some(GroupBy::Gpu)>"GPU"</option>
                                <option value="family" selected=move || group_by.get() == Some(GroupBy::Family)>"Family"</option>
                                <option value="vendor" selected=move || group_by.get() == Some(GroupBy::Vendor)>"Vendor"</option>
                                <option value="status" selected=move || group_by.get() == Some(GroupBy::Status)>"Status"</option>
                            </select>
                            <span class="text-muted">{format!("{} models", all_models.len())}</span>
                        </div>
                    </div>
                    {
                        if all_models.is_empty() {
                            view! {
                                <div class="card card--centered">
                                    <p class="text-muted">"No models configured yet."</p>
                                </div>
                            }.into_any()
                        } else {
                            // Clone, sort, and optionally group the models
                            let mut sorted_models = all_models.clone();
                            sort_models_status(&mut sorted_models, sort_by.get());

                            // Build grouped output
                            let groups: Vec<(Option<String>, Vec<ModelStatus>)> = {
                                let group_by_val = group_by.get();
                                if let Some(group_by_type) = group_by_val {
                                    let mut groups_map: BTreeMap<String, Vec<ModelStatus>> = BTreeMap::new();
                                    let mut group_order: Vec<String> = Vec::new();
                                    for m in &sorted_models {
                                        let key = extract_group_key_model_status(m, group_by_type);
                                        if !groups_map.contains_key(&key) {
                                            group_order.push(key.clone());
                                        }
                                        groups_map.entry(key).or_default().push(m.clone());
                                    }
                                    group_order.sort_by(|a, b| {
                                        let oa = group_display_order(group_by_type, a.as_str());
                                        let ob = group_display_order(group_by_type, b.as_str());
                                        oa.cmp(&ob).then_with(|| a.cmp(b))
                                    });
                                    group_order.into_iter()
                                        .map(|key| {
                                            let models_in_group = groups_map.remove(&key).unwrap();
                                            (Some(capitalize_first(&key)), models_in_group)
                                        })
                                        .collect()
                                } else {
                                    vec![(None, sorted_models)]
                                }
                            };

                            view! {
                                <div class="models-list">
                                    {groups.into_iter().flat_map(|(label, models_in_group)| {
                                        let group_len = models_in_group.len();
                                        let cards: Vec<AnyView> = models_in_group.into_iter().map(|m| {
                                            let on_load_cb = Callback::new(move |id: String| {
                                                load_action.dispatch(id);
                                            });
                                            let on_unload_cb = Callback::new(move |id: String| {
                                                unload_action.dispatch(id);
                                            });
                                            let on_cancel_cb = Callback::new(move |id: String| {
                                                cancel_action.dispatch(id);
                                            });
                                            let gpu_label = model_gpu_label(&gpus_for_labels, &m);
                                            view! {
                                                <ModelCard
                                                    id=m.id.clone()
                                                    db_id=m.db_id
                                                    display_name=model_display_name(&m)
                                                    quant=m.quant.clone()
                                                    context_length=m.context_length
                                                    hf_architecture_type=m.hf_architecture_type.clone()
                                                    hf_base_model=m.hf_base_model.clone()
                                                    pips=ModelPips {
                                                        gpu_variant: m.gpu_variant.clone(),
                                                        cache_type_k: m.cache_type_k.clone(),
                                                        cache_type_v: m.cache_type_v.clone(),
                                                        spec_types: m.spec_types.clone(),
                                                        gpu_label,
                                                    }
                                                    backend=m.backend.clone()
                                                    log_source=Some(format!("{}_{}", m.backend, m.id))
                                                    state=m.state.clone()
                                                    loaded=None
                                                    enabled=None
                                                    error_message=m.error_message.clone()
                                                    on_load=on_load_cb
                                                    on_unload=on_unload_cb
                                                    on_cancel=on_cancel_cb
                                                    load_busy=load_busy
                                                    unload_busy=unload_busy
                                                    cancel_busy=cancel_busy
                                                />
                                            }.into_any()
                                        }).collect();

                                        if let Some(l) = label {
                                            let header: AnyView = view! {
                                                <div class="model-section__title">
                                                    {l} " (" {group_len} " " {if group_len == 1 { "model" } else { "models" }} ")"
                                                </div>
                                            }.into_any();
                                            std::iter::once(header).chain(cards.into_iter()).collect::<Vec<AnyView>>().into_iter()
                                        } else {
                                            cards.into_iter()
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        }
                    }
                </section>
            }.into_any()
        }}

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
                    connect_trigger.update(|n| *n += 1);
                })
                on_close=Callback::new(move |_| pull_modal_open.set(false))
            />
        </Modal>
    }
}
