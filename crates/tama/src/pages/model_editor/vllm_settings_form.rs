use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::utils::set_input_value;

use super::types::{is_transformers, ModelForm};
use super::vllm_form::{args_to_vllm_form, vllm_form_to_args, VllmSettings};
use crate::utils::target_value;

/// Helper to read checkbox state from an event target.
fn checked_from_event(e: &web_sys::Event) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.checked())
        .unwrap_or(false)
}

/// Renders the vLLM settings form for transformers-format models.
#[component]
pub fn ModelEditorVllmForm(
    form: RwSignal<Option<ModelForm>>,
    vllm_settings: RwSignal<VllmSettings>,
) -> impl IntoView {
    // Populate numeric input values when the model loads (or model changes).
    // Uses imperative set_input_value to avoid the reactive prop:value eating
    // decimal points during typing (e.g. "0." → parse fails → "0").
    let last_init_id = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            if is_transformers(f.hf_format.as_deref())
                && last_init_id.get_value() != Some(f.id.clone())
            {
                set_input_value(
                    "field-tensor-parallel-size",
                    &vllm_settings
                        .get()
                        .tensor_parallel_size
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-gpu-memory-utilization",
                    &vllm_settings
                        .get()
                        .gpu_memory_utilization
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-max-model-len",
                    &vllm_settings
                        .get()
                        .max_model_len
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-max-num-batched-tokens",
                    &vllm_settings
                        .get()
                        .max_num_batched_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                last_init_id.set_value(Some(f.id.clone()));
            }
        }
    });

    // Sync vLLM settings from form args whenever form changes
    // Guard: only update when settings actually differ to prevent infinite loops
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            if is_transformers(f.hf_format.as_deref()) {
                let settings = args_to_vllm_form(&f.args);
                if vllm_settings.with_untracked(|s| *s != settings) {
                    vllm_settings.set(settings);
                }
            }
        }
    });

    // Sync args to form when vLLM settings change
    // Guard: only update when args actually differ to prevent infinite loops
    Effect::new(move |_| {
        let settings = vllm_settings.get();
        let needs_write = form.with_untracked(|f| {
            f.as_ref()
                .map(|f| {
                    let current_args = vllm_form_to_args(&settings, &f.args);
                    current_args != f.args
                })
                .unwrap_or(false)
        });
        if needs_write {
            form.update(|f| {
                if let Some(f) = f {
                    f.args = vllm_form_to_args(&settings, &f.args);
                }
            });
        }
    });

    view! {
        <div class="vllm-settings-form">
            // Quantization
            <div class="form-group">
                <label>"Quantization"</label>
                <input
                    type="text"
                    placeholder="e.g. fp8, awq, none"
                    prop:value=move || vllm_settings.get().quantization.clone().unwrap_or_default()
                    on:input=move |ev| {
                        let value = target_value(&ev);
                        vllm_settings.update(|s| s.quantization = if value.is_empty() { None } else { Some(value) });
                    }
                />
            </div>

            // KV Cache Dtype
            <div class="form-group">
                <label>"KV Cache Dtype"</label>
                <input
                    type="text"
                    placeholder="e.g. auto, fp8, bf16"
                    prop:value=move || vllm_settings.get().kv_cache_dtype.clone().unwrap_or_default()
                    on:input=move |ev| {
                        let value = target_value(&ev);
                        vllm_settings.update(|s| s.kv_cache_dtype = if value.is_empty() { None } else { Some(value) });
                    }
                />
            </div>

            // Tensor Parallel Size
            <div class="form-group">
                <label>"Tensor Parallel Size"</label>
                <input
                    id="field-tensor-parallel-size"
                    type="number"
                    min="1"
                    placeholder="1"
                    on:input=move |ev| {
                        let value = target_value(&ev);
                        vllm_settings.update(|s| {
                            s.tensor_parallel_size = if value.is_empty() { None } else { value.parse().ok() };
                        });
                    }
                />
            </div>

            // GPU Memory Utilization
            <div class="form-group">
                <label>"GPU Memory Utilization"</label>
                <input
                    id="field-gpu-memory-utilization"
                    type="number"
                    min="0.0"
                    max="1.0"
                    step="0.01"
                    placeholder="0.9"
                    on:input=move |ev| {
                        let value = target_value(&ev);
                        vllm_settings.update(|s| {
                            s.gpu_memory_utilization = if value.is_empty() { None } else { value.parse().ok() };
                        });
                    }
                />
            </div>

            // Max Model Length
            <div class="form-group">
                <label>"Max Model Length"</label>
                <input
                    id="field-max-model-len"
                    type="number"
                    min="1"
                    placeholder="e.g. 4096"
                    on:input=move |ev| {
                        let value = target_value(&ev);
                        vllm_settings.update(|s| {
                            s.max_model_len = if value.is_empty() { None } else { value.parse().ok() };
                        });
                    }
                />
            </div>

            // Max Num Batched Tokens
            <div class="form-group">
                <label>"Max Num Batched Tokens"</label>
                <input
                    id="field-max-num-batched-tokens"
                    type="number"
                    min="1"
                    placeholder="e.g. 2048"
                    on:input=move |ev| {
                        let value = target_value(&ev);
                        vllm_settings.update(|s| {
                            s.max_num_batched_tokens = if value.is_empty() { None } else { value.parse().ok() };
                        });
                    }
                />
            </div>

            // Enable Prefix Caching (checkbox)
            <div class="form-group">
                <label class="checkbox-label">
                    <input
                        type="checkbox"
                        prop:checked=move || vllm_settings.get().enable_prefix_caching
                        on:change=move |ev| {
                            let checked = checked_from_event(&ev);
                            vllm_settings.update(|s| s.enable_prefix_caching = checked);
                        }
                    />
                    "Enable Prefix Caching"
                </label>
            </div>

            // Trust Remote Code (checkbox)
            <div class="form-group">
                <label class="checkbox-label">
                    <input
                        type="checkbox"
                        prop:checked=move || vllm_settings.get().trust_remote_code
                        on:change=move |ev| {
                            let checked = checked_from_event(&ev);
                            vllm_settings.update(|s| s.trust_remote_code = checked);
                        }
                    />
                    "Trust Remote Code"
                </label>
            </div>
        </div>
    }
}
