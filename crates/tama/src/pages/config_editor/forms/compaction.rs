use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::section_card::SectionCard;
use crate::core_mirrors::CompactionDevice as CoreCompactionDevice;
use crate::utils::target_value;

#[component]
pub fn CompactionForm(config: RwSignal<Option<crate::types::config::Config>>) -> impl IntoView {
    let get_compaction = move || config.get().map(|c| c.compaction).unwrap_or_default();

    view! {
        <SectionCard title="Compaction (LLMLingua-2)".to_string() description=Some("Compress prompts before they hit the LLM to reduce token costs. Requires uv installed (pipx install uv).".to_string())>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label class="checkbox-label">
                        <input
                            type="checkbox"
                            prop:checked=move || get_compaction().enabled
                            on:change=move |ev| {
                                let checked = ev.target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                    .map(|el| el.checked())
                                    .unwrap_or(false);
                                config.update(|c| if let Some(c) = c { c.compaction.enabled = checked; });
                            }
                        />
                        "Enable compaction"
                    </label>
                </div>

                <div>
                    <label>"Device"</label>
                    <select
                        on:change=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| {
                                if let Some(c) = c {
                                    c.compaction.device = CoreCompactionDevice::from_str(&v).unwrap_or_default();
                                }
                            });
                        }
                        prop:value=move || get_compaction().device.as_str()
                    >
                        <option value="cpu">"cpu"</option>
                        <option value="cuda">"cuda"</option>
                        <option value="cuda:0">"cuda:0"</option>
                        <option value="cuda:1">"cuda:1"</option>
                        <option value="cuda:2">"cuda:2"</option>
                        <option value="cuda:3">"cuda:3"</option>
                        <option value="cuda:4">"cuda:4"</option>
                        <option value="cuda:5">"cuda:5"</option>
                        <option value="cuda:6">"cuda:6"</option>
                        <option value="cuda:7">"cuda:7"</option>
                        <option value="mps">"mps"</option>
                    </select>
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "Compute device for the compaction model. CUDA is recommended if available."
                    </p>
                </div>

                <div>
                    <label>"Port"</label>
                    <input
                        type="number"
                        min="0"
                        placeholder="auto-assigned"
                        prop:value=move || get_compaction().port.map(|p| p.to_string()).unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c {
                                c.compaction.port = if v.is_empty() { None } else { v.parse::<u16>().ok() };
                            });
                        }
                    />
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "Leave empty for auto-assignment."
                    </p>
                </div>

                <div>
                    <label>"Request Timeout (ms)"</label>
                    <input
                        type="number"
                        min="1000"
                        step="1000"
                        prop:value=move || get_compaction().request_timeout_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.compaction.request_timeout_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Custom Server Path"</label>
                    <input
                        type="text"
                        placeholder="use embedded default"
                        prop:value=move || get_compaction().server_path.clone().unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c {
                                c.compaction.server_path = if v.is_empty() { None } else { Some(v) };
                            });
                        }
                    />
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "Path to a custom main.py. Leave empty to use the embedded server."
                    </p>
                </div>
            </div>
        </SectionCard>
    }
}
