//! GPU Device Card component — renders per-GPU stats on the dashboard.

use leptos::prelude::*;

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

/// Derive the device state from a list of models, given the target device
/// (identified by vendor + index, e.g. vendor="nvidia", index=0).
/// Priority: Loading > Active > Failed > Idle.
pub fn derive_device_state(
    loaded_models: &[ModelStatus],
    device_vendor: &str,
    device_index: u32,
) -> GpuDeviceState {
    let mut has_loading = false;
    let mut has_active = false;
    let mut has_failed = false;

    for model in loaded_models {
        let Some(ref gpu_device) = model.gpu_device else {
            continue;
        };

        // Determine if this model targets the same vendor as the device.
        let gpu_lower = gpu_device.to_lowercase();
        let is_nvidia = gpu_lower.contains("cuda");
        let is_amd = gpu_lower.contains("rocm") || gpu_lower.contains("amd");

        let vendor_matches = match device_vendor {
            "nvidia" => is_nvidia,
            "amd" => is_amd,
            _ => false,
        };
        if !vendor_matches {
            continue;
        }

        // Extract trailing numeric suffix from gpu_device (e.g. "CUDA0" → "0").
        let chars: Vec<char> = gpu_device.chars().collect();
        let mut suffix_start = chars.len();
        for (i, c) in chars.iter().enumerate() {
            if c.is_ascii_digit() {
                suffix_start = i;
            } else if i > suffix_start {
                break;
            }
        }
        let suffix: String = chars[suffix_start..].iter().collect();
        let Ok(idx) = suffix.parse::<u32>() else {
            continue;
        };
        if idx != device_index {
            continue;
        }

        // Model matches this device — track its state.
        match model.state.as_str() {
            "loading" => has_loading = true,
            "ready" | "unloading" => has_active = true,
            "failed" => has_failed = true,
            _ => {}
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
/// `gpu_device` value. Matches by:
/// 1. Direct case-insensitive comparison of device_id against gpu_device.
/// 2. Vendor-aware numeric suffix matching (e.g. "CUDA0" → nvidia index 0,
///    "ROCm0" → amd index 0).
pub fn find_device_index(gpus: &[GpuDeviceStats], gpu_device: &str) -> Option<usize> {
    // 1. Direct case-insensitive match
    let gpu_lower = gpu_device.to_lowercase();
    for (i, gpu) in gpus.iter().enumerate() {
        if gpu.device_id.to_lowercase() == gpu_lower {
            return Some(i);
        }
    }

    // 2. Vendor-aware numeric suffix match
    let is_nvidia = gpu_lower.contains("cuda");
    let is_amd = gpu_lower.contains("rocm") || gpu_lower.contains("amd");

    // Extract trailing numeric suffix from gpu_device
    let chars: Vec<char> = gpu_device.chars().collect();
    let mut suffix_start = chars.len();
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_digit() {
            suffix_start = i;
        } else if i > suffix_start {
            break;
        }
    }
    let suffix: String = chars[suffix_start..].iter().collect();
    let Ok(target_idx) = suffix.parse::<u32>() else {
        return None;
    };

    for (i, gpu) in gpus.iter().enumerate() {
        let vendor_matches = match gpu.vendor.as_str() {
            "nvidia" => is_nvidia,
            "amd" => is_amd,
            _ => false,
        };
        if !vendor_matches {
            continue;
        }

        // Extract trailing numeric suffix from device_id (e.g. "nvidia0" → 0).
        let dev_chars: Vec<char> = gpu.device_id.chars().collect();
        let mut dev_suffix_start = dev_chars.len();
        for (i, c) in dev_chars.iter().enumerate() {
            if c.is_ascii_digit() {
                dev_suffix_start = i;
            } else if i > dev_suffix_start {
                break;
            }
        }
        let dev_suffix: String = dev_chars[dev_suffix_start..].iter().collect();
        if let Ok(dev_idx) = dev_suffix.parse::<u32>() {
            if dev_idx == target_idx {
                return Some(i);
            }
        }
    }

    None
}

/// Returns the display label of the GPU a model is loaded on, e.g.
/// Some("GPU 0"). Returns None if the model has no `gpu_device` or no
/// matching device is found.
pub fn model_gpu_label(gpus: &[GpuDeviceStats], model: &ModelStatus) -> Option<String> {
    let gpu_device = model.gpu_device.as_deref()?;
    let index = find_device_index(gpus, gpu_device)?;
    Some(device_display_label(index))
}

/// Returns the first model targeting `device_vendor` + `device_index` that is
/// in `ready`, `loading`, or `unloading` state, with a synthetic "TRANSFERRING…"
/// prefix when state is `loading`. Returns None if no such model exists.
pub fn loaded_model_display(
    loaded_models: &[ModelStatus],
    device_vendor: &str,
    device_index: u32,
) -> Option<LoadedModelDisplay> {
    for model in loaded_models {
        let Some(ref gpu_device) = model.gpu_device else {
            continue;
        };

        // Determine if this model targets the same vendor as the device.
        let gpu_lower = gpu_device.to_lowercase();
        let is_nvidia = gpu_lower.contains("cuda");
        let is_amd = gpu_lower.contains("rocm") || gpu_lower.contains("amd");

        let vendor_matches = match device_vendor {
            "nvidia" => is_nvidia,
            "amd" => is_amd,
            _ => false,
        };
        if !vendor_matches {
            continue;
        }

        // Extract trailing numeric suffix from gpu_device.
        let chars: Vec<char> = gpu_device.chars().collect();
        let mut suffix_start = chars.len();
        for (i, c) in chars.iter().enumerate() {
            if c.is_ascii_digit() {
                suffix_start = i;
            } else if i > suffix_start {
                break;
            }
        }
        let suffix: String = chars[suffix_start..].iter().collect();
        let Ok(idx) = suffix.parse::<u32>() else {
            continue;
        };
        if idx != device_index {
            continue;
        }

        match model.state.as_str() {
            "ready" | "loading" | "unloading" => {
                let name = model
                    .display_name
                    .clone()
                    .or_else(|| model.api_name.clone())
                    .unwrap_or_else(|| model.id.clone());
                let transferring = model.state == "loading";
                return Some(LoadedModelDisplay { name, transferring });
            }
            _ => {}
        }
    }
    None
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
/// VRAM, the loaded model, and telemetry.
#[component]
pub fn GpuDeviceCard(
    /// The GPU device statistics.
    device: GpuDeviceStats,
    /// Display label, e.g. "GPU 0".
    display_label: String,
    /// All loaded models (used to derive state and loaded model).
    loaded_models: Vec<ModelStatus>,
) -> impl IntoView {
    // Extract device index from device_id for matching.
    let dev_chars: Vec<char> = device.device_id.chars().collect();
    let mut suffix_start = dev_chars.len();
    for (i, c) in dev_chars.iter().enumerate() {
        if c.is_ascii_digit() {
            suffix_start = i;
        } else if i > suffix_start {
            break;
        }
    }
    let dev_suffix: String = dev_chars[suffix_start..].iter().collect();
    let device_index: u32 = dev_suffix.parse().unwrap_or(0);
    let vendor = device.vendor.clone();

    let state = derive_device_state(&loaded_models, &vendor, device_index);
    let loaded = loaded_model_display(&loaded_models, &vendor, device_index);

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
            // Header
            <div class="gpu-device-card__header">
                <span class="gpu-device-card__title">{display_label}</span>
                <span class={badge_class}>{badge_text}</span>
            </div>

            // Utilization row
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

            // VRAM row
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

            // Model section
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
                            if let Some(model) = loaded {
                                if model.transferring {
                                    view! { <span>"TRANSFERRING… " {model.name}</span> }.into_any()
                                } else {
                                    view! { <span>{model.name}</span> }.into_any()
                                }
                            } else {
                                view! { <span class="text-muted">"No model loaded"</span> }.into_any()
                            }
                        }
                    }}
                </div>
            </div>

            // Telemetry row
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model(id: &str, state: &str, gpu_device: Option<&str>) -> ModelStatus {
        ModelStatus {
            id: id.to_string(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".to_string(),
            #[allow(deprecated)]
            loaded: state == "ready",
            state: state.to_string(),
            quant: None,
            context_length: None,
            hf_architecture_type: None,
            hf_base_model: None,
            gpu_variant: None,
            cache_type_k: None,
            cache_type_v: None,
            spec_types: vec![],
            gpu_device: gpu_device.map(|s| s.to_string()),
            error_message: None,
        }
    }

    fn make_gpu(device_id: &str, vendor: &str) -> GpuDeviceStats {
        GpuDeviceStats {
            device_id: device_id.to_string(),
            vendor: vendor.to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
        }
    }

    #[test]
    fn test_derive_state_active_when_ready_model() {
        let models = vec![make_model("m1", "ready", Some("CUDA0"))];
        assert_eq!(
            derive_device_state(&models, "nvidia", 0),
            GpuDeviceState::Active
        );
    }

    #[test]
    fn test_derive_state_loading_when_loading_model() {
        let models = vec![make_model("m1", "loading", Some("CUDA0"))];
        assert_eq!(
            derive_device_state(&models, "nvidia", 0),
            GpuDeviceState::Loading
        );
    }

    #[test]
    fn test_derive_state_failed_when_only_failed() {
        let models = vec![make_model("m1", "failed", Some("CUDA0"))];
        assert_eq!(
            derive_device_state(&models, "nvidia", 0),
            GpuDeviceState::Failed
        );
    }

    #[test]
    fn test_derive_state_idle_when_no_models() {
        let models: Vec<ModelStatus> = vec![];
        assert_eq!(
            derive_device_state(&models, "nvidia", 0),
            GpuDeviceState::Idle
        );
    }

    #[test]
    fn test_derive_state_loading_overrides_ready() {
        let models = vec![
            make_model("m1", "ready", Some("CUDA0")),
            make_model("m2", "loading", Some("CUDA0")),
        ];
        assert_eq!(
            derive_device_state(&models, "nvidia", 0),
            GpuDeviceState::Loading
        );
    }

    #[test]
    fn test_device_display_label_format() {
        assert_eq!(device_display_label(0), "GPU 0");
        assert_eq!(device_display_label(3), "GPU 3");
    }

    #[test]
    fn test_find_device_index_direct_match() {
        let gpus = vec![make_gpu("nvidia0", "nvidia"), make_gpu("nvidia1", "nvidia")];
        assert_eq!(find_device_index(&gpus, "nvidia0"), Some(0));
        assert_eq!(find_device_index(&gpus, "nvidia1"), Some(1));
    }

    #[test]
    fn test_find_device_index_numeric_match() {
        let gpus = vec![make_gpu("nvidia0", "nvidia"), make_gpu("nvidia1", "nvidia")];
        assert_eq!(find_device_index(&gpus, "CUDA0"), Some(0));
        assert_eq!(find_device_index(&gpus, "CUDA1"), Some(1));
    }

    #[test]
    fn test_find_device_index_no_match_different_vendor() {
        let gpus = vec![make_gpu("nvidia0", "nvidia")];
        // ROCm targets AMD, not nvidia
        assert_eq!(find_device_index(&gpus, "ROCm0"), None);
    }

    #[test]
    fn test_model_gpu_label_resolves_to_position() {
        let gpus = vec![make_gpu("nvidia0", "nvidia"), make_gpu("nvidia1", "nvidia")];
        let model = make_model("m1", "ready", Some("nvidia0"));
        assert_eq!(model_gpu_label(&gpus, &model), Some("GPU 0".to_string()));
    }

    #[test]
    fn test_loaded_model_display_transferring() {
        let models = vec![make_model("m1", "loading", Some("CUDA0"))];
        let display = loaded_model_display(&models, "nvidia", 0);
        assert!(display.is_some());
        let d = display.unwrap();
        assert_eq!(d.name, "m1");
        assert!(d.transferring);
    }

    #[test]
    fn test_loaded_model_display_active() {
        let models = vec![make_model("m1", "ready", Some("CUDA0"))];
        let display = loaded_model_display(&models, "nvidia", 0);
        assert!(display.is_some());
        let d = display.unwrap();
        assert_eq!(d.name, "m1");
        assert!(!d.transferring);
    }

    #[test]
    fn test_loaded_model_display_none_when_idle() {
        let models: Vec<ModelStatus> = vec![];
        assert_eq!(loaded_model_display(&models, "nvidia", 0), None);
    }

    #[test]
    fn test_format_vram_short() {
        let vram = VramInfo {
            used_mib: 22937,
            total_mib: 24576,
        };
        assert_eq!(format_vram_short(&vram), "22.4 / 24.0 GB");
    }
}
