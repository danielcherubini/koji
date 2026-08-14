use crate::components::pull_wizard::{format_bytes, RepoPullStatus};
use leptos::prelude::*;

/// Transformers-branch download step: shows whole-repo pull progress
/// (polled, single job) with cancel / retry / continue actions.
#[component]
pub fn RepoPullStep(
    /// Live status of the repo pull job (seeded `running` before start).
    status: Signal<RepoPullStatus>,
    /// True while a pull start request is in flight — disables
    /// "Retry" so a fast double-click can't fire a second
    /// (409-rejecting) POST.
    starting: Signal<bool>,
    /// Called when the user clicks "Retry" after a failure or cancellation.
    on_retry: Callback<()>,
    /// Called when the user clicks "Cancel" while the pull is running.
    on_cancel: Callback<()>,
    /// Called when the user clicks "Back".
    on_back: Callback<()>,
    /// Called when the user clicks "Configure vLLM" after completion.
    on_continue: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Downloading Repo"</h2>
            <p class="form-card__desc text-muted">
                "Pulling the whole repo with the hf CLI."
            </p>
        </div>

        // Running: progress bar + cancel.
        <Show when=move || status.get().status == "running">
            <div class="pull-jobs">
                <div class="pull-job-card card mb-2">
                    <div class="flex-between mb-1">
                        <span class="badge badge-info">
                            {move || {
                                let st = status.get();
                                let total = st.total_bytes.map(format_bytes).unwrap_or_else(|| "?".to_string());
                                format!("{}/ {}", format_bytes(st.bytes_done), total)
                            }}
                        </span>
                    </div>
                    <div class="progress-bar">
                        {move || {
                            let st = status.get();
                            let pct = st
                                .total_bytes
                                .filter(|&total| total > 0)
                                .map(|total| (st.bytes_done as f64 / total as f64 * 100.0) as u32);
                            if let Some(pct) = pct {
                                view! {
                                    <div
                                        class="progress-bar-fill"
                                        style=format!("width:{}%", pct)
                                    />
                                }.into_any()
                            } else {
                                view! {
                                    <div class="progress-bar-fill indeterminate" />
                                }.into_any()
                            }
                        }}
                    </div>
                </div>
            </div>
            <div class="form-actions mt-3">
                <button class="btn btn-secondary" on:click=move |_| on_cancel.run(())>
                    "Cancel"
                </button>
            </div>
        </Show>

        // Failed: error + retry/back.
        <Show when=move || status.get().status == "failed">
            <div class="alert alert--error mb-3">
                <span class="alert__icon">"✕"</span>
                <span>
                    {move || {
                        status
                            .get()
                            .error
                            .clone()
                            .unwrap_or_else(|| "Unknown error".to_string())
                    }}
                </span>
            </div>
            <div class="form-actions mt-3">
                <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                    "Back"
                </button>
                <button
                    class="btn btn-primary"
                    prop:disabled=move || starting.get()
                    on:click=move |_| on_retry.run(())
                >
                    "Retry"
                </button>
            </div>
        </Show>

        // Cancelled: info + retry/back.
        <Show when=move || status.get().status == "cancelled">
            <div class="alert alert--info mb-3">
                <span class="alert__icon">"ℹ"</span>
                <span>"Download cancelled."</span>
            </div>
            <div class="form-actions mt-3">
                <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                    "Back"
                </button>
                <button
                    class="btn btn-primary"
                    prop:disabled=move || starting.get()
                    on:click=move |_| on_retry.run(())
                >
                    "Retry"
                </button>
            </div>
        </Show>

        // Completed: normally transient — the wizard auto-advances to
        // SetContext when the job completes — but SetContext's "Back" lands
        // back here, so the state offers explicit exits: "Configure vLLM"
        // (primary) and "Back" (secondary).
        <Show when=move || status.get().status == "completed">
            <div class="alert alert--success mb-3">
                <span class="alert__icon">"✓"</span>
                <span>"Download complete"</span>
            </div>
            <div class="form-actions mt-3">
                <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                    "Back"
                </button>
                <button class="btn btn-primary" on:click=move |_| on_continue.run(())>
                    "Configure vLLM"
                </button>
            </div>
        </Show>
    }
}
