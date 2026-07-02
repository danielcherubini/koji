use std::collections::BTreeMap;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use web_sys::window;

use crate::components::alert_banner::{AlertBanner, AlertVariant};
use crate::components::modal::Modal;
use crate::components::model_card::{ModelCard, ModelPips};
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
use crate::utils::{get_request, post_request, rw_signal_to_signal, CheckAllModelsApiResponse};

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

// ── Data structs ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelEntry {
    id: i64,
    backend: String,
    model: Option<String>,
    quant: Option<String>,
    enabled: bool,
    #[serde(default)]
    loaded: bool,
    /// Lifecycle state: idle, loading, ready, unloading, failed.
    #[serde(default)]
    state: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsResponse {
    models: Vec<ModelEntry>,
}

// ── Helper functions ─────────────────────────────────────────────────────────

/// Returns the preferred display name for a model, preferring `display_name`,
/// then `api_name`, falling back to the model `id` otherwise.
fn model_display_name(m: &ModelEntry) -> String {
    m.display_name
        .clone()
        .or(m.api_name.clone())
        .unwrap_or_else(|| m.id.to_string())
}

/// Extract trailing numeric index from a GPU device string (e.g. "CUDA10" → 10).
/// Returns `None` if no trailing digits found.
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

/// Extract vendor from a model entry using a chain of fallbacks:
/// 1. `display_name` — split on `:`, take prefix
/// 2. `api_name` — split on `:`, take prefix
/// 3. `hf_base_model` — split on `/`, take first segment
/// 4. Fallback: `"other"`
fn extract_vendor(entry: &ModelEntry) -> String {
    if let Some(ref name) = entry.display_name {
        if let Some(vendor) = name.split(':').next() {
            let vendor = vendor.trim();
            if !vendor.is_empty() {
                return vendor.to_string();
            }
        }
    }
    if let Some(ref name) = entry.api_name {
        if let Some(vendor) = name.split(':').next() {
            let vendor = vendor.trim();
            if !vendor.is_empty() {
                return vendor.to_string();
            }
        }
    }
    if let Some(ref base) = entry.hf_base_model {
        if let Some(vendor) = base.split('/').next() {
            let vendor = vendor.trim();
            if !vendor.is_empty() {
                return vendor.to_string();
            }
        }
    }
    "other".to_string()
}

/// Returns `(priority, index)` for GPU sorting.
/// GPU models get priority 0 (sort first), non-GPU get priority 1 (sort last).
/// Index is extracted from the device string (e.g. "CUDA10" → 10).
fn extract_gpu_sort_key(gpu_device: &Option<String>) -> (u32, u32) {
    match gpu_device {
        Some(device) => {
            let index = extract_gpu_index(device).unwrap_or(0);
            (0, index)
        }
        None => (1, 0),
    }
}

/// Human-readable GPU label for grouping.
/// "CUDA1" → "GPU 1", "ROCm0" → "GPU 0", "GPU" → "GPU", None → "No GPU"
fn gpu_group_label(gpu_device: &Option<String>) -> String {
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
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Returns a comparable string for sorting (all non-GPU sorts).
fn extract_sort_key(entry: &ModelEntry, sort_by: SortBy) -> String {
    match sort_by {
        SortBy::Name => model_display_name(entry),
        SortBy::Family => entry.hf_architecture_type.clone().unwrap_or_default(),
        SortBy::Vendor => extract_vendor(entry),
        SortBy::Status => entry.state.clone(),
        SortBy::Gpu => unreachable!("GPU sort handled separately"),
    }
}

/// Returns the grouping key for a model entry.
fn extract_group_key(entry: &ModelEntry, group_by: GroupBy) -> String {
    match group_by {
        GroupBy::Gpu => gpu_group_label(&entry.gpu_device),
        GroupBy::Family => entry
            .hf_architecture_type
            .clone()
            .unwrap_or_else(|| String::from("Unknown")),
        GroupBy::Vendor => extract_vendor(entry),
        GroupBy::Status => match entry.state.as_str() {
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
/// GPU groups: numeric index (No GPU → u32::MAX to sort last).
/// All others: 0 (alphabetical ordering).
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
fn sort_models(models: &mut [ModelEntry], sort_by: SortBy) {
    match sort_by {
        SortBy::Gpu => {
            models.sort_by(|a, b| {
                let ka = extract_gpu_sort_key(&a.gpu_device);
                let kb = extract_gpu_sort_key(&b.gpu_device);
                ka.cmp(&kb)
            });
        }
        _ => {
            models.sort_by_key(|a| extract_sort_key(a, sort_by));
        }
    }
}

/// Parse a string into a SortBy enum. Defaults to Name.
fn parse_sort_by(s: &str) -> SortBy {
    match s {
        "gpu" => SortBy::Gpu,
        "family" => SortBy::Family,
        "vendor" => SortBy::Vendor,
        "status" => SortBy::Status,
        _ => SortBy::Name,
    }
}

/// Parse a string into an Option<GroupBy> enum. Unknown values return None.
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

// ── Component ────────────────────────────────────────────────────────────────

#[component]
pub fn Models() -> impl IntoView {
    // Refresh trigger signal — increment to force a refetch
    let refresh = RwSignal::new(0u32);
    let pull_modal_open = RwSignal::new(false);

    // Model count for the toolbar (updated when models are fetched)
    let model_count = RwSignal::new(0u32);

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

    // Global "Check all for updates" status
    let check_all_busy = RwSignal::new(false);
    let check_all_status = RwSignal::new(Option::<(bool, String)>::None);

    let models = LocalResource::new(move || async move {
        let _ = refresh.get(); // track the signal
        let resp = get_request("/tama/v1/models").send().await.ok()?;
        resp.json::<ModelsResponse>().await.ok()
    });

    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            let _ = post_request(&format!("/tama/v1/models/{}/load", id))
                .send()
                .await;
            refresh.update(|n| *n += 1);
        }
    });

    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            let _ = post_request(&format!("/tama/v1/models/{}/unload", id))
                .send()
                .await;
            refresh.update(|n| *n += 1);
        }
    });

    let cancel_busy = RwSignal::new(false);
    let cancel_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            cancel_busy.set(true);
            let _ = post_request(&format!("/tama/v1/models/{}/cancel", id))
                .send()
                .await;
            refresh.update(|n| *n += 1);
            cancel_busy.set(false);
        }
    });

    // Fire POST /api/models/:id/refresh for every model sequentially. Safe to
    // run without progress streaming because refresh is a pair of small HTTP
    // calls per model (no downloads, no hashing).
    let check_all_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            check_all_busy.set(true);
            check_all_status.set(None);
            // Fetch the list directly from the backend that exposes `id`s with
            // DB metadata so we iterate over the same set the editor operates on.
            let resp = match get_request("/tama/v1/models").send().await {
                Ok(r) => r,
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
                    Ok(r) if r.status() == 200 => ok_count += 1,
                    Ok(r) => {
                        let text = r.text().await.unwrap_or_default();
                        failed.push(format!("{}: {}", id, text));
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
        });

    // Clone signals for closures in the toolbar
    let sort_by_for_toolbar = sort_by;
    let group_by_for_toolbar = group_by;

    view! {
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
        {move || check_all_status.get().map(|(ok, msg)| {
            let variant = if ok { AlertVariant::Success } else { AlertVariant::Error };
            view! { <AlertBanner variant=variant>{msg}</AlertBanner> }
        })}

        // Sort/group toolbar
        <div class="models-toolbar">
            <div class="models-toolbar__controls">
                <select
                    class="btn btn-secondary btn-sm"
                    on:change=move |e| {
                        let val = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
                            .map(|s| s.value())
                            .unwrap_or_default();
                        sort_by_for_toolbar.set(parse_sort_by(&val));
                    }
                >
                    <option value="name" selected=move || sort_by_for_toolbar.get() == SortBy::Name>"Sort: Name"</option>
                    <option value="gpu" selected=move || sort_by_for_toolbar.get() == SortBy::Gpu>"Sort: GPU"</option>
                    <option value="family" selected=move || sort_by_for_toolbar.get() == SortBy::Family>"Sort: Family"</option>
                    <option value="vendor" selected=move || sort_by_for_toolbar.get() == SortBy::Vendor>"Sort: Vendor"</option>
                    <option value="status" selected=move || sort_by_for_toolbar.get() == SortBy::Status>"Sort: Status"</option>
                </select>
                <select
                    class="btn btn-secondary btn-sm"
                    on:change=move |e| {
                        let val = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
                            .map(|s| s.value())
                            .unwrap_or_default();
                        group_by_for_toolbar.set(parse_group_by(&val));
                    }
                >
                    <option value="none" selected=move || group_by_for_toolbar.get().is_none()>"Group: None"</option>
                    <option value="gpu" selected=move || group_by_for_toolbar.get() == Some(GroupBy::Gpu)>"Group: GPU"</option>
                    <option value="family" selected=move || group_by_for_toolbar.get() == Some(GroupBy::Family)>"Group: Family"</option>
                    <option value="vendor" selected=move || group_by_for_toolbar.get() == Some(GroupBy::Vendor)>"Group: Vendor"</option>
                    <option value="status" selected=move || group_by_for_toolbar.get() == Some(GroupBy::Status)>"Group: Status"</option>
                </select>
            </div>
            <span class="models-toolbar__count">
                {move || {
                    let count = model_count.get();
                    format!("{} {}", count, if count == 1 { "model" } else { "models" })
                }}
            </span>
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
                            model_count.set(0);
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
                            // Clone, sort, and optionally group the models
                            let mut sorted_models = data.models.clone();
                            model_count.set(sorted_models.len() as u32);
                            sort_models(&mut sorted_models, sort_by.get());

                            // Build grouped output
                            let groups: Vec<(Option<String>, Vec<ModelEntry>)> = {
                                let group_by_val = group_by.get();
                                if let Some(group_by_type) = group_by_val {
                                    let mut groups_map: BTreeMap<String, Vec<ModelEntry>> = BTreeMap::new();
                                    let mut group_order: Vec<String> = Vec::new();
                                    for m in &sorted_models {
                                        let key = extract_group_key(m, group_by_type);
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
                                            view! {
                                                <ModelCard
                                                    id=m.id.to_string()
                                                    db_id=Some(m.id)
                                                    display_name=model_display_name(&m)
                                                    quant=m.quant.clone()
                                                    context_length=None
                                                    pips=ModelPips {
                                                        gpu_variant: m.gpu_variant.clone(),
                                                        gpu_label: Some(gpu_group_label(&m.gpu_device)),
                                                        ..Default::default()
                                                    }
                                                    backend=m.backend.clone()
                                                    log_source=Some(m.backend.clone())
                                                    state=m.state.clone()
                                                    loaded=Some(m.loaded)
                                                    enabled=Some(m.enabled)
                                                    hf_architecture_type=m.hf_architecture_type.clone()
                                                    hf_base_model=m.hf_base_model.clone()
                                                    on_load=on_load_cb
                                                    on_unload=on_unload_cb
                                                    on_cancel=on_cancel_cb
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
                        },
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
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        display_name: Option<&str>,
        api_name: Option<&str>,
        hf_base_model: Option<&str>,
        gpu_device: Option<&str>,
        state: &str,
    ) -> ModelEntry {
        ModelEntry {
            id: 1,
            backend: "test".to_string(),
            model: None,
            quant: None,
            enabled: true,
            loaded: false,
            state: state.to_string(),
            api_name: api_name.map(|s| s.to_string()),
            display_name: display_name.map(|s| s.to_string()),
            gpu_device: gpu_device.map(|s| s.to_string()),
            gpu_variant: None,
            hf_architecture_type: None,
            hf_base_model: hf_base_model.map(|s| s.to_string()),
        }
    }

    // ── extract_vendor tests ────────────────────────────────────────────────

    #[test]
    fn test_extract_vendor_from_display_name() {
        let entry = make_entry(Some("Unsloth: Qwen3.6 27B"), None, None, None, "idle");
        assert_eq!(extract_vendor(&entry), "Unsloth");
    }

    #[test]
    fn test_extract_vendor_from_api_name() {
        let entry = make_entry(None, Some("vendor:model-name"), None, None, "idle");
        assert_eq!(extract_vendor(&entry), "vendor");
    }

    #[test]
    fn test_extract_vendor_from_hf_base_model() {
        let entry = make_entry(None, None, Some("Qwen/Qwen3.6-27B"), None, "idle");
        assert_eq!(extract_vendor(&entry), "Qwen");
    }

    #[test]
    fn test_extract_vendor_fallback_other() {
        let entry = make_entry(None, None, None, None, "idle");
        assert_eq!(extract_vendor(&entry), "other");
    }

    // ── extract_gpu_sort_key tests ──────────────────────────────────────────

    #[test]
    fn test_extract_gpu_sort_key_cuda() {
        let entry = make_entry(None, None, None, Some("CUDA1"), "idle");
        assert_eq!(extract_gpu_sort_key(&entry.gpu_device), (0, 1));
    }

    #[test]
    fn test_extract_gpu_sort_key_rocm() {
        let entry = make_entry(None, None, None, Some("ROCm0"), "idle");
        assert_eq!(extract_gpu_sort_key(&entry.gpu_device), (0, 0));
    }

    #[test]
    fn test_extract_gpu_sort_key_none() {
        let entry = make_entry(None, None, None, None, "idle");
        assert_eq!(extract_gpu_sort_key(&entry.gpu_device), (1, 0));
    }

    #[test]
    fn test_extract_gpu_sort_key_multidigit() {
        let entry = make_entry(None, None, None, Some("CUDA10"), "idle");
        assert_eq!(extract_gpu_sort_key(&entry.gpu_device), (0, 10));
    }

    #[test]
    fn test_extract_gpu_sort_key_no_number() {
        let entry = make_entry(None, None, None, Some("GPU"), "idle");
        assert_eq!(extract_gpu_sort_key(&entry.gpu_device), (0, 0));
    }

    // ── gpu_group_label tests ───────────────────────────────────────────────

    #[test]
    fn test_gpu_group_label_cuda() {
        let entry = make_entry(None, None, None, Some("CUDA0"), "idle");
        assert_eq!(gpu_group_label(&entry.gpu_device), "GPU 0");
    }

    #[test]
    fn test_gpu_group_label_rocm() {
        let entry = make_entry(None, None, None, Some("ROCm1"), "idle");
        assert_eq!(gpu_group_label(&entry.gpu_device), "GPU 1");
    }

    #[test]
    fn test_gpu_group_label_none() {
        let entry = make_entry(None, None, None, None, "idle");
        assert_eq!(gpu_group_label(&entry.gpu_device), "No GPU");
    }

    #[test]
    fn test_gpu_group_label_no_number() {
        let entry = make_entry(None, None, None, Some("GPU"), "idle");
        assert_eq!(gpu_group_label(&entry.gpu_device), "GPU");
    }

    // ── capitalize_first tests ──────────────────────────────────────────────

    #[test]
    fn test_capitalize_first() {
        assert_eq!(capitalize_first("qwen35"), "Qwen35");
        assert_eq!(capitalize_first(""), "");
    }

    // ── parse_sort_by tests ─────────────────────────────────────────────────

    #[test]
    fn test_parse_sort_by() {
        assert_eq!(parse_sort_by("name"), SortBy::Name);
        assert_eq!(parse_sort_by("gpu"), SortBy::Gpu);
        assert_eq!(parse_sort_by("unknown"), SortBy::Name);
    }

    // ── parse_group_by tests ────────────────────────────────────────────────

    #[test]
    fn test_parse_group_by() {
        assert_eq!(parse_group_by("gpu"), Some(GroupBy::Gpu));
        assert_eq!(parse_group_by("none"), None);
        assert_eq!(parse_group_by("unknown"), None);
    }
}
