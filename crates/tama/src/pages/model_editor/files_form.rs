use std::sync::Arc;

use leptos::prelude::*;

use super::types::{ModelForm, QuantInfo, QuantKind};
use crate::utils::target_value;

fn format_bytes_opt(bytes: Option<u64>) -> String {
    let Some(b) = bytes else {
        return "—".to_string();
    };
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bf = b as f64;
    if bf >= GIB {
        format!("{:.2} GiB", bf / GIB)
    } else if bf >= MIB {
        format!("{:.1} MiB", bf / MIB)
    } else if bf >= KIB {
        format!("{:.1} KiB", bf / KIB)
    } else {
        format!("{} B", b)
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(10).collect()
}

#[component]
pub fn ModelEditorFilesForm(
    form: RwSignal<Option<ModelForm>>,
    repo_commit_sha: RwSignal<Option<String>>,
    repo_pulled_at: RwSignal<Option<String>>,
    refresh_busy: RwSignal<bool>,
    verify_busy: RwSignal<bool>,
    refresh_status: RwSignal<Option<(bool, String)>>,
    verify_status: RwSignal<Option<(bool, String)>>,
    pull_modal_open_signal: RwSignal<bool>,
    delete_quant_action: Action<(String, String), (), LocalStorage>,
    original_id: RwSignal<String>,
    refresh_action: Action<(), (), LocalStorage>,
    verify_action: Action<(), (), LocalStorage>,
) -> impl IntoView {
    view! {
        // Repo-level metadata + refresh/verify actions
        <div class="quants-meta-bar">
            <div class="quants-meta">
                {move || match repo_commit_sha.get() {
                    Some(sha) => view! {
                        <span class="text-muted">"Commit: "</span>
                        <code class="quants-sha">{short_sha(&sha)}</code>
                    }.into_any(),
                    None => view! {
                        <span class="text-muted">"No commit recorded"</span>
                    }.into_any(),
                }}
                {move || repo_pulled_at.get().map(|when| view! {
                    <span class="text-muted ml-2">"· Pulled: "{when}</span>
                })}
            </div>
            <div class="quants-actions">
                <button
                    type="button"
                    class="btn btn-secondary btn-sm"
                    prop:disabled=move || refresh_busy.get()
                    on:click=move |_| { refresh_action.dispatch(()); }
                    title="Check HuggingFace for updated quant metadata"
                >
                    {move || if refresh_busy.get() { "Checking..." } else { "Check for updates" }}
                </button>
                <button
                    type="button"
                    class="btn btn-secondary btn-sm ml-1"
                    prop:disabled=move || verify_busy.get()
                    on:click=move |_| { verify_action.dispatch(()); }
                    title="Verify downloaded files against HuggingFace LFS hashes"
                >
                    {move || if verify_busy.get() { "Verifying..." } else { "Verify files" }}
                </button>
            </div>
        </div>
        {move || refresh_status.get().map(|(ok, msg)| {
            let cls = if ok { "alert alert--success" } else { "alert alert--error" };
            view! { <div class=cls>{msg}</div> }
        })}
        {move || verify_status.get().map(|(ok, msg)| {
            let cls = if ok { "alert alert--success" } else { "alert alert--error" };
            view! { <div class=cls>{msg}</div> }
        })}

        // ── Subsection 1: Model Quants ──────────────────────────────────────
        <div class="mt-3">
            <h3 class="form-section-title">"Model Quants"</h3>
            <table class="quants-table">
                <thead>
                    <tr>
                        <th>"Active"</th>
                        <th>"Name"</th>
                        <th>"Size"</th>
                        <th>"SHA"</th>
                        <th>"Verified"</th>
                        <th></th>
                    </tr>
                </thead>
                <tbody>
                    <For
                        each=move || {
                            form.get().map(|f| {
                                f.quants.iter()
                                    .filter(|(_, q)| q.kind == QuantKind::Model)
                                    .enumerate()
                                    .map(|(i, (name, q))| (i, name.clone(), q.clone()))
                                    .collect::<Vec<_>>()
                            }).unwrap_or_default()
                        }
                        key=|(_i, name, _)| name.clone()
                        children=move |(_i, name, q)| {
                            let name_arc = Arc::new(name.clone());
                            let _name_for_check = Arc::clone(&name_arc);
                            let name_for_change = Arc::clone(&name_arc);
                            let q_arc = Arc::new(q);
                            let q_arc_size = Arc::clone(&q_arc);
                            let q_arc_sha = Arc::clone(&q_arc);
                            let q_arc_verified = Arc::clone(&q_arc);
                            let q_arc_del = Arc::clone(&q_arc);
                            view! {
                                <tr>
                                    <td>
                                        <input
                                            type="checkbox"

                                            on:change=move |e| {
                                                use wasm_bindgen::JsCast;
                                                let checked = e.target()
                                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                    .map(|el| el.checked())
                                                    .unwrap_or(false);
                                                let selected_name = name_for_change.to_string();
                                                form.update(|f| {
                                                    if let Some(form) = f {
                                                        form.quant = if checked {
                                                            Some(selected_name)
                                                        } else {
                                                            None
                                                        };
                                                    }
                                                });
                                            }
                                        />
                                    </td>
                                    <td><span class="text-muted">{name.clone()}</span></td>
                                    <td>
                                        // Read-only: size is authoritative from HF/filesystem.
                                        // Use `Check for updates` or `Verify files` to refresh.
                                        <span
                                            class="text-muted"
                                            title=move || q_arc_size.size_bytes
                                                .map(|v| format!("{} bytes", v))
                                                .unwrap_or_else(|| "Unknown".to_string())
                                        >
                                            {move || format_bytes_opt(q_arc.size_bytes)}
                                        </span>
                                    </td>
                                    <td>
                                        {
                                            let sha_opt = q_arc_sha.lfs_oid.clone();
                                            let title = sha_opt.clone().unwrap_or_else(|| "No hash".to_string());
                                            let display = sha_opt.as_deref().map(short_sha).unwrap_or_else(|| "—".to_string());
                                            view! {
                                                <code class="text-muted" title=title>{display}</code>
                                            }
                                        }
                                    </td>
                                    <td>
                                        {
                                            let (icon, cls, title) = match q_arc_verified.verified_ok {
                                                Some(true) => ("✓", "text-success", q_arc_verified.last_verified_at.clone().unwrap_or_else(|| "Verified".to_string())),
                                                Some(false) => ("✗", "text-error", q_arc_verified.verify_error.clone().unwrap_or_else(|| "Verification failed".to_string())),
                                                None => ("—", "text-muted", "Not verified".to_string()),
                                            };
                                            view! { <span class=cls title=title>{icon}</span> }
                                        }
                                    </td>
                                    <td>
                                        {
                                            let name_ref = Arc::clone(&name_arc);
                                            let size_display = format_bytes_opt(q_arc_del.size_bytes);
                                            let key_for_action = name_arc.to_string();
                                            view! {
                                                <button
                                                    type="button"
                                                    class="btn btn-danger btn-sm"
                                                    on:click=move |_| {
                                                        let msg = format!(
                                                            "Delete \"{}\" ({}) from disk?\nThis cannot be undone.",
                                                            name_ref.as_str(),
                                                            size_display
                                                        );
                                                        let confirmed = web_sys::window()
                                                            .and_then(|w| w.confirm_with_message(&msg).ok())
                                                            .unwrap_or(false);
                                                        if confirmed {
                                                            let current_id = original_id.get();
                                                            delete_quant_action.dispatch((current_id, key_for_action.clone()));
                                                        }
                                                    }
                                                >"✕"</button>
                                            }
                                        }
                                    </td>
                                </tr>
                            }
                        }
                    />
                </tbody>
            </table>
        </div>

        // ── Subsection 2: Vision Projector ──────────────────────────────────
        <div class="mt-3">
            <h3 class="form-section-title">"Vision Projector"</h3>
            {move || {
                let mmproj_entries: Vec<(String, QuantInfo)> = form.get().map(|f| {
                    f.quants.iter()
                        .filter(|(_, q)| q.kind == QuantKind::Mmproj)
                        .map(|(name, q)| (name.clone(), q.clone()))
                        .collect()
                }).unwrap_or_default();

                if mmproj_entries.is_empty() {
                    view! {
                        <p class="text-muted form-hint">"No vision projector files pulled yet. Use \"+ Pull Quant\" to add one."</p>
                    }.into_any()
                } else {
                    view! {
                        <table class="quants-table">
                            <thead>
                                <tr>
                                    <th>"Active"</th>
                                    <th>"Name"</th>
                                    <th>"Size"</th>
                                    <th>"SHA"</th>
                                    <th>"Verified"</th>
                                </tr>
                            </thead>
                            <tbody>
                                <For
                                    each=move || {
                                        form.get().map(|f| {
                                            f.quants.iter()
                                                .filter(|(_, q)| q.kind == QuantKind::Mmproj)
                                                .map(|(name, q)| (name.clone(), q.clone()))
                                                .collect::<Vec<_>>()
                                        }).unwrap_or_default()
                                    }
                                    key=|(name, _)| name.clone()
                                    children=move |(name, q)| {
                                        let name_arc = Arc::new(name.clone());
                                        let _name_for_check = Arc::clone(&name_arc);
                                        let size = format_bytes_opt(q.size_bytes);
                                        let sha_opt = q.lfs_oid.clone();
                                        let sha_display = sha_opt.as_deref().map(short_sha).unwrap_or_else(|| "—".to_string());
                                        let sha_title = sha_opt.clone().unwrap_or_else(|| "No hash".to_string());
                                        let (v_icon, v_cls, v_title) = match q.verified_ok {
                                            Some(true) => ("✓", "text-success", q.last_verified_at.clone().unwrap_or_else(|| "Verified".to_string())),
                                            Some(false) => ("✗", "text-error", q.verify_error.clone().unwrap_or_else(|| "Verification failed".to_string())),
                                            None => ("—", "text-muted", "Not verified".to_string()),
                                        };
                                        view! {
                                            <tr>
                                                <td>
                                                    <input
                                                        type="checkbox"

                                                        on:change=move |e| {
                                                            use wasm_bindgen::JsCast;
                                                            let checked = e.target()
                                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                                .map(|el| el.checked())
                                                                .unwrap_or(false);
                                                            let selected_name = name_arc.to_string();
                                                            form.update(|f| {
                                                                if let Some(form) = f {
                                                                    form.mmproj = if checked {
                                                                        Some(selected_name)
                                                                    } else {
                                                                        None
                                                                    };
                                                                }
                                                            });
                                                        }
                                                    />
                                                </td>
                                                <td>{name.clone()}</td>
                                                <td><span class="text-muted">{size}</span></td>
                                                <td><code class="text-muted" title=sha_title>{sha_display}</code></td>
                                                <td><span class=v_cls title=v_title>{v_icon}</span></td>
                                            </tr>
                                        }
                                    }
                                />
                            </tbody>
                        </table>
                        <span class="form-hint">"Check one file to use as the active vision projector."</span>
                    }.into_any()
                }
            }}
        </div>

        // ── Subsection 3: MTP Draft Model ───────────────────────────────────
        {move || {
            let mtp_entries: Vec<(String, QuantInfo)> = form.get().map(|f| {
                f.quants.iter()
                    .filter(|(_, q)| q.kind == QuantKind::Mtp)
                    .map(|(name, q)| (name.clone(), q.clone()))
                    .collect()
            }).unwrap_or_default();

            if mtp_entries.is_empty() {
                return None;
            }
            let _current_mtp = Signal::derive(move || {
                form.get().and_then(|f| f.mtp_model.clone())
            });
            let mtp_entries_clone = mtp_entries;
            Some(view! {
                <div class="mt-3">
                    <h3 class="form-section-title">"MTP Draft Model"</h3>
                    <div class="form-grid">
                        <label>
                            <span class="form-label">Draft model for speculative decoding</span>
                            <select
                                class="form-select"

                                on:change=move |e| {
                                    let target = target_value(&e);
                                    let selected = if target == "(none)" { None } else { Some(target) };
                                    form.update(|f| {
                                        if let Some(form) = f {
                                            form.mtp_model = selected;
                                        }
                                    });
                                }
                            >
                                {move || {
                                    let entries = mtp_entries_clone.clone();
                                    let mut opts: Vec<_> = vec![view! { <option value="(none)">"(none)"</option> }.into_any()];
                                    for (name, _q) in &entries {
                                        let value = name.clone();
                                        let display = name.clone();
                                        opts.push(view! { <option value=value>{display}</option> }.into_any());
                                    }
                                    opts
                                }}
                            </select>
                            <span class="form-hint">"Select an MTP quant file to use as the draft model for speculative decoding."</span>
                        </label>
                    </div>
                </div>
            }.into_any())
        }}

        // ── Pull Quant button ───────────────────────────────────────────────
        <div class="mt-3">
            <span title=move || {
                if let Some(form) = form.get() {
                    if form.model.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
                        "Enter the HuggingFace repo above before pulling quants".to_string()
                    } else {
                        "Pull a new quant from HuggingFace".to_string()
                    }
                } else {
                    "Loading...".to_string()
                }
            }>
                <button
                    type="button"
                    class="btn btn-primary btn-sm"
                    prop:disabled=move || form.get().map(|f| f.model.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true)).unwrap_or(true)
                    on:click=move |_| pull_modal_open_signal.set(true)
                >"+ Pull Quant"</button>
            </span>
        </div>
    }
}
