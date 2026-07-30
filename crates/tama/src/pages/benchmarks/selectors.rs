//! Shared selector components for benchmark pages.
//!
//! Extracted from [`llama_bench`](super::llama_bench), [`mtp_bench`](super::mtp_bench),
//! and [`spec_bench`](super::spec_bench) to eliminate copy-pasted Model+Quant and Backend
//! dropdown markup (~360 lines across three files).

use std::collections::BTreeMap;

use leptos::prelude::*;

use crate::utils::target_value;

/// Available model entry type: `(id, display_name, quants, n_batch, n_ubatch)`.
type ModelEntry = (String, String, Vec<String>, Option<u32>, Option<u32>);

/// Shared Model + Quant selector dropdowns.
///
/// Renders two `<select>` elements in a grid:
/// 1. **Model** — deduplicated by display name via `BTreeMap` (sorted alphabetically).
/// 2. **Quant** — flattened from all quant entries of the selected model, rendered as
///    `"id:quant"` values.
#[component]
pub fn ModelQuantSelect(
    /// Available models signal containing typed model entries.
    models: ReadSignal<Vec<ModelEntry>>,
    /// Selected display name (model name) signal.
    selected_model: RwSignal<String>,
    /// Selected quant signal (holds `"id:quant"` format).
    selected_quant: RwSignal<String>,
) -> impl IntoView {
    let (selected_display_sig, _) = selected_model.split();
    let (selected_quant_sig, _) = selected_quant.split();

    view! {
        <div class="grid-2">
            <div class="form-group">
                <label>"Model"</label>
                <select
                    class="form-select"
                    on:change=move |e| {
                        let val = target_value(&e);
                        selected_model.set(val);
                    }
                >
                    <option value="" disabled selected=move || selected_display_sig.get().is_empty()>"Select a model..."</option>
                    {move || {
                        let models = models.get();
                        // Deduplicate by display_name; BTreeMap keeps them sorted
                        // alphabetically for stable rendering.
                        let mut grouped: BTreeMap<String, ()> = BTreeMap::new();
                        for (_, name, _, _, _) in models.iter() {
                            grouped.insert(name.clone(), ());
                        }
                        grouped.keys().map(|name| {
                            let value = name.clone();
                            let label = name.clone();
                            view! {
                                <option value=value>{label}</option>
                            }.into_any()
                        }).collect::<Vec<_>>()
                    }}
                </select>
            </div>
            <div class="form-group">
                <label>"Quant"</label>
                <select
                    class="form-select"
                    prop:disabled=move || selected_display_sig.get().is_empty()
                    on:change=move |e| {
                        let val = target_value(&e);
                        selected_quant.set(val);
                    }
                >
                    <option value="" disabled>"Select quant..."</option>
                    {move || {
                        let models = models.get();
                        let dn = selected_display_sig.get();
                        let selected_id = selected_quant_sig.get();
                        // Flatten all quants from matching model entries into individual options.
                        models.iter()
                            .filter(|(_, name, _, _, _)| name == &dn)
                            .flat_map(|(id, _, quants, _, _)| {
                                quants.iter().map(move |quant| (id.clone(), quant.clone()))
                            })
                            .map(|(id_clone, quant)| {
                                let value = format!("{}:{}", id_clone, quant);
                                let is_selected = value == selected_id;
                                view! {
                                    <option value=value selected=is_selected>{quant}</option>
                                }.into_any()
                            }).collect::<Vec<_>>()
                    }}
                </select>
            </div>
        </div>
    }
}

/// Backend entry type: `(name, display)`.
type BackendEntry = (String, String);

/// Shared Backend selector dropdown.
///
/// Renders a single `<select>` with an empty "Auto (model's backend)" option
/// followed by all installed backends. Accepts a `hint_text` parameter so each
/// tab can customise its help text.
#[component]
pub fn BackendSelect(
    /// Available backends signal containing typed backend entries.
    backends: ReadSignal<Vec<BackendEntry>>,
    /// Selected backend signal (holds `"name:variant"` format, or empty for auto).
    selected_backend: RwSignal<String>,
    /// Help text shown below the dropdown.
    hint_text: &'static str,
) -> impl IntoView {
    view! {
        <select
            class="form-select"
            on:change=move |e| {
                let val = target_value(&e);
                selected_backend.set(val);
            }
        >
            <option value="">"Auto (model's backend)"</option>
            {move || {
                let backends = backends.get();
                backends.iter().map(|(name, display)| {
                    let name_clone = name.clone();
                    let display_clone = display.clone();
                    view! {
                        <option value=name_clone>{display_clone}</option>
                    }.into_any()
                }).collect::<Vec<_>>()
            }}
        </select>
        <small class="bench-hint">{hint_text}</small>
    }
}
