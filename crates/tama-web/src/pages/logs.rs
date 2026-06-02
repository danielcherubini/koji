use gloo_net::http::Request;
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::utils::extract_and_store_csrf_token;

/// Response from GET /tama/v1/logs — grouped by source.
#[derive(Debug, Clone, Deserialize)]
struct AllLogsResponse {
    sources: Vec<SourceLogs>,
}

/// Logs for a single source (e.g. "tama", "llama_cpp_1").
#[derive(Debug, Clone, Deserialize)]
struct SourceLogs {
    name: String,
    lines: Vec<String>,
}

/// Classify a log line and return the CSS modifier class suffix.
fn log_level_class(line: &str) -> &'static str {
    let lower = line.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("fatal") {
        "log-line--error"
    } else if lower.contains("warn") {
        "log-line--warn"
    } else if lower.contains("debug") {
        "log-line--debug"
    } else {
        "log-line--info"
    }
}

/// Update the ?source= query param in the URL without triggering a full navigation.
fn set_source_in_url(source: &str) {
    if let Some(window) = web_sys::window() {
        let mut url = url::Url::parse(window.location().href().unwrap().as_str()).unwrap();
        url.query_pairs_mut().clear().append_pair("source", source);
        let new_href = url.to_string();
        window
            .history()
            .unwrap()
            .push_state_with_url(&js_sys::Object::new(), "", Some(&new_href))
            .ok();
    }
}

#[component]
pub fn Logs() -> impl IntoView {
    // Reactive query params from leptos_router — updates on SPA navigation
    let query = use_query_map();
    let selected_source = move || {
        query
            .with(|q| q.get("source"))
            .unwrap_or_else(|| "tama".to_string())
    };

    // Log data grouped by source
    let sources = RwSignal::new(Vec::<SourceLogs>::new());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load all logs from the API
    let load_logs = move || {
        spawn_local(async move {
            loading.set(true);
            error.set(None);

            match Request::get("/tama/v1/logs").send().await {
                Ok(resp) => {
                    extract_and_store_csrf_token(&resp);
                    let status = resp.status();
                    if (200..300).contains(&status) {
                        match resp.text().await {
                            Ok(text) => match serde_json::from_str::<AllLogsResponse>(&text) {
                                Ok(data) => sources.set(data.sources),
                                Err(e) => error.set(Some(format!(
                                    "Parse error: {e} (body len={})",
                                    text.len()
                                ))),
                            },
                            Err(e) => error.set(Some(format!("Failed to read body: {e}"))),
                        }
                    } else {
                        error.set(Some(format!(
                            "HTTP {} — logs_dir may not be configured",
                            resp.status()
                        )));
                        sources.set(Vec::new());
                    }
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load logs: {e}")));
                    sources.set(Vec::new());
                }
            }
            loading.set(false);
        });
    };

    // Set default source in URL if not present, then load logs
    Effect::new(move |_| {
        if query.with(|q| q.get("source").is_none()) {
            set_source_in_url("tama");
        }
        load_logs();
        spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(5_000).await;
                load_logs();
            }
        });
    });

    view! {
        <div class="page-header">
            <h1>"Log Viewer"</h1>
            <div class="log-toolbar">
                <select
                    class="form-select form-select-sm"
                    on:change=move |e| {
                        let val = e
                            .target()
                            .unwrap()
                            .dyn_into::<web_sys::HtmlSelectElement>()
                            .unwrap();
                        set_source_in_url(&val.value());
                    }
                >
                    <option
                        value="tama"
                        selected=move || selected_source() == "tama"
                    >
                        "tama"
                    </option>
                    {move || {
                        sources.get().into_iter().filter(|s| s.name != "tama").map(|s| {
                            let val = s.name.clone();
                            let sel = val.clone();
                            view! {
                                <option
                                    value=val
                                    selected=move || selected_source() == sel
                                >
                                    {sel.clone()}
                                </option>
                            }.into_any()
                        }).collect::<Vec<_>>()
                    }}
                </select>
                <button
                    class="btn btn-secondary btn-sm"
                    prop:disabled=loading.get()
                    on:click=move |_| { load_logs(); }
                >
                    "↻ Refresh"
                </button>
            </div>
        </div>

        // Loading state
        {move || {
            let all_sources = sources.get();
            let err = error.get();
            let is_loading = loading.get();
            if is_loading && all_sources.is_empty() {
                view! {
                    <div class="spinner-container mt-4">
                        <span class="spinner"></span>
                        <span class="text-muted">"Loading logs..."</span>
                    </div>
                }.into_any()
            } else if let Some(e) = err {
                view! {
                    <div class="alert alert--warning mt-2">
                        <span class="alert__icon">"⚠"</span>
                        <span>{e}</span>
                    </div>
                }.into_any()
            } else if all_sources.is_empty() {
                view! {
                    <div class="alert alert--info mt-2">
                        <span class="alert__icon">"ℹ"</span>
                        <span>"No logs yet. Logs will appear here after backend processes are started."</span>
                    </div>
                }.into_any()
            } else {
                let selected = selected_source();
                view! {
                    <div class="log-viewer card">
                        {all_sources.into_iter().filter(move |s| s.name == selected).flat_map(|source| {
                            source.lines.into_iter().map(|line| {
                                let cls = format!("log-line {}", log_level_class(&line));
                                view! { <div class=cls>{line}</div> }
                            }).collect::<Vec<_>>()
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }
        }}
    }
}
