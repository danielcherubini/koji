use crate::components::pull_wizard::{format_bytes, HfModelMetadata};
use leptos::prelude::*;

/// Transformers-branch confirmation step: shows a summary of the repo's
/// metadata and lets the user start the whole-repo download.
#[component]
pub fn ConfirmStep(
    /// The HF repo id being pulled.
    repo_id: Signal<String>,
    /// Pre-fetched HF metadata (architecture, params, size, file count).
    metadata: Signal<HfModelMetadata>,
    /// True while a pull start request is in flight — disables
    /// "Start Download" so a fast double-click can't fire a second
    /// (409-rejecting) POST.
    starting: Signal<bool>,
    /// Called when the user clicks "Start Download".
    on_start: Callback<()>,
    /// Called when the user clicks "Back".
    on_back: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Confirm Download"</h2>
            <p class="form-card__desc text-muted">
                "Review the repo before downloading all of its files."
            </p>
        </div>

        <div class="alert alert--info mb-3">
            <span class="alert__icon">"ℹ"</span>
            <span>
                "This repo contains safetensors (transformers) weights. Tama will download the whole repo with the hf CLI and set it up as a vLLM model."
            </span>
        </div>

        <div class="form-section mb-3">
            <div class="flex-between mb-1">
                <span class="text-muted">"Repo"</span>
                <code>{move || repo_id.get()}</code>
            </div>
            <div class="flex-between mb-1">
                <span class="text-muted">"Architecture"</span>
                <span>
                    {move || {
                        metadata
                            .get()
                            .hf_architecture_type
                            .clone()
                            .unwrap_or_else(|| "—".to_string())
                    }}
                </span>
            </div>
            <div class="flex-between mb-1">
                <span class="text-muted">"Total parameters"</span>
                <span>
                    {move || {
                        metadata
                            .get()
                            .hf_total_params
                            .clone()
                            .unwrap_or_else(|| "—".to_string())
                    }}
                </span>
            </div>
            <div class="flex-between mb-1">
                <span class="text-muted">"Total size"</span>
                <span>
                    {move || {
                        metadata
                            .get()
                            .hf_total_size_bytes
                            .map(format_bytes)
                            .unwrap_or_else(|| "—".to_string())
                    }}
                </span>
            </div>
            <div class="flex-between mb-1">
                <span class="text-muted">"File count"</span>
                <span>
                    {move || {
                        metadata
                            .get()
                            .hf_file_count
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "—".to_string())
                    }}
                </span>
            </div>
        </div>

        <div class="form-actions mt-3">
            <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                "Back"
            </button>
            <button
                class="btn btn-primary"
                prop:disabled=move || starting.get()
                on:click=move |_| on_start.run(())
            >
                "Start Download"
            </button>
        </div>
    }
}
