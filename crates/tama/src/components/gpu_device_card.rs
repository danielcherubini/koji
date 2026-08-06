//! GPU Device Card component — renders per-GPU stats on the dashboard.

use leptos::prelude::*;

use crate::pages::dashboard::{GpuDeviceStats, ModelStateSnapshot, VramInfo};

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
pub fn model_gpu_label(gpus: &[GpuDeviceStats], model: &ModelStateSnapshot) -> Option<String> {
    if let Some(gpu_device) = model.gpu_device.as_deref() {
        let index = find_device_index(gpus, gpu_device)?;
        Some(device_display_label(index))
    } else {
        // Fallback: models without gpu_device target the first GPU.
        (!gpus.is_empty()).then(|| device_display_label(0))
    }
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
/// VRAM, and telemetry (temp/power/fan).
#[component]
pub fn GpuDeviceCard(
    /// The GPU device statistics.
    device: GpuDeviceStats,
    /// Display label, e.g. "GPU 0".
    display_label: String,
) -> impl IntoView {
    view! {
        <div class="card gpu-device-card">
            <div class="gpu-device-card__internal">
                // Column 1: Identity
                <div class="gpu-device-card__identity">
                    <div class="gpu-device-card__header">
                        <span class="gpu-device-card__title">{display_label}</span>
                    </div>
                    <div class="gpu-device-card__subtitle">
                        {if let Some(vram) = &device.vram {
                            format_card_subtitle(&device.name, vram)
                        } else {
                            device.name.clone()
                        }}
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
                                <span class="gpu-device-card__label">"VRAM"</span>
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

                // Column 3: Telemetry
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_display_label_format() {
        assert_eq!(device_display_label(0), "GPU 0");
        assert_eq!(device_display_label(3), "GPU 3");
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
}
