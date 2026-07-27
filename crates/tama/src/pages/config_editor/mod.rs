mod types;
pub use types::*;

mod forms;

// ─── Section tabs ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Proxy,
    Lifecycle,
    Sampling,
    Compaction,
    Langfuse,
    // Backup, // Re-enable when BackupForm component is implemented
}

impl Section {
    fn name(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Proxy => "Proxy",
            Section::Lifecycle => "Lifecycle",
            Section::Sampling => "Sampling Templates",
            Section::Compaction => "Compaction",
            Section::Langfuse => "Langfuse",
            // Section::Backup => "Backup & Restore", // Re-enable when BackupForm is implemented
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Section::General => "⚙️",
            Section::Proxy => "🌐",
            Section::Lifecycle => "👀",
            Section::Sampling => "🎲",
            Section::Compaction => "📦",
            Section::Langfuse => "📊",
            // Section::Backup => "💾", // Re-enable when BackupForm is implemented
        }
    }
}

// ─── Main Page ────────────────────────────────────────────────────────────

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::utils::extract_and_store_csrf_token;
use crate::utils::get_request;
use crate::utils::post_request;

use crate::components::section_card::SectionCard;
use crate::pages::config_editor::forms::{
    CompactionForm, GeneralForm, LangfuseForm, LifecycleForm, ProxyAdvancedFields,
    ProxyBasicFields, SamplingForm,
};

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
        // Validate: Langfuse enabled requires credentials
        if cfg.langfuse.enabled
            && (cfg.langfuse.public_key.trim().is_empty()
                || cfg.langfuse.secret_key.trim().is_empty())
        {
            save_status.set(Some(
                "❌ Langfuse is enabled but public/secret key is empty".to_string(),
            ));
            return;
        }
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
                                {[Section::General, Section::Proxy, Section::Lifecycle, Section::Sampling, Section::Compaction, Section::Langfuse]
                                    .into_iter().map(|s| {
                                        let scroll_id = match s {
                                            Section::General => "cfg-general",
                                            Section::Proxy => "cfg-proxy",
                                            Section::Lifecycle => "cfg-lifecycle",
                                            Section::Sampling => "cfg-sampling",
                                            Section::Compaction => "cfg-compaction",
                                            Section::Langfuse => "cfg-langfuse",
                                            // Section::Backup => "cfg-backup", // Re-enable when BackupForm is implemented
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
                            <div id="cfg-proxy">
                                <SectionCard
                                    title="Proxy".to_string()
                                    description=Some("Network and authentication settings for the proxy.".to_string())
                                >
                                    <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                                        <ProxyBasicFields config=config />
                                        <ProxyAdvancedFields config=config />
                                    </div>
                                </SectionCard>
                            </div>
                            <div id="cfg-lifecycle"><LifecycleForm config=config /></div>
                            <div id="cfg-sampling"><SamplingForm config=config /></div>
                            <div id="cfg-compaction"><CompactionForm config=config /></div>
                            <div id="cfg-langfuse"><LangfuseForm config=config /></div>
                        </div>
                    </div>
                }.into_any()
            }
        }}
    }
}
