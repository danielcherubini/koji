//! Shared "active model" row — one row per loaded/starting model, rendered
//! both inside host cards (models attributed to that host) and in the
//! dashboard Hosts section's "Unassigned" group (hostless or unmatched
//! models). Extracted so the row markup lives in exactly one place.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::components::gpu_device_card::model_gpu_label;
use crate::core_mirrors::ModelState;
use crate::pages::dashboard::{
    format_model_meta_parts, model_display_name, GpuDeviceStats, ModelStateSnapshot,
};

/// One active-model row: status dot (Ready) or spinner (Starting), primary
/// display name (+ api name), GPU allocation chip, meta line
/// (`gpu_variant · quant · Nk ctx · format`), tok/s badge when generating,
/// an Unload button honoring the shared busy flag, and the `▷` benchmark
/// link.
#[component]
pub fn ActiveModelRow(
    /// The model to render (Ready or Starting).
    model: ModelStateSnapshot,
    /// GPU devices used to resolve the GPU allocation chip label; the raw
    /// `gpu_device` string is shown as fallback when no device matches.
    #[prop(default = Vec::new())]
    gpus_for_labels: Vec<GpuDeviceStats>,
    /// Shared unload-in-progress flag — disables the button and swaps its
    /// label while any unload is in flight.
    unload_busy: Signal<bool>,
    /// Dispatched with the model id when the Unload button is clicked.
    on_unload: Callback<String>,
) -> impl IntoView {
    let ready = model.state == ModelState::Ready;
    let status_class = if ready {
        "active-model-status active-model-status--ready"
    } else {
        "active-model-status active-model-status--starting"
    };
    let status_title = if ready { "Ready" } else { "Starting" };
    let display = model_display_name(&model);
    let api_name = model.api_name.clone();
    // GPU allocation chip: the host-resolved label, falling back to the raw
    // device string when no host reports that GPU yet.
    let gpu_chip = model_gpu_label(&gpus_for_labels, &model).or_else(|| model.gpu_device.clone());
    let meta = format_model_meta_parts(&model);
    let tps = model.tps.filter(|t| *t > 0.0);
    let id_for_unload = model.id.clone();
    let bench_href = format!(
        "/tama/benchmarks?tab=suite&model={}",
        urlencoding::encode(&model.id)
    );
    view! {
        <div class="active-model-row">
            <span class={status_class} title={status_title}></span>
            <div class="active-model-name">
                <span class="active-model-name--primary">{display}</span>
                {if let Some(api) = api_name {
                    view! {
                        <span class="active-model-name--api">" · "{api}</span>
                    }
                    .into_any()
                } else {
                    view! { <span></span> }.into_any()
                }}
            </div>
            {if let Some(chip) = gpu_chip {
                view! {
                    <span class="active-model-gpu-chip">{chip}</span>
                }
                .into_any()
            } else {
                view! { <span></span> }.into_any()
            }}
            <span class="active-model-meta">{meta.join(" · ")}</span>
            {if let Some(t) = tps {
                view! {
                    <span class="active-model-tps">{format!("{t:.0} tok/s")}</span>
                }
                .into_any()
            } else {
                view! { <span></span> }.into_any()
            }}
            <div class="active-model-actions">
                <button
                    class="btn btn-secondary btn-sm"
                    disabled=move || unload_busy.get()
                    on:click=move |_| {
                        on_unload.run(id_for_unload.clone());
                    }
                >
                    {move || {
                        if unload_busy.get() {
                            "Unloading…"
                        } else {
                            "Unload"
                        }
                    }}
                </button>
                <A
                    attr:class="btn btn-secondary btn-sm active-model-bench"
                    attr:title="Run benchmark suite"
                    href=bench_href
                >
                    "▷"
                </A>
            </div>
        </div>
    }
}
