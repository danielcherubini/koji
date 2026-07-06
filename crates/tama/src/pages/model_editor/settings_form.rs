use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::{BackendOption, ModelForm};
use crate::utils::target_value;

/// Set an input's value by DOM id.
fn set_input_value(id: &str, value: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    {
        if let Ok(input) = el.clone().dyn_into::<web_sys::HtmlInputElement>() {
            input.set_value(value);
            return;
        }
        if let Ok(select) = el.dyn_into::<web_sys::HtmlSelectElement>() {
            select.set_value(value);
        }
    }
}

/// Set a checkbox's checked state by DOM id.
fn set_checked(id: &str, checked: bool) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    {
        if let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() {
            input.set_checked(checked);
        }
    }
}

#[component]
pub fn ModelEditorSettingsForm(
    form: RwSignal<Option<ModelForm>>,
    backends: RwSignal<Vec<BackendOption>>,
) -> impl IntoView {
    // Populate input values when the form data loads (or model changes).
    // Uses get_element_by_id because prop:value doesn't work reliably
    // inside Suspense + conditional rendering.
    // Only runs when the model ID changes, not on every keystroke —
    // otherwise set_input_value resets the cursor mid-edit.
    let last_init_id = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            web_sys::console::log_1(
                &format!(
                    "[settings] Effect fired, form id={}, last_init={:?}",
                    f.id,
                    last_init_id.get_value()
                )
                .into(),
            );
            if last_init_id.get_value() != Some(f.id.clone()) {
                web_sys::console::log_1(&"[settings] Populating inputs".into());
                set_input_value(
                    "field-display-name",
                    f.display_name.as_deref().unwrap_or_default(),
                );
                set_input_value("field-model", f.model.as_deref().unwrap_or_default());
                set_input_value("field-api-name", f.api_name.as_deref().unwrap_or_default());
                set_input_value(
                    "field-port",
                    &f.port.map(|v| v.to_string()).unwrap_or_default(),
                );
                set_checked("field-enabled", f.enabled);
                last_init_id.set_value(Some(f.id.clone()));
            }
        } else {
            web_sys::console::log_1(&"[settings] Effect fired, form is None".into());
        }
    });

    view! {
        <div class="form-grid">
            <label class="form-label" for="field-display-name">"Display Name"</label>
            <input
                id="field-display-name"
                class="form-input"
                type="text"
                placeholder="Auto-generated from HF repo name"
                on:input=move |ev| {
                    let val = target_value(&ev);
                    web_sys::console::log_1(&format!("[settings] on:input display_name={}", val).into());
                    form.update(|f| {
                        if let Some(form) = f {
                            form.display_name = if val.is_empty() { None } else { Some(val) };
                        }
                    });
                }
            />

            <label class="form-label" for="field-model">"Model (HF repo)"</label>
            <div style="display: flex; align-items: center; gap: 0.5rem;">
                <input
                    id="field-model"
                    class="form-input"
                    type="text"
                    placeholder="e.g. unsloth/gemma-4-26B-A4B-it-GGUF"
                    on:input=move |ev| {
                        form.update(|f| {
                            if let Some(form) = f {
                                let val = target_value(&ev);
                                form.model = if val.is_empty() { None } else { Some(val) };
                            }
                        });
                    }
                />
                {move || {
                    let repo = form.get().as_ref().and_then(|f| f.model.clone());
                    repo.as_ref().map(|r| {
                        let url = format!("https://huggingface.co/{}", r);
                        view! {
                            <a
                                href=url
                                target="_blank"
                                rel="noopener"
                                class="hf-repo-link"
                            >
                                "↗"
                            </a>
                        }
                    })
                }}
            </div>

            <label class="form-label" for="field-api-name">"API Name"</label>
            <input
                id="field-api-name"
                class="form-input"
                type="text"
                disabled=true
                title="API Name is auto-derived from the HF repo name"
            />

            <label class="form-label" for="field-backend">"Backend"</label>
            <select
                id="field-backend"
                class="form-select"
                on:change=move |e| {
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            // Parse "name:variant" or just "name"
                            if let Some((name, variant)) = val.split_once(':') {
                                form.backend = name.to_string();
                                form.gpu_variant = Some(variant.to_string());
                            } else {
                                form.backend = val;
                                form.gpu_variant = None;
                            }
                        }
                    });
                }
            >
                {move || backends.get().into_iter().map(|opt| {
                    let value = if let Some(ref v) = opt.variant {
                        format!("{}:{}", opt.name, v)
                    } else {
                        opt.name.clone()
                    };
                    let selected = form.get_untracked().as_ref().map(|f| {
                        let expected = if let Some(ref v) = f.gpu_variant {
                            format!("{}:{}", f.backend, v)
                        } else {
                            f.backend.clone()
                        };
                        expected == value
                    }).unwrap_or(false);
                    let value2 = value.clone();
                    view! { <option value=value2 selected=selected>{opt.label.clone()}</option> }
                }).collect::<Vec<_>>()}
            </select>

            <label class="form-label">"Enabled"</label>
            <div class="form-check">
                <input
                    id="field-enabled"
                    type="checkbox"
                    on:change=move |e| {
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.enabled = checked;
                            }
                        });
                    }
                />
                <label class="form-check-label" for="field-enabled">"Enabled"</label>
            </div>

            <label class="form-label" for="field-port">"Port override"</label>
            <input
                id="field-port"
                class="form-input"
                type="number"
                placeholder="leave blank for default"
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.port = target_value(&ev).parse::<u16>().ok();
                        }
                    });
                }
            />
        </div>
    }
}
