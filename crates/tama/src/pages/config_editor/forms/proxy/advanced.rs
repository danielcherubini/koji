use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::utils::{get_request, handle_response, target_value};

/// Frontend slice of a tamad connection from `GET /tama/v1/tamads`
/// (deserialize only the fields the pull-host select needs).
#[derive(Debug, Clone, serde::Deserialize)]
struct TamadRef {
    id: String,
    name: String,
}

/// Option entries for the pull-host select: ("", "None") first, then one
/// per tamad labeled "<name> · <short id>".
fn pull_host_options(tamads: &[TamadRef]) -> Vec<(String, String)> {
    let mut out = vec![("".to_string(), "None".to_string())];
    for t in tamads {
        out.push((t.id.clone(), format!("{} · {}", t.name, short_id(&t.id))));
    }
    out
}

/// If `current` is `Some(id)` and absent from `opts` (by value), append an
/// unregistered placeholder option for it; otherwise return `opts` unchanged.
///
/// Without this, a preconfigured `pull_backend` whose id is not in the fetched
/// tamad list (fetch still pending / fetch failed / host gone) has no matching
/// `<option>`: the browser visually selects "None" while the config still holds
/// the host id, and the `prop:value` observer never re-fires to correct it.
/// Label wording: real entries are `<name> · <short id>` — the second token of
/// a real entry is always an id (alnum, or `first4…`), never the word
/// "unregistered", so `"<short id> · unregistered"` cannot be mistaken for a
/// registered tamad.
fn ensure_current_present(
    mut opts: Vec<(String, String)>,
    current: Option<&str>,
) -> Vec<(String, String)> {
    if let Some(id) = current {
        if !id.is_empty() && !opts.iter().any(|(v, _)| v == id) {
            opts.push((id.to_string(), format!("{} · unregistered", short_id(id))));
        }
    }
    opts
}

/// "3f2a9c1b…" → "3f2a…" (first 4 chars + ellipsis); ids ≤ 8 chars pass through.
/// ASSUMPTION: ids from `GET /tama/v1/tamads` are UUIDs (36 ASCII chars),
/// so `&id[..4]` is always char-boundary safe for the values the API emits.
fn short_id(id: &str) -> String {
    if id.len() <= 8 {
        id.to_string()
    } else {
        format!("{}…", &id[..4])
    }
}

/// True only when the tamads fetch resolved to an explicit empty list
/// (still loading or fetch failed → false).
fn no_tamads_resolved(tamads: &LocalResource<Option<Vec<TamadRef>>>) -> bool {
    match tamads.get() {
        Some(wrapper) => matches!(wrapper.as_deref(), Some(t) if t.is_empty()),
        None => false,
    }
}

#[component]
pub fn ProxyAdvancedFields(
    config: RwSignal<Option<crate::types::config::Config>>,
) -> impl IntoView {
    let get_proxy = move || config.get().map(|c| c.proxy).unwrap_or_default();

    // Fetch registered tamads once — options for the Pull host select.
    // POLARITY: `handle_response` returns TRUE when a 401 redirect was
    // triggered (caller must bail) and FALSE for a valid response.
    let tamads = LocalResource::new(|| async move {
        let resp = get_request("/tama/v1/tamads").send().await.ok()?;
        if handle_response(&resp) {
            return None;
        }
        resp.json::<Vec<TamadRef>>().await.ok()
    });

    view! {
        <h3 style="font-size:1rem;font-weight:600;color:var(--text-secondary);margin:1.5rem 0 0.25rem 0;">
            "Tuning"
        </h3>

        <div class="form-group">
            <label class="form-label">"Circuit Breaker Threshold"</label>
            <input
                class="form-input"
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

        <div class="form-group">
            <label class="form-label">"Circuit Breaker Cooldown (seconds)"</label>
            <input
                class="form-input"
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

        <div class="form-group">
            <label class="form-label">"Metrics Retention (seconds)"</label>
            <input
                class="form-input"
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

        <div class="form-group">
            <label class="form-label">"Max Loaded Models (per GPU)"</label>
            <input
                class="form-input"
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

        <div class="form-group">
            <label class="form-label">"Download Queue Poll Interval (seconds)"</label>
            <input
                class="form-input"
                type="number"
                min="1"
                prop:value=move || get_proxy().pull_queue_poll_interval_secs.to_string()
                on:input=move |ev| {
                    if let Ok(v) = target_value(&ev).parse::<u64>() {
                        config.update(|c| if let Some(c) = c { c.proxy.pull_queue_poll_interval_secs = v; });
                    }
                }
            />
            <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                "How often the pull queue checks for new items. Minimum: 1 second."
            </p>
        </div>

        <div class="form-group">
            <label class="form-label">"Pull Host"</label>
            <select
                class="form-input"
                prop:value=move || get_proxy().pull_backend.clone().unwrap_or_default()
                on:change=move |ev| {
                    let v = target_value(&ev);
                    config.update(|c| if let Some(c) = c {
                        c.proxy.pull_backend = if v.is_empty() { None } else { Some(v) };
                    });
                }
            >
                {move || {
                    let opts = pull_host_options(&[]);
                    let opts = if let Some(wrapper) = tamads.get() {
                        if let Some(tamads) = &*wrapper {
                            pull_host_options(tamads)
                        } else {
                            opts
                        }
                    } else {
                        opts
                    };
                    // Ensure a preconfigured (or persisting) pull host that is
                    // missing from the tamad list stays selectable — see
                    // `ensure_current_present`. Reading `config` here makes the
                    // option list (re)render when the configured id changes.
                    let pull_backend = config.get().and_then(|c| c.proxy.pull_backend);
                    let current = pull_backend.as_deref().unwrap_or_default();
                    ensure_current_present(opts, Some(current))
                        .into_iter()
                        .map(|(val, label)| view! { <option value={val}>{label}</option> })
                        .collect::<Vec<_>>()
                }}
            </select>
            <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                "Select the tamad that executes queued model pulls. The proxy itself never downloads (ADR-0010)."
            </p>
            <Show when=move || no_tamads_resolved(&tamads)>
                <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                    "No tamads registered — register one first (see docs/api/tamads.md). Pulls will fail with 'no pull host configured' until one is set."
                </p>
            </Show>
        </div>

        // ─── Security / Authentication ────────────────────────────────────
        <h3 style="font-size:1rem;font-weight:600;color:var(--text-secondary);margin:1.5rem 0 0.25rem 0;">
            "Authentication"
        </h3>

        <div class="form-group">
            <label class="form-label">"Authenticator URL"</label>
            <input
                class="form-input"
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
            <div class="form-group">
                <label class="form-label">"Auth Skip Paths"</label>
                <textarea
                    class="form-textarea"
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
                />
                <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                    "Paths exempt from authentication, one per line. Default: /health, /metrics"
                </p>
            </div>
        </Show>

        <div class="form-group">
            <label class="checkbox-label">
                <input
                    type="checkbox"
                    prop:checked=move || get_proxy().api_keys_enabled
                    on:change=move |ev| {
                        let checked = ev.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|i| i.checked())
                            .unwrap_or(false);
                        config.update(|c| if let Some(c) = c { c.proxy.api_keys_enabled = checked; });
                    }
                />
                "Enable API key authentication"
            </label>
            <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                "When enabled, bearer tokens starting with " <code>"tama_"</code> " are validated against the database. "
                "Manage keys on the " <a href="/tama/keys">"API Keys"</a> " page. "
                "This flag is auto-set to true when you create your first key and false when the last key is revoked."
            </p>
        </div>

        <div class="form-group">
            <label class="checkbox-label">
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
                "Enable OAuth2/OIDC login"
            </label>
            <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
                "When enabled, browser sessions are authenticated via your OAuth2 provider (e.g. Authentik). "
                "Browser requests without a valid session are redirected to the login flow."
            </p>
        </div>

        <Show when=move || get_proxy().oauth2.enabled>
            <fieldset class="form-subsection">
                <legend>"OAuth2/OIDC Provider"</legend>

                <div class="form-group">
                    <label class="form-label">"Client ID"</label>
                    <input
                        class="form-input"
                        type="text"
                        prop:value=move || get_proxy().oauth2.client_id.clone()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.client_id = v; });
                        }
                    />
                </div>

                <div class="form-group">
                    <label class="form-label">"Client Secret"</label>
                    <input
                        class="form-input"
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

                <div class="form-group">
                    <label class="form-label">"Authorize URL"</label>
                    <input
                        class="form-input"
                        type="text"
                        placeholder="https://auth.example.com/application/o/authorize/"
                        prop:value=move || get_proxy().oauth2.authorize_url.clone()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.authorize_url = v; });
                        }
                    />
                </div>

                <div class="form-group">
                    <label class="form-label">"Token URL"</label>
                    <input
                        class="form-input"
                        type="text"
                        placeholder="https://auth.example.com/application/o/token/"
                        prop:value=move || get_proxy().oauth2.token_url.clone()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.token_url = v; });
                        }
                    />
                </div>

                <div class="form-group">
                    <label class="form-label">"Userinfo URL (optional)"</label>
                    <input
                        class="form-input"
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

                <div class="form-group">
                    <label class="form-label">"Logout URL (optional)"</label>
                    <input
                        class="form-input"
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

                <div class="form-group">
                    <label class="form-label">"Redirect URI"</label>
                    <input
                        class="form-input"
                        type="text"
                        placeholder="http://localhost:11434/login/callback"
                        prop:value=move || get_proxy().oauth2.redirect_uri.clone()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.oauth2.redirect_uri = v; });
                        }
                    />
                </div>

                <div class="form-group">
                    <label class="form-label">"Scopes (comma-separated)"</label>
                    <input
                        class="form-input"
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

                <div class="form-group">
                    <label class="form-label">"Session TTL (seconds)"</label>
                    <input
                        class="form-input"
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
            </fieldset>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty tamad list → exactly one option: ("", "None").
    #[test]
    fn test_pull_host_options_empty_list_has_none_only() {
        let opts = pull_host_options(&[]);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0], ("".to_string(), "None".to_string()));
    }

    /// Each tamad gets one option labeled "<name> · <short id>".
    #[test]
    fn test_pull_host_options_labels_name_and_short_id() {
        let tamads = vec![TamadRef {
            id: "3f2a9c1b-0000-0000-0000-000000000000".to_string(),
            name: "gpu-box".to_string(),
        }];
        let opts = pull_host_options(&tamads);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0], ("".to_string(), "None".to_string()));
        assert_eq!(
            opts[1],
            (
                "3f2a9c1b-0000-0000-0000-000000000000".to_string(),
                "gpu-box · 3f2a…".to_string()
            )
        );
    }

    /// ids ≤ 8 chars pass through unchanged; longer ids become "first4…".
    #[test]
    fn test_short_id_passthrough_under_8_chars() {
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id("abcdefg"), "abcdefg");
        assert_eq!(short_id("abcdefgh"), "abcdefgh");
        assert_eq!(short_id("aabbccdde"), "aabb…");
    }

    /// A configured id missing from the options gets an explicit
    /// "unregistered" placeholder so the selected value always exists
    /// among the rendered <option>s.
    #[test]
    fn test_ensure_current_present_appends_unregistered_option() {
        let tamads = vec![TamadRef {
            id: "11111111-0000-0000-0000-000000000000".to_string(),
            name: "gpu-box".to_string(),
        }];
        let opts = pull_host_options(&tamads);
        let out =
            ensure_current_present(opts, Some("3f2a9c1b-deadbeef-0000-0000-0000-deadbeef0000"));
        assert_eq!(out.len(), 3);
        // "None" stays first; the placeholder is appended last.
        assert_eq!(out[0], ("".to_string(), "None".to_string()));
        assert_eq!(
            out[2].0,
            "3f2a9c1b-deadbeef-0000-0000-0000-deadbeef0000".to_string()
        );
        assert!(
            out[2].1.contains("unregistered"),
            "label should mention 'unregistered': {}",
            out[2].1
        );
    }

    /// If the current id is already an option, nothing is appended.
    #[test]
    fn test_ensure_current_present_absent_from_opts_left_unchanged() {
        let tamads = vec![TamadRef {
            id: "11111111-0000-0000-0000-000000000000".to_string(),
            name: "gpu-box".to_string(),
        }];
        let opts = pull_host_options(&tamads);
        let before = opts.clone();
        let out = ensure_current_present(opts, Some("11111111-0000-0000-0000-000000000000"));
        assert_eq!(before.len(), out.len());
        assert_eq!(before, out);
    }

    /// current == None leaves the options untouched.
    #[test]
    fn test_ensure_current_present_none_unchanged() {
        let tamads = vec![TamadRef {
            id: "11111111-0000-0000-0000-000000000000".to_string(),
            name: "gpu-box".to_string(),
        }];
        let opts = pull_host_options(&tamads);
        let before = opts.clone();
        let out = ensure_current_present(opts, None);
        assert_eq!(before, out);
    }

    /// current == Some("") is the "None" slot — leave the options untouched.
    #[test]
    fn test_ensure_current_present_empty_id_unchanged() {
        let tamads = vec![TamadRef {
            id: "11111111-0000-0000-0000-000000000000".to_string(),
            name: "gpu-box".to_string(),
        }];
        let opts = pull_host_options(&tamads);
        let before = opts.clone();
        let out = ensure_current_present(opts, Some(""));
        assert_eq!(before, out);
    }
}
