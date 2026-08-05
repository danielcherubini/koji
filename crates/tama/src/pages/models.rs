use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::alert_banner::{AlertBanner, AlertVariant};
use crate::components::modal::Modal;
use crate::components::model_card::{ModelCard, ModelPips};
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
use crate::core_mirrors::ModelState;
use crate::utils::{get_request, post_request, rw_signal_to_signal, CheckAllModelsApiResponse};

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

// ── Component ────────────────────────────────────────────────────────────────

#[component]
pub fn Models() -> impl IntoView {
    // Refresh trigger signal — increment to force a refetch
    let refresh = RwSignal::new(0u32);
    let pull_modal_open = RwSignal::new(false);

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
                            view! {
                                <div class="models-list">
                                    {data.models.into_iter().map(|m| {
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
}
