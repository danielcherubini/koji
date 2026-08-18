//! GPU label helpers for the dashboard model pips and per-tamad host
//! section (plan-191 Task 9 re-pointed the dashboard to tamad host
//! cards; the standalone GpuDeviceCard was superseded by HostCard).

use crate::pages::dashboard::{GpuDeviceStats, ModelStateSnapshot};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_display_label_format() {
        assert_eq!(device_display_label(0), "GPU 0");
        assert_eq!(device_display_label(3), "GPU 3");
    }
}
