use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::{is_transformers, ModelForm};
use crate::components::context_length_selector::ContextLengthSelector;
use crate::utils::{set_checked, set_input_value, target_value};

const KV_QUANT_OPTIONS: &[&str] = &[
    "f32", "f16", "bf16", "q8_0", "q4_0", "q4_1", "iq4_nl", "q5_0", "q5_1",
];

pub(crate) const KV_CACHE_DTYPE_OPTIONS: &[&str] = &["auto", "fp8", "bf16"];

const BATCH_OPTIONS: &[u32] = &[128, 256, 512, 1024, 2048, 4096, 8192];
const UBATCH_OPTIONS: &[u32] = &[32, 64, 128, 256, 512, 1024, 2048, 4096];

#[derive(Clone, Copy, PartialEq, Eq)]
enum KvQuantField {
    K,
    V,
}

/// Custom KV quant text input that appears when the selected value is not in the known options.
#[component]
fn KvQuantCustomInput(form: RwSignal<Option<ModelForm>>, field: KvQuantField) -> impl IntoView {
    let is_custom = Signal::derive(move || {
        let f = form.get();
        let current = f.as_ref().and_then(|f| match field {
            KvQuantField::K => f.cache_type_k.as_deref(),
            KvQuantField::V => f.cache_type_v.as_deref(),
        });
        matches!(current, Some("__custom"))
            || matches!(current, Some(val) if !KV_QUANT_OPTIONS.contains(&val))
    });
    let _current_value = Signal::derive(move || {
        let f = form.get();
        f.as_ref().and_then(|f| match field {
            KvQuantField::K => f.cache_type_k.clone(),
            KvQuantField::V => f.cache_type_v.clone(),
        })
    });

    view! {
        <Show when=move || is_custom.get()>
            {move || {
                view! {
                    <input
                        class="form-input"
                        type="text"
                        maxlength="32"
                        placeholder="Custom quant value..."
                        id=format!("field-kv-custom-{}", match field { KvQuantField::K => "k", KvQuantField::V => "v" })
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            form.update(|f| {
                                if let Some(form) = f {
                                    match field {
                                        KvQuantField::K => form.cache_type_k = if v.is_empty() { None } else { Some(v) },
                                        KvQuantField::V => form.cache_type_v = if v.is_empty() { None } else { Some(v) },
                                    }
                                }
                            });
                        }
                    />
                }
            }}
        </Show>
    }
}

#[component]
pub fn ModelEditorHardwareForm(form: RwSignal<Option<ModelForm>>) -> impl IntoView {
    // Populate input values when the form data loads (or model changes).
    let last_init_id = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            if last_init_id.get_value() != Some(f.id.clone()) {
                // Transformers-format (vLLM) fields
                if is_transformers(f.hf_format.as_deref()) {
                    let v = &f.vllm;
                    set_input_value(
                        "field-max-model-len",
                        &v.max_model_len.map(|v| v.to_string()).unwrap_or_default(),
                    );
                    set_input_value(
                        "field-kv-cache-dtype",
                        v.kv_cache_dtype.as_deref().unwrap_or_default(),
                    );
                    set_input_value(
                        "field-max-num-batched-tokens",
                        &v.max_num_batched_tokens
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                    );
                }
                set_input_value(
                    "field-num-parallel",
                    &f.num_parallel.map(|v| v.to_string()).unwrap_or_default(),
                );
                set_checked("field-kv-unified", f.kv_unified);
                set_input_value(
                    "field-kv-quant-k",
                    f.cache_type_k.as_deref().unwrap_or_default(),
                );
                set_input_value(
                    "field-kv-quant-v",
                    f.cache_type_v.as_deref().unwrap_or_default(),
                );
                // Custom KV quant inputs (only visible when custom is selected)
                set_input_value(
                    "field-kv-custom-k",
                    f.cache_type_k.as_deref().unwrap_or_default(),
                );
                set_input_value(
                    "field-kv-custom-v",
                    f.cache_type_v.as_deref().unwrap_or_default(),
                );
                // Batch / µ-batch selects
                set_input_value(
                    "field-batch",
                    &f.n_batch.map(|v| v.to_string()).unwrap_or_default(),
                );
                set_input_value(
                    "field-ubatch",
                    &f.n_ubatch.map(|v| v.to_string()).unwrap_or_default(),
                );
                last_init_id.set_value(Some(f.id.clone()));
            }
        }
    });

    view! {
        // ── GGUF / llama.cpp fields ─────────────────────────────────────
        <Show when=move || !is_transformers(form.get().and_then(|f| f.hf_format.clone()).as_deref())>
        <div class="form-grid">
            <label class="form-label" for="field-ctx">"Context length"</label>
            <ContextLengthSelector
                value=Signal::derive(move || form.get().and_then(|f| f.context_length))
                on_change=Callback::new(move |v| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.context_length = v;
                        }
                    });
                })
                reset_key=Signal::derive(move || form.get().map(|f| f.id.clone()).unwrap_or_default())
                max_context=Signal::derive(move || form.get().and_then(|f| f.hf_context_length))
            />

            <label class="form-label" for="field-num-parallel">"Num parallel slots"</label>
            <input
                id="field-num-parallel"
                class="form-input"
                type="number"
                min="0"
                placeholder="0 = auto"
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            let val = target_value(&ev);
                            form.num_parallel = if val.is_empty() {
                                None
                            } else {
                                val.parse::<u32>().ok()
                            };
                        }
                    });
                }
            />

            <label class="form-label" for="field-kv-unified">
                "Unified KV cache"
                <div class="form-hint">All parallel slots share a single context pool. Better for agent+subagent workflows.</div>
            </label>
            <div class="form-check">
                <input
                    id="field-kv-unified"
                    type="checkbox"
                    on:change=move |e| {
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.kv_unified = checked;
                            }
                        });
                    }
                />
                <label class="form-check-label" for="field-kv-unified">"Unified KV cache"</label>
            </div>

            <label class="form-label" for="field-kv-quant-k">
                "KV cache type K"
                <div class="form-hint">Quantize the K cache to reduce VRAM usage. Lower precision = less memory, slightly slower inference.</div>
            </label>
            <select
                id="field-kv-quant-k"
                class="form-select"
                on:change=move |e| {
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.cache_type_k = if val.is_empty() { None } else { Some(val) };
                        }
                    });
                }
            >
                <option value="">"Default (f16)"</option>
                {KV_QUANT_OPTIONS.iter().map(|opt| {
                    let selected = form.get_untracked().as_ref()
                        .and_then(|f| f.cache_type_k.as_deref())
                        == Some(*opt);
                    let opt_str = *opt;
                    view! { <option value=opt_str selected=selected>{opt_str}</option> }
                }).collect::<Vec<_>>()}
                <option
                    value="__custom"
                    selected=move || form.get().as_ref()
                        .and_then(|f| f.cache_type_k.as_deref())
                        .map(|v| v == "__custom" || !KV_QUANT_OPTIONS.contains(&v))
                        .unwrap_or(false)
                >
                    "Custom…"
                </option>
            </select>
            <KvQuantCustomInput form=form field=KvQuantField::K />

            <label class="form-label" for="field-kv-quant-v">
                "KV cache type V"
                <div class="form-hint">Quantize the V cache to reduce VRAM usage. Lower precision = less memory, slightly slower inference.</div>
            </label>
            <select
                id="field-kv-quant-v"
                class="form-select"
                on:change=move |e| {
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.cache_type_v = if val.is_empty() { None } else { Some(val) };
                        }
                    });
                }
            >
                <option value="">"Default (f16)"</option>
                {KV_QUANT_OPTIONS.iter().map(|opt| {
                    let selected = form.get_untracked().as_ref()
                        .and_then(|f| f.cache_type_v.as_deref())
                        == Some(*opt);
                    let opt_str = *opt;
                    view! { <option value=opt_str selected=selected>{opt_str}</option> }
                }).collect::<Vec<_>>()}
                <option
                    value="__custom"
                    selected=move || form.get().as_ref()
                        .and_then(|f| f.cache_type_v.as_deref())
                        .map(|v| v == "__custom" || !KV_QUANT_OPTIONS.contains(&v))
                        .unwrap_or(false)
                >
                    "Custom…"
                </option>
            </select>
            <KvQuantCustomInput form=form field=KvQuantField::V />

            <label class="form-label" for="field-batch">
                "Batch size"
                <div class="form-hint">Pre-allocated context KV cache size (llama.cpp --batch). Default = backend default.</div>
            </label>
            <select
                id="field-batch"
                class="form-select"
                on:change=move |e| {
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.n_batch = if val.is_empty() {
                                None
                            } else {
                                val.parse::<u32>().ok()
                            };
                        }
                    });
                }
            >
                <option value="">"Default (backend default)"</option>
                {BATCH_OPTIONS.iter().map(|opt| {
                    let selected = form.get_untracked()
                        .as_ref()
                        .and_then(|f| f.n_batch)
                        == Some(*opt);
                    let val = opt.to_string();
                    let label = opt.to_string();
                    view! { <option value=val selected=selected>{label}</option> }
                }).collect::<Vec<_>>()}
            </select>

            <label class="form-label" for="field-ubatch">
                "µ-batch size"
                <div class="form-hint">Maximum number of unique sequences per batch (llama.cpp --ubatch). Must be at most equal to batch size.</div>
            </label>
            <select
                id="field-ubatch"
                class="form-select"
                on:change=move |e| {
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.n_ubatch = if val.is_empty() {
                                None
                            } else {
                                val.parse::<u32>().ok()
                            };
                        }
                    });
                }
            >
                <option value="">"Default (backend default)"</option>
                {UBATCH_OPTIONS.iter().map(|opt| {
                    let selected = form.get_untracked()
                        .as_ref()
                        .and_then(|f| f.n_ubatch)
                        == Some(*opt);
                    let val = opt.to_string();
                    let label = opt.to_string();
                    view! { <option value=val selected=selected>{label}</option> }
                }).collect::<Vec<_>>()}
            </select>
        </div>
        </Show>

        // ── Transformers / vLLM fields ──────────────────────────────────
        <Show when=move || is_transformers(form.get().and_then(|f| f.hf_format.clone()).as_deref())>
        <div class="form-grid">
            <label class="form-label" for="field-max-model-len">
                "Max model length"
                <div class="form-hint">{"vLLM --max-model-len. Leave blank for the model's default."}</div>
            </label>
            <input
                id="field-max-model-len"
                class="form-input"
                type="number"
                min="1"
                placeholder="e.g. 32768"
                on:input=move |ev| {
                    let val = target_value(&ev);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.vllm.max_model_len = if val.is_empty() { None } else { val.parse::<u32>().ok() };
                        }
                    });
                }
            />

            <label class="form-label" for="field-kv-cache-dtype">
                "KV cache dtype"
                <div class="form-hint">{"vLLM --kv-cache-dtype. fp8 reduces VRAM at a small quality cost."}</div>
            </label>
            <select
                id="field-kv-cache-dtype"
                class="form-select"
                on:change=move |e| {
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.vllm.kv_cache_dtype = if val.is_empty() { None } else { Some(val) };
                        }
                    });
                }
            >
                <option value="">"Default (auto)"</option>
                {KV_CACHE_DTYPE_OPTIONS.iter().map(|opt| {
                    let opt_str = *opt;
                    view! { <option value=opt_str>{opt_str}</option> }
                }).collect::<Vec<_>>()}
            </select>

            <label class="form-label" for="field-max-num-batched-tokens">
                "Max batched tokens"
                <div class="form-hint">{"vLLM --max-num-batched-tokens. Caps prefill batch size."}</div>
            </label>
            <input
                id="field-max-num-batched-tokens"
                class="form-input"
                type="number"
                min="1"
                placeholder="e.g. 8192"
                on:input=move |ev| {
                    let val = target_value(&ev);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.vllm.max_num_batched_tokens = if val.is_empty() { None } else { val.parse::<u32>().ok() };
                        }
                    });
                }
            />
        </div>
        </Show>
    }
}
