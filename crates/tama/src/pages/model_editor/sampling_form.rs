use leptos::prelude::*;

use super::types::{ModelForm, SamplingField};
use crate::utils::target_value;

use leptos::ev::{KeyboardEvent, MouseEvent};

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
    // Inline preset name input state
    let show_preset_input = RwSignal::new(false);
    let preset_name_input = RwSignal::new(String::new());

    // Per-field expanded state: HashMap<String, bool> keyed by field key
    let field_expanded = RwSignal::new(std::collections::HashMap::new());

    // ── Helpers ──────────────────────────────────────────────────────────

    // Get or default the enabled state for a field.
    let is_enabled = move |key: &str| -> bool {
        form.get()
            .and_then(|f| f.sampling.get(key).cloned())
            .map(|field| field.enabled)
            .unwrap_or(false)
    };

    // Get or default the value for a field.
    let get_value = move |key: &str| -> String {
        form.get()
            .and_then(|f| f.sampling.get(key).cloned())
            .map(|field| field.value)
            .unwrap_or_default()
    };

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

    // Expand a field (enable it if disabled).
    let expand_field = move |key: &str| {
        if !is_enabled(key) {
            toggle_enabled(key, true);
        }
        field_expanded.update(|map| {
            map.insert(key.to_string(), true);
        });
    };

    // Collapse a field (disable it).
    let collapse_field = move |key: &str| {
        toggle_enabled(key, false);
        field_expanded.update(|map| {
            map.insert(key.to_string(), false);
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

    let handle_save_preset = move |_e: MouseEvent| {
        save_preset_action_inner();
    };

    let save_preset_on_enter = move |e: KeyboardEvent| {
        if e.key() == "Enter" {
            save_preset_action_inner();
        } else if e.key() == "Escape" {
            show_preset_input.set(false);
        }
    };

    // ── Field rendering ──────────────────────────────────────────────────

    let render_fields = move || {
        FieldDef::all()
            .iter()
            .map(|field| {
                let key = field.key;
                let label = field.label;
                let placeholder = field.placeholder;
                let step = field.step;
                let enabled = is_enabled(key);
                let value = get_value(key);

                // Track expanded state on toggle
                let on_expand = move |_| {
                    expand_field(key);
                };
                let on_collapse = move |_| {
                    collapse_field(key);
                };

                view! {
                    // ── Disabled state: compact row ──────────────────────
                    <div
                        class="sampling-field-row"
                        class:sampling-field-row--hidden=move || enabled
                    >
                        <span class="sampling-field-row__label">{label}</span>
                        <span class="sampling-field-row__status">[off]</span>
                        <button
                            type="button"
                            class="sampling-toggle-btn sampling-toggle-btn--expand"
                            on:click=on_expand
                            title="Enable and expand"
                        >
                            "(+)"
                        </button>
                    </div>

                    // ── Enabled state: expanded card ─────────────────────
                    <div
                        class="sampling-field-expanded"
                        class:sampling-field-expanded--hidden=move || !enabled
                    >
                        <div class="sampling-field-expanded__header">
                            <label class="form-check">
                                <input
                                    type="checkbox"
                                    prop:checked=enabled
                                    on:change=move |e| {
                                        use wasm_bindgen::JsCast;
                                        let checked = e.target()
                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            .map(|el| el.checked())
                                            .unwrap_or(false);
                                        toggle_enabled(key, checked);
                                        if !checked {
                                            field_expanded.update(|map| {
                                                map.insert(key.to_string(), false);
                                            });
                                        }
                                    }
                                />
                                <span class="form-check-label">{label}</span>
                            </label>
                            <button
                                type="button"
                                class="sampling-toggle-btn sampling-toggle-btn--collapse"
                                on:click=on_collapse
                                title="Disable and collapse"
                            >
                                "(×)"
                            </button>
                        </div>
                        <div class="sampling-field-expanded__input">
                            <input
                                class="form-input form-input--number"
                                type="number"
                                step=step
                                placeholder=placeholder
                                prop:value=value
                                on:input=move |e| {
                                    update_value(key, target_value(&e));
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
                        {preset_options()}
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
                            view! {
                                <div class="sampling-preset-input">
                                    <input
                                        type="text"
                                        class="form-input form-input--sm"
                                        placeholder="Preset name"
                                        prop:value=preset_name_input
                                        on:keydown=save_preset_on_enter
                                    />
                                    <button
                                        type="button"
                                        class="btn btn-primary btn-sm"
                                        on:click=handle_save_preset
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
