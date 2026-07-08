use leptos::prelude::*;

use crate::components::section_card::SectionCard;
use crate::utils::target_value;

// ─── Sampling form field macros ───────────────────────────────────────────

macro_rules! sampling_float {
    ($config:expr, $key:expr, $label:expr, $field:ident) => {{
        let key = $key.clone();
        let key2 = $key.clone();
        let config = $config;
        view! {
            <div>
                <label>{$label}</label>
                <input
                    type="number"
                    step="0.01"
                    prop:value=move || config.get()
                        .and_then(|c| c.sampling_templates.get(&key).and_then(|t| t.$field))
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                    on:input=move |ev| {
                        let v = target_value(&ev);
                        let k = key2.clone();
                        config.update(|c| if let Some(c) = c {
                            if let Some(t) = c.sampling_templates.get_mut(&k) {
                                t.$field = if v.is_empty() { None } else { v.parse::<f64>().ok() };
                            }
                        });
                    }
                />
            </div>
        }
    }};
}

macro_rules! sampling_u32 {
    ($config:expr, $key:expr, $label:expr, $field:ident) => {{
        let key = $key.clone();
        let key2 = $key.clone();
        let config = $config;
        view! {
            <div>
                <label>{$label}</label>
                <input
                    type="number"
                    min="0"
                    step="1"
                    prop:value=move || config.get()
                        .and_then(|c| c.sampling_templates.get(&key).and_then(|t| t.$field))
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                    on:input=move |ev| {
                        let v = target_value(&ev);
                        let k = key2.clone();
                        config.update(|c| if let Some(c) = c {
                            if let Some(t) = c.sampling_templates.get_mut(&k) {
                                t.$field = if v.is_empty() { None } else { v.parse::<u32>().ok() };
                            }
                        });
                    }
                />
            </div>
        }
    }};
}

// ─── Sampling Templates Form ──────────────────────────────────────────────

#[component]
pub fn SamplingForm(
    config: RwSignal<Option<crate::pages::config_editor::types::Config>>,
) -> impl IntoView {
    let template_keys = move || {
        config
            .get()
            .map(|c| c.sampling_templates.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    };

    view! {
        <SectionCard title="Sampling Templates".to_string() description=Some("Reusable named sets of LLM sampling parameters.".to_string())>

            {move || {
                let keys = template_keys();
                if keys.is_empty() {
                    view! { <p class="text-muted">"No sampling templates defined."</p> }.into_any()
                } else {
                    view! {
                        <div style="display:flex;flex-direction:column;gap:1.5rem;margin-top:1rem;">
                            {keys.into_iter().map(|key| {
                                view! {
                                    <fieldset style="border:1px solid var(--border,#ccc);padding:1rem;border-radius:6px;">
                                        <legend style="font-weight:600;">{key.clone()}</legend>
                                        <div style="display:grid;grid-template-columns:1fr 1fr;gap:0.75rem;">
                                            {sampling_float!(config, key, "Temperature", temperature)}
                                            {sampling_u32!(config, key, "Top K", top_k)}
                                            {sampling_float!(config, key, "Top P", top_p)}
                                            {sampling_float!(config, key, "Min P", min_p)}
                                            {sampling_float!(config, key, "Presence Penalty", presence_penalty)}
                                            {sampling_float!(config, key, "Frequency Penalty", frequency_penalty)}
                                            {sampling_float!(config, key, "Repeat Penalty", repeat_penalty)}
                                        </div>
                                    </fieldset>
                                }.into_any()
                            }).collect::<Vec<_>>()}
                        </div>
                    }.into_any()
                }
            }}
        </SectionCard>
    }
}
