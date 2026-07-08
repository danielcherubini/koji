use leptos::prelude::*;
use wasm_bindgen::JsCast;

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
                "How often the pull queue checks for new items. Minimum: 1 second."
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

        <div>
            <label>"OAuth2 Login Enabled"</label>
            <input
                type="checkbox"
                prop:checked=move || get_proxy().oauth2.enabled
                on:change=move |ev| {
                    let checked = ev.target()
                        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                        .map(|i| i.checked())
                        .unwrap_or(false);
                    config.update(|c| if let Some(c) = c { c.proxy.oauth2.enabled = checked; });
                }
            />
        </div>

        <Show when=move || get_proxy().oauth2.enabled>
            <div
                class="oauth2-config"
                style="border:1px solid #ddd;padding:1rem;margin-top:0.5rem;border-radius:0.5rem;"
            >
                <h3>"OAuth2/OIDC Provider Configuration"</h3>

                <div>
                    <label>"Client ID"</label>
                    <input
                        type="text"
                        prop:value=move || get_proxy().oauth2.client_id.clone()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.client_id = v; });
                        }
                    />
                </div>

                <div>
                    <label>"Client Secret"</label>
                    <input
                        type="password"
                        prop:value=move || get_proxy().oauth2.client_secret.clone()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.client_secret = v; });
                        }
                    />
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "Supports ${ENV_VAR} syntax for environment variable references."
                    </p>
                </div>

                <div>
                    <label>"Authorize URL"</label>
                    <input
                        type="text"
                        placeholder="https://auth.example.com/application/o/authorize/"
                        prop:value=move || get_proxy().oauth2.authorize_url.clone()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.authorize_url = v; });
                        }
                    />
                </div>

                <div>
                    <label>"Token URL"</label>
                    <input
                        type="text"
                        placeholder="https://auth.example.com/application/o/token/"
                        prop:value=move || get_proxy().oauth2.token_url.clone()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.token_url = v; });
                        }
                    />
                </div>

                <div>
                    <label>"Userinfo URL (optional)"</label>
                    <input
                        type="text"
                        placeholder="https://auth.example.com/application/o/userinfo/"
                        prop:value=move || get_proxy().oauth2.userinfo_url.clone().unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c {
                                c.proxy.oauth2.userinfo_url = if v.trim().is_empty() { None } else { Some(v) };
                            });
                        }
                    />
                </div>

                <div>
                    <label>"Logout URL (optional)"</label>
                    <input
                        type="text"
                        placeholder="https://auth.example.com/application/o/app-slug/end-session/"
                        prop:value=move || get_proxy().oauth2.logout_url.clone().unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c {
                                c.proxy.oauth2.logout_url = if v.trim().is_empty() { None } else { Some(v) };
                            });
                        }
                    />
                </div>

                <div>
                    <label>"Redirect URI"</label>
                    <input
                        type="text"
                        placeholder="http://localhost:11434/login/callback"
                        prop:value=move || get_proxy().oauth2.redirect_uri.clone()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.redirect_uri = v; });
                        }
                    />
                </div>

                <div>
                    <label>"Scopes (comma-separated)"</label>
                    <input
                        type="text"
                        placeholder="openid,profile,email"
                        prop:value=move || get_proxy().oauth2.scopes.join(", ")
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            let scopes: Vec<String> = v
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.scopes = scopes; });
                        }
                    />
                </div>

                <div>
                    <label>"Session TTL (seconds)"</label>
                    <input
                        type="number"
                        min="300"
                        prop:value=move || get_proxy().oauth2.session_ttl_secs.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.proxy.oauth2.session_ttl_secs = v; });
                            }
                        }
                    />
                    <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                        "How long a login session lasts. Default: 86400 (24 hours)."
                    </p>
                </div>
            </div>
        </Show>
    }
}
