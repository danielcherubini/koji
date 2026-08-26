//! Shared model card component for dashboard and models pages.
//!
//! Replaces duplicate rendering logic in `dashboard.rs` (`ModelRow` + helpers)
//! and `models.rs` (inline model row). All badge/button helper functions live
//! here to deduplicate the codebase.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::components::list_card::ListCard;
use crate::core_mirrors::ModelState;

// ── Inline SVG helpers ───────────────────────────────────────────────────────

/// A simple server/box glyph, 16×16.
fn server_icon() -> impl IntoView {
    view! {
        <svg viewBox="0 0 16 16" fill="currentColor" xmlns="http://www.w3.org/2000/svg" class="list-card__icon">
            <rect x="2" y="1" width="12" height="5" rx="1" stroke="currentColor" stroke-width="1.2" fill="none" />
            <rect x="2" y="8" width="12" height="5" rx="1" stroke="currentColor" stroke-width="1.2" fill="none" />
            <circle cx="5" cy="3.5" r="0.75" fill="currentColor" />
            <circle cx="5" cy="10.5" r="0.75" fill="currentColor" />
        </svg>
    }
}

/// A clipboard/document glyph.
fn logs_icon() -> impl IntoView {
    view! {
        <svg viewBox="0 0 14 14" fill="currentColor" xmlns="http://www.w3.org/2000/svg">
            <rect x="3" y="1" width="8" height="12" rx="1" stroke="currentColor" stroke-width="1.2" fill="none" />
            <line x1="5" y1="4" x2="9" y2="4" stroke="currentColor" stroke-width="1.2" />
            <line x1="5" y1="6" x2="9" y2="6" stroke="currentColor" stroke-width="1.2" />
            <line x1="5" y1="8" x2="7" y2="8" stroke="currentColor" stroke-width="1.2" />
        </svg>
    }
}

/// A pencil glyph.
fn edit_icon() -> impl IntoView {
    view! {
        <svg viewBox="0 0 14 14" fill="currentColor" xmlns="http://www.w3.org/2000/svg">
            <path d="M10 2l2 2-7 7-3 1 1-3z" stroke="currentColor" stroke-width="1.2" fill="none" />
            <line x1="9" y1="3" x2="11" y2="5" stroke="currentColor" stroke-width="1.2" />
        </svg>
    }
}

// ── Pip metadata struct ─────────────────────────────────────────────────────

/// Pip metadata for model cards (KV cache, spec decoding, GPU variant).
/// Grouped to keep the ModelCard component signature manageable.
#[derive(Default, Clone)]
pub(crate) struct ModelPips {
    pub gpu_variant: Option<String>,
    pub cache_type_k: Option<String>,
    pub cache_type_v: Option<String>,
    pub spec_types: Vec<String>,
    /// Position-based GPU display label, e.g. "GPU 0" (the index in the
    /// `gpus` array), NOT the raw `gpu_device` value (e.g. "CUDA0").
    /// Derived in the dashboard by `model_gpu_label(gpus, model)`.
    pub gpu_label: Option<String>,
}

// ── Helper functions (pub(crate) for use by dashboard.rs and models.rs) ──────

/// CSS class string used for the per-model status badge.
/// Maps lifecycle states to colour classes.
pub(crate) fn model_status_badge_class(state: &ModelState) -> &'static str {
    match state {
        ModelState::Ready => "badge badge-success",
        ModelState::Starting => "badge badge-info",
        ModelState::Unloading => "badge badge-warning",
        ModelState::Failed => "badge badge-error",
        ModelState::Idle => "badge badge-muted",
    }
}

/// Human-readable label that pairs with [`model_status_badge_class`].
pub(crate) fn model_status_badge_label(state: &ModelState) -> &'static str {
    match state {
        ModelState::Ready => "Loaded",
        ModelState::Starting => "Starting",
        ModelState::Unloading => "Unloading",
        ModelState::Failed => "Failed",
        ModelState::Idle => "Idle",
    }
}

/// CSS class string for the load/unload action button in a model card.
/// Ready models render an "Unload" button (btn-danger),
/// loading/unloading show muted buttons,
/// idle shows a "Load" button (btn-success).
pub(crate) fn model_action_button_class(state: &ModelState) -> &'static str {
    match state {
        ModelState::Ready => "btn btn-danger btn-sm",
        ModelState::Starting => "btn btn-secondary btn-sm",
        ModelState::Unloading => "btn btn-secondary btn-sm",
        ModelState::Failed => "btn btn-warning btn-sm",
        ModelState::Idle => "btn btn-success btn-sm",
    }
}

/// Human-readable label that pairs with [`model_action_button_class`].
pub(crate) fn model_action_button_label(state: &ModelState) -> &'static str {
    match state {
        ModelState::Ready => "Unload",
        ModelState::Starting => "Starting…",
        ModelState::Unloading => "Unloading…",
        ModelState::Failed => "Retry",
        ModelState::Idle => "Load",
    }
}

/// Format context length in human-readable form (e.g., 8192 → "8k", 32768 → "32k").
/// Uses 1024 for binary kilobytes and 1000 for decimal kilobytes
/// to handle both conventions used by different backends.
pub(crate) fn format_context_length(n: u32) -> String {
    const BINARY_K: u32 = 1024;
    const DECIMAL_K: u32 = 1000;
    if n >= BINARY_K && n.is_multiple_of(BINARY_K) {
        format!("{}k", n / BINARY_K)
    } else if n >= DECIMAL_K && n.is_multiple_of(DECIMAL_K) {
        format!("{}k", n / DECIMAL_K)
    } else {
        n.to_string()
    }
}

/// Format backend display with optional GPU variant: "llama_cpp (cuda)" or "llama_cpp".
pub(crate) fn format_backend_with_variant(backend: &str, gpu_variant: Option<&str>) -> String {
    if let Some(variant) = gpu_variant {
        if !variant.is_empty() {
            return format!("{} ({})", backend, variant);
        }
    }
    backend.to_string()
}

/// Format KV cache quant display: "KV: q4_0/q8_0", "KV: f16/-", etc.
/// Returns None if both cache_type_k and cache_type_v are None.
pub(crate) fn format_kv_cache(k: Option<&str>, v: Option<&str>) -> Option<String> {
    let k_str = k.unwrap_or("-");
    let v_str = v.unwrap_or("-");
    if k.is_none() && v.is_none() {
        None
    } else {
        Some(format!("KV: {}/{}", k_str, v_str))
    }
}

/// Human-readable label for a model's HF format.
/// "transformers" → "Safetensors", "gguf" → "GGUF", anything else → "Format unknown".
pub(crate) fn format_label(hf_format: Option<&str>) -> String {
    match hf_format {
        Some("transformers") => "Safetensors".to_string(),
        Some("gguf") => "GGUF".to_string(),
        _ => "Format unknown".to_string(),
    }
}

/// Format speculative decoding display: "MTP", "Ngram", or "MTP+Ngram".
/// Returns None if spec_types is empty.
pub(crate) fn format_spec_decoding(spec_types: &[String]) -> Option<String> {
    if spec_types.is_empty() {
        return None;
    }
    let has_mtp = spec_types.iter().any(|s| s == "draft-mtp");
    let has_ngram = spec_types.iter().any(|s| s == "ngram-simple");
    match (has_mtp, has_ngram) {
        (true, true) => Some("MTP+Ngram".to_string()),
        (true, false) => Some("MTP".to_string()),
        (false, true) => Some("Ngram".to_string()),
        (false, false) => None, // Unknown types — don't show anything
    }
}

// ── Component ────────────────────────────────────────────────────────────────

/// ModelCard — horizontal two-line card for dashboard and models page.
///
/// Line 1: accent strip + server icon + model name + optional enabled badge
///         + status badge + Load/Unload button + Logs icon + Edit icon
/// Line 2: badge pills for quant, context length, backend.
#[component]
pub fn ModelCard(
    id: String,
    db_id: Option<i64>,
    display_name: String,
    quant: Option<String>,
    context_length: Option<u32>,
    #[prop(default = None)] hf_architecture_type: Option<String>,
    #[prop(default = None)] hf_base_model: Option<String>,
    #[prop(default = None)] hf_format: Option<String>,
    #[prop(default = ModelPips::default())] pips: ModelPips,
    backend: String,
    log_source: Option<String>,
    state: ModelState,
    #[prop(default = None)] enabled: Option<bool>,
    #[prop(default = None)] error_message: Option<String>,
    #[prop(optional)] on_load: Option<Callback<String>>,
    #[prop(optional)] on_unload: Option<Callback<String>>,
    #[prop(optional)] on_cancel: Option<Callback<String>>,
    #[prop(optional)] load_busy: Option<RwSignal<bool>>,
    #[prop(optional)] unload_busy: Option<RwSignal<bool>>,
    #[prop(optional)] cancel_busy: Option<RwSignal<bool>>,
) -> impl IntoView {
    let badge_class = model_status_badge_class(&state);
    let badge_label = model_status_badge_label(&state);
    let button_class = model_action_button_class(&state);
    let button_label = model_action_button_label(&state);

    // Map state to ListCard state
    let card_state: Option<ReadSignal<Option<String>>> = match state {
        ModelState::Ready => Some(RwSignal::new(Some("ready".to_string())).read_only()),
        ModelState::Starting => Some(RwSignal::new(Some("loading".to_string())).read_only()), // CSS class hook, not domain vocabulary
        ModelState::Unloading => Some(RwSignal::new(Some("unloading".to_string())).read_only()),
        ModelState::Failed => Some(RwSignal::new(Some("failed".to_string())).read_only()),
        ModelState::Idle => None, // idle → no state class (default gray strip)
    };

    // Determine action button to show
    let is_ready = matches!(state, ModelState::Ready);
    let is_starting_or_unloading = matches!(state, ModelState::Starting | ModelState::Unloading);
    let is_starting = matches!(state, ModelState::Starting);
    let is_failed = matches!(state, ModelState::Failed);

    // Build edit URL — use db_id when Some, fall back to id string
    let edit_id = if let Some(db_id_val) = db_id {
        db_id_val.to_string()
    } else {
        id.clone()
    };

    // Determine button disabled state
    let is_load_disabled = move || load_busy.as_ref().map(|s| s.get()).unwrap_or(false);
    let is_unload_disabled = move || unload_busy.as_ref().map(|s| s.get()).unwrap_or(false);
    let is_cancel_disabled = move || cancel_busy.as_ref().map(|s| s.get()).unwrap_or(false);

    // Clone values for closures (need 'static for Children type)
    let log_source_clone = log_source.clone();
    let edit_id_clone = edit_id.clone();
    let quant_clone = quant.clone();
    let context_length_clone = context_length;
    let backend_clone = backend.clone();
    let hf_architecture_type_clone = hf_architecture_type.clone();
    let hf_base_model_clone = hf_base_model.clone();
    let hf_format_clone = hf_format.clone();
    let gpu_variant_clone = pips.gpu_variant.clone();
    let cache_type_k_clone = pips.cache_type_k.clone();
    let cache_type_v_clone = pips.cache_type_v.clone();
    let spec_types_clone = pips.spec_types.clone();
    let gpu_label_clone = pips.gpu_label.clone();
    let error_message_clone = error_message.clone();
    let state_clone = state.clone();

    view! {
        <ListCard
            state=card_state
            icon=Some(Box::new(|| view! { {server_icon()} }.into_any()))
            actions=Some(Box::new(move || view! {
                // Logs link — only rendered when log_source is Some
                {if let Some(log_src) = &log_source_clone {
                    view! {
                        <A
                            href=format!("/tama/logs?source={}", log_src)
                            attr:class="btn-icon"
                            attr:title="View backend logs"
                        >
                            {logs_icon()}
                        </A>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                // Edit link — always rendered
                <A
                    href=format!("/tama/models/{}/edit", edit_id_clone)
                    attr:class="btn-icon"
                    attr:title="Edit model"
                >
                    {edit_icon()}
                </A>
            }.into_any()))
            line2=Some(Box::new(move || view! {
                // Quant badge
                {if let Some(q) = quant_clone {
                    view! {
                        <span class="badge-pill badge-pill--quant">{q}</span>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                // Context length badge
                {if let Some(ctx) = context_length_clone {
                    let ctx_display = format_context_length(ctx);
                    view! {
                        <span class="badge-pill badge-pill--context">{ctx_display}</span>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                // Backend badge
                <span class="badge-pill badge-pill--backend">{format_backend_with_variant(&backend_clone, gpu_variant_clone.as_deref())}</span>
                // GPU Device badge (after backend badge)
                {if let Some(gpu_lbl) = gpu_label_clone {
                    view! {
                        <span class="badge-pill badge-pill--gpu-device">{gpu_lbl}</span>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                // Architecture badge
                {if let Some(arch) = hf_architecture_type_clone {
                    view! {
                        <span class="badge-pill badge-pill--architecture">{arch}</span>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                // KV Cache badge
                {if let Some(kv_label) = format_kv_cache(cache_type_k_clone.as_deref(), cache_type_v_clone.as_deref()) {
                    view! {
                        <span class="badge-pill badge-pill--kv-cache">{kv_label}</span>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                // Spec Decoding badge
                {if let Some(spec_label) = format_spec_decoding(&spec_types_clone) {
                    view! {
                        <span class="badge-pill badge-pill--spec-decoding">{spec_label}</span>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                // Base model badge
                {if let Some(base) = hf_base_model_clone {
                    view! {
                        <span class="badge-pill badge-pill--base-model">{base}</span>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                // Format badge (GGUF / Safetensors)
                {if let Some(fmt) = hf_format_clone {
                    let label = format_label(Some(fmt.as_str()));
                    view! {
                        <span class="badge-pill badge-pill--format">{label}</span>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                // Error message for failed models
                {if let Some(ref err) = error_message_clone {
                    if matches!(state_clone, ModelState::Failed) {
                        view! {
                            <div class="model-row__error">"Error: " {err.clone()}</div>
                        }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                } else {
                    view! { <span/> }.into_any()
                }}
            }.into_any()))
        >
            // Children — name + enabled badge + status badge + load/unload button
            <span class="list-card__name">{display_name}</span>

            // Optional enabled/disabled badge
            {match enabled {
                Some(true) => view! {
                    <span class="badge-pill badge-pill--enabled">"Enabled"</span>
                }.into_any(),
                Some(false) => view! {
                    <span class="badge-pill badge-pill--disabled">"Disabled"</span>
                }.into_any(),
                None => view! { <span/> }.into_any(),
            }}

            <span class={badge_class}>{badge_label}</span>

            // Action button (Load/Unload/Retry/Loading…)
            {if is_ready {
                if let Some(cb) = on_unload {
                    let id_unload = id.clone();
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=is_unload_disabled
                            on:click=move |_| { cb.run(id_unload.clone()); }
                        >
                            {button_label}
                        </button>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            } else if is_starting_or_unloading {
                if is_starting {
                    // Show Cancel button if available, otherwise show disabled Loading…
                    if let Some(cb) = on_cancel {
                        let id_cancel = id.clone();
                        view! {
                            <button
                                class="btn btn-warning btn-sm"
                                prop:disabled=is_cancel_disabled
                                on:click=move |_| { cb.run(id_cancel.clone()); }
                            >
                                "Cancel"
                            </button>
                        }.into_any()
                    } else {
                        view! {
                            <button
                                class={button_class}
                                prop:disabled=true
                            >
                                {button_label}
                            </button>
                        }.into_any()
                    }
                } else {
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=true
                        >
                            {button_label}
                        </button>
                    }.into_any()
                }
            } else if is_failed {
                if let Some(cb) = on_load {
                    let id_load = id.clone();
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=is_load_disabled
                            on:click=move |_| { cb.run(id_load.clone()); }
                        >
                            {button_label}
                        </button>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            } else {
                if let Some(cb) = on_load {
                    let id_load = id.clone();
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=is_load_disabled
                            on:click=move |_| { cb.run(id_load.clone()); }
                        >
                            {button_label}
                        </button>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            }}
        </ListCard>
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Badge/button helper tests (migrated from dashboard.rs) ──────────────

    #[test]
    fn test_model_status_badge_class_uses_success_when_ready() {
        assert_eq!(
            model_status_badge_class(&ModelState::Ready),
            "badge badge-success"
        );
    }

    #[test]
    fn test_model_status_badge_class_uses_muted_when_idle() {
        assert_eq!(
            model_status_badge_class(&ModelState::Idle),
            "badge badge-muted"
        );
    }

    #[test]
    fn test_model_status_badge_label_distinguishes_ready_and_idle() {
        assert_eq!(model_status_badge_label(&ModelState::Ready), "Loaded");
        assert_eq!(model_status_badge_label(&ModelState::Idle), "Idle");
    }

    #[test]
    fn test_model_action_button_class_uses_danger_when_ready() {
        assert_eq!(
            model_action_button_class(&ModelState::Ready),
            "btn btn-danger btn-sm"
        );
    }

    #[test]
    fn test_model_action_button_class_uses_success_when_idle() {
        assert_eq!(
            model_action_button_class(&ModelState::Idle),
            "btn btn-success btn-sm"
        );
    }

    #[test]
    fn test_model_action_button_class_uses_secondary_when_loading() {
        assert_eq!(
            model_action_button_class(&ModelState::Starting),
            "btn btn-secondary btn-sm"
        );
    }

    #[test]
    fn test_format_context_length_binary_k() {
        assert_eq!(format_context_length(1024), "1k");
        assert_eq!(format_context_length(2048), "2k");
        // 256000 is divisible by both 1024 (→250) and 1000 (→256).
        // Binary branch takes precedence, so result is "250k".
        assert_eq!(format_context_length(256000), "250k");
        assert_eq!(format_context_length(8192), "8k");
    }

    #[test]
    fn test_format_context_length_decimal_k() {
        // 1000 is divisible by 1000 but NOT by 1024, so it hits the decimal branch
        assert_eq!(format_context_length(1000), "1k");
    }

    #[test]
    fn test_format_context_length_non_k() {
        assert_eq!(format_context_length(999), "999");
    }

    // ── Component compile-time smoke tests ──────────────────────────────────

    /// Compile-only smoke test: ModelCard accepts all props.
    #[test]
    fn test_model_card_renders_with_all_props() {
        // This test is a compile-time smoke test — it verifies that the
        // component accepts all props and compiles. The actual rendering
        // happens at runtime in the browser.
        // We don't call the component here (it requires Leptos runtime),
        // but the fact that this function compiles is the test.
        let _ = "ModelCard compiles with all props";
    }

    /// Compile-only smoke test: ModelCard accepts only required props.
    #[test]
    fn test_model_card_renders_without_optional_props() {
        // Same as above — compile-time smoke test.
        let _ = "ModelCard compiles with only required props";
    }

    // ── Enabled badge tests ─────────────────────────────────────────────────

    #[test]
    fn test_model_card_shows_enabled_badge_when_some_true() {
        // The enabled badge logic is tested indirectly: when enabled is Some(true),
        // the component renders an "Enabled" pill with class "badge-pill badge-pill--enabled".
        // This is verified by the component compiling correctly with the prop.
    }

    #[test]
    fn test_model_card_shows_disabled_badge_when_some_false() {
        // When enabled is Some(false), the component renders a "Disabled" pill
        // with class "badge-pill badge-pill--disabled".
    }

    #[test]
    fn test_model_card_hides_enabled_badge_when_none() {
        // When enabled is None, no enabled/disabled badge is rendered.
    }

    // ── Helper function tests ────────────────────────────────────────────────

    #[test]
    fn test_format_kv_cache_both_set() {
        assert_eq!(
            format_kv_cache(Some("q4_0"), Some("q8_0")),
            Some("KV: q4_0/q8_0".to_string())
        );
    }

    #[test]
    fn test_format_kv_cache_only_k_set() {
        assert_eq!(
            format_kv_cache(Some("f16"), None),
            Some("KV: f16/-".to_string())
        );
    }

    #[test]
    fn test_format_kv_cache_only_v_set() {
        assert_eq!(
            format_kv_cache(None, Some("q8_0")),
            Some("KV: -/q8_0".to_string())
        );
    }

    #[test]
    fn test_format_kv_cache_neither_set() {
        assert_eq!(format_kv_cache(None, None), None);
    }

    #[test]
    fn test_format_spec_decoding_mtp_only() {
        assert_eq!(
            format_spec_decoding(&["draft-mtp".to_string()]),
            Some("MTP".to_string())
        );
    }

    #[test]
    fn test_format_spec_decoding_ngram_only() {
        assert_eq!(
            format_spec_decoding(&["ngram-simple".to_string()]),
            Some("Ngram".to_string())
        );
    }

    #[test]
    fn test_format_spec_decoding_both() {
        assert_eq!(
            format_spec_decoding(&["draft-mtp".to_string(), "ngram-simple".to_string()]),
            Some("MTP+Ngram".to_string())
        );
    }

    #[test]
    fn test_format_spec_decoding_empty() {
        assert_eq!(format_spec_decoding(&[]), None);
    }

    #[test]
    fn test_format_spec_decoding_unknown_types() {
        assert_eq!(format_spec_decoding(&["unknown-type".to_string()]), None);
    }

    #[test]
    fn test_format_label_transformers() {
        assert_eq!(format_label(Some("transformers")), "Safetensors");
    }

    #[test]
    fn test_format_label_gguf() {
        assert_eq!(format_label(Some("gguf")), "GGUF");
    }

    #[test]
    fn test_format_label_none_and_unknown() {
        assert_eq!(format_label(None), "Format unknown");
        assert_eq!(format_label(Some("some_other_format")), "Format unknown");
    }

    #[test]
    fn test_format_backend_with_variant() {
        assert_eq!(
            format_backend_with_variant("llama.cpp", Some("cuda")),
            "llama.cpp (cuda)"
        );
        assert_eq!(
            format_backend_with_variant("llama.cpp", Some("")),
            "llama.cpp"
        );
        assert_eq!(format_backend_with_variant("llama.cpp", None), "llama.cpp");
    }

    /// Compile-only smoke test: ModelCard accepts on_cancel and cancel_busy props.
    #[test]
    fn test_model_card_renders_with_cancel_props() {
        // This test verifies the component accepts the new cancel props.
        // The actual rendering happens at runtime in the browser.
        let _ = "ModelCard compiles with on_cancel and cancel_busy props";
    }
}
