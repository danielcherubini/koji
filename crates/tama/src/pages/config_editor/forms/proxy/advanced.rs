use leptos::prelude::*;

use crate::utils::target_value;

#[component]
pub fn ProxyAdvancedFields(
    config: RwSignal<Option<crate::pages::config_editor::types::Config>>,
) -> impl IntoView {
    let get_proxy = move || config.get().map(|c| c.proxy).unwrap_or_default();

    view! {
        <div>
            <label>"Circuit Breaker Threshold"</label>
            <input
                type="number"
                min="0"
                prop:value=move || get_proxy().circuit_breaker_threshold.to_string()
                on:input=move |ev| {
                    if let Ok(v) = target_value(&ev).parse::<u32>() {
                        config.update(|c| if let Some(c) = c { c.proxy.circuit_breaker_threshold = v; });
                    }
                }
            />
        </div>

        <div>
            <label>"Circuit Breaker Cooldown (seconds)"</label>
            <input
                type="number"
                min="0"
                prop:value=move || get_proxy().circuit_breaker_cooldown_seconds.to_string()
                on:input=move |ev| {
                    if let Ok(v) = target_value(&ev).parse::<u64>() {
                        config.update(|c| if let Some(c) = c { c.proxy.circuit_breaker_cooldown_seconds = v; });
                    }
                }
            />
        </div>

        <div>
            <label>"Metrics Retention (seconds)"</label>
            <input
                type="number"
                min="0"
                prop:value=move || get_proxy().metrics_retention_secs.to_string()
                on:input=move |ev| {
                    if let Ok(v) = target_value(&ev).parse::<u64>() {
                        config.update(|c| if let Some(c) = c { c.proxy.metrics_retention_secs = v; });
                    }
                }
            />
        </div>

        <div>
            <label>"Max Loaded Models (per GPU)"</label>
            <input
                type="number"
                min="0"
                prop:value=move || get_proxy().max_loaded_models.to_string()
                on:input=move |ev| {
                    if let Ok(v) = target_value(&ev).parse::<u32>() {
                        config.update(|c| if let Some(c) = c { c.proxy.max_loaded_models = v; });
                    }
                }
            />
            <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                "Maximum number of models loaded simultaneously per GPU device. Set to 0 for unlimited."
            </p>
        </div>

        <div>
            <label>"Download Queue Poll Interval (seconds)"</label>
            <input
                type="number"
                min="1"
                prop:value=move || get_proxy().download_queue_poll_interval_secs.to_string()
                on:input=move |ev| {
                    if let Ok(v) = target_value(&ev).parse::<u64>() {
                        config.update(|c| if let Some(c) = c { c.proxy.download_queue_poll_interval_secs = v; });
                    }
                }
            />
            <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                "How often the download queue checks for new items. Minimum: 1 second."
            </p>
        </div>

        <div>
            <label>"Authenticator URL"</label>
            <input
                type="text"
                placeholder="https://auth.example.com"
                prop:value=move || get_proxy().authenticator_url.clone().unwrap_or_default()
                on:input=move |ev| {
                    let v = target_value(&ev);
                    config.update(|c| if let Some(c) = c {
                        c.proxy.authenticator_url = if v.trim().is_empty() { None } else { Some(v) };
                    });
                }
            />
            <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                "Authentik instance URL for bearer token validation. When set, all requests require auth. Leave empty to disable."
            </p>
        </div>

        <Show when=move || config.get()
            .and_then(|c| c.proxy.authenticator_url)
            .map(|u| !u.is_empty())
            .unwrap_or(false)>
            <div>
                <label>"Auth Skip Paths"</label>
                <textarea
                    rows="3"
                    placeholder="/health\n/metrics"
                    prop:value=move || config.get()
                        .map(|c| c.proxy.authenticator_skip_paths.join("\n"))
                        .unwrap_or_default()
                    on:input=move |ev| {
                        let v = target_value(&ev);
                        let paths: Vec<String> = v.lines()
                            .map(|l| l.trim().to_string())
                            .filter(|l| !l.is_empty())
                            .collect();
                        config.update(|c| if let Some(c) = c {
                            c.proxy.authenticator_skip_paths = paths;
                        });
                    }
                    class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                />
                <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                    "Paths exempt from authentication, one per line. Default: /health, /metrics"
                </p>
            </div>
        </Show>
    }
}
