use wasm_bindgen::JsCast;

use leptos::prelude::*;

use crate::utils::target_value;

#[component]
pub fn ProxyBasicFields(
    config: RwSignal<Option<crate::pages::config_editor::types::Config>>,
) -> impl IntoView {
    let get_proxy = move || config.get().map(|c| c.proxy).unwrap_or_default();

    view! {
        <h3 style="font-size:1rem;font-weight:600;color:var(--text-secondary);margin:0 0 0.25rem 0;">
            "Network"
        </h3>

        <div class="form-group">
            <label class="form-label">"Host"</label>
            <input
                class="form-input"
                type="text"
                prop:value=move || get_proxy().host
                on:input=move |ev| {
                    let v = target_value(&ev);
                    config.update(|c| if let Some(c) = c { c.proxy.host = v; });
                }
            />
        </div>

        <div class="form-group">
            <label class="form-label">"Port"</label>
            <input
                class="form-input"
                type="number"
                min="1"
                max="65535"
                prop:value=move || get_proxy().port.to_string()
                on:input=move |ev| {
                    if let Ok(v) = target_value(&ev).parse::<u16>() {
                        config.update(|c| if let Some(c) = c { c.proxy.port = v; });
                    }
                }
            />
        </div>

        <div class="form-group">
            <label class="checkbox-label">
                <input
                    type="checkbox"
                    prop:checked=move || get_proxy().auto_unload
                    on:change=move |ev| {
                        let checked = ev.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        config.update(|c| if let Some(c) = c { c.proxy.auto_unload = checked; });
                    }
                />
                "Auto-unload idle models"
            </label>
        </div>

        <div class="form-group">
            <label class="form-label">"Idle Timeout (seconds)"</label>
            <input
                class="form-input"
                type="number"
                min="1"
                prop:value=move || get_proxy().idle_timeout_secs.to_string()
                on:input=move |ev| {
                    if let Ok(v) = target_value(&ev).parse::<u64>() {
                        config.update(|c| if let Some(c) = c { c.proxy.idle_timeout_secs = v; });
                    }
                }
            />
        </div>

        <div class="form-group">
            <label class="form-label">"Startup Timeout (seconds)"</label>
            <input
                class="form-input"
                type="number"
                min="0"
                prop:value=move || get_proxy().startup_timeout_secs.to_string()
                on:input=move |ev| {
                    if let Ok(v) = target_value(&ev).parse::<u64>() {
                        config.update(|c| if let Some(c) = c { c.proxy.startup_timeout_secs = v; });
                    }
                }
            />
        </div>
    }
}
