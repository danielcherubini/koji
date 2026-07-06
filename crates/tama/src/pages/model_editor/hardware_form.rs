use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::ModelForm;
use crate::components::context_length_selector::ContextLengthSelector;
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

const KV_QUANT_OPTIONS: &[&str] = &[
    "f32", "f16", "bf16", "q8_0", "q4_0", "q4_1", "iq4_nl", "q5_0", "q5_1",
];

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
                last_init_id.set_value(Some(f.id.clone()));
            }
        }
    });

    view! {
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
        </div>
    }
}
