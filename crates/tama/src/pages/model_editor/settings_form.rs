use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use super::api::{fetch_gpu_devices, refresh_gpu_devices};
use super::types::{BackendOption, GpuDeviceInfo, ModelForm};
use crate::utils::{set_checked, set_input_value, target_value};

const MODALITY_OPTIONS: &[(&str, &str)] = &[
    ("text", "Text"),
    ("image", "Image"),
    ("audio", "Audio"),
    ("video", "Video"),
    ("pdf", "PDF"),
];

#[component]
pub fn ModelEditorSettingsForm(
    form: RwSignal<Option<ModelForm>>,
    backends: RwSignal<Vec<BackendOption>>,
) -> impl IntoView {
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

    // Populate input values when the form data loads (or model changes).
    let last_init_id = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            if last_init_id.get_value() != Some(f.id.clone()) {
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
                // GPU layers and device
                set_input_value(
                    "field-gpu-layers",
                    &f.gpu_layers.map(|v| v.to_string()).unwrap_or_default(),
                );
                set_input_value(
                    "field-gpu-device",
                    f.gpu_device.as_deref().unwrap_or_default(),
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
                last_init_id.set_value(Some(f.id.clone()));
            }
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
                            // Sentinel value "__clear__" tells the API to clear gpu_device
                            // to None (since the partial-update body uses `null` to mean
                            // "preserve", we need a distinct marker for "clear to None").
                            form.gpu_device = if val.is_empty() {
                                Some("__clear__".to_string())
                            } else {
                                Some(val)
                            };
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

            <label class="form-label">"Input Modalities"</label>
            <div class="form-check-group modality-row">
                <For
                    each=move || MODALITY_OPTIONS.iter().enumerate().map(|(i, (v, l))| (i, *v, *l))
                    key=|(i, v, _)| (*i, v.to_string())
                    children=move |(_i, value, label)| {
                        let value_str = value.to_string();
                        let input_id = format!("field-modality-input-{}", value);
                        let label_for = format!("field-modality-input-{}", value);
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
