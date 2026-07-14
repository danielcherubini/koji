use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::section_card::SectionCard;
use crate::utils::target_value;

#[component]
pub fn LangfuseForm(
    config: RwSignal<Option<crate::pages::config_editor::types::Config>>,
) -> impl IntoView {
    let get_langfuse = move || config.get().map(|c| c.langfuse).unwrap_or_default();
    let enabled = move || get_langfuse().enabled;

    view! {
        <SectionCard title="Langfuse".to_string() description=Some("Observability and tracing via Langfuse. Records request/response data, token usage, and costs.".to_string())>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                // Enable toggle
                <div>
                    <label class="checkbox-label">
                        <input
                            type="checkbox"
                            prop:checked=enabled
                            on:change=move |ev| {
                                let checked = ev.target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                    .map(|el| el.checked())
                                    .unwrap_or(false);
                                config.update(|c| if let Some(c) = c { c.langfuse.enabled = checked; });
                            }
                        />
                        "Enable Langfuse"
                    </label>
                </div>

                // Public Key
                <div>
                    <label>"Public Key"</label>
                    <input
                        type="text"
                        placeholder="pk-lf-..."
                        prop:value=move || get_langfuse().public_key.clone()
                        disabled=move || !enabled()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.langfuse.public_key = v; });
                        }
                    />
                </div>

                // Secret Key
                <div>
                    <label>"Secret Key"</label>
                    <input
                        type="password"
                        placeholder="sk-lf-..."
                        prop:value=move || get_langfuse().secret_key.clone()
                        disabled=move || !enabled()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.langfuse.secret_key = v; });
                        }
                    />
                </div>

                // Host
                <div>
                    <label>"Host"</label>
                    <input
                        type="text"
                        placeholder="https://cloud.langfuse.com"
                        prop:value=move || get_langfuse().host.clone()
                        disabled=move || !enabled()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.langfuse.host = v; });
                        }
                    />
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "Langfuse API host. Default: https://cloud.langfuse.com"
                    </p>
                </div>

                // Environment
                <div>
                    <label>"Environment"</label>
                    <input
                        type="text"
                        placeholder="default"
                        prop:value=move || get_langfuse().environment.clone()
                        disabled=move || !enabled()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.langfuse.environment = v; });
                        }
                    />
                </div>

                // Capture Input checkbox
                <div>
                    <label class="checkbox-label">
                        <input
                            type="checkbox"
                            prop:checked=move || get_langfuse().capture_input
                            disabled=move || !enabled()
                            on:change=move |ev| {
                                let checked = ev.target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                    .map(|el| el.checked())
                                    .unwrap_or(false);
                                config.update(|c| if let Some(c) = c { c.langfuse.capture_input = checked; });
                            }
                        />
                        "Capture input"
                    </label>
                </div>

                // Capture Output checkbox
                <div>
                    <label class="checkbox-label">
                        <input
                            type="checkbox"
                            prop:checked=move || get_langfuse().capture_output
                            disabled=move || !enabled()
                            on:change=move |ev| {
                                let checked = ev.target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                    .map(|el| el.checked())
                                    .unwrap_or(false);
                                config.update(|c| if let Some(c) = c { c.langfuse.capture_output = checked; });
                            }
                        />
                        "Capture output"
                    </label>
                </div>

                // Capture Streaming checkbox
                <div>
                    <label class="checkbox-label">
                        <input
                            type="checkbox"
                            prop:checked=move || get_langfuse().capture_streaming
                            disabled=move || !enabled()
                            on:change=move |ev| {
                                let checked = ev.target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                    .map(|el| el.checked())
                                    .unwrap_or(false);
                                config.update(|c| if let Some(c) = c { c.langfuse.capture_streaming = checked; });
                            }
                        />
                        "Capture streaming"
                    </label>
                </div>

                // Telemetry Max Bytes
                <div>
                    <label>"Telemetry Max Bytes"</label>
                    <input
                        type="number"
                        min="0"
                        step="1"
                        prop:value=move || get_langfuse().telemetry_max_bytes.to_string()
                        disabled=move || !enabled()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<usize>() {
                                config.update(|c| if let Some(c) = c { c.langfuse.telemetry_max_bytes = v; });
                            }
                        }
                    />
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "Maximum size of captured input/output in bytes. Default: 1048576 (1 MB)."
                    </p>
                </div>

                // Electricity Price per kWh
                <div>
                    <label>"Electricity Price ($/kWh)"</label>
                    <input
                        type="number"
                        min="0"
                        step="0.01"
                        prop:value=move || get_langfuse().electricity_price_per_kwh.to_string()
                        disabled=move || !enabled()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<f64>() {
                                config.update(|c| if let Some(c) = c { c.langfuse.electricity_price_per_kwh = v; });
                            }
                        }
                    />
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "Used to calculate energy cost in Langfuse cost details. Set to 0 to disable."
                    </p>
                </div>
            </div>
        </SectionCard>
    }
}
