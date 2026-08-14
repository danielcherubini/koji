//! Providers tab content

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use urlencoding::encode as url_encode;
use wasm_bindgen::JsCast;

use crate::components::alert_banner::{AlertBanner, AlertVariant};
use crate::components::docker_register_modal::DockerRegisterModal;
use crate::components::install_modal::{CapabilitiesDto, InstallModal, InstallRequest};
use crate::components::installation_card::{InstallationCard, InstallationCardDto};
use crate::components::job_log_panel::JobLogPanel;
use crate::pages::models::tab::{Tab, TabPills};
use crate::utils::{delete_request, get_request, handle_response, post_request};

/// Construct a URL path for updating a backend, properly encoding the backend name.
fn backend_update_url(backend_name: &str, gpu_variant: &str) -> String {
    let encoded_name = url_encode(backend_name);
    let encoded_variant = url_encode(gpu_variant);
    format!("/tama/v1/backends/{encoded_name}/update?gpu_variant={encoded_variant}")
}

/// Construct a check-updates URL for a backend, properly encoding the backend name.
fn backend_check_updates_url(backend_name: &str, gpu_variant: &str) -> String {
    let encoded_name = url_encode(backend_name);
    let encoded_variant = url_encode(gpu_variant);
    format!("/tama/v1/updates/check/backend/{encoded_name}?gpu_variant={encoded_variant}")
}

/// Construct a build-method source URL for a backend, properly encoding the backend name.
fn backend_source_url(backend_name: &str, gpu_variant: &str) -> String {
    let encoded_name = url_encode(backend_name);
    let encoded_variant = url_encode(gpu_variant);
    format!("/tama/v1/backends/{encoded_name}/source?gpu_variant={encoded_variant}")
}

/// Construct a URL path for backend delete, properly encoding the backend name.
fn backend_delete_url(backend_name: &str, gpu_variant: &str) -> String {
    let encoded_name = url_encode(backend_name);
    let encoded_variant = url_encode(gpu_variant);
    format!("/tama/v1/backends/{encoded_name}?gpu_variant={encoded_variant}")
}

/// Parse newline-separated text into a Vec<String>, trimming whitespace and filtering empty lines.
pub fn parse_newline_separated(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let t = l.trim();
            (!t.is_empty()).then(|| t.to_string())
        })
        .collect()
}

#[cfg(test)]
mod backend_url_tests {
    use super::{
        backend_check_updates_url, backend_delete_url, backend_source_url, backend_update_url,
    };

    #[test]
    fn test_backend_update_url_encodes_name() {
        // Native backend: name == type, no special chars
        let url = backend_update_url("llama_cpp", "cpu");
        assert_eq!(url, "/tama/v1/backends/llama_cpp/update?gpu_variant=cpu");
    }

    #[test]
    fn test_backend_update_url_encodes_special_chars() {
        // Docker backend with special characters in name
        let url = backend_update_url("my-vllm:latest", "cpu");
        assert_eq!(
            url,
            "/tama/v1/backends/my-vllm%3Alatest/update?gpu_variant=cpu"
        );
    }

    #[test]
    fn test_backend_check_updates_url_encodes_name() {
        let url = backend_check_updates_url("vllm", "cuda_12");
        assert_eq!(
            url,
            "/tama/v1/updates/check/backend/vllm?gpu_variant=cuda_12"
        );
    }

    #[test]
    fn test_backend_source_url_encodes_name() {
        let url = backend_source_url("docker_vllm", "cpu");
        assert_eq!(url, "/tama/v1/backends/docker_vllm/source?gpu_variant=cpu");
    }

    #[test]
    fn test_backend_delete_url() {
        let url = backend_delete_url("vllm", "cpu");
        assert_eq!(url, "/tama/v1/backends/vllm?gpu_variant=cpu");
    }

    #[test]
    fn test_backend_delete_url_encodes_name() {
        let url = backend_delete_url("my-vllm:latest", "cuda_12");
        assert_eq!(
            url,
            "/tama/v1/backends/my-vllm%3Alatest?gpu_variant=cuda_12"
        );
    }
}

#[cfg(test)]
mod newline_parsing_tests {
    use super::parse_newline_separated;

    #[test]
    fn test_parse_single_line() {
        assert_eq!(parse_newline_separated("--threads 4"), vec!["--threads 4"]);
    }

    #[test]
    fn test_parse_multiple_lines() {
        let input = "--max-num-seqs 4\n--enable-prefix-caching\n--gpu-layers 32";
        assert_eq!(
            parse_newline_separated(input),
            vec![
                "--max-num-seqs 4",
                "--enable-prefix-caching",
                "--gpu-layers 32"
            ]
        );
    }

    #[test]
    fn test_parse_empty_string() {
        assert!(parse_newline_separated("").is_empty());
    }

    #[test]
    fn test_parse_whitespace_only_lines() {
        let input = "   \n\n  \t  ";
        assert!(parse_newline_separated(input).is_empty());
    }

    #[test]
    fn test_parse_mixed_empty_and_content() {
        let input = "--threads 4\n\n--gpu-layers 32\n   \n";
        assert_eq!(
            parse_newline_separated(input),
            vec!["--threads 4", "--gpu-layers 32"]
        );
    }

    #[test]
    fn test_parse_trims_whitespace() {
        let input = "  --threads 4  \n  --gpu-layers 32  ";
        assert_eq!(
            parse_newline_separated(input),
            vec!["--threads 4", "--gpu-layers 32"]
        );
    }

    #[test]
    fn test_parse_env_vars() {
        let input = "RADV_PERFTEST=nogttspill\nFOO=bar\nOTHER_VAR=123";
        assert_eq!(
            parse_newline_separated(input),
            vec!["RADV_PERFTEST=nogttspill", "FOO=bar", "OTHER_VAR=123"]
        );
    }

    #[test]
    fn test_parse_env_vars_with_empty_lines() {
        let input = "RADV_PERFTEST=nogttspill\n\nFOO=bar\n";
        assert_eq!(
            parse_newline_separated(input),
            vec!["RADV_PERFTEST=nogttspill", "FOO=bar"]
        );
    }
}

/// Compaction backend card DTO (mirrors the SSR-side type).
/// Defined here because `crate::api` is gated behind `ssr` feature.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
struct CompactionCardDto {
    enabled: bool,
    device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server_url: Option<String>,
    #[allow(dead_code)] // Deserialized from API but not displayed in UI
    #[serde(default)]
    request_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct BackendListResponse {
    #[serde(default)]
    backends: Vec<InstallationCardDto>,
    #[serde(default)]
    custom: Vec<InstallationCardDto>,
    #[serde(default)]
    docker: Vec<InstallationCardDto>,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API but not used by page
    available: Vec<String>,
    #[serde(default)]
    compaction: CompactionCardDto,
}

#[derive(Debug, Clone, Serialize)]
struct CompactionToggleRequest {
    enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct InstallResponse {
    job_id: String,
}

/// Providers tab content, rendered inside the Models page.
#[component]
pub fn ProvidersTab(active_tab: RwSignal<Tab>) -> impl IntoView {
    // ── State ────────────────────────────────────────────────────────────────
    let backends_list = RwSignal::new(BackendListResponse::default());
    let capabilities = RwSignal::new(CapabilitiesDto::default());
    let install_modal_for = RwSignal::new(Option::<String>::None);
    let docker_register_open = RwSignal::new(false);
    let active_job_id = RwSignal::new(Option::<String>::None);
    let action_error = RwSignal::new(Option::<String>::None);
    let refresh_tick = RwSignal::new(0u32);
    let default_args_edits: RwSignal<std::collections::HashMap<String, String>> =
        RwSignal::new(std::collections::HashMap::new());
    let default_env_edits: RwSignal<std::collections::HashMap<String, String>> =
        RwSignal::new(std::collections::HashMap::new());
    let save_status: RwSignal<Option<String>> = RwSignal::new(None);
    let saving: RwSignal<bool> = RwSignal::new(false);
    let show_backend_dropdown = RwSignal::new(false);

    // ── Fetch backends list (re-runs on refresh_tick) ────────────────────────
    Effect::new(move |_| {
        let _ = refresh_tick.get();
        wasm_bindgen_futures::spawn_local(async move {
            match get_request("/tama/v1/backends").send().await {
                Ok(resp) => {
                    if handle_response(&resp) {
                        return;
                    }
                    if let Ok(list) = resp.json::<BackendListResponse>().await {
                        backends_list.set(list);
                    }
                }
                Err(e) => leptos::logging::warn!("Failed to fetch backends: {e:?}"),
            }
        });
    });

    // ── Fetch capabilities once ──────────────────────────────────────────────
    Effect::new(move |prev: Option<()>| {
        if prev.is_some() {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match get_request("/tama/v1/system/capabilities").send().await {
                Ok(resp) => {
                    if handle_response(&resp) {
                        return;
                    }
                    if let Ok(caps) = resp.json::<CapabilitiesDto>().await {
                        capabilities.set(caps);
                    }
                }
                Err(e) => leptos::logging::warn!("Failed to fetch capabilities: {e:?}"),
            }
        });
    });

    // ── Callbacks ────────────────────────────────────────────────────────────
    let on_install_click = Callback::new(move |backend_type: String| {
        action_error.set(None);
        install_modal_for.set(Some(backend_type));
    });

    let on_update_click = Callback::new(move |(backend_name, gpu_variant): (String, String)| {
        action_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            let url = backend_update_url(&backend_name, &gpu_variant);
            match post_request(&url).send().await {
                Ok(resp) => {
                    if handle_response(&resp) {
                        return;
                    }
                    if resp.ok() {
                        if let Ok(r) = resp.json::<InstallResponse>().await {
                            active_job_id.set(Some(r.job_id));
                        }
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        action_error.set(Some(format!("Update failed: {text}")));
                    }
                }
                Err(e) => action_error.set(Some(format!("Update request failed: {e}"))),
            }
        });
    });

    let on_check_updates_click =
        Callback::new(move |(backend_name, gpu_variant): (String, String)| {
            action_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                // Check a single backend variant via the updates API
                let url = backend_check_updates_url(&backend_name, &gpu_variant);
                match post_request(&url).send().await {
                    Ok(resp) => {
                        if handle_response(&resp) {
                            return;
                        }
                        if resp.ok() {
                            // After checking, refresh the full backend list to get updated status
                            match get_request("/tama/v1/backends").send().await {
                                Ok(resp2) => {
                                    if handle_response(&resp2) {
                                        return;
                                    }
                                    if let Ok(list) = resp2.json::<BackendListResponse>().await {
                                        backends_list.set(list);
                                    }
                                }
                                Err(e) => action_error
                                    .set(Some(format!("Failed to refresh backends: {e}"))),
                            }
                        } else {
                            let text = resp.text().await.unwrap_or_default();
                            action_error.set(Some(format!("Check updates failed: {text}")));
                        }
                    }
                    Err(e) => action_error.set(Some(format!("Check updates request failed: {e}"))),
                }
            });
        });

    let on_delete_click = Callback::new(move |(backend_name, gpu_variant): (String, String)| {
        action_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            let url = backend_delete_url(&backend_name, &gpu_variant);
            match delete_request(&url).send().await {
                Ok(resp) => {
                    if handle_response(&resp) {
                        return;
                    }
                    if resp.ok() {
                        refresh_tick.update(|n| *n += 1);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        action_error.set(Some(format!("Uninstall failed: {text}")));
                    }
                }
                Err(e) => action_error.set(Some(format!("Uninstall request failed: {e}"))),
            }
        });
    });

    let on_build_method_change = Callback::new(
        move |(backend_name, gpu_variant, build_from_source): (String, String, bool)| {
            action_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                let url = backend_source_url(&backend_name, &gpu_variant);
                let body = serde_json::json!({ "build_from_source": build_from_source });
                match post_request(&url).json(&body).unwrap().send().await {
                    Ok(resp) => {
                        if handle_response(&resp) {
                            return;
                        }
                        if resp.ok() {
                            // Success — no need to refresh, toggle already reflects the change
                        } else {
                            let text = resp.text().await.unwrap_or_default();
                            action_error
                                .set(Some(format!("Failed to update build method: {text}")));
                        }
                    }
                    Err(e) => action_error.set(Some(format!("Request failed: {e}"))),
                }
            });
        },
    );

    let on_install_submit = Callback::new(move |req: InstallRequest| {
        install_modal_for.set(None);
        action_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            let request = match post_request("/tama/v1/backends/install").json(&req) {
                Ok(r) => r,
                Err(e) => {
                    action_error.set(Some(format!("Failed to encode install request: {e}")));
                    return;
                }
            };
            match request.send().await {
                Ok(resp) => {
                    if handle_response(&resp) {
                        return;
                    }
                    if resp.ok() {
                        if let Ok(r) = resp.json::<InstallResponse>().await {
                            active_job_id.set(Some(r.job_id));
                        }
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        action_error.set(Some(format!("Install failed: {text}")));
                    }
                }
                Err(e) => action_error.set(Some(format!("Install request failed: {e}"))),
            }
        });
    });

    let on_install_cancel = Callback::new(move |_: ()| {
        install_modal_for.set(None);
    });

    let on_job_close = Callback::new(move |_: ()| {
        active_job_id.set(None);
        refresh_tick.update(|n| *n += 1);
    });

    // Key by "backend_name:gpu_variant" so each variant has its own args.
    // e.g. "llama_cpp:vulkan" vs "vllm:cpu"
    let on_default_args_change = Callback::new(
        move |(backend_name, gpu_variant, new_value): (String, String, String)| {
            let key = format!("{}:{}", backend_name, gpu_variant);
            default_args_edits.update(|edits| {
                edits.insert(key, new_value);
            });
            save_status.set(None); // Clear status when user makes new edits
        },
    );

    let on_default_env_change = Callback::new(
        move |(backend_name, gpu_variant, new_value): (String, String, String)| {
            let key = format!("{}:{}", backend_name, gpu_variant);
            default_env_edits.update(|edits| {
                edits.insert(key, new_value);
            });
            save_status.set(None);
        },
    );

    // Track version selection changes: key = "backend_name:gpu_variant", value = (name, version, variant)
    let version_edits: RwSignal<std::collections::HashMap<String, (String, String, String)>> =
        RwSignal::new(std::collections::HashMap::new());

    let on_version_change = Callback::new(
        move |(backend_name, version, gpu_variant): (String, String, String)| {
            let key = format!("{}:{}", backend_name, gpu_variant);
            version_edits.update(|edits| {
                edits.insert(key, (backend_name, version, gpu_variant));
            });
            save_status.set(None);
        },
    );

    let save = move |_| {
        if saving.get() {
            return;
        }
        let args_edits = default_args_edits.get();
        let env_edits = default_env_edits.get();
        let ver_edits = version_edits.get();
        if args_edits.is_empty() && env_edits.is_empty() && ver_edits.is_empty() {
            return;
        }
        saving.set(true);
        save_status.set(Some("Saving…".to_string()));
        wasm_bindgen_futures::spawn_local(async move {
            let mut errors = Vec::new();

            // Apply version changes first
            for (backend_name, ver, gv) in ver_edits.values() {
                let encoded = url_encode(backend_name);
                let url = format!("/tama/v1/backends/{}/activate?gpu_variant={}", encoded, gv);
                let body = serde_json::json!({ "version": ver });
                match post_request(&url).json(&body).unwrap().send().await {
                    Ok(resp) => {
                        if handle_response(&resp) {
                            return;
                        }
                        if resp.ok() {
                        } else {
                            let status = resp.status();
                            let text = resp.text().await.unwrap_or_default();
                            errors.push(format!(
                                "Activate {}: HTTP {} - {}",
                                backend_name, status, text
                            ));
                        }
                    }
                    Err(e) => errors.push(format!("Activate {}: {}", backend_name, e)),
                }
            }

            // Apply default args changes — key is "backend_name:gpu_variant"
            let edit_keys: Vec<String> = args_edits.keys().cloned().collect();
            for key in edit_keys {
                let args_str = args_edits.get(&key).cloned().unwrap_or_default();
                let parts: Vec<String> = parse_newline_separated(&args_str);
                // Parse "backend_name:gpu_variant" from key
                let parts_key: Vec<&str> = key.splitn(2, ':').collect();
                let bt = parts_key[0];
                let gv = parts_key.get(1).copied().unwrap_or("cpu");
                let body = serde_json::json!({ "default_args": parts });
                let encoded = url_encode(bt);
                let url = format!(
                    "/tama/v1/backends/{}/default-args?gpu_variant={}",
                    encoded, gv
                );
                let res = post_request(&url).json(&body).unwrap().send().await;
                match res {
                    Ok(response) => {
                        if handle_response(&response) {
                            return;
                        }
                        if response.ok() {
                        } else {
                            let status = response.status();
                            let text = response.text().await.unwrap_or_default();
                            errors.push(format!("{}: HTTP {} - {}", key, status, text));
                        }
                    }
                    Err(e) => errors.push(format!("{}: {}", key, e)),
                }
            }

            // Apply default env changes — key is "backend_name:gpu_variant"
            let env_edit_keys: Vec<String> = env_edits.keys().cloned().collect();
            for key in env_edit_keys {
                let env_str = env_edits.get(&key).cloned().unwrap_or_default();
                let parts: Vec<String> = parse_newline_separated(&env_str);
                // Parse "backend_name:gpu_variant" from key
                let parts_key: Vec<&str> = key.splitn(2, ':').collect();
                let bt = parts_key[0];
                let gv = parts_key.get(1).copied().unwrap_or("cpu");
                let body = serde_json::json!({ "default_env": parts });
                let encoded = url_encode(bt);
                let url = format!(
                    "/tama/v1/backends/{}/default-env?gpu_variant={}",
                    encoded, gv
                );
                let res = post_request(&url).json(&body).unwrap().send().await;
                match res {
                    Ok(response) => {
                        if handle_response(&response) {
                            return;
                        }
                        if response.ok() {
                        } else {
                            let status = response.status();
                            let text = response.text().await.unwrap_or_default();
                            errors.push(format!("{}: HTTP {} - {}", key, status, text));
                        }
                    }
                    Err(e) => errors.push(format!("{}: {}", key, e)),
                }
            }

            if errors.is_empty() {
                save_status.set(Some("✅ Saved".to_string()));
                default_args_edits.set(std::collections::HashMap::new());
                default_env_edits.set(std::collections::HashMap::new());
                version_edits.set(std::collections::HashMap::new());
                refresh_tick.update(|n| *n += 1);
            } else {
                save_status.set(Some(format!("❌ {}", errors.join(", "))));
            }
            saving.set(false);
        });
    };

    // ── View ─────────────────────────────────────────────────────────────────
    view! {
        <div class="page-header">
            <h1>"Providers"</h1>
            <div style="display:flex;gap:0.5rem;align-items:center;">
                {move || save_status.get().map(|s| view! { <span class="text-muted">{s}</span> })}
                <button
                    class="btn btn-primary"
                    disabled=move || saving.get()
                    on:click=save
                >
                    "Save Changes"
                </button>
                <div style="position:relative;">
                    <button
                        class="btn btn-success"
                        on:click=move |_| {
                            show_backend_dropdown.update(|v| *v = !*v);
                        }
                    >
                        "+ Add Provider"
                    </button>
                    {move || {
                        if !show_backend_dropdown.get() {
                            return view! { <span/> }.into_any();
                        }
                        let all = vec![
                            ("llama_cpp", "llama.cpp"),
                            ("ik_llama", "ik_llama.cpp"),
                            ("tts_kokoro", "Kokoro TTS"),
                            ("docker", "Docker (vLLM)"),
                        ];
                        let mut items = all;
                        items.sort_by_key(|(_, d)| *d);

                        view! {
                            <div style="position:absolute;right:0;top:100%;margin-top:4px;background:#1e293b;border:1px solid #334155;border-radius:6px;padding:0.5rem 0;z-index:100;width:200px;box-shadow:0 4px 12px rgba(0,0,0,0.3);">
                                {items.into_iter().map(|(backend_type, display_name): (&str, &str)| {
                                    let bt = backend_type.to_string();
                                    let is_docker = bt == "docker";
                                    view! {
                                        <button
                                            style="width:100%;text-align:left;padding:0.5rem 0.75rem;background:none;border:none;color:#e2e8f0;cursor:pointer;font-size:0.875rem;"
                                            on:click=move |_| {
                                                action_error.set(None);
                                                if is_docker {
                                                    docker_register_open.set(true);
                                                } else {
                                                    install_modal_for.set(Some(bt.clone()));
                                                }
                                                show_backend_dropdown.set(false);
                                            }
                                        >
                                            {display_name}
                                        </button>
                                    }.into_any()
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }}
                </div>
            </div>
        </div>

        <TabPills active_tab=active_tab />

        <div class="card">
            <p class="text-muted">"Manage inference provider installations."</p>

            {/* Error banner */}
            {move || action_error.get().map(|err| view! {
                <AlertBanner variant=AlertVariant::Error>{err}</AlertBanner>
            }.into_any())}

            {/* Active job log panel */}
            {move || {
                if let Some(jid) = active_job_id.get() {
                    view! {
                        <JobLogPanel job_id=jid on_close=on_job_close />
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            }}

            {/* Compaction card */}
            {move || {
                let comp = backends_list.get().compaction.clone();
                let border_color = if comp.running {
                    "#22c55e"
                } else {
                    "#475569"
                };
                let status = if comp.running {
                    "Running"
                } else if comp.enabled {
                    "Enabled (not running)"
                } else {
                    "Disabled"
                };
                let url_suffix = comp
                    .server_url
                    .as_ref()
                    .map(|u| format!(" — {}", u))
                    .unwrap_or_default();
                let port_str = comp
                    .port
                    .map(|p| format!(", Port: {}", p))
                    .unwrap_or_else(|| ", Port: auto".to_string());

                view! {
                    <div class="card" style=format!("margin-bottom:1rem;border-left:3px solid {}", border_color)>
                        <div style="display:flex;justify-content:space-between;align-items:center;">
                            <div>
                                <h3 style="margin:0;">"LLMLingua Compaction"</h3>
                                <p class="text-muted">
                                    {status}
                                    {url_suffix}
                                </p>
                                <p class="text-muted" style="font-size:0.8rem;">
                                    "Device: " {comp.device} {port_str}
                                </p>
                            </div>
                            <label class="form-check" style="display:flex;align-items:center;gap:0.5rem;">
                                <input
                                    type="checkbox"
                                    class="form-check-input"
                                    prop:checked=move || comp.enabled
                                    on:change=move |ev| {
                                        let enabled = ev
                                            .target()
                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            .map(|el| el.checked())
                                            .unwrap_or(false);
                                        let req = CompactionToggleRequest { enabled };
                                        wasm_bindgen_futures::spawn_local(async move {
                                            match post_request("/tama/v1/backends/compaction")
                                                .json(&req)
                                                .unwrap()
                                                .send()
                                                .await
                                            {
                                                Ok(resp) => {
                                                    if handle_response(&resp) {
                                                        return;
                                                    }
                                                    if resp.ok() {
                                                        leptos::logging::log!("Compaction toggle succeeded");
                                                    } else {
                                                        let text = resp.text().await.unwrap_or_default();
                                                        leptos::logging::error!("Toggle failed: {}", text);
                                                    }
                                                }
                                                Err(e) => {
                                                    leptos::logging::error!("Toggle request failed: {}", e);
                                                }
                                            }
                                        });
                                        // Refresh to pick up updated compaction status
                                        // Note: refresh fires immediately; if the API call fails
                                        // the re-fetch will restore the previous state (prop:checked
                                        // is bound to server state), so the checkbox snaps back.
                                        refresh_tick.update(|n| *n += 1);
                                    }
                                />
                                <span class="form-check-label">"Enable"</span>
                            </label>
                        </div>
                    </div>
                }
            }.into_any()}

            {/* Backend cards */}
            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                {move || {
                    let list = backends_list.get();
                    let combined: Vec<_> = list.backends.into_iter()
                        .chain(list.custom.into_iter())
                        .collect();
                    let docker_cards = list.docker;

                    if combined.is_empty() && docker_cards.is_empty() {
                        return view! {
                            <div style="text-align:center;padding:2.5rem 2rem;color:#64748b;">
                                <div style="font-size:1.125rem;font-weight:500;margin-bottom:0.5rem;">
                                    "No providers installed"
                                </div>
                                <div style="font-size:0.875rem;margin-bottom:1.5rem;">
                                    "Click the + Add Provider button to get started."
                                </div>
                            </div>
                        }.into_any();
                    }

                    let mut rows = Vec::new();

                    // Main section: native backends + custom
                    for backend in combined {
                        rows.push(view! {
                            <InstallationCard
                                installation=backend
                                on_install=on_install_click
                                on_update=on_update_click
                                on_check_updates=on_check_updates_click
                                on_delete=on_delete_click
                                on_default_args_change=on_default_args_change
                                on_default_env_change=on_default_env_change
                                on_version_change=on_version_change
                                on_build_method_change=on_build_method_change
                            />
                        }.into_any());
                    }

                    // Dedicated Docker backends section
                    if !docker_cards.is_empty() {
                        let mut docker_rows = Vec::new();
                        for backend in docker_cards {
                            docker_rows.push(view! {
                                <InstallationCard
                                    installation=backend
                                    on_install=on_install_click
                                    on_update=on_update_click
                                    on_check_updates=on_check_updates_click
                                    on_delete=on_delete_click
                                    on_default_args_change=on_default_args_change
                                    on_default_env_change=on_default_env_change
                                    on_version_change=on_version_change
                                    on_build_method_change=on_build_method_change
                                />
                            }.into_any());
                        }
                        rows.push(view! {
                            <div style="margin-top:0.25rem;">
                                <h3 style="margin:0 0 0.5rem 0;font-size:1rem;color:#e2e8f0;">"Docker Providers"</h3>
                                <div style="display:flex;flex-direction:column;gap:1rem;">{docker_rows}</div>
                            </div>
                        }.into_any());
                    }

                    view! { <>{rows}</> }.into_any()
                }}
            </div>

            {/* Docker registration modal */}
            {move || {
                if docker_register_open.get() {
                    view! {
                        <DockerRegisterModal
                            on_close=Callback::new(move |_: ()| {
                                docker_register_open.set(false);
                                refresh_tick.update(|n| *n += 1);
                            })
                        />
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            }}

            {/* Install modal */}
            {move || {
                if let Some(bt) = install_modal_for.get() {
                    let caps = capabilities.get();
                    view! {
                        <InstallModal
                            backend_type=bt
                            capabilities=caps
                            on_submit=on_install_submit
                            on_cancel=on_install_cancel
                        />
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            }}
        </div>
    }
}
