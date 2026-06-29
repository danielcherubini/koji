use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::components::section_card::SectionCard;
use crate::utils::{extract_and_store_csrf_token, get_request, post_request};

// ─── WASM-safe JSON mirror types ──────────────────────────────────────────
// These match the shape served by /api/config/structured and accepted by POST.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub backends: BTreeMap<String, BackendConfig>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelConfig>,
    #[serde(default)]
    pub supervisor: Supervisor,
    #[serde(default)]
    pub sampling_templates: BTreeMap<String, SamplingParams>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct General {
    #[serde(default)]
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    #[serde(default)]
    pub update_check_interval: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendConfig {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub health_check_url: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sampling: Option<SamplingParams>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_layers: Option<u32>,
    /// Forward-compat: preserve any additional fields we don't know about
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Supervisor {
    #[serde(default)]
    pub restart_policy: String,
    #[serde(default)]
    pub max_restarts: u32,
    #[serde(default)]
    pub restart_delay_ms: u64,
    #[serde(default)]
    pub health_check_interval_ms: u64,
    #[serde(default)]
    pub health_check_timeout_ms: u64,
    #[serde(default)]
    pub health_check_retries: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub auto_unload: bool,
    #[serde(default)]
    pub idle_timeout_secs: u64,
    #[serde(default)]
    pub startup_timeout_secs: u64,
    #[serde(default)]
    pub circuit_breaker_threshold: u32,
    #[serde(default)]
    pub circuit_breaker_cooldown_seconds: u64,
    #[serde(default)]
    pub metrics_retention_secs: u64,
    #[serde(default)]
    pub max_loaded_models: u32,
    #[serde(default)]
    pub download_queue_poll_interval_secs: u64,
    #[serde(default)]
    pub authenticator_url: Option<String>,
    #[serde(default)]
    pub authenticator_skip_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server_path: Option<String>,
    #[serde(default = "default_compaction_device")]
    pub device: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "default_compaction_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

fn default_compaction_device() -> String {
    "cpu".to_string()
}

fn default_compaction_request_timeout_ms() -> u64 {
    30_000
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SamplingParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
}

// ─── Section tabs ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Proxy,
    Supervisor,
    Sampling,
    Compaction,
    // Backup, // TODO: Fix compilation
}

impl Section {
    fn name(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Proxy => "Proxy",
            Section::Supervisor => "Supervisor",
            Section::Sampling => "Sampling Templates",
            Section::Compaction => "Compaction",
            // Section::Backup => "Backup & Restore", // TODO: Fix compilation
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Section::General => "⚙️",
            Section::Proxy => "🌐",
            Section::Supervisor => "👀",
            Section::Sampling => "🎲",
            Section::Compaction => "📦",
            // Section::Backup => "💾", // TODO: Fix compilation
        }
    }
}

// ─── Main Page ────────────────────────────────────────────────────────────

#[component]
pub fn ConfigEditor() -> impl IntoView {
    let current = RwSignal::new(Section::General);
    let config: RwSignal<Option<Config>> = RwSignal::new(None);
    let loading = RwSignal::new(true);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let save_status: RwSignal<Option<String>> = RwSignal::new(None);

    // Initial fetch
    Effect::new(move |_| {
        spawn_local(async move {
            loading.set(true);
            error.set(None);
            match get_request("/tama/v1/config/structured").send().await {
                Ok(resp) => {
                    // Store CSRF token from response header (fallback when cookie unavailable)
                    extract_and_store_csrf_token(&resp);
                    match resp.json::<Config>().await {
                        Ok(cfg) => config.set(Some(cfg)),
                        Err(e) => error.set(Some(format!("Failed to parse config: {}", e))),
                    }
                }
                Err(e) => error.set(Some(format!("Failed to fetch config: {}", e))),
            }
            loading.set(false);
        });
    });

    let save = move |_| {
        let Some(cfg) = config.get() else {
            return;
        };
        save_status.set(Some("Saving…".to_string()));
        spawn_local(async move {
            let body = match serde_json::to_string(&cfg) {
                Ok(s) => s,
                Err(e) => {
                    save_status.set(Some(format!("Serialize error: {}", e)));
                    return;
                }
            };
            let res = post_request("/tama/v1/config/structured")
                .header("Content-Type", "application/json")
                .body(body)
                .expect("failed to build request")
                .send()
                .await;
            match res {
                Ok(resp) if resp.ok() => {
                    save_status.set(Some("✅ Saved".to_string()));
                }
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    save_status.set(Some(format!("❌ {} — {}", status, text)));
                }
                Err(e) => {
                    save_status.set(Some(format!("❌ {}", e)));
                }
            }
        });
    };

    view! {
        <div class="page-header">
            <h1>"Configuration"</h1>
            <div style="display:flex;gap:0.5rem;align-items:center;">
                {move || save_status.get().map(|s| view! { <span class="text-muted">{s}</span> })}
                <button class="btn btn-primary" on:click=save>"Save Changes"</button>
            </div>
        </div>

        {move || {
            if loading.get() {
                view! { <div class="card card--centered"><span class="spinner">"Loading config..."</span></div> }.into_any()
            } else if let Some(err) = error.get() {
                view! { <div class="card"><p class="text-error">{err}</p></div> }.into_any()
            } else if config.get().is_none() {
                view! { <div class="card"><p>"No config data"</p></div> }.into_any()
            } else {
                view! {
                    <div style="display:flex;gap:1.5rem;align-items:flex-start;">
                        // Side nav
                        <nav class="card" style="width:220px;flex-shrink:0;padding:0.75rem;position:sticky;top:0;">
                            <ul style="list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:0.25rem;">
                                {[Section::General, Section::Proxy, Section::Supervisor, Section::Sampling, Section::Compaction]
                                    .into_iter().map(|s| {
                                        let scroll_id = match s {
                                            Section::General => "cfg-general",
                                            Section::Proxy => "cfg-proxy",
                                            Section::Supervisor => "cfg-supervisor",
                                            Section::Sampling => "cfg-sampling",
                                            Section::Compaction => "cfg-compaction",
                                            // Section::Backup => "cfg-backup", // TODO: Fix compilation
                                        };
                                        let active = move || current.get() == s;
                                        view! {
                                            <li>
                                                <button
                                                    class:btn=true
                                                    class:btn-primary=active
                                                    class:btn-secondary=move || !active()
                                                    style="width:100%;text-align:left;display:flex;gap:0.5rem;align-items:center;"
                                                    on:click=move |_| {
                                                        current.set(s);
                                                        if let Some(el) = web_sys::window()
                                                            .and_then(|w| w.document())
                                                            .and_then(|d| d.get_element_by_id(scroll_id))
                                                        {
                                                            el.scroll_into_view_with_bool(true);
                                                        }
                                                    }
                                                >
                                                    <span>{s.icon()}</span>
                                                    <span>{s.name()}</span>
                                                </button>
                                            </li>
                                        }
                                    }).collect::<Vec<_>>()}
                            </ul>
                        </nav>

                        // Main form area — all sections visible, stacked
                        <div style="flex:1;min-width:0;display:flex;flex-direction:column;gap:1rem;">
                            <div id="cfg-general"><GeneralForm config=config /></div>
                            <div id="cfg-proxy"><ProxyForm config=config /></div>
                            <div id="cfg-supervisor"><SupervisorForm config=config /></div>
                            <div id="cfg-sampling"><SamplingForm config=config /></div>
                            <div id="cfg-compaction"><CompactionForm config=config /></div>
                        </div>
                    </div>
                }.into_any()
            }
        }}
    }
}

// ─── Helper: get event.target.value as String ─────────────────────────────
use crate::utils::target_value;

// ─── General Form ─────────────────────────────────────────────────────────

#[component]
fn GeneralForm(config: RwSignal<Option<Config>>) -> impl IntoView {
    let get_general = move || config.get().map(|c| c.general).unwrap_or_default();

    view! {
        <SectionCard title="General Settings".to_string() description=Some("Global Tama settings.".to_string())>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Log Level"</label>
                    <select
                        on:change=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.general.log_level = v; });
                        }
                        prop:value=move || get_general().log_level
                    >
                        <option value="trace">"trace"</option>
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

// ─── Proxy Form ───────────────────────────────────────────────────────────

#[component]
fn ProxyForm(config: RwSignal<Option<Config>>) -> impl IntoView {
    let get_proxy = move || config.get().map(|c| c.proxy).unwrap_or_default();

    view! {
        <SectionCard title="Proxy Settings".to_string() description=Some("Configure the proxy server that routes OpenAI/Ollama-compatible requests.".to_string())>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Host"</label>
                    <input
                        type="text"
                        prop:value=move || get_proxy().host
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.host = v; });
                        }
                    />
                </div>

                <div>
                    <label>"Port"</label>
                    <input
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

                <div>
                    <label class="checkbox-label">
                        <input
                            type="checkbox"
                            prop:checked=move || get_proxy().auto_unload
                            on:change=move |ev| {
                                use wasm_bindgen::JsCast;
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

                <div>
                    <label>"Idle Timeout (seconds)"</label>
                    <input
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

                <div>
                    <label>"Startup Timeout (seconds)"</label>
                    <input
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
            </div>
        </SectionCard>
    }
}

// ─── Supervisor Form ──────────────────────────────────────────────────────

#[component]
fn SupervisorForm(config: RwSignal<Option<Config>>) -> impl IntoView {
    let get_sup = move || config.get().map(|c| c.supervisor).unwrap_or_default();

    view! {
        <SectionCard title="Supervisor".to_string() description=Some("Process restart and health-check behavior for managed models.".to_string())>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Restart Policy"</label>
                    <select
                        prop:value=move || get_sup().restart_policy
                        on:change=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.supervisor.restart_policy = v; });
                        }
                    >
                        <option value="always">"always"</option>
                        <option value="on-failure">"on-failure"</option>
                        <option value="never">"never"</option>
                    </select>
                </div>

                <div>
                    <label>"Max Restarts"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().max_restarts.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.max_restarts = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Restart Delay (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().restart_delay_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.restart_delay_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Interval (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().health_check_interval_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.health_check_interval_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Timeout (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().health_check_timeout_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.health_check_timeout_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Retries"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().health_check_retries.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.health_check_retries = v; });
                            }
                        }
                    />
                </div>
            </div>
        </SectionCard>
    }
}

// ─── Sampling Templates Form ──────────────────────────────────────────────

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

#[component]
fn SamplingForm(config: RwSignal<Option<Config>>) -> impl IntoView {
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

// ─── Compaction Form ──────────────────────────────────────────────────────

#[component]
fn CompactionForm(config: RwSignal<Option<Config>>) -> impl IntoView {
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
                                use wasm_bindgen::JsCast;
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
                            config.update(|c| if let Some(c) = c { c.compaction.device = v; });
                        }
                        prop:value=move || get_compaction().device
                    >
                        <option value="cpu">"cpu"</option>
                        <option value="cuda">"cuda"</option>
                        <option value="cuda:0">"cuda:0"</option>
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
