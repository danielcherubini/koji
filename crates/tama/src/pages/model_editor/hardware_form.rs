use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use super::api::{fetch_gpu_devices, refresh_gpu_devices};
use super::types::{GpuDeviceInfo, ModelForm};
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

const MODALITY_OPTIONS: &[(&str, &str)] = &[
    ("text", "Text"),
    ("image", "Image"),
    ("audio", "Audio"),
    ("video", "Video"),
    ("pdf", "PDF"),
];

const KV_QUANT_OPTIONS: &[&str] = &[
    "f32", "f16", "bf16", "q8_0", "q4_0", "q4_1", "iq4_nl", "q5_0", "q5_1",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum KvQuantField {
    K,
    V,
}

/// Custom KV quant text input that appears when the selected value is not in the known options.
/// Shows when value is the "__custom" sentinel or any value not in KV_QUANT_OPTIONS.
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
    // GPU devices discovered for the current backend
    let gpu_devices: RwSignal<Vec<GpuDeviceInfo>> = RwSignal::new(Vec::new());
    let gpu_fetching: RwSignal<bool> = RwSignal::new(false);

    // Fetch GPU devices for the given backend name and variant.
    let fetch_devices_for_backend =
        Callback::new(move |(backend_name, gpu_variant): (String, String)| {
            if backend_name.is_empty() {
                gpu_devices.set(Vec::new());
                return;
            }
            let devices_signal = gpu_devices;
            let fetching_signal = gpu_fetching;
            spawn_local(async move {
                fetching_signal.set(true);
                let devices = fetch_gpu_devices(&backend_name, &gpu_variant).await;
                devices_signal.set(devices);
                fetching_signal.set(false);
            });
        });

    // Refresh GPU devices for the current backend.
    let refresh_devices = Callback::new(move |_| {
        let (backend_name, gpu_variant) = form.with(|f| {
            let variant = f
                .as_ref()
                .and_then(|f| f.gpu_variant.as_deref())
                .filter(|s| !s.is_empty())
                .unwrap_or("cpu");
            (
                f.as_ref().map(|f| f.backend.clone()).unwrap_or_default(),
                variant.to_string(),
            )
        });
        if backend_name.is_empty() {
            return;
        }
        let devices_signal = gpu_devices;
        let fetching_signal = gpu_fetching;
        spawn_local(async move {
            fetching_signal.set(true);
            let devices = refresh_gpu_devices(&backend_name, &gpu_variant).await;
            devices_signal.set(devices);
            fetching_signal.set(false);
        });
    });

    // When backend changes, fetch GPU devices
    let last_backend = StoredValue::new(String::new());
    Effect::new(move |_| {
        let (current_backend, current_variant) = form
            .get()
            .as_ref()
            .map(|f| {
                let variant = f
                    .gpu_variant
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("cpu");
                (f.backend.clone(), variant.to_string())
            })
            .unwrap_or_default();
        let prev = last_backend.get_value();
        if current_backend != prev && !current_backend.is_empty() {
            last_backend.set_value(current_backend.clone());
            fetch_devices_for_backend.run((current_backend, current_variant));
        } else if current_backend.is_empty() {
            last_backend.set_value(String::new());
            gpu_devices.set(Vec::new());
        }
    });

    // Populate input values when the form data loads.
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            set_input_value(
                "field-gpu-layers",
                &f.gpu_layers.map(|v| v.to_string()).unwrap_or_default(),
            );
            set_input_value(
                "field-gpu-device",
                f.gpu_device.as_deref().unwrap_or_default(),
            );
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
            // Modality checkboxes
            if let Some(m) = &f.modalities {
                for (val, _) in MODALITY_OPTIONS {
                    set_checked(
                        &format!("field-modality-input-{}", val),
                        m.input.contains(&val.to_string()),
                    );
                    set_checked(
                        &format!("field-modality-output-{}", val),
                        m.output.contains(&val.to_string()),
                    );
                }
            }
        }
    });

    view! {
        <div class="form-grid">
            <label class="form-label" for="field-gpu-layers">"GPU Layers"</label>
            <input
                id="field-gpu-layers"
                class="form-input"
                type="number"
                placeholder="e.g. 999"

                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.gpu_layers = target_value(&ev).parse::<u32>().ok();
                        }
                    });
                }
            />

            <label class="form-label" for="field-gpu-device">
                "GPU Isolation"
                <button
                        class="form-icon-button"
                        title="Refresh GPU devices"
                        disabled=move || gpu_fetching.get()
                        on:click=move |_| {
                            refresh_devices.run(());
                        }
                    >
                        {move || {
                            if gpu_fetching.get() {
                                "⟳".to_string()
                            } else {
                                "↻".to_string()
                            }
                        }}
                    </button>
                </label>
            <select
                id="field-gpu-device"
                class="form-select"

                on:change=move |e| {
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.gpu_device = if val.is_empty() { None } else { Some(val) };
                        }
                    });
                }
            >
                <option value="">
                    {move || {
                        if gpu_devices.get().is_empty() && !gpu_fetching.get() {
                            "None (could not list devices)"
                        } else {
                            "None"
                        }
                    }}
                </option>
                {move || {
                    let current = form.get().as_ref().and_then(|f| f.gpu_device.clone()).unwrap_or_default();
                    gpu_devices.get().into_iter().enumerate().map(|(i, dev)| {
                        let gpu_id = format!("GPU{i}");
                        let selected = current == gpu_id;
                        let label = if dev.vram_total_mib.is_some() {
                            format!("{} — {} ({} MiB)", gpu_id, dev.name, dev.vram_total_mib.unwrap_or(0))
                        } else {
                            format!("{} — {}", gpu_id, dev.name)
                        };
                        view! { <option value=gpu_id.clone() selected=selected>{label}</option> }
                    }).collect::<Vec<_>>()
                }}
            </select>
            <Show when=move || gpu_devices.get().is_empty() && !gpu_fetching.get() && form.get().as_ref().map(|f| !f.backend.is_empty()).unwrap_or(false)>
                <div class="form-hint">Could not list devices - leave blank for default</div>
            </Show>

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

            <label class="form-label">"Input Modalities"</label>
            <div class="form-check-group modality-row">
                <For
                    each=move || MODALITY_OPTIONS.iter().enumerate().map(|(i, (v, l))| (i, *v, *l))
                    key=|(i, v, _)| (*i, v.to_string())
                    children=move |(_i, value, label)| {
                        let value_str = value.to_string();
                        let input_id = format!("field-modality-input-{}", value);
                        let label_for = format!("field-modality-input-{}", value);
                        let _checked_value = value_str.clone();
                        let onchange_value = value_str.clone();
                        view! {
                            <div class="form-check">
                                <input
                                    id=input_id
                                    type="checkbox"

                                    on:change=move |e| {
                                        let checked = e.target()
                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            .map(|el| el.checked())
                                            .unwrap_or(false);
                                        let v = onchange_value.clone();
                                        form.update(move |f| {
                                            if let Some(form) = f {
                                                if let Some(m) = form.modalities.as_mut() {
                                                    if checked {
                                                        if !m.input.contains(&v) {
                                                            m.input.push(v.clone());
                                                        }
                                                    } else {
                                                        m.input.retain(|x| *x != v);
                                                    }
                                                }
                                            }
                                        });
                                    }
                                />
                                <label class="form-check-label" for=label_for>{label}</label>
                            </div>
                        }
                    }
                />
            </div>

            <label class="form-label">"Output Modalities"</label>
            <div class="form-check-group modality-row">
                <For
                    each=move || MODALITY_OPTIONS.iter().enumerate().map(|(i, (v, l))| (i, *v, *l))
                    key=|(i, v, _)| (*i, format!("out-{}", v))
                    children=move |(_i, value, label)| {
                        let value_str = value.to_string();
                        let input_id = format!("field-modality-output-{}", value);
                        let label_for = format!("field-modality-output-{}", value);
                        let _checked_value = value_str.clone();
                        let onchange_value = value_str.clone();
                        view! {
                            <div class="form-check">
                                <input
                                    id=input_id
                                    type="checkbox"

                                    on:change=move |e| {
                                        let checked = e.target()
                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            .map(|el| el.checked())
                                            .unwrap_or(false);
                                        let v = onchange_value.clone();
                                        form.update(move |f| {
                                            if let Some(form) = f {
                                                if let Some(m) = form.modalities.as_mut() {
                                                    if checked {
                                                        if !m.output.contains(&v) {
                                                            m.output.push(v.clone());
                                                        }
                                                    } else {
                                                        m.output.retain(|x| *x != v);
                                                    }
                                                }
                                            }
                                        });
                                    }
                                />
                                <label class="form-check-label" for=label_for>{label}</label>
                            </div>
                        }
                    }
                />
            </div>
        </div>
    }
}
