//! GPU Device Card component — renders per-GPU stats on the dashboard.

use leptos::prelude::*;

use crate::gpu_types::ModelState;

#[cfg(test)]
use crate::gpu_types::GpuVendor;
use crate::pages::dashboard::{GpuDeviceStats, ModelStatus, VramInfo};

/// Lifecycle state of a GPU device, derived from loaded model states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuDeviceState {
    /// At least one ready model targeting this device.
    Active,
    /// A model is currently loading (transferring VRAM).
    Loading,
    /// A model targeting this device is in failed state.
    Failed,
    /// No model loaded, device healthy.
    Idle,
}

/// Display information for a loaded model.
#[derive(Debug, PartialEq)]
pub struct LoadedModelDisplay {
    /// Model display name (or synthetic "TRANSFERRING…" prefix).
    pub name: String,
    /// True when the model is in `loading` state.
    pub transferring: bool,
}

/// Derive the device state from a list of models targeting this device.
/// Matches `model.gpu_device` (e.g. "GPU0") against `device_id` (e.g. "GPU0").
/// Priority: Loading > Active > Failed > Idle.
pub fn derive_device_state(loaded_models: &[ModelStatus], device_id: &str) -> GpuDeviceState {
    let mut has_loading = false;
    let mut has_active = false;
    let mut has_failed = false;

    for model in loaded_models {
        let targets_device = match &model.gpu_device {
            Some(gpu_device) if gpu_device == device_id => true,
            // Fallback: models without gpu_device target the first GPU.
            None if device_id == "GPU0" => true,
            _ => false,
        };

        if !targets_device {
            continue;
        }

        // Model matches this device — track its state.
        match model.state {
            ModelState::Loading => has_loading = true,
            ModelState::Ready | ModelState::Unloading => has_active = true,
            ModelState::Failed => has_failed = true,
            ModelState::Idle => {}
        }
    }

    // Priority: Loading > Active > Failed > Idle
    if has_loading {
        GpuDeviceState::Loading
    } else if has_active {
        GpuDeviceState::Active
    } else if has_failed {
        GpuDeviceState::Failed
    } else {
        GpuDeviceState::Idle
    }
}

/// Returns the display label for a GPU device, e.g. "GPU 0", "GPU 1".
pub fn device_display_label(index: usize) -> String {
    format!("GPU {index}")
}

/// Returns the index of the GPU device whose `device_id` matches the given
/// `gpu_device` value. Direct string match (both are "GPU0", "GPU1", etc.).
pub fn find_device_index(gpus: &[GpuDeviceStats], gpu_device: &str) -> Option<usize> {
    gpus.iter().position(|g| g.device_id == gpu_device)
}

/// Returns the display label of the GPU a model is loaded on, e.g.
/// Some("GPU 0"). Returns None if the model has no `gpu_device` or no
/// matching device is found.
pub fn model_gpu_label(gpus: &[GpuDeviceStats], model: &ModelStatus) -> Option<String> {
    if let Some(gpu_device) = model.gpu_device.as_deref() {
        let index = find_device_index(gpus, gpu_device)?;
        Some(device_display_label(index))
    } else {
        // Fallback: models without gpu_device target the first GPU.
        (!gpus.is_empty()).then(|| device_display_label(0))
    }
}

/// Find the first loaded model targeting `device_id` (e.g. "GPU0").
/// Only considers models in `ready`, `loading`, or `unloading` state.
/// Models without `gpu_device` set fall back to the first GPU ("GPU0").
pub fn model_for_device<'a>(
    loaded_models: &'a [ModelStatus],
    device_id: &str,
) -> Option<&'a ModelStatus> {
    loaded_models.iter().find(|m| {
        let targets_device = match &m.gpu_device {
            Some(g) if g == device_id => true,
            None if device_id == "GPU0" => true,
            _ => false,
        };
        targets_device
            && matches!(
                m.state,
                ModelState::Ready | ModelState::Loading | ModelState::Unloading
            )
    })
}

/// Returns the first model targeting `device_id` (e.g. "GPU0") that is
/// in `ready`, `loading`, or `unloading` state, with a synthetic "TRANSFERRING…"
/// prefix when state is `loading`. Returns None if no such model exists.
/// Models without `gpu_device` set fall back to the first GPU ("GPU0").
pub fn loaded_model_display(
    loaded_models: &[ModelStatus],
    device_id: &str,
) -> Option<LoadedModelDisplay> {
    for model in loaded_models {
        let targets_device = match &model.gpu_device {
            Some(gpu_device) if gpu_device == device_id => true,
            // Fallback: models without gpu_device target the first GPU.
            None if device_id == "GPU0" => true,
            _ => false,
        };

        if !targets_device {
            continue;
        }

        match model.state {
            ModelState::Ready | ModelState::Loading | ModelState::Unloading => {
                let name = model
                    .display_name
                    .clone()
                    .or_else(|| model.api_name.clone())
                    .unwrap_or_else(|| model.id.clone());
                let transferring = matches!(model.state, ModelState::Loading);
                return Some(LoadedModelDisplay { name, transferring });
            }
            ModelState::Failed | ModelState::Idle => {}
        }
    }
    None
}

/// Format GPU card subtitle as "Name · 32 GB".
/// Total VRAM is rounded to the nearest integer GB.
pub fn format_card_subtitle(name: &str, vram: &VramInfo) -> String {
    let total_gb = (vram.total_mib as f64 / 1024.0 + 0.5) as u64;
    format!("{name} \u{00B7} {total_gb} GB")
}

/// Format VRAM as "used / total GB" with 1 decimal place.
/// E.g. 22937/24576 MiB → "22.4 / 24.0 GB".
pub fn format_vram_short(vram: &VramInfo) -> String {
    let used_gb = vram.used_mib as f64 / 1024.0;
    let total_gb = vram.total_mib as f64 / 1024.0;
    format!("{used_gb:.1} / {total_gb:.1} GB")
}

// ── Component ────────────────────────────────────────────────────────────────

/// GpuDeviceCard — renders one card per detected GPU showing utilization,
/// VRAM, the loaded model, inference stats, and telemetry.
#[component]
pub fn GpuDeviceCard(
    /// The GPU device statistics.
    device: GpuDeviceStats,
    /// Display label, e.g. "GPU 0".
    display_label: String,
    /// All loaded models (used to derive state and loaded model).
    loaded_models: Vec<ModelStatus>,
    /// Prompt throughput (tokens per second during prompt processing).
    #[prop(default = None)]
    prompt_tps: Option<f32>,
    /// Generation throughput (tokens per second during generation).
    #[prop(default = None)]
    tps: Option<f32>,
) -> impl IntoView {
    let state = derive_device_state(&loaded_models, &device.device_id);
    let loaded = loaded_model_display(&loaded_models, &device.device_id);

    let badge_class = match state {
        GpuDeviceState::Active => "badge badge-success",
        GpuDeviceState::Loading => "badge badge-warning",
        GpuDeviceState::Failed => "badge badge-error",
        GpuDeviceState::Idle => "badge badge-muted",
    };
    let badge_text = match state {
        GpuDeviceState::Active => "ACTIVE",
        GpuDeviceState::Loading => "LOADING",
        GpuDeviceState::Failed => "FAILED",
        GpuDeviceState::Idle => "IDLE",
    };

    let vram_label = if state == GpuDeviceState::Loading {
        "VRAM Allocation"
    } else {
        "VRAM"
    };

    let model_section_header = if state == GpuDeviceState::Loading {
        "TRANSFERRING…"
    } else {
        "LOADED MODEL"
    };

    view! {
        <div class="card gpu-device-card">
            <div class="gpu-device-card__internal">
                // Column 1: Identity
                <div class="gpu-device-card__identity">
                    <div class="gpu-device-card__identity-top">
                        <div class="gpu-device-card__header">
                            <span class="gpu-device-card__title">{display_label}</span>
                            <span class={badge_class}>{badge_text}</span>
                        </div>
                        <div class="gpu-device-card__subtitle">
                            {if let Some(vram) = &device.vram {
                                format_card_subtitle(&device.name, vram)
                            } else {
                                device.name.clone()
                            }}
                        </div>
                    </div>
                    <div class="gpu-device-card__identity-bottom">
                        <div class="gpu-device-card__model-section">
                            <div class="gpu-device-card__model-header">{model_section_header}</div>
                            <div class="gpu-device-card__model-body">
                                {match state {
                                    GpuDeviceState::Idle => {
                                        view! { <span class="text-muted">"No model loaded"</span> }.into_any()
                                    }
                                    GpuDeviceState::Failed => {
                                        view! { <span></span> }.into_any()
                                    }
                                    _ => {
                                        if let Some(model) = loaded.as_ref() {
                                            if model.transferring {
                                                view! {
                                                    <span title={model.name.clone()}>"TRANSFERRING… " {model.name.clone()}</span>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <span title={model.name.clone()}>{model.name.clone()}</span>
                                                }.into_any()
                                            }
                                        } else {
                                            view! { <span class="text-muted">"No model loaded"</span> }.into_any()
                                        }
                                    }
                                }}
                            </div>
                        </div>
                    </div>
                </div>

                // Column 2: Bars (utilization + vram)
                <div class="gpu-device-card__bars">
                    <div class="gpu-device-card__bars-top">
                        <div class="gpu-device-card__row">
                            <div class="gpu-device-card__row-header">
                                <span class="gpu-device-card__label">"Utilization"</span>
                                <span class="gpu-device-card__value">
                                    {device.utilization_pct.map(|p| format!("{p}%")).unwrap_or_else(|| "—".to_string())}
                                </span>
                            </div>
                            <div class="progress-bar">
                                <div
                                    class="progress-bar-fill gpu-device-card__bar-fill"
                                    style=format!("width: {}%", device.utilization_pct.unwrap_or(0))
                                />
                            </div>
                        </div>
                    </div>
                    <div class="gpu-device-card__bars-bottom">
                        <div class="gpu-device-card__row">
                            <div class="gpu-device-card__row-header">
                                <span class="gpu-device-card__label">{vram_label}</span>
                                <span class="gpu-device-card__value">
                                    {device.vram.as_ref().map(format_vram_short).unwrap_or_else(|| "—".to_string())}
                                </span>
                            </div>
                            {if let Some(vram) = &device.vram {
                                let vram_pct = if vram.total_mib > 0 {
                                    (vram.used_mib as f64 / vram.total_mib as f64 * 100.0).min(100.0)
                                } else {
                                    0.0
                                };
                                view! {
                                    <div class="progress-bar">
                                        <div
                                            class="progress-bar-fill gpu-device-card__bar-fill"
                                            style=format!("width: {:.1}%", vram_pct)
                                        />
                                    </div>
                                }.into_any()
                            } else {
                                view! { <span/> }.into_any()
                            }}
                        </div>
                    </div>
                </div>

                // Column 3: Combined Throughput + Telemetry (2 sub-columns)
                <div class="gpu-device-card__combined">
                    <div class="gpu-device-card__combined-top">
                        <div class="gpu-device-card__throughput">
                            <div class="gpu-device-card__inference-cell">
                                <div class="gpu-device-card__inference-value">
                                    {prompt_tps.map(|v| format!("{v:.0} tok/s")).unwrap_or_else(|| "0 tok/s".to_string())}
                                </div>
                                <div class="gpu-device-card__inference-label">"Processing"</div>
                            </div>
                            <div class="gpu-device-card__inference-cell">
                                <div class="gpu-device-card__inference-value">
                                    {tps.map(|v| format!("{v:.0} tok/s")).unwrap_or_else(|| "0 tok/s".to_string())}
                                </div>
                                <div class="gpu-device-card__inference-label">"Generation"</div>
                            </div>
                        </div>
                    </div>

                    <div class="gpu-device-card__combined-bottom">
                        <div class="gpu-device-card__telemetry">
                            <div class="gpu-device-card__telemetry-cell">
                                <div class="gpu-device-card__telemetry-value">
                                    {device.temperature_c.map(|t| format!("{t}°C")).unwrap_or_else(|| "—".to_string())}
                                </div>
                                <div class="gpu-device-card__telemetry-label">"Temp"</div>
                            </div>
                            <div class="gpu-device-card__telemetry-cell">
                                <div class="gpu-device-card__telemetry-value">
                                    {device.power_w.map(|p| format!("{p}W")).unwrap_or_else(|| "—".to_string())}
                                </div>
                                <div class="gpu-device-card__telemetry-label">"Power"</div>
                            </div>
                            <div class="gpu-device-card__telemetry-cell">
                                <div class="gpu-device-card__telemetry-value">
                                    {device.fan_pct.map(|f| format!("{f}%")).unwrap_or_else(|| "—".to_string())}
                                </div>
                                <div class="gpu-device-card__telemetry-label">"Fan"</div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model(id: &str, state: &str, gpu_device: Option<&str>) -> ModelStatus {
        let model_state = match state {
            "idle" => ModelState::Idle,
            "loading" => ModelState::Loading,
            "ready" => ModelState::Ready,
            "unloading" => ModelState::Unloading,
            "failed" => ModelState::Failed,
            _ => ModelState::Idle,
        };
        ModelStatus {
            id: id.to_string(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".to_string(),
            state: model_state,
            quant: None,
            context_length: None,
            hf_architecture_type: None,
            hf_base_model: None,
            gpu_variant: None,
            cache_type_k: None,
            cache_type_v: None,
            spec_types: vec![],
            gpu_device: gpu_device.map(|s| s.to_string()),
            tps: None,
            prompt_tps: None,
            error_message: None,
        }
    }

    fn make_gpu(device_id: &str, vendor: &str) -> GpuDeviceStats {
        let gpu_vendor = match vendor {
            "amd" => GpuVendor::Amd,
            _ => GpuVendor::Nvidia,
        };
        GpuDeviceStats {
            device_id: device_id.to_string(),
            name: "Test GPU".to_string(),
            vendor: gpu_vendor,
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
        }
    }

    #[test]
    fn test_derive_state_active_when_ready_model() {
        let models = vec![make_model("m1", "ready", Some("GPU0"))];
        assert_eq!(derive_device_state(&models, "GPU0"), GpuDeviceState::Active);
    }

    #[test]
    fn test_derive_state_loading_when_loading_model() {
        let models = vec![make_model("m1", "loading", Some("GPU0"))];
        assert_eq!(
            derive_device_state(&models, "GPU0"),
            GpuDeviceState::Loading
        );
    }

    #[test]
    fn test_derive_state_failed_when_only_failed() {
        let models = vec![make_model("m1", "failed", Some("GPU0"))];
        assert_eq!(derive_device_state(&models, "GPU0"), GpuDeviceState::Failed);
    }

    #[test]
    fn test_derive_state_idle_when_no_models() {
        let models: Vec<ModelStatus> = vec![];
        assert_eq!(derive_device_state(&models, "GPU0"), GpuDeviceState::Idle);
    }

    #[test]
    fn test_derive_state_loading_overrides_ready() {
        let models = vec![
            make_model("m1", "ready", Some("GPU0")),
            make_model("m2", "loading", Some("GPU0")),
        ];
        assert_eq!(
            derive_device_state(&models, "GPU0"),
            GpuDeviceState::Loading
        );
    }

    #[test]
    fn test_derive_state_idle_when_model_on_different_gpu() {
        let models = vec![make_model("m1", "ready", Some("GPU1"))];
        assert_eq!(derive_device_state(&models, "GPU0"), GpuDeviceState::Idle);
    }

    #[test]
    fn test_derive_state_fallback_no_gpu_device_to_gpu0() {
        let models = vec![make_model("m1", "ready", None)];
        assert_eq!(derive_device_state(&models, "GPU0"), GpuDeviceState::Active);
        assert_eq!(derive_device_state(&models, "GPU1"), GpuDeviceState::Idle);
    }

    #[test]
    fn test_device_display_label_format() {
        assert_eq!(device_display_label(0), "GPU 0");
        assert_eq!(device_display_label(3), "GPU 3");
    }

    #[test]
    fn test_find_device_index_direct_match() {
        let gpus = vec![make_gpu("GPU0", "nvidia"), make_gpu("GPU1", "nvidia")];
        assert_eq!(find_device_index(&gpus, "GPU0"), Some(0));
        assert_eq!(find_device_index(&gpus, "GPU1"), Some(1));
    }

    #[test]
    fn test_find_device_index_no_match() {
        let gpus = vec![make_gpu("GPU0", "nvidia"), make_gpu("GPU1", "nvidia")];
        assert_eq!(find_device_index(&gpus, "GPU2"), None);
    }

    #[test]
    fn test_model_gpu_label_resolves_to_position() {
        let gpus = vec![make_gpu("GPU0", "nvidia"), make_gpu("GPU1", "nvidia")];
        let model = make_model("m1", "ready", Some("GPU0"));
        assert_eq!(model_gpu_label(&gpus, &model), Some("GPU 0".to_string()));
    }

    #[test]
    fn test_model_gpu_label_fallback_no_gpu_device() {
        let gpus = vec![make_gpu("GPU0", "nvidia")];
        let model = make_model("m1", "ready", None);
        assert_eq!(model_gpu_label(&gpus, &model), Some("GPU 0".to_string()));
    }

    #[test]
    fn test_loaded_model_display_transferring() {
        let models = vec![make_model("m1", "loading", Some("GPU0"))];
        let display = loaded_model_display(&models, "GPU0");
        assert!(display.is_some());
        let d = display.unwrap();
        assert_eq!(d.name, "m1");
        assert!(d.transferring);
    }

    #[test]
    fn test_loaded_model_display_active() {
        let models = vec![make_model("m1", "ready", Some("GPU0"))];
        let display = loaded_model_display(&models, "GPU0");
        assert!(display.is_some());
        let d = display.unwrap();
        assert_eq!(d.name, "m1");
        assert!(!d.transferring);
    }

    #[test]
    fn test_loaded_model_display_none_when_idle() {
        let models: Vec<ModelStatus> = vec![];
        assert_eq!(loaded_model_display(&models, "GPU0"), None);
    }

    #[test]
    fn test_loaded_model_display_fallback_no_gpu_device_to_gpu0() {
        let models = vec![make_model("m1", "ready", None)];
        assert!(loaded_model_display(&models, "GPU0").is_some());
        assert!(loaded_model_display(&models, "GPU1").is_none());
    }

    #[test]
    fn test_format_vram_short() {
        let vram = VramInfo {
            used_mib: 22937,
            total_mib: 24576,
        };
        assert_eq!(format_vram_short(&vram), "22.4 / 24.0 GB");
    }

    #[test]
    fn test_model_for_device_direct_match() {
        let models = vec![
            make_model("m1", "ready", Some("GPU0")),
            make_model("m2", "ready", Some("GPU1")),
        ];
        assert!(model_for_device(&models, "GPU0").is_some());
        assert!(model_for_device(&models, "GPU1").is_some());
        assert_eq!(model_for_device(&models, "GPU0").unwrap().id, "m1");
        assert_eq!(model_for_device(&models, "GPU1").unwrap().id, "m2");
    }

    #[test]
    fn test_model_for_device_fallback_no_gpu_device() {
        let models = vec![make_model("m1", "ready", None)];
        assert!(model_for_device(&models, "GPU0").is_some());
        assert!(model_for_device(&models, "GPU1").is_none());
        assert_eq!(model_for_device(&models, "GPU0").unwrap().id, "m1");
    }

    #[test]
    fn test_model_for_device_empty_models() {
        let models: Vec<ModelStatus> = vec![];
        assert!(model_for_device(&models, "GPU0").is_none());
    }

    #[test]
    fn test_format_card_subtitle() {
        let vram = VramInfo {
            used_mib: 0,
            total_mib: 32768,
        }; // 32 GB
        assert_eq!(
            format_card_subtitle("Radeon AI PRO R9700", &vram),
            "Radeon AI PRO R9700 \u{00B7} 32 GB"
        );
    }

    #[test]
    fn test_format_card_subtitle_rounds_31_9_to_32() {
        let vram = VramInfo {
            used_mib: 0,
            total_mib: 32760,
        }; // ~31.9 GB
        assert_eq!(
            format_card_subtitle("Radeon AI PRO R9700", &vram),
            "Radeon AI PRO R9700 \u{00B7} 32 GB"
        );
    }

    #[test]
    fn test_model_for_device_returns_first_match() {
        let models = vec![
            make_model("m1", "ready", Some("GPU0")),
            make_model("m2", "ready", Some("GPU0")),
        ];
        assert_eq!(model_for_device(&models, "GPU0").unwrap().id, "m1");
    }
}
