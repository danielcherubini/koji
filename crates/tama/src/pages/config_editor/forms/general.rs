use leptos::prelude::*;

use crate::components::section_card::SectionCard;
use crate::gpu_types::LogLevel as CoreLogLevel;
use crate::utils::target_value;

#[component]
pub fn GeneralForm(
    config: RwSignal<Option<crate::pages::config_editor::types::Config>>,
) -> impl IntoView {
    let get_general = move || config.get().map(|c| c.general).unwrap_or_default();

    view! {
        <SectionCard title="General Settings".to_string() description=Some("Global Tama settings.".to_string())>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Log Level"</label>
                    <select
                        on:change=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| {
                                if let Some(c) = c {
                                    c.general.log_level = CoreLogLevel::from_str(&v);
                                }
                            });
                        }
                        prop:value=move || get_general().log_level.as_str().to_string()
                    >
                        <option value="debug">"debug"</option>
                        <option value="info">"info"</option>
                        <option value="warn">"warn"</option>
                        <option value="error">"error"</option>
                    </select>
                </div>

                <div>
                    <label>"Models Directory"</label>
                    <input
                        type="text"
                        placeholder="/path/to/models"
                        prop:value=move || get_general().models_dir.unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c {
                                c.general.models_dir = if v.is_empty() { None } else { Some(v) };
                            });
                        }
                    />
                </div>

                <div>
                    <label>"Logs Directory"</label>
                    <input
                        type="text"
                        placeholder="/path/to/logs"
                        prop:value=move || get_general().logs_dir.unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c {
                                c.general.logs_dir = if v.is_empty() { None } else { Some(v) };
                            });
                        }
                    />
                </div>

                <div>
                    <label>"HuggingFace Token"</label>
                    <input
                        type="password"
                        placeholder="hf_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
                        prop:value=move || get_general().hf_token.unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c {
                                c.general.hf_token = if v.is_empty() { None } else { Some(v) };
                            });
                        }
                    />
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "API token for downloading gated models from HuggingFace. "
                        "Get your token at " <a href="https://huggingface.co/settings/tokens" target="_blank" rel="noopener">"huggingface.co/settings/tokens"</a>
                    </p>
                </div>

                <div>
                    <label>"Update Check Interval (hours)"</label>
                    <input
                        type="number"
                        min="1"
                        prop:value=move || get_general().update_check_interval.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.general.update_check_interval = v; });
                            }
                        }
                    />
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "How often to check for Tama updates (in hours). Default: 12."
                    </p>
                </div>
            </div>
        </SectionCard>
    }
}
