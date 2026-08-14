use crate::components::pull_wizard::VllmWizardSettings;
use crate::pages::model_editor::hardware_form::KV_CACHE_DTYPE_OPTIONS;
use crate::utils::target_value;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// Transformers-branch configuration step: vLLM launch settings for the
/// freshly pulled model. Fields are prefilled from the model's stored vllm
/// config on entry (see `vllm_settings_prefill`); the save overlays the five
/// fields onto that fetched config, so blank fields keep the stored value
/// instead of resetting it.
#[component]
pub fn VllmConfigStep(
    /// The vLLM settings being edited (written directly on input).
    settings: RwSignal<VllmWizardSettings>,
    /// Fallback max model length (the job's config.json context length, else
    /// the repo metadata's `hf_context_length`), used to prefill when unset.
    initial_max_model_len: Signal<Option<u32>>,
    /// "Save & Finish" — persist the settings, then advance to Done.
    on_next: Callback<()>,
    /// "Back" — return to the (completed) download step.
    on_back: Callback<()>,
    /// "Skip for now" — advance to Done without persisting.
    on_skip: Callback<()>,
) -> impl IntoView {
    // Prefill max_model_len from the initial value when the user hasn't
    // typed one yet (mirrors the GGUF ContextStep prefill; the pre-entry
    // fetch in the wizard normally fills this in already).
    Effect::new(move |_| {
        if settings.get().max_model_len.is_none() {
            if let Some(len) = initial_max_model_len.get() {
                settings.update(|s| s.max_model_len = Some(len));
            }
        }
    });

    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Configure vLLM"</h2>
            <p class="form-card__desc text-muted">
                "Optional vLLM launch settings. Values are prefilled from the model's current configuration; blank fields keep the stored value."
            </p>
        </div>

        <div class="form-group mb-2">
            <label class="form-label text-sm" for="field-vllm-max-model-len">
                "Max model length"
                <div class="form-hint">{"vLLM --max-model-len. Leave blank for the model's default."}</div>
            </label>
            <input
                id="field-vllm-max-model-len"
                class="form-input"
                type="number"
                min="1"
                placeholder="e.g. 32768"
                prop:value=move || {
                    settings
                        .get()
                        .max_model_len
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                }
                on:input=move |ev| {
                    let val = target_value(&ev);
                    settings.update(|s| {
                        s.max_model_len = if val.is_empty() {
                            None
                        } else {
                            val.parse::<u32>().ok()
                        };
                    });
                }
            />
        </div>

        <div class="form-group mb-2">
            <label class="form-label text-sm" for="field-vllm-kv-cache-dtype">
                "KV cache dtype"
                <div class="form-hint">{"vLLM --kv-cache-dtype. fp8 reduces VRAM at a small quality cost."}</div>
            </label>
            <select
                id="field-vllm-kv-cache-dtype"
                class="form-select"
                prop:value=move || {
                    settings
                        .get()
                        .kv_cache_dtype
                        .clone()
                        .unwrap_or_default()
                }
                on:change=move |e| {
                    let val = target_value(&e);
                    settings.update(|s| {
                        s.kv_cache_dtype = if val.is_empty() { None } else { Some(val) };
                    });
                }
            >
                <option value="">"Default (auto)"</option>
                {KV_CACHE_DTYPE_OPTIONS.iter().map(|opt| {
                    let opt_str = *opt;
                    view! { <option value=opt_str>{opt_str}</option> }
                }).collect::<Vec<_>>()}
            </select>
        </div>

        <div class="form-group mb-2">
            <label class="form-label text-sm" for="field-vllm-tensor-parallel-size">
                "Tensor parallel size"
                <div class="form-hint">{"vLLM --tensor-parallel-size. Number of GPUs to shard across."}</div>
            </label>
            <input
                id="field-vllm-tensor-parallel-size"
                class="form-input"
                type="number"
                min="1"
                placeholder="1"
                prop:value=move || {
                    settings
                        .get()
                        .tensor_parallel_size
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                }
                on:input=move |ev| {
                    let val = target_value(&ev);
                    settings.update(|s| {
                        s.tensor_parallel_size = if val.is_empty() {
                            None
                        } else {
                            val.parse::<u32>().ok()
                        };
                    });
                }
            />
        </div>

        <div class="form-group mb-2">
            <label class="form-label text-sm" for="field-vllm-gpu-memory-utilization">
                "GPU memory utilization"
                <div class="form-hint">{"vLLM --gpu-memory-utilization. Fraction of VRAM to use, 0-1."}</div>
            </label>
            <input
                id="field-vllm-gpu-memory-utilization"
                class="form-input"
                type="number"
                min="0"
                max="1"
                step="0.05"
                placeholder="0.9"
                prop:value=move || {
                    settings
                        .get()
                        .gpu_memory_utilization
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                }
                on:input=move |ev| {
                    let val = target_value(&ev);
                    settings.update(|s| {
                        s.gpu_memory_utilization = if val.is_empty() {
                            None
                        } else {
                            val.parse::<f64>().ok()
                        };
                    });
                }
            />
        </div>

        <div class="form-group mb-2">
            <div class="form-check">
                <input
                    id="field-vllm-trust-remote-code"
                    type="checkbox"
                    prop:checked=move || settings.get().trust_remote_code
                    on:change=move |e| {
                        let checked = e
                            .target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        settings.update(|s| s.trust_remote_code = checked);
                    }
                />
                <label class="form-check-label" for="field-vllm-trust-remote-code">
                    "Trust remote code"
                </label>
            </div>
        </div>

        <div class="form-actions mt-3">
            <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                "Back"
            </button>
            <button class="btn btn-secondary" on:click=move |_| on_skip.run(())>
                "Skip for now"
            </button>
            <button class="btn btn-primary" on:click=move |_| on_next.run(())>
                "Save & Finish"
            </button>
        </div>
    }
}
