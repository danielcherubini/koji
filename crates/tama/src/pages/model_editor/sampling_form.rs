use std::sync::Arc;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::{ModelForm, SamplingField};
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

// ── Sampling field definitions ───────────────────────────────────────────────

/// Metadata for a single sampling parameter.
struct FieldDef {
    key: &'static str,
    label: &'static str,
    placeholder: &'static str,
    step: &'static str,
    default_value: &'static str,
}

impl FieldDef {
    fn all() -> &'static [Self] {
        &[
            Self {
                key: "temperature",
                label: "Temperature",
                placeholder: "0.3",
                step: "0.01",
                default_value: "0.3",
            },
            Self {
                key: "top_k",
                label: "Top K",
                placeholder: "40",
                step: "1",
                default_value: "40",
            },
            Self {
                key: "top_p",
                label: "Top P",
                placeholder: "0.9",
                step: "0.01",
                default_value: "0.9",
            },
            Self {
                key: "min_p",
                label: "Min P",
                placeholder: "0.05",
                step: "0.01",
                default_value: "0.05",
            },
            Self {
                key: "presence_penalty",
                label: "Presence Penalty",
                placeholder: "0.1",
                step: "0.01",
                default_value: "0.1",
            },
            Self {
                key: "frequency_penalty",
                label: "Frequency Penalty",
                placeholder: "0.1",
                step: "0.01",
                default_value: "0.1",
            },
            Self {
                key: "repeat_penalty",
                label: "Repeat Penalty",
                placeholder: "1.1",
                step: "0.01",
                default_value: "1.1",
            },
        ]
    }
}

// ── Component ────────────────────────────────────────────────────────────────

#[component]
pub fn ModelEditorSamplingForm(
    form: RwSignal<Option<ModelForm>>,
    templates: LocalResource<Option<std::collections::HashMap<String, serde_json::Value>>>,
    load_preset_action: Action<String, (), LocalStorage>,
    active_preset: RwSignal<String>,
    save_preset_action: Action<String, (), LocalStorage>,
) -> impl IntoView {
    // Populate input values when the form data loads (or model changes).
    let last_init_id = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            if last_init_id.get_value() != Some(f.id.clone()) {
                for (key, field) in &f.sampling {
                    set_checked(&format!("field-sampling-{}-enabled", key), field.enabled);
                    set_input_value(&format!("field-sampling-{}-value", key), &field.value);
                }
                last_init_id.set_value(Some(f.id.clone()));
            }
        }
    });

    // Inline preset name input state
    let show_preset_input = RwSignal::new(false);
    let preset_name_input = RwSignal::new(String::new());

    // Per-field expanded state: HashMap<String, bool> keyed by field key
    let field_expanded = RwSignal::new(std::collections::HashMap::new());

    // ── Helpers ──────────────────────────────────────────────────────────

    // Toggle enabled/disabled for a field.
    let toggle_enabled = move |key: &str, enabled: bool| {
        form.update(|f| {
            if let Some(form) = f {
                let entry = form
                    .sampling
                    .entry(key.to_string())
                    .or_insert_with(SamplingField::default);
                entry.enabled = enabled;
                if enabled && entry.value.is_empty() {
                    // Populate with default value on first enable
                    if let Some(def) = FieldDef::all().iter().find(|d| d.key == key) {
                        entry.value = def.default_value.to_string();
                    }
                }
            }
        });
    };

    // Update a field's value.
    let update_value = move |key: &str, value: String| {
        form.update(|f| {
            if let Some(form) = f {
                form.sampling
                    .entry(key.to_string())
                    .or_insert_with(SamplingField::default)
                    .value = value;
            }
        });
    };

    // ── Preset save handler ──────────────────────────────────────────────

    let save_preset_action_inner = move || {
        let name = preset_name_input.get();
        if !name.is_empty() {
            save_preset_action.dispatch(name.clone());
            show_preset_input.set(false);
            preset_name_input.set(String::new());
        }
    };

    // ── Field rendering ──────────────────────────────────────────────────

    let render_fields = move || {
        FieldDef::all()
            .iter()
            .map(|field| {
                let key = Arc::new(field.key.to_string());
                let label = field.label.to_string();
                let placeholder = field.placeholder.to_string();
                let step = field.step.to_string();

                // Clone Arc for each closure that needs it
                let k_enabled = Arc::clone(&key);
                let k_value = Arc::clone(&key);
                let k_expand = Arc::clone(&key);
                let k_checkbox = Arc::clone(&key);
                let k_collapse = Arc::clone(&key);
                let k_input = Arc::clone(&key);

                // Reactive signals for this field's state
                let enabled_signal = Signal::derive(move || {
                    form.get()
                        .and_then(|f| f.sampling.get(&*k_enabled).cloned())
                        .map(|f| f.enabled)
                        .unwrap_or(false)
                });
                let _value_signal = Signal::derive(move || {
                    form.get()
                        .and_then(|f| f.sampling.get(&*k_value).cloned())
                        .map(|f| f.value)
                        .unwrap_or_default()
                });

                view! {
                    // ── Disabled state: compact row ──────────────────────
                    <div
                        class="sampling-field-row"
                        class:sampling-field-row--hidden=move || enabled_signal.get()
                    >
                        <span class="sampling-field-row__label">{label.clone()}</span>
                        <span class="sampling-field-row__status">"[off]"</span>
                        <button
                            type="button"
                            class="sampling-toggle-btn sampling-toggle-btn--expand"
                            on:click=move |_| {
                                toggle_enabled(&k_expand, true);
                                field_expanded.update(|map| { map.insert((*k_expand).clone(), true); });
                            }
                            title="Enable and expand"
                        >
                            "(+)"
                        </button>
                    </div>

                    // ── Enabled state: expanded card ─────────────────────
                    <div
                        class="sampling-field-expanded"
                        class:sampling-field-expanded--hidden=move || !enabled_signal.get()
                    >
                        <div class="sampling-field-expanded__header">
                            <label class="form-check">
                                <input
                                    type="checkbox"
                                    id=format!("field-sampling-{}-enabled", key)

                                    on:change=move |e| {
                                        use wasm_bindgen::JsCast;
                                        let checked = e.target()
                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            .map(|el| el.checked())
                                            .unwrap_or(false);
                                        toggle_enabled(&k_checkbox, checked);
                                        if !checked {
                                            field_expanded.update(|map| {
                                                map.insert((*k_checkbox).clone(), false);
                                            });
                                        }
                                    }
                                />
                                <span class="form-check-label">{label.clone()}</span>
                            </label>
                            <button
                                type="button"
                                class="sampling-toggle-btn sampling-toggle-btn--collapse"
                                on:click=move |_| {
                                    toggle_enabled(&k_collapse, false);
                                    field_expanded.update(|map| { map.insert((*k_collapse).clone(), false); });
                                }
                                title="Disable and collapse"
                            >
                                "(×)"
                            </button>
                        </div>
                        <div class="sampling-field-expanded__input">
                            <input
                                class="form-input form-input--number"
                                type="number"
                                step=step.clone()
                                placeholder=placeholder.clone()
                                id=format!("field-sampling-{}-value", key)

                                on:input=move |e| {
                                    update_value(&k_input, target_value(&e));
                                }
                            />
                        </div>
                    </div>
                }
            })
            .collect::<Vec<_>>()
    };

    // ── Preset dropdown options ──────────────────────────────────────────

    let preset_options = move || {
        if let Some(guard) = templates.get() {
            if let Some(templates_map) = &*guard {
                return templates_map
                    .keys()
                    .cloned()
                    .map(|k| {
                        let k_clone = k.clone();
                        view! { <option value=k_clone>{k}</option> }
                    })
                    .collect::<Vec<_>>();
            }
        }
        vec![]
    };

    // ── View ─────────────────────────────────────────────────────────────

    view! {
        <div class="sampling-form">
            // ── Preset bar ───────────────────────────────────────────────
            <div class="sampling-preset-bar">
                <div class="sampling-preset-bar__left">
                    <label class="form-label" for="field-profile">"Load Preset"</label>
                    <select
                        id="field-profile"
                        class="form-select"
                        on:change=move |e| {
                            let name = target_value(&e);
                            if !name.is_empty() {
                                load_preset_action.dispatch(name);
                            }
                        }
                    >
                        <option value="">"(select a preset)"</option>
                        {move || preset_options()}
                    </select>
                </div>

                <div class="sampling-preset-bar__right">
                    <button
                        type="button"
                        class="btn btn-secondary btn-sm"
                        on:click=move |_| {
                            show_preset_input.set(true);
                            preset_name_input.set(String::new());
                        }
                    >
                        "Save as preset"
                    </button>

                    // Inline preset name input
                    {move || {
                        show_preset_input.get().then(|| {
                            let _preset_name_ref = preset_name_input;
                            view! {
                                <div class="sampling-preset-input">
                                    <input
                                        type="text"
                                        class="form-input form-input--sm"
                                        placeholder="Preset name"

                                        on:input=move |e| { preset_name_input.set(target_value(&e)); }
                                        on:keydown=move |e| {
                                            if e.key() == "Enter" { save_preset_action_inner(); }
                                            else if e.key() == "Escape" { show_preset_input.set(false); }
                                        }
                                    />
                                    <button
                                        type="button"
                                        class="btn btn-primary btn-sm"
                                        on:click=move |_| { save_preset_action_inner(); }
                                    >
                                        "Save"
                                    </button>
                                    <button
                                        type="button"
                                        class="btn btn-ghost btn-sm"
                                        on:click=move |_| { show_preset_input.set(false); }
                                    >
                                        "Cancel"
                                    </button>
                                </div>
                            }
                        })
                    }}
                </div>

                // Active preset label
                <div class="sampling-preset-bar__info">
                    {move || {
                        let name = active_preset.get();
                        (!name.is_empty()).then(|| {
                            view! { <span class="text-muted">"Currently using: \"" {name} "\""</span> }.into_any()
                        })
                    }}
                </div>
            </div>

            // ── Sampling fields ──────────────────────────────────────────
            <div class="sampling-fields-list">
                {render_fields()}
            </div>
        </div>
    }
}
