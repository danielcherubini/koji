use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::ModelForm;
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
        if let Ok(textarea) = el.dyn_into::<web_sys::HtmlTextAreaElement>() {
            textarea.set_value(value);
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

const SPEC_TYPE_DRAFT_MTP: &str = "draft-mtp";
const SPEC_TYPE_NGRAM_SIMPLE: &str = "ngram-simple";

/// Advanced form section combining Speculative Decoding and Extra Args.
#[component]
pub fn ModelEditorAdvancedForm(form: RwSignal<Option<ModelForm>>) -> impl IntoView {
    // Checkboxes for spec types
    let toggle_spec_type = move |e: web_sys::Event, spec_type: String| {
        let checked = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|el| el.checked())
            .unwrap_or(false);
        form.update(move |f| {
            if let Some(form) = f {
                if checked {
                    if !form.spec_decoding.spec_types.contains(&spec_type) {
                        form.spec_decoding.spec_types.push(spec_type);
                    }
                } else {
                    form.spec_decoding.spec_types.retain(|s| s != &spec_type);
                }
            }
        });
    };

    let has_any_type = Signal::derive(move || {
        form.get()
            .as_ref()
            .map(|f| !f.spec_decoding.spec_types.is_empty())
            .unwrap_or(false)
    });

    let has_draft_mtp = Signal::derive(move || {
        form.get()
            .as_ref()
            .map(|f| {
                f.spec_decoding
                    .spec_types
                    .contains(&SPEC_TYPE_DRAFT_MTP.to_string())
            })
            .unwrap_or(false)
    });

    // Populate input values when the form data loads (or model changes).
    // Only runs when the model ID changes, not on every keystroke.
    let last_init_id = StoredValue::new(None::<String>);
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            if last_init_id.get_value() != Some(f.id.clone()) {
                set_checked(
                    "field-spec-draft-mtp",
                    f.spec_decoding
                        .spec_types
                        .contains(&SPEC_TYPE_DRAFT_MTP.to_string()),
                );
                set_checked(
                    "field-spec-ngram-simple",
                    f.spec_decoding
                        .spec_types
                        .contains(&SPEC_TYPE_NGRAM_SIMPLE.to_string()),
                );
                set_input_value("field-args", &f.args);
                // Spec decoding selects and input
                set_input_value(
                    "field-spec-n-max",
                    &f.spec_decoding
                        .n_max
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-spec-n-min",
                    &f.spec_decoding
                        .n_min
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                set_input_value(
                    "field-spec-draft-ngl",
                    &f.spec_decoding
                        .draft_ngl
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                );
                last_init_id.set_value(Some(f.id.clone()));
            }
        }
    });

    view! {
        // ── Speculative Decoding subsection ──────────────────────────────
        <h3 class="form-section-title">"Speculative Decoding"</h3>
        <div class="form-grid">
            // Spec type checkboxes
            <label class="form-label">"Spec Types"</label>
            <div class="form-check-group">
                // draft-mtp checkbox
                <div class="form-check">
                    <input
                        id="field-spec-draft-mtp"
                        type="checkbox"
                        on:change=move |e| {
                            toggle_spec_type(e, SPEC_TYPE_DRAFT_MTP.to_string());
                        }
                    />
                    <label class="form-check-label" for="field-spec-draft-mtp">
                        "draft-mtp"
                        <div class="form-hint">"Multi-Token Prediction — uses a draft model for speculative decoding"</div>
                    </label>
                </div>

                // ngram-simple checkbox
                <div class="form-check">
                    <input
                        id="field-spec-ngram-simple"
                        type="checkbox"
                        on:change=move |e| {
                            toggle_spec_type(e, SPEC_TYPE_NGRAM_SIMPLE.to_string());
                        }
                    />
                    <label class="form-check-label" for="field-spec-ngram-simple">
                        "ngram-simple"
                        <div class="form-hint">"Simple n-gram speculative decoding — lightweight, no extra model needed"</div>
                    </label>
                </div>
            </div>

            // Draft Max (n_max) — shown when any type is checked
            <Show when=move || has_any_type.get()>
                <label class="form-label" for="field-spec-n-max">"Draft Max"</label>
                <select
                    id="field-spec-n-max"
                    class="form-select"
                    on:change=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.spec_decoding.n_max = val.parse::<u32>().ok();
                            }
                        });
                    }
                >
                    <option value="">"(select)"</option>
                    {(1..=8).map(|v| {
                        let selected = form.get_untracked()
                            .as_ref()
                            .map(|f| f.spec_decoding.n_max == Some(v))
                            .unwrap_or(false);
                        let val = v.to_string();
                        view! { <option value=val selected=selected>{v}</option> }
                    }).collect::<Vec<_>>()}
                </select>

                // Draft Min (n_min) — shown when any type is checked
                <label class="form-label" for="field-spec-n-min">"Draft Min"</label>
                <select
                    id="field-spec-n-min"
                    class="form-select"
                    on:change=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.spec_decoding.n_min = val.parse::<u32>().ok();
                            }
                        });
                    }
                >
                    <option value="">"(select)"</option>
                    {(1..=8).map(|v| {
                        let selected = form.get_untracked()
                            .as_ref()
                            .map(|f| f.spec_decoding.n_min == Some(v))
                            .unwrap_or(false);
                        let val = v.to_string();
                        view! { <option value=val selected=selected>{v}</option> }
                    }).collect::<Vec<_>>()}
                </select>
            </Show>

            // Draft GPU Layers (draft_ngl) — shown when draft-mtp is checked
            <Show when=move || has_draft_mtp.get()>
                <label class="form-label" for="field-spec-draft-ngl">
                    "Draft GPU Layers"
                    <div class="form-hint">"99 = all layers"</div>
                </label>
                <input
                    id="field-spec-draft-ngl"
                    class="form-input"
                    type="number"
                    min="0"
                    max="999"
                    placeholder="e.g. 99"
                    on:input=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.spec_decoding.draft_ngl = if val.is_empty() {
                                    None
                                } else {
                                    val.parse::<u32>().ok()
                                };
                            }
                        });
                    }
                />
            </Show>
        </div>

        // ── Extra Args subsection ────────────────────────────────────────
        <h3 class="form-section-title mt-2">"Extra Args"</h3>
        <textarea
            id="field-args"
            class="form-textarea"
            rows="6"
            placeholder="One flag per line, e.g. -fa 1, -b 4096, --mlock"
            on:input=move |e| {
                form.update(|f| {
                    if let Some(form) = f {
                        form.args = target_value(&e);
                    }
                });
            }
        />
        <span class="form-hint">"One flag per line, e.g. -fa 1, --mlock, or -b 4096. Quote values containing spaces"</span>
    }
}
