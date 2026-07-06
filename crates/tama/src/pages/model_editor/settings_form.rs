use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::{BackendOption, ModelForm};
use crate::utils::target_value;

#[component]
pub fn ModelEditorSettingsForm(
    form: RwSignal<Option<ModelForm>>,
    backends: RwSignal<Vec<BackendOption>>,
) -> impl IntoView {
    view! {
        <div class="form-grid">
            <label class="form-label" for="field-display-name">"Display Name"</label>
            <input
                id="field-display-name"
                class="form-input"
                type="text"
                placeholder="Auto-generated from HF repo name"
                on:mount=move |el: web_sys::Element| {
                    let input = el.unchecked_into::<web_sys::HtmlInputElement>();
                    let val = form.get().as_ref().and_then(|f| f.display_name.clone()).unwrap_or_default();
                    input.set_value(&val);
                }
                on:input=move |ev| {
                    let val = target_value(&ev);
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
                    on:mount=move |el: web_sys::Element| {
                        let input = el.unchecked_into::<web_sys::HtmlInputElement>();
                        let val = form.get().as_ref().and_then(|f| f.model.clone()).unwrap_or_default();
                        input.set_value(&val);
                    }
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
                on:mount=move |el: web_sys::Element| {
                    let input = el.unchecked_into::<web_sys::HtmlInputElement>();
                    let val = form.get().as_ref().and_then(|f| f.api_name.clone()).unwrap_or_default();
                    input.set_value(&val);
                }
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
                    on:mount=move |el: web_sys::Element| {
                        let input = el.unchecked_into::<web_sys::HtmlInputElement>();
                        let checked = form.get().as_ref().map(|f| f.enabled).unwrap_or(true);
                        input.set_checked(checked);
                    }
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
                on:mount=move |el: web_sys::Element| {
                    let input = el.unchecked_into::<web_sys::HtmlInputElement>();
                    let val = form.get().as_ref().and_then(|f| f.port).map(|v| v.to_string()).unwrap_or_default();
                    input.set_value(&val);
                }
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
