//! Shared "active model" row — one row per loaded/starting model, rendered
//! both inside host cards (models attributed to that host) and in the
//! dashboard Hosts section's "Unassigned" group (hostless or unmatched
//! models). Extracted so the row markup lives in exactly one place.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::components::gpu_device_card::model_gpu_label;
use crate::core_mirrors::ModelState;
use crate::pages::dashboard::{
    format_model_meta_parts, format_tok_s, model_display_name, GpuDeviceStats, ModelStateSnapshot,
};

/// Build the logs-link target for a tamad-hosted model: `/tama/logs?source=`
/// plus the URL-encoded `tamad:<host_name>:model:<model_id>` source label —
/// exactly the shape the structured-log store indexes tamad engine lines
/// under (see `docs/api/logs.md`, "Source vocabulary"). The tail adapter
/// fronts that label when the store has no rows for it yet.
///
/// `None` when the model has no host: file-based models have no
/// `tamad:` engine-log source, so the link is omitted.
pub fn model_logs_href(host_name: Option<&str>, model_id: &str) -> Option<String> {
    let host = host_name?;
    Some(format!(
        "/tama/logs?source={}",
        urlencoding::encode(&format!("tamad:{host}:model:{model_id}"))
    ))
}

/// Build the row's name block — the bold primary display name. A secondary
/// api name is intentionally NOT rendered: it duplicates the primary name
/// and the user reads it as redundant noise, so only the bold primary is
/// shown.
fn active_model_name_markup(display: String) -> impl IntoView {
    view! {
        <div class="active-model-name">
            <span class="active-model-name--primary">{display}</span>
        </div>
    }
}

/// One active-model row: status dot (Ready) or spinner (Starting), primary
/// display name, GPU allocation chip, meta line
/// (`gpu_variant · quant · Nk ctx · format`), tok/s badge when generating,
/// an Unload button honoring the shared busy flag, a `📄` logs link for
/// tamad-hosted models (the engine container tail, via
/// `/tama/logs?source=tamad:{host}:model:{model_id}`), and the `✎` edit link.
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
    // GPU allocation chip: the host-resolved label, falling back to the raw
    // device string when no host reports that GPU yet.
    let gpu_chip = model_gpu_label(&gpus_for_labels, &model).or_else(|| model.gpu_device.clone());
    let meta = format_model_meta_parts(&model);
    let tps = model.tps.filter(|t| *t > 0.0);
    let id_for_unload = model.id.clone();
    // Edit link — use db_id when Some, fall back to the id string (same
    // convention as `ModelCard`).
    let edit_id = model
        .db_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| model.id.clone());
    view! {
        <div class="active-model-row">
            <span class={status_class} title={status_title}></span>
            {active_model_name_markup(display)}
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
                    <span class="active-model-tps">{format_tok_s(t as f64)}</span>
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
                {if let Some(logs_href) = model_logs_href(model.host_name.as_deref(), &model.id) {
                    view! {
                        <A
                            attr:class="btn btn-secondary btn-sm"
                            attr:title="Open logs"
                            href=logs_href
                        >
                            "📄"
                        </A>
                    }
                    .into_any()
                } else {
                    view! { <span></span> }.into_any()
                }}
                <A
                    attr:class="btn btn-secondary btn-sm active-model-edit"
                    attr:title="Edit model"
                    href=format!("/tama/models/{}/edit", edit_id)
                >
                    "✎"
                </A>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One Ready model whose `api_name` differs from its display name —
    /// the live case where the row used to render a redundant grey
    /// `· Qwen/Qwen3.8-27B-FP8` next to the bold `Qwen: Qwen3.8-27B-FP8`.
    fn qwen_fixture() -> ModelStateSnapshot {
        ModelStateSnapshot {
            id: "qwen--qwen3.8-27b-fp8".to_string(),
            api_name: Some("Qwen/Qwen3.8-27B-FP8".to_string()),
            display_name: Some("Qwen: Qwen3.8-27B-FP8".to_string()),
            state: ModelState::Ready,
            quant: Some("fp8".to_string()),
            context_length: Some(262144),
            gpu_variant: Some("rocm".to_string()),
            hf_format: Some("transformers".to_string()),
            host_name: Some("gpu-box".to_string()),
            ..Default::default()
        }
    }

    /// The grey secondary `· {api_name}` span is dropped — the bold
    /// primary name is all the row shows for the model identity. The
    /// status dot, meta line data (`rocm · fp8 · 262k ctx ·
    /// transformers`), Unload button, and links are untouched markup that
    /// the row keeps rendering.
    ///
    /// The name markup is asserted through [`active_model_name_markup`]
    /// (the exact view the row inserts) because the full row's router
    /// links need a browser to render statically.
    #[test]
    fn test_active_model_row_omits_secondary_api_name() {
        let model = qwen_fixture();
        let html = active_model_name_markup(model_display_name(&model)).to_html();
        // The grey secondary span (and with it the api name) is gone.
        assert!(
            !html.contains("active-model-name--api"),
            "secondary span must be gone: {html}"
        );
        assert!(
            !html.contains("Qwen/Qwen3.8-27B-FP8"),
            "api name must no longer render: {html}"
        );
        // Bold primary name stays, carrying the visible identity.
        assert!(
            html.contains(
                "<span class=\"active-model-name--primary\">Qwen: Qwen3.8-27B-FP8</span>"
            ),
            "primary name: {html}"
        );
        // The rest of the row's content is untouched: its meta line still
        // joins the same parts.
        assert_eq!(
            format_model_meta_parts(&model).join(" · "),
            "rocm · fp8 · 262k ctx · transformers"
        );

        // Nominal case: without an api name the shape is unchanged and the
        // secondary span is still absent.
        let absent = active_model_name_markup("model".to_string()).to_html();
        assert!(
            !absent.contains("active-model-name--api"),
            "no class: {absent}"
        );
        assert!(
            absent.contains("<span class=\"active-model-name--primary\">model</span>"),
            "primary name placeholder: {absent}"
        );
    }

    /// A tamad-hosted model (host_name Some) yields the encoded
    /// `tamad:<host>:model:<model_id>` source link — the new structured-log
    /// vocabulary (plan-195), with `:` percent-encoded so it survives the
    /// `?source=` query param.
    #[test]
    fn test_model_logs_href_tamad_hosted_encodes_colon() {
        assert_eq!(
            model_logs_href(Some("gpu-box"), "qwen--qwen3.8-27b-fp8"),
            Some("/tama/logs?source=tamad%3Agpu-box%3Amodel%3Aqwen--qwen3.8-27b-fp8".to_string())
        );
    }

    /// A hostless model (host_name None) has no `tamad:` engine-log source →
    /// the link is omitted entirely.
    #[test]
    fn test_model_logs_href_local_model_none() {
        assert_eq!(model_logs_href(None, "qwen--qwen3.8-27b-fp8"), None);
    }
}
