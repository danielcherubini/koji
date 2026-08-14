use leptos::prelude::*;
use std::collections::HashSet;
#[cfg(not(feature = "ssr"))]
use wasm_bindgen::closure::Closure;
#[cfg(not(feature = "ssr"))]
use wasm_bindgen::JsCast;

use crate::utils::{delete_request, get_request, handle_response, post_request, put_request};

use crate::components::pull_wizard::*;

// Re-export CompletedQuant for use in pages
use crate::components::pull_wizard::components::{
    confirm_step::ConfirmStep, context_step::ContextStep, done_step::DoneStep, pull_step::PullStep,
    repo_input::RepoInput, repo_pull_step::RepoPullStep, selection_step::SelectionStep,
    vllm_config_step::VllmConfigStep,
};
pub use crate::components::pull_wizard::CompletedQuant;

#[component]
pub fn PullQuantWizard(
    /// Pre-set HF repo ID. If non-empty AND `is_open` transitions to true,
    /// the wizard skips step 1 and immediately fetches quants. If empty,
    /// the wizard starts at the repo-input step.
    #[prop(into)]
    initial_repo: Signal<String>,

    /// Whether the wizard is currently visible. Convention: `None` means
    /// "hosted directly on a page, always visible, never auto-reset" — the
    /// reset Effect is not registered. `Some(signal)` enables the modal
    /// lifecycle where (closed → open) transitions drive reset/refetch.
    #[prop(optional)]
    is_open: Option<Signal<bool>>,

    /// Called once after all pulls in the current session reach a terminal
    /// state. Receives the list of quants that completed successfully (failed
    /// jobs are filtered out). Fires exactly once per session, guarded by
    /// `did_complete`.
    #[prop(optional)]
    on_complete: Option<Callback<Vec<CompletedQuant>>>,

    /// Called when the user dismisses via in-step Cancel/Hide/Close button.
    /// Wizard never hides itself — host decides what happens.
    #[prop(optional)]
    on_close: Option<Callback<()>>,
) -> impl IntoView {
    // ── Signals ──────────────────────────────────────────────────────────────
    let wizard_step = RwSignal::new(WizardStep::RepoInput);
    let repo_id = RwSignal::new(String::new());
    let available_quants = RwSignal::new(Vec::<QuantEntry>::new());
    let available_mmprojs = RwSignal::new(Vec::<QuantEntry>::new());
    let available_mtps = RwSignal::new(Vec::<QuantEntry>::new());
    let selected_filenames = RwSignal::new(HashSet::<String>::new());
    let selected_mmproj_filenames = RwSignal::new(HashSet::<String>::new());
    let selected_mtp_filenames = RwSignal::new(HashSet::<String>::new());
    let gguf_context_length = RwSignal::new(None::<u64>);
    let context_settings = RwSignal::new(ContextSettings::default());
    let model_id = RwSignal::new(None::<u32>);
    let hf_metadata = RwSignal::new(HfModelMetadata::default());
    let branch = RwSignal::new(WizardBranch::Gguf);
    let repo_pull_status = RwSignal::new(None::<RepoPullStatus>);
    let repo_pull_job_id = RwSignal::new(None::<String>);
    // True while a `POST /tama/v1/pulls/repo` start request is in flight —
    // disables the start/retry buttons and rejects re-entrant starts (B2).
    let repo_pull_starting = RwSignal::new(false);
    let vllm_settings = RwSignal::new(VllmWizardSettings::default());
    // The model's stored `vllm` config JSON (fetched on SetContext entry),
    // used as the base for the overlay save. `None` for fresh pulls or when
    // the pre-entry fetch failed (falls back to the 5-field body).
    let vllm_existing = RwSignal::new(None::<serde_json::Value>);
    let pull_jobs = RwSignal::new(Vec::<JobProgress>::new());
    let error_msg = RwSignal::new(Option::<String>::None);
    let did_complete = RwSignal::new(false);

    // ── Cancel flag: flipped on component unmount ───────────────────────────
    let cancelled = RwSignal::new(false);

    // ── EventSource handle: closed on component unmount ──────────────────────
    let es_ref = RwSignal::new(None::<web_sys::EventSource>);
    on_cleanup(move || {
        cancelled.set(true);
        if let Some(es) = es_ref.get() {
            es.close();
        }
    });

    // ── on_complete Effect (only if on_complete is Some) ─────────────────────
    // Watches pull_jobs signal for terminal state transitions.
    // Moved out of the view closure to avoid calling during render.
    if let Some(cb) = on_complete {
        Effect::new(move |_| {
            let step = wizard_step.get();
            if step != WizardStep::Done {
                return;
            }
            if did_complete.get_untracked() {
                return;
            }
            did_complete.set(true);

            let jobs = pull_jobs.get_untracked();
            let quants_listing = available_quants.get_untracked();
            let mmprojs = available_mmprojs.get_untracked();
            let mtps = available_mtps.get_untracked();
            let repo = repo_id.get_untracked();

            // Filter to only primary shard filenames — non-primary shards
            // (whose filenames only appear in `shards` vectors) would overwrite
            // the primary file reference in the model editor.
            let completed = build_completed_quants(&jobs, &quants_listing, &mmprojs, &mtps, &repo);

            cb.run(completed);
        });
    }

    // ── Downloading → SetContext transition Effect ──────────────────────────
    // Watches pull_jobs for terminal-state transitions and advances to
    // WizardStep::SetContext so the user can configure model settings.
    Effect::new(move |_| {
        let jobs = pull_jobs.get();
        if jobs.is_empty() {
            return;
        }
        let all_terminal = jobs
            .iter()
            .all(|j| j.status == "completed" || j.status == "failed");
        if !all_terminal {
            return;
        }
        // Only transition if we're currently on the Downloading step.
        let current_step = wizard_step.get();
        if current_step == WizardStep::Downloading {
            wizard_step.set(WizardStep::SetContext);
        }
    });

    // ── Reset Effect (only if is_open is Some) ──────────────────────────────
    if let Some(is_open_sig) = is_open {
        Effect::new(move |_| {
            let open = is_open_sig.get();
            if !open {
                return;
            }
            let step = wizard_step.get_untracked();
            if !matches!(step, WizardStep::RepoInput | WizardStep::Done) {
                return;
            }
            selected_filenames.set(std::collections::HashSet::new());
            selected_mmproj_filenames.set(std::collections::HashSet::new());
            selected_mtp_filenames.set(std::collections::HashSet::new());
            gguf_context_length.set(None);
            model_id.set(None);
            hf_metadata.set(HfModelMetadata::default());
            context_settings.set(ContextSettings::default());
            pull_jobs.set(Vec::new());
            error_msg.set(None);
            did_complete.set(false);
            wizard_step.set(WizardStep::RepoInput);
            branch.set(WizardBranch::Gguf);
            repo_pull_status.set(None);
            repo_pull_job_id.set(None);
            repo_pull_starting.set(false);
            vllm_settings.set(VllmWizardSettings::default());
            vllm_existing.set(None);

            let repo = initial_repo.get_untracked();
            if repo.trim().is_empty() {
                return;
            }
            repo_id.set(repo.clone());
            wizard_step.set(WizardStep::LoadingQuants);

            // Shared fetch + branch handling with the search callback, so the
            // modal-lifecycle path cannot drift from the search path.
            wasm_bindgen_futures::spawn_local(async move {
                let (listing, metadata) = fetch_repo_listing(&repo).await;
                apply_search_result(
                    &repo,
                    listing,
                    metadata,
                    SearchSignals {
                        model_id,
                        hf_metadata,
                        available_quants,
                        available_mmprojs,
                        available_mtps,
                        branch,
                        wizard_step,
                        error_msg,
                    },
                )
                .await;
            });
        });
    }

    // ── Step dispatch ───────────────────────────────────────────────────────
    view! {
        <div class="wizard-steps mb-3">
            {move || {
                let step = wizard_step.get();
                let show_repo_step = initial_repo.get().trim().is_empty();
                view! {
                    {show_repo_step.then(|| view! {
                        <div class=step_class(&step, &WizardStep::RepoInput, 0)>
                            "1. Repo"
                        </div>
                    })}
                    <div class=step_class(&step, &WizardStep::SelectQuants, 1)>
                        {move || if branch.get() == WizardBranch::Transformers {
                            "2. Confirm"
                        } else {
                            "2. Select"
                        }}
                    </div>
                    <div class=step_class(&step, &WizardStep::Downloading, 2)>
                        "3. Download"
                    </div>
                    <div class=step_class(&step, &WizardStep::SetContext, 3)>
                        "4. Configure"
                    </div>
                    <div class=step_class(&step, &WizardStep::Done, 4)>
                        "5. Done"
                    </div>
                }
            }}
        </div>

        <div class="card">
            {move || match wizard_step.get() {
                WizardStep::RepoInput => view! {
                    <RepoInput
                        repo_id=repo_id
                        error_msg=error_msg
                        on_close=on_close
                        on_search=Callback::new(move |rid: String| {
                            error_msg.set(None);
                            selected_filenames.set(std::collections::HashSet::new());
                            selected_mmproj_filenames.set(std::collections::HashSet::new());
                            selected_mtp_filenames.set(std::collections::HashSet::new());
                            gguf_context_length.set(None);
                            model_id.set(None);
                            context_settings.set(ContextSettings::default());
                            hf_metadata.set(HfModelMetadata::default());
                            available_quants.set(Vec::new());
                            branch.set(WizardBranch::Gguf);
                            repo_pull_status.set(None);
                            repo_pull_job_id.set(None);
                            repo_pull_starting.set(false);
                            vllm_settings.set(VllmWizardSettings::default());
                            vllm_existing.set(None);
                            // Fetch quants + metadata in parallel, then decide the
                            // branch from `hf_format` and create the stub with the
                            // branch-correct backend (shared with the reset path).
                            wasm_bindgen_futures::spawn_local(async move {
                                let (listing, metadata) = fetch_repo_listing(&rid).await;
                                apply_search_result(
                                    &rid,
                                    listing,
                                    metadata,
                                    SearchSignals {
                                        model_id,
                                        hf_metadata,
                                        available_quants,
                                        available_mmprojs,
                                        available_mtps,
                                        branch,
                                        wizard_step,
                                        error_msg,
                                    },
                                )
                                .await;
                            });
                        })
                    />
                }.into_any(),

                WizardStep::LoadingQuants => {
                    // Folded into RepoInput — stub model created during search.
                    // This arm is unreachable in normal flow, retained for safety.
                    view! { <div></div> }.into_any()
                },

                WizardStep::SelectQuants => {
                    // Transformers branch: Confirm step (whole-repo download).
                    // GGUF branch: the original quant-selection step, unchanged.
                    if branch.get() == WizardBranch::Transformers {
                        view! {
                            <ConfirmStep
                                repo_id=repo_id.into()
                                metadata=hf_metadata.into()
                                starting=repo_pull_starting.into()
                                on_start=Callback::new(move |_| {
                                    start_repo_pull_job(RepoPullSignals {
                                        repo_id,
                                        model_id,
                                        repo_pull_status,
                                        repo_pull_job_id,
                                        repo_pull_starting,
                                        wizard_step,
                                        error_msg,
                                        vllm_settings,
                                        vllm_existing,
                                        hf_metadata,
                                        cancelled,
                                    });
                                })
                                on_back=Callback::new(move |_| {
                                    wizard_step.set(WizardStep::RepoInput);
                                })
                            />
                        }
                        .into_any()
                    } else {
                        view! {
                            <SelectionStep
                                repo_id=repo_id.into()
                                available_quants=available_quants.into()
                                available_mmprojs=available_mmprojs.into()
                                available_mtps=available_mtps.into()
                                selected_filenames=selected_filenames
                                selected_mmproj_filenames=selected_mmproj_filenames
                                selected_mtp_filenames=selected_mtp_filenames
                                on_next=Callback::new(move |_| {
                                    let rid = repo_id.get();
                                    let filenames: Vec<String> = selected_filenames.get().into_iter().collect();
                                    let mmproj_filenames: Vec<String> = selected_mmproj_filenames
                                        .get()
                                        .into_iter()
                                        .collect();
                                    let mtp_filenames: Vec<String> = selected_mtp_filenames
                                        .get()
                                        .into_iter()
                                        .collect();

                                    let body = PullRequest {
                                        repo_id: rid,
                                        model_id: model_id.get_untracked(),
                                        filenames,
                                        mmproj_filenames,
                                        mtp_filenames,
                                    };

                                    wasm_bindgen_futures::spawn_local(async move {
                                        let build_result = post_request("/tama/v1/pulls")
                                            .json(&body);
                                        let resp = match build_result {
                                            Ok(req) => req.send().await,
                                            Err(e) => {
                                                error_msg.set(Some(format!("Failed to build request: {e}")));
                                                return;
                                            }
                                        };
                                        match resp {
                                            Ok(r) => {
                                                if handle_response(&r) {
                                                    return;
                                                }
                                                match r.json::<Vec<PullJobEntry>>().await {
                                                    Ok(entries) => {
                                                        let jobs: Vec<JobProgress> = entries
                                                            .iter()
                                                            .map(|e| JobProgress {
                                                                job_id: e.job_id.clone(),
                                                                filename: e.filename.clone(),
                                                                status: e.status.clone(),
                                                                bytes_pulled: 0,
                                                                total_bytes: None,
                                                                error: None,
                                                            })
                                                            .collect();
                                                        pull_jobs.set(jobs);
                                                        wizard_step.set(WizardStep::Downloading);

                                                        // Subscribe to global pull events SSE stream.
                                                        #[cfg(not(feature = "ssr"))]
                                                        spawn_pull_events_listener(entries, pull_jobs, wizard_step, cancelled, es_ref);
                                                        #[cfg(feature = "ssr")]
                                                        let _ = entries;
                                                    }
                                                    Err(e) => {
                                                        error_msg.set(Some(format!("Failed to parse response: {e}")));
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                error_msg.set(Some(format!("Request failed: {e}")));
                                            }
                                        }
                                    });
                                })
                                    on_back=Callback::new(move |_| {
                                        wizard_step.set(WizardStep::RepoInput);
                                    })
                                />
                        }
                        .into_any()
                    }
                }

                WizardStep::SetContext => {
                    // Transformers branch: vLLM config step.
                    // GGUF branch: the original ContextStep, unchanged.
                    if branch.get() == WizardBranch::Transformers {
                        view! {
                            {move || error_msg.get().map(|e| view! {
                                <div class="alert alert--error mb-2">
                                    <span class="alert__icon">"✕"</span>
                                    <span>{e}</span>
                                </div>
                            })}
                            <VllmConfigStep
                                settings=vllm_settings
                                initial_max_model_len=Signal::derive(move || {
                                    repo_pull_status
                                        .get()
                                        .and_then(|s| s.context_length)
                                        .or_else(|| hf_metadata.get().hf_context_length)
                                })
                                on_next=Callback::new(move |_| {
                                    let settings = vllm_settings.get();
                                    let existing = vllm_existing.get_untracked();
                                    let mid = model_id.get_untracked();
                                    let repo = repo_id.get_untracked();

                                    wasm_bindgen_futures::spawn_local(async move {
                                        // Overlay the wizard's five fields onto the fetched
                                        // existing vllm config so fields the wizard does not
                                        // expose (attention_backend, spec_decoding, …) survive
                                        // the server's whole-struct vllm replace. Null base
                                        // (fresh pull / failed pre-entry fetch) → 5-field body.
                                        let base = existing
                                            .as_ref()
                                            .unwrap_or(&serde_json::Value::Null);
                                        let payload = apply_vllm_wizard_overlays(base, &settings);

                                        // Use numeric DB id for the PUT
                                        let model_key = if let Some(id) = mid {
                                            id.to_string()
                                        } else {
                                            crate::utils::config_key_from_repo_id(&repo)
                                        };

                                        match put_request(&format!("/tama/v1/models/{}", model_key))
                                            .json(&payload)
                                        {
                                            Ok(req) => {
                                                match req.send().await {
                                                    Ok(resp) => {
                                                        if handle_response(&resp) {
                                                            return;
                                                        }
                                                        if resp.status() < 400 {
                                                            wizard_step.set(WizardStep::Done);
                                                        } else {
                                                            // Surface the
                                                            // server's
                                                            // `error.message`
                                                            // when the body
                                                            // carries one
                                                            // (e.g. vLLM
                                                            // config
                                                            // validation),
                                                            // else the bare
                                                            // status.
                                                            let body = resp
                                                                .json::<serde_json::Value>()
                                                                .await
                                                                .ok();
                                                            let message =
                                                                server_error_message(
                                                                    body.as_ref(),
                                                                    format!(
                                                                        "Failed to save settings (HTTP {})",
                                                                        resp.status()
                                                                    ),
                                                                );
                                                            error_msg.set(Some(message));
                                                        }
                                                    }
                                                    Err(e) => {
                                                        error_msg.set(Some(format!(
                                                            "Failed to save settings: {e}"
                                                        )));
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                error_msg.set(Some(format!(
                                                    "Failed to build request: {e}"
                                                )));
                                            }
                                        }
                                    });
                                })
                                on_back=Callback::new(move |_| {
                                    wizard_step.set(WizardStep::Downloading);
                                })
                                on_skip=Callback::new(move |_| {
                                    wizard_step.set(WizardStep::Done);
                                })
                            />
                        }
                        .into_any()
                    } else {
                        view! {
                            <ContextStep
                                gguf_context_length=gguf_context_length.into()
                                pull_jobs=pull_jobs.into()
                                settings=context_settings
                                on_next=Callback::new(move |_| {
                                    let settings = context_settings.get();
                                    let mid = model_id.get_untracked();
                                    let repo = repo_id.get_untracked();

                                    wasm_bindgen_futures::spawn_local(async move {
                                        let payload = serde_json::json!({
                                            "backend": "llama_cpp",
                                            "context_length": settings.context_length,
                                            "kv_unified": Some(settings.kv_unified),
                                            "cache_type_k": settings.cache_type_k,
                                            "cache_type_v": settings.cache_type_v,
                                        });

                                        // Use numeric DB id for the PUT
                                        let model_key = if let Some(id) = mid {
                                            id.to_string()
                                        } else {
                                            crate::utils::config_key_from_repo_id(&repo)
                                        };

                                        match put_request(&format!("/tama/v1/models/{}", model_key))
                                            .json(&payload)
                                        {
                                            Ok(req) => {
                                                match req.send().await {
                                                    Ok(resp) => {
                                                        if handle_response(&resp) {
                                                            return;
                                                        }
                                                        if resp.status() < 400 {
                                                            wizard_step.set(WizardStep::Done);
                                                        } else {
                                                            error_msg.set(Some(format!("Failed to save settings (HTTP {})", resp.status())));
                                                        }
                                                    }
                                                    Err(e) => {
                                                        error_msg.set(Some(format!("Failed to save settings: {}", e)));
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                error_msg.set(Some(format!("Failed to build request: {}", e)));
                                            }
                                        }
                                    });
                                })
                                    on_back=Callback::new(move |_| {
                                        wizard_step.set(WizardStep::Downloading);
                                    })
                                />
                        }
                        .into_any()
                    }
                }

                WizardStep::Downloading => {
                    // Transformers branch: single polled repo-pull job.
                    // GGUF branch: the original per-file PullStep, unchanged.
                    if branch.get() == WizardBranch::Transformers {
                        view! {
                            <RepoPullStep
                                status=Signal::derive(move || {
                                    repo_pull_status.get().unwrap_or_else(|| RepoPullStatus {
                                        status: "running".to_string(),
                                        bytes_done: 0,
                                        total_bytes: None,
                                        error: None,
                                        context_length: None,
                                    })
                                })
                                starting=repo_pull_starting.into()
                                on_retry=Callback::new(move |_| {
                                    start_repo_pull_job(RepoPullSignals {
                                        repo_id,
                                        model_id,
                                        repo_pull_status,
                                        repo_pull_job_id,
                                        repo_pull_starting,
                                        wizard_step,
                                        error_msg,
                                        vllm_settings,
                                        vllm_existing,
                                        hf_metadata,
                                        cancelled,
                                    });
                                })
                                on_cancel=Callback::new(move |_| {
                                    let Some(jid) = repo_pull_job_id.get_untracked() else {
                                        return;
                                    };
                                    wasm_bindgen_futures::spawn_local(async move {
                                        let resp = delete_request(&format!(
                                            "/tama/v1/pulls/repo/{}",
                                            jid
                                        ))
                                        .send()
                                        .await;
                                        match resp {
                                            Ok(r) => {
                                                if handle_response(&r) {
                                                    return;
                                                }
                                                if (200..300).contains(&r.status()) {
                                                    // Optimistic update; the poll loop confirms.
                                                    repo_pull_status.update(|s| {
                                                        if let Some(st) = s {
                                                            st.status = "cancelled".to_string();
                                                        }
                                                    });
                                                }
                                            }
                                            Err(e) => {
                                                error_msg.set(Some(format!(
                                                    "Failed to cancel pull: {e}"
                                                )));
                                            }
                                        }
                                    });
                                })
                                on_back=Callback::new(move |_| {
                                    wizard_step.set(WizardStep::SelectQuants);
                                })
                                on_continue=Callback::new(move |_| {
                                    wizard_step.set(WizardStep::SetContext);
                                })
                            />
                        }
                        .into_any()
                    } else {
                        view! {
                            <PullStep
                                pull_jobs=pull_jobs.into()
                                on_close=on_close
                                error_msg=error_msg
                            />
                        }
                        .into_any()
                    }
                }

                WizardStep::Done => view! {
                    <DoneStep
                        pull_jobs=pull_jobs.into()
                        on_close=on_close
                    />
                }.into_any(),
            }}
        </div>
    }
}

/// Helper: advance to Done step when all jobs are terminal AND we're past the Downloading step.
/// The Downloading → SetContext transition is handled by the dedicated Effect, not this function.
#[cfg(not(feature = "ssr"))]
fn advance_if_all_terminal(dj: &RwSignal<Vec<JobProgress>>, ws: &RwSignal<WizardStep>) {
    let jobs = dj.get_untracked();
    let current_step = ws.get_untracked();
    // Only advance to Done if we're on SetContext (user already configured settings).
    // If still on Downloading, let the transition Effect handle Downloading → SetContext.
    if current_step != WizardStep::SetContext {
        return;
    }
    if !jobs.is_empty()
        && jobs
            .iter()
            .all(|j| j.status == "completed" || j.status == "failed")
    {
        ws.set(WizardStep::Done);
    }
}

/// Subscribe to the global pull events SSE stream and update job progress.
/// Replaces per-job SSE streams + polling fallback with a single EventSource.
#[cfg(not(feature = "ssr"))]
fn spawn_pull_events_listener(
    entries: Vec<PullJobEntry>,
    dj: RwSignal<Vec<JobProgress>>,
    ws: RwSignal<WizardStep>,
    cancel: RwSignal<bool>,
    es_ref: RwSignal<Option<web_sys::EventSource>>,
) {
    let job_ids: std::collections::HashSet<String> =
        entries.iter().map(|e| e.job_id.clone()).collect();

    let es = match web_sys::EventSource::new("/tama/v1/pulls/events") {
        Ok(es) => es,
        Err(e) => {
            web_sys::console::warn_1(&format!("[events] failed to connect: {:?}", e).into());
            return;
        }
    };

    // Register handlers for each event type
    for event_name in [
        "Started",
        "Progress",
        "Verifying",
        "Completed",
        "Failed",
        "Cancelled",
    ] {
        let es = es.clone();
        let job_ids = job_ids.clone();
        // RwSignal is Copy — no clone needed
        let event_name = event_name.to_string();
        let event_name_for_listener = event_name.clone();

        let closure =
            wasm_bindgen::closure::Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
                if cancel.get_untracked() {
                    return;
                }

                let data = match event.data().as_string() {
                    Some(d) => d,
                    None => return,
                };

                // Parse as generic JSON to extract job_id
                let json: serde_json::Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(e) => {
                        web_sys::console::warn_1(&format!("[events] parse error: {}", e).into());
                        return;
                    }
                };

                let job_id = match json.get("job_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return,
                };

                // Only process events for our jobs
                if !job_ids.contains(job_id) {
                    return;
                }

                // Update job progress based on event type
                dj.update(|jobs| {
                    if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job_id) {
                        match event_name.as_str() {
                            "Started" => {
                                j.status = "running".to_string();
                                if let Some(tb) = json.get("total_bytes").and_then(|v| v.as_u64()) {
                                    j.total_bytes = Some(tb);
                                }
                            }
                            "Progress" => {
                                j.status = "running".to_string();
                                if let Some(bd) = json.get("bytes_pulled").and_then(|v| v.as_u64())
                                {
                                    j.bytes_pulled = bd;
                                }
                                if let Some(tb) = json.get("total_bytes").and_then(|v| v.as_u64()) {
                                    j.total_bytes = Some(tb);
                                }
                            }
                            "Verifying" => {
                                j.status = "verifying".to_string();
                            }
                            "Completed" => {
                                j.status = "completed".to_string();
                                if let Some(sb) = json.get("size_bytes").and_then(|v| v.as_u64()) {
                                    j.bytes_pulled = sb;
                                    // Use size_bytes as total if we never got it from Progress
                                    if j.total_bytes.is_none() {
                                        j.total_bytes = Some(sb);
                                    }
                                }
                            }
                            "Failed" => {
                                j.status = "failed".to_string();
                                if let Some(err) = json.get("error").and_then(|v| v.as_str()) {
                                    j.error = Some(err.to_string());
                                }
                            }
                            "Cancelled" => {
                                j.status = "failed".to_string();
                            }
                            _ => {}
                        }
                    }
                });

                // Check if all jobs are terminal
                advance_if_all_terminal(&dj, &ws);
            }) as Box<dyn FnMut(_)>);
        let _ = es.add_event_listener_with_callback(
            &event_name_for_listener,
            closure.as_ref().unchecked_ref(),
        );
        closure.forget(); // Keep the closure alive
    }

    // Error handler — detect auth failures on the EventSource.
    let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
        crate::utils::sse_session_check();
    });
    es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();

    // Store EventSource handle so on_cleanup can close it on unmount.
    es_ref.set(Some(es.clone()));
}

// ── Whole-repo (transformers) pull plumbing ─────────────────────────────────

/// The wizard signals touched by the whole-repo pull flow. Grouped so the
/// start helper's parameter list stays short (mirrors `SearchSignals`).
struct RepoPullSignals {
    repo_id: RwSignal<String>,
    model_id: RwSignal<Option<u32>>,
    repo_pull_status: RwSignal<Option<RepoPullStatus>>,
    repo_pull_job_id: RwSignal<Option<String>>,
    repo_pull_starting: RwSignal<bool>,
    wizard_step: RwSignal<WizardStep>,
    error_msg: RwSignal<Option<String>>,
    vllm_settings: RwSignal<VllmWizardSettings>,
    vllm_existing: RwSignal<Option<serde_json::Value>>,
    hf_metadata: RwSignal<HfModelMetadata>,
    cancelled: RwSignal<bool>,
}

/// Extract the server's `error.message` from a parsed JSON error body,
/// falling back to `fallback` when the body is missing (JSON parse failed)
/// or has no string `message`. Shared by the repo-pull start path and the
/// vLLM save path so both surface identical server messages.
fn server_error_message(body: Option<&serde_json::Value>, fallback: String) -> String {
    body.and_then(|v| v.get("error"))
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .unwrap_or(fallback)
}

/// Start a whole-repo pull job (`POST /tama/v1/pulls/repo`): on success,
/// seed the status signal, advance to Downloading, and start the status poll
/// loop. On failure (422/409/502), show the server's message and return to
/// the repo-input step. Shared by the Confirm step's "Start Download" button
/// and the RepoPullStep's "Retry" button.
fn start_repo_pull_job(sigs: RepoPullSignals) {
    let repo_id = sigs.repo_id;
    let model_id = sigs.model_id;
    let repo_pull_status = sigs.repo_pull_status;
    let repo_pull_job_id = sigs.repo_pull_job_id;
    let repo_pull_starting = sigs.repo_pull_starting;
    let wizard_step = sigs.wizard_step;
    let error_msg = sigs.error_msg;
    let vllm_settings = sigs.vllm_settings;
    let vllm_existing = sigs.vllm_existing;
    let hf_metadata = sigs.hf_metadata;
    let cancelled = sigs.cancelled;

    // In-flight guard: a fast double-click on "Start Download" (or "Retry")
    // would otherwise fire two POSTs; the second 409s and bounces the wizard
    // back to RepoInput while job 1 keeps running in the background. The
    // check + set are synchronous so a second click in the same tick is
    // rejected before the first request is even sent.
    if repo_pull_starting.get_untracked() {
        return;
    }
    repo_pull_starting.set(true);

    wasm_bindgen_futures::spawn_local(async move {
        // Dropped at the end of this block — resets the guard on every
        // exit path (see ReleaseStartingGuard).
        let _release = ReleaseStartingGuard(repo_pull_starting);
        let body = RepoPullStartRequest {
            repo_id: repo_id.get(),
            model_id: model_id.get_untracked(),
        };
        let resp = match post_request("/tama/v1/pulls/repo").json(&body) {
            Ok(req) => req.send().await,
            Err(e) => {
                error_msg.set(Some(format!("Failed to build request: {e}")));
                return;
            }
        };
        match resp {
            Ok(r) => {
                if handle_response(&r) {
                    return;
                }
                if (200..300).contains(&r.status()) {
                    match r.json::<serde_json::Value>().await {
                        Ok(json) => {
                            let job_id = json
                                .get("job_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let total_bytes = json.get("total_bytes").and_then(|v| v.as_u64());
                            repo_pull_job_id.set(Some(job_id.clone()));
                            repo_pull_status.set(Some(RepoPullStatus {
                                status: "running".to_string(),
                                bytes_done: 0,
                                total_bytes,
                                error: None,
                                context_length: None,
                            }));
                            wizard_step.set(WizardStep::Downloading);
                            spawn_repo_poll(
                                job_id,
                                repo_pull_status,
                                repo_pull_job_id,
                                wizard_step,
                                SetContextSignals {
                                    vllm_settings,
                                    vllm_existing,
                                    model_id,
                                    repo_id,
                                    hf_metadata,
                                },
                                cancelled,
                            );
                        }
                        Err(e) => {
                            error_msg.set(Some(format!("Failed to parse response: {e}")));
                        }
                    }
                } else {
                    // 422 (invalid repo id / missing hf CLI / repo not
                    // found), 409 (duplicate), or 502 (upstream) — show the
                    // server's message and return to the repo-input step.
                    let body = r.json::<serde_json::Value>().await.ok();
                    let message = server_error_message(
                        body.as_ref(),
                        format!("Failed to start download (HTTP {})", r.status()),
                    );
                    error_msg.set(Some(message));
                    wizard_step.set(WizardStep::RepoInput);
                }
            }
            Err(e) => {
                error_msg.set(Some(format!("Request failed: {e}")));
            }
        }
    });
}

/// Outcome of one poll-loop write attempt on the shared status signal.
enum RepoPollWriteOutcome {
    /// The polled job is no longer the current job (a Retry seeded a new
    /// one) — the loop must stop and must not write.
    Stale,
    /// The status was written; the job is still running — keep polling.
    Continue,
    /// A terminal status was written — stop polling.
    Terminal,
}

/// Write one fetched poll status to the shared `repo_pull_status` signal,
/// gated on `polled` still being the wizard's current job.
///
/// After a Retry, a new job is seeded while the old loop may still be
/// asleep; an ungated write would clobber the new job's state (the old
/// job's `cancelled`/terminal status flashes in the UI and a Retry during
/// that window 409s). Every shared write from the loop goes through here.
fn apply_repo_poll_status(
    polled: &str,
    current: Option<&str>,
    fetched: &RepoPullStatus,
    status: RwSignal<Option<RepoPullStatus>>,
) -> RepoPollWriteOutcome {
    if current != Some(polled) {
        return RepoPollWriteOutcome::Stale;
    }
    status.set(Some(fetched.clone()));
    if fetched.is_terminal() {
        RepoPollWriteOutcome::Terminal
    } else {
        RepoPollWriteOutcome::Continue
    }
}

/// Write a terminal `failed` status for a lost repo-pull job through the
/// same guarded path as normal poll writes (`apply_repo_poll_status`), so a
/// stale loop (a Retry already seeded a newer job) can't clobber the new
/// job's state.
fn surface_repo_poll_lost(
    polled: &str,
    current: Option<&str>,
    status: RwSignal<Option<RepoPullStatus>>,
) {
    let lost = RepoPullStatus {
        status: "failed".to_string(),
        bytes_done: 0,
        total_bytes: None,
        error: Some(REPO_PULL_JOB_LOST_MESSAGE.to_string()),
        context_length: None,
    };
    // `failed` is terminal — the guarded write reports Terminal (or Stale
    // when a Retry replaced this job); the loop stops either way.
    let _ = apply_repo_poll_status(polled, current, &lost, status);
}

/// Record one failed (non-2xx / transport-error) poll of the repo-pull job.
/// Below `REPO_POLL_FAILURE_THRESHOLD` consecutive failures the loop keeps
/// polling; at or above it the job is presumed lost and
/// `surface_repo_poll_lost` writes the terminal error. Returns `true` when
/// the loop must stop.
fn repo_poll_consecutive_failures_stop(
    consecutive: &mut u32,
    polled: &str,
    current: Option<&str>,
    status: RwSignal<Option<RepoPullStatus>>,
) -> bool {
    *consecutive += 1;
    match repo_poll_consecutive_failures_action(*consecutive, REPO_POLL_FAILURE_THRESHOLD) {
        RepoPollFailureAction::KeepPolling => false,
        RepoPollFailureAction::SurfaceError => {
            surface_repo_poll_lost(polled, current, status);
            true
        }
    }
}

/// RAII release for the `repo_pull_starting` in-flight guard: held for the
/// lifetime of `start_repo_pull_job`'s async block and dropped at the end,
/// so every exit path (2xx, non-2xx, parse error, 401, transport error)
/// resets the guard without each branch having to remember.
struct ReleaseStartingGuard(RwSignal<bool>);

impl Drop for ReleaseStartingGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Signals touched by the SetContext prefill after a whole-repo pull
/// completes. Grouped so `spawn_repo_poll`'s parameter list stays short
/// (mirrors `RepoPullSignals`).
struct SetContextSignals {
    vllm_settings: RwSignal<VllmWizardSettings>,
    vllm_existing: RwSignal<Option<serde_json::Value>>,
    model_id: RwSignal<Option<u32>>,
    repo_id: RwSignal<String>,
    hf_metadata: RwSignal<HfModelMetadata>,
}

/// Poll `GET /tama/v1/pulls/repo/{job_id}` every 1.5 s until the job reaches
/// a terminal state. Failed polls (non-2xx / transport errors) are transient
/// — the consecutive-failure counter resets on any 2xx response — but after
/// `REPO_POLL_FAILURE_THRESHOLD` consecutive failures the job is presumed
/// lost (e.g. a server restart cleared the in-memory job map) and the loop
/// writes a terminal `failed` status through the guarded path and stops, so
/// the RepoPullStep renders its failed UI (Retry + Back) instead of polling
/// forever. On completion, prefill the vLLM settings from the model's stored
/// `vllm` config (fetched via `GET /tama/v1/models/{id}`; a failed fetch
/// falls back to the job's config.json context length, else the repo
/// metadata's `hf_context_length`) and advance to SetContext.
/// Failed/cancelled jobs stay on Downloading — the RepoPullStep renders the
/// error and the retry/back buttons.
fn spawn_repo_poll(
    job_id: String,
    repo_pull_status: RwSignal<Option<RepoPullStatus>>,
    repo_pull_job_id: RwSignal<Option<String>>,
    wizard_step: RwSignal<WizardStep>,
    prefill: SetContextSignals,
    cancelled: RwSignal<bool>,
) {
    wasm_bindgen_futures::spawn_local(async move {
        let url = format!("/tama/v1/pulls/repo/{}", job_id);
        // Consecutive failed (non-2xx / transport-error) polls — reset on
        // any 2xx response. After `REPO_POLL_FAILURE_THRESHOLD` in a row
        // the job is presumed lost and the loop surfaces a terminal error.
        let mut consecutive_failures: u32 = 0;
        loop {
            if cancelled.get_untracked() {
                break;
            }
            match get_request(&url).send().await {
                Ok(r) => {
                    if handle_response(&r) {
                        // 401 — the page is navigating to /login; stop polling.
                        break;
                    }
                    if (200..300).contains(&r.status()) {
                        // A live job — reset the consecutive-failure count.
                        consecutive_failures = 0;
                        match r.json::<RepoPullStatus>().await {
                            Ok(st) => {
                                let current = repo_pull_job_id.get_untracked();
                                match apply_repo_poll_status(
                                    &job_id,
                                    current.as_deref(),
                                    &st,
                                    repo_pull_status,
                                ) {
                                    RepoPollWriteOutcome::Stale => {
                                        // A Retry replaced this job while we
                                        // were asleep — the new loop owns
                                        // the shared state; stop.
                                        break;
                                    }
                                    RepoPollWriteOutcome::Terminal => break,
                                    RepoPollWriteOutcome::Continue => {}
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to parse repo pull status: {e}");
                            }
                        }
                    } else {
                        log::warn!("Repo pull status poll returned HTTP {}", r.status());
                        if repo_poll_consecutive_failures_stop(
                            &mut consecutive_failures,
                            &job_id,
                            repo_pull_job_id.get_untracked().as_deref(),
                            repo_pull_status,
                        ) {
                            break;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Repo pull status poll failed: {e}");
                    if repo_poll_consecutive_failures_stop(
                        &mut consecutive_failures,
                        &job_id,
                        repo_pull_job_id.get_untracked().as_deref(),
                        repo_pull_status,
                    ) {
                        break;
                    }
                }
            }
            gloo_timers::future::sleep(std::time::Duration::from_millis(1500)).await;
        }
        // A Retry may have replaced this job after the loop exited — a stale
        // loop must not prefill settings or advance the step using the last
        // stored status (which now belongs to the new job).
        if repo_pull_job_id.get_untracked().as_deref() != Some(job_id.as_str()) {
            return;
        }
        // On completion, prefill the vLLM settings from the model's stored
        // vllm config (so the overlay save can't wipe fields the wizard
        // doesn't expose) and advance to SetContext.
        if let Some(st) = repo_pull_status.get_untracked() {
            if st.is_completed() {
                let ctx_len = st
                    .context_length
                    .or_else(|| prefill.hf_metadata.get_untracked().hf_context_length);
                let model_key = match prefill.model_id.get_untracked() {
                    Some(id) => id.to_string(),
                    None => crate::utils::config_key_from_repo_id(&prefill.repo_id.get_untracked()),
                };
                match fetch_existing_vllm(&model_key).await {
                    Ok(existing) => {
                        prefill.vllm_existing.set(Some(existing.clone()));
                        prefill
                            .vllm_settings
                            .set(vllm_settings_prefill(&existing, ctx_len));
                    }
                    Err(e) => {
                        // Transient fetch failure — keep the previous
                        // behavior: empty settings + context-length prefill.
                        log::warn!(
                            "Failed to fetch existing vllm config for '{}': {e}; \
                             falling back to context-length prefill",
                            model_key
                        );
                        prefill.vllm_existing.set(None);
                        prefill.vllm_settings.update(|s| {
                            if s.max_model_len.is_none() {
                                s.max_model_len = ctx_len;
                            }
                        });
                    }
                }
                if wizard_step.get_untracked() == WizardStep::Downloading {
                    wizard_step.set(WizardStep::SetContext);
                }
            }
        }
    });
}

/// Fetch the model's stored `vllm` config object from
/// `GET /tama/v1/models/{model_key}` for the SetContext prefill. Returns
/// `Value::Null` when the model has no stored vllm config (fresh pull). Any
/// transport/parse/HTTP error is returned as `Err` so the caller can fall
/// back to the previous empty-prefill behavior.
async fn fetch_existing_vllm(model_key: &str) -> Result<serde_json::Value, String> {
    let resp = get_request(&format!("/tama/v1/models/{}", model_key))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if handle_response(&resp) {
        // 401 — the page is navigating to /login; treat as a failure so no
        // SetContext transition is attempted with stale state.
        return Err("401 — redirected to login".to_string());
    }
    if !(200..300).contains(&resp.status()) {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;
    Ok(json.get("vllm").cloned().unwrap_or(serde_json::Value::Null))
}

// ── Shared search plumbing (search callback + is_open reset path) ────────────

/// Outcome of the quant-listing fetch in `fetch_repo_listing`.
enum ListingOutcome {
    /// The listing parsed successfully.
    Ok(Vec<QuantEntry>),
    /// `handle_response` triggered a 401 redirect — the page is navigating
    /// away, so the caller must do nothing.
    Redirect,
    /// The request or its JSON parse failed; holds the message to display.
    Failed(String),
}

/// Fetch a repo's quant listing and HF metadata in parallel.
///
/// Shared by the `on_search` callback and the `is_open` reset path so the two
/// search flows cannot drift. The metadata fetch is soft-failing: a failed
/// request just yields `None` (the branch decision degrades to the listing).
async fn fetch_repo_listing(rid: &str) -> (ListingOutcome, Option<HfModelMetadata>) {
    let quants_url = format!("/tama/v1/hf/{}", rid);
    let metadata_url = format!("/tama/v1/hf/{}/metadata", rid);
    let quants_future = get_request(&quants_url).send();
    let metadata_future = get_request(&metadata_url).send();

    let (quants_resp, metadata_resp) = futures_util::join!(quants_future, metadata_future);

    // Parse metadata (soft failure — flow continues without it)
    let metadata = match metadata_resp {
        Ok(r) => {
            if handle_response(&r) {
                None
            } else if (200..300).contains(&r.status()) {
                match r.json::<HfModelMetadata>().await {
                    Ok(m) => Some(m),
                    Err(e) => {
                        log::warn!("Failed to parse metadata: {}", e);
                        None
                    }
                }
            } else {
                log::warn!("Failed to fetch metadata for '{}'", rid);
                None
            }
        }
        Err(_) => {
            log::warn!("Failed to fetch metadata for '{}'", rid);
            None
        }
    };

    // Parse the quant listing
    let listing = match quants_resp {
        Ok(resp) => {
            if handle_response(&resp) {
                ListingOutcome::Redirect
            } else {
                match resp.json::<Vec<QuantEntry>>().await {
                    Ok(quants) => ListingOutcome::Ok(quants),
                    Err(e) => ListingOutcome::Failed(format!("Failed to parse response: {e}")),
                }
            }
        }
        Err(e) => ListingOutcome::Failed(format!("Request failed: {e}")),
    };

    (listing, metadata)
}

/// The wizard signals touched by search-result handling. Grouped so the
/// `on_search` callback and the `is_open` reset path can share one handler.
struct SearchSignals {
    model_id: RwSignal<Option<u32>>,
    hf_metadata: RwSignal<HfModelMetadata>,
    available_quants: RwSignal<Vec<QuantEntry>>,
    available_mmprojs: RwSignal<Vec<QuantEntry>>,
    available_mtps: RwSignal<Vec<QuantEntry>>,
    branch: RwSignal<WizardBranch>,
    wizard_step: RwSignal<WizardStep>,
    error_msg: RwSignal<Option<String>>,
}

/// Shared search-result handling, called by both the `on_search` callback and
/// the `is_open` reset path:
///
/// 1. Resolve the branch from `hf_format` + the listing (`resolve_branch`).
///    No recognizable model files → set the "no model files" error, stay on
///    `RepoInput`, and create NO stub.
/// 2. Create the stub model with the branch-correct backend (`llama_cpp` for
///    GGUF, `vllm` for transformers) — only after the branch decision.
/// 3. Store the metadata and route to `SelectQuants`.
async fn apply_search_result(
    rid: &str,
    listing: ListingOutcome,
    metadata: Option<HfModelMetadata>,
    sigs: SearchSignals,
) {
    let quants = match listing {
        // `handle_response` redirected to /login — the page is leaving.
        ListingOutcome::Redirect => return,
        ListingOutcome::Failed(msg) => {
            sigs.error_msg.set(Some(msg));
            sigs.wizard_step.set(WizardStep::RepoInput);
            return;
        }
        ListingOutcome::Ok(quants) => quants,
    };

    // Decide the branch BEFORE creating any model row.
    let branch = match resolve_branch(
        metadata.as_ref().and_then(|m| m.hf_format.as_deref()),
        !quants.is_empty(),
    ) {
        Some(b) => b,
        None => {
            sigs.error_msg.set(Some(
                "No model files found in this repo (no .gguf or .safetensors files). Check the repo ID and try again."
                    .to_string(),
            ));
            sigs.wizard_step.set(WizardStep::RepoInput);
            return;
        }
    };

    // Create stub model with metadata
    let backend = match branch {
        WizardBranch::Gguf => "llama_cpp",
        WizardBranch::Transformers => "vllm",
    };
    let stub_body = serde_json::json!({
        "repo_id": &rid,
        "backend": backend,
        "metadata": metadata,
    });
    let stub_resp = post_request("/tama/v1/models")
        .json(&stub_body)
        .unwrap()
        .send()
        .await;

    // Handle stub creation response
    match stub_resp {
        Ok(r) => {
            if handle_response(&r) {
                return;
            }
            if (200..300).contains(&r.status()) {
                if let Ok(json) = r.json::<serde_json::Value>().await {
                    if let Some(id) = json.get("id").and_then(|v| v.as_u64()) {
                        sigs.model_id.set(Some(id as u32));
                    }
                }
            } else {
                log::warn!("Failed to create stub model for '{}'", rid);
            }
        }
        Err(_) => {
            log::warn!("Failed to create stub model for '{}'", rid);
        }
    }

    // Store metadata for later use
    if let Some(m) = metadata {
        sigs.hf_metadata.set(m);
    }

    sigs.branch.set(branch);

    let mut model_quants: Vec<QuantEntry> = Vec::new();
    let mut mmprojs: Vec<QuantEntry> = Vec::new();
    let mut mtps: Vec<QuantEntry> = Vec::new();
    for q in quants {
        match q.kind {
            QuantKind::Mmproj => mmprojs.push(q),
            QuantKind::Mtp => mtps.push(q),
            _ => model_quants.push(q),
        }
    }
    sigs.available_quants.set(model_quants);
    sigs.available_mmprojs.set(mmprojs);
    sigs.available_mtps.set(mtps);
    // The SelectQuants arm renders the Confirm step for the transformers
    // branch and the quant-selection step for the GGUF branch.
    sigs.wizard_step.set(WizardStep::SelectQuants);
}

// ── Pure helper function (extracted for testability) ─────────────────────────

/// Build the list of `CompletedQuant` from pull jobs, filtering to only
/// primary shard filenames (those that appear as `filename` in any of the
/// three quant listings). Non-primary shard filenames (which only appear in
/// `shards` vectors) are excluded to prevent them from overwriting the primary
/// file reference in the model editor.
fn build_completed_quants(
    jobs: &[JobProgress],
    quants_listing: &[QuantEntry],
    mmprojs: &[QuantEntry],
    mtps: &[QuantEntry],
    repo: &str,
) -> Vec<CompletedQuant> {
    jobs.iter()
        .filter(|j| j.status == "completed")
        .filter(|j| {
            quants_listing.iter().any(|q| q.filename == j.filename)
                || mmprojs.iter().any(|q| q.filename == j.filename)
                || mtps.iter().any(|q| q.filename == j.filename)
        })
        .map(|j| {
            let entry = quants_listing.iter().find(|q| q.filename == j.filename);
            let quant = entry
                .and_then(|e| e.quant.clone())
                .or_else(|| infer_quant_from_filename(&j.filename));
            CompletedQuant {
                repo_id: repo.to_string(),
                filename: j.filename.clone(),
                quant,
                size_bytes: Some(j.bytes_pulled),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::pull_wizard::QuantKind;

    fn make_job(job_id: &str, filename: &str, status: &str, bytes: u64) -> JobProgress {
        JobProgress {
            job_id: job_id.to_string(),
            filename: filename.to_string(),
            status: status.to_string(),
            bytes_pulled: bytes,
            total_bytes: None,
            error: None,
        }
    }

    fn make_quant(filename: &str) -> QuantEntry {
        QuantEntry {
            filename: filename.to_string(),
            quant: None,
            size_bytes: None,
            kind: QuantKind::Model,
            shards: vec![],
        }
    }

    fn make_repo_pull_status(status: &str) -> RepoPullStatus {
        RepoPullStatus {
            status: status.to_string(),
            bytes_done: 0,
            total_bytes: None,
            error: None,
            context_length: None,
        }
    }

    /// B1: a Retry seeds job-2 while the job-1 poll loop is still sleeping.
    /// When the stale loop wakes with the old job's `cancelled` status, the
    /// write must be rejected — the new job's seeded `running` status must
    /// survive so the UI doesn't flash "cancelled" mid-pull.
    #[test]
    fn test_repo_poll_status_write_rejected_for_stale_job() {
        let status = RwSignal::new(Some(make_repo_pull_status("running")));

        let decision = apply_repo_poll_status(
            "job-1",
            Some("job-2"),
            &make_repo_pull_status("cancelled"),
            status,
        );

        assert!(matches!(decision, RepoPollWriteOutcome::Stale));
        let st = status.get().expect("new job's seeded status must survive");
        assert_eq!(st.status, "running");
    }

    /// The loop for the current job writes the status and keeps polling while
    /// the job is still running.
    #[test]
    fn test_repo_poll_status_write_current_job_running() {
        let status = RwSignal::new(None);

        let decision = apply_repo_poll_status(
            "job-1",
            Some("job-1"),
            &make_repo_pull_status("running"),
            status,
        );

        assert!(matches!(decision, RepoPollWriteOutcome::Continue));
        assert_eq!(status.get().unwrap().status, "running");
    }

    /// The loop for the current job writes the terminal status and reports
    /// that the loop should stop.
    #[test]
    fn test_repo_poll_status_write_current_job_terminal() {
        let status = RwSignal::new(None);
        let fetched = make_repo_pull_status("completed");

        let decision = apply_repo_poll_status("job-1", Some("job-1"), &fetched, status);

        assert!(matches!(decision, RepoPollWriteOutcome::Terminal));
        assert_eq!(status.get().unwrap().status, "completed");
    }

    /// No job seeded yet — nothing may write (defensive: a loop must never
    /// run without a job id, but if one does, it must not clobber state).
    #[test]
    fn test_repo_poll_status_write_rejected_without_current_job() {
        let status = RwSignal::new(Some(make_repo_pull_status("running")));

        let decision =
            apply_repo_poll_status("job-1", None, &make_repo_pull_status("running"), status);

        assert!(matches!(decision, RepoPollWriteOutcome::Stale));
        assert_eq!(status.get().unwrap().status, "running");
    }

    /// B2: the in-flight guard's release drops back to `false` when the
    /// async start block finishes, so every exit path (2xx, non-2xx, parse
    /// error, 401, transport error) resets it without each branch having to
    /// remember.
    #[test]
    fn test_release_starting_guard_resets_on_drop() {
        let starting = RwSignal::new(false);
        starting.set(true);

        {
            let _guard = ReleaseStartingGuard(starting);
            assert!(starting.get(), "guard holds while in flight");
        }

        assert!(!starting.get(), "guard must release on drop");
    }

    /// C: for the CURRENT job the job-lost write lands as a terminal `failed`
    /// status carrying the job-lost message — the RepoPullStep renders its
    /// failed UI (Retry + Back) from this state.
    #[test]
    fn test_repo_poll_lost_write_current_job() {
        let status = RwSignal::new(Some(make_repo_pull_status("running")));

        surface_repo_poll_lost("job-1", Some("job-1"), status);

        let st = status.get().expect("job-lost status must be written");
        assert_eq!(st.status, "failed");
        assert!(st.is_terminal());
        assert_eq!(st.error.as_deref(), Some(REPO_PULL_JOB_LOST_MESSAGE));
    }

    /// C: the job-lost write is guarded like any other poll write — a Retry
    /// that seeded a newer job while this loop was running must survive; the
    /// stale loop's terminal `failed` write is rejected.
    #[test]
    fn test_repo_poll_lost_write_rejected_for_stale_job() {
        let status = RwSignal::new(Some(make_repo_pull_status("running")));

        surface_repo_poll_lost("job-1", Some("job-2"), status);

        let st = status
            .get()
            .expect("newer job's seeded status must survive");
        assert_eq!(st.status, "running");
        assert!(st.error.is_none());
    }

    /// C: the loop keeps polling through 4 consecutive failed polls and
    /// stops on the 5th, having written the terminal error for the current
    /// job.
    #[test]
    fn test_repo_poll_consecutive_failures_stop_at_threshold() {
        let status = RwSignal::new(Some(make_repo_pull_status("running")));
        let mut consecutive = 0;

        let mut stops = Vec::new();
        for _ in 0..REPO_POLL_FAILURE_THRESHOLD {
            stops.push(repo_poll_consecutive_failures_stop(
                &mut consecutive,
                "job-1",
                Some("job-1"),
                status,
            ));
        }

        assert_eq!(consecutive, REPO_POLL_FAILURE_THRESHOLD);
        assert_eq!(
            stops,
            [false, false, false, false, true],
            "only the 5th consecutive failure stops the loop"
        );
        let st = status
            .get()
            .expect("terminal error must be written on stop");
        assert_eq!(st.status, "failed");
        assert!(st.is_terminal());
    }

    /// When a sharded quant is pulled, non-primary shard jobs (whose filenames
    /// only appear in `shards`, not as a primary `filename`) should be filtered
    /// out. Only the primary shard's CompletedQuant should be emitted.
    #[test]
    fn test_filter_completed_only_primary_shards() {
        let jobs = vec![
            make_job("job-1", "model-Q4_K_M.gguf", "completed", 100),
            make_job("job-2", "model-Q4_K_M-00001-of-00003.gguf", "completed", 50),
            make_job("job-3", "model-Q4_K_M-00002-of-00003.gguf", "completed", 50),
        ];
        // Only the primary filename is in the listing; shard filenames are NOT.
        let quants_listing = vec![make_quant("model-Q4_K_M.gguf")];
        let mmprojs: Vec<QuantEntry> = vec![];
        let mtps: Vec<QuantEntry> = vec![];

        let completed =
            build_completed_quants(&jobs, &quants_listing, &mmprojs, &mtps, "owner/repo");

        assert_eq!(completed.len(), 1, "only primary shard should be emitted");
        assert_eq!(completed[0].filename, "model-Q4_K_M.gguf");
        assert_eq!(completed[0].repo_id, "owner/repo");
        assert_eq!(completed[0].size_bytes, Some(100));
    }

    /// Failed jobs should be filtered out entirely.
    #[test]
    fn test_filter_skips_failed_jobs() {
        let jobs = vec![make_job("job-1", "model-Q4_K_M.gguf", "failed", 0)];
        let quants_listing = vec![make_quant("model-Q4_K_M.gguf")];

        let completed = build_completed_quants(&jobs, &quants_listing, &[], &[], "owner/repo");
        assert!(
            completed.is_empty(),
            "failed jobs should produce no CompletedQuant"
        );
    }

    /// Completed jobs whose filename doesn't match any listing (neither primary
    /// nor mmproj nor mtp) should be filtered out.
    #[test]
    fn test_filter_unmatched_filename_excluded() {
        let jobs = vec![make_job("job-1", "unknown-file.gguf", "completed", 42)];
        let quants_listing: Vec<QuantEntry> = vec![];
        let mmprojs: Vec<QuantEntry> = vec![];
        let mtps: Vec<QuantEntry> = vec![];

        let completed =
            build_completed_quants(&jobs, &quants_listing, &mmprojs, &mtps, "owner/repo");
        assert!(
            completed.is_empty(),
            "unmatched filename should be filtered out"
        );
    }

    /// mmproj and mtp completed jobs should be included when their filename
    /// matches an entry in the respective listing.
    #[test]
    fn test_filter_includes_mmproj_and_mtp() {
        let jobs = vec![
            make_job("j1", "mmproj.gguf", "completed", 30),
            make_job("j2", "mtp.gguf", "completed", 20),
        ];
        let mmprojs = vec![make_quant("mmproj.gguf")];
        let mtps = vec![make_quant("mtp.gguf")];

        let completed = build_completed_quants(&jobs, &[], &mmprojs, &mtps, "owner/repo");
        assert_eq!(completed.len(), 2);
        assert!(completed.iter().any(|c| c.filename == "mmproj.gguf"));
        assert!(completed.iter().any(|c| c.filename == "mtp.gguf"));
    }

    /// A JSON body carrying the server's `error.message` surfaces it verbatim
    /// instead of the bare HTTP-status fallback.
    #[test]
    fn test_server_error_message_extracts_message() {
        let body = serde_json::json!({"error": {"message": "vllm: invalid kv_cache_dtype"}});
        assert_eq!(
            server_error_message(
                Some(&body),
                "Failed to save settings (HTTP 422)".to_string()
            ),
            "vllm: invalid kv_cache_dtype"
        );
    }

    /// A missing body, a body without an `error` object, an `error` without
    /// a `message`, and a non-string `message` all fall back to the caller's
    /// message — mirroring the pre-extraction start-path semantics.
    #[test]
    fn test_server_error_message_falls_back() {
        assert_eq!(
            server_error_message(None, "fallback".to_string()),
            "fallback"
        );
        let no_error = serde_json::json!({"ok": true});
        assert_eq!(
            server_error_message(Some(&no_error), "fallback".to_string()),
            "fallback"
        );
        let no_message = serde_json::json!({"error": {"code": 422}});
        assert_eq!(
            server_error_message(Some(&no_message), "fallback".to_string()),
            "fallback"
        );
        let non_string_message = serde_json::json!({"error": {"message": 422}});
        assert_eq!(
            server_error_message(Some(&non_string_message), "fallback".to_string()),
            "fallback"
        );
    }
}
