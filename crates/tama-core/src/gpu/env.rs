use super::system::{detect_gpu_devices, GpuDeviceStats};

/// Map a GPU vendor to the correct visibility env var name and value.
/// Returns None for vendors that have no env-var GPU isolation mechanism.
///
/// - AMD → `ROCR_VISIBLE_DEVICES=<uuid>` (supports UUIDs; preferred over HIP_VISIBLE_DEVICES)
/// - NVIDIA → `CUDA_VISIBLE_DEVICES=<uuid>`
/// - Vulkan/Metal/unknown → None (no UUID env-var mechanism)
pub fn vendor_env_var(vendor: &str, uuid: &str) -> Option<(String, String)> {
    match vendor {
        "amd" => Some(("ROCR_VISIBLE_DEVICES".to_string(), uuid.to_string())),
        "nvidia" => Some(("CUDA_VISIBLE_DEVICES".to_string(), uuid.to_string())),
        _ => None,
    }
}

/// Resolve a `gpu_device` string (e.g. "GPU1") to (env_var_name, value) for
/// driver-level GPU isolation, using the provided GPU list.
/// Returns None if the device is not found, has no UUID, or the vendor has no env-var mechanism.
pub fn resolve_gpu_device_env_from(
    gpu_device: &str,
    gpus: &[GpuDeviceStats],
) -> Option<(String, String)> {
    let device = gpu_device.trim();
    if device.is_empty() {
        return None;
    }
    let gpu = gpus.iter().find(|g| g.device_id == device)?;
    let uuid = gpu.uuid.as_ref()?;
    vendor_env_var(&gpu.vendor, uuid)
}

/// Resolve a `gpu_device` string (e.g. "GPU1") to (env_var_name, value) for
/// driver-level GPU isolation. Enumerates GPUs via `detect_gpu_devices()`.
/// Returns None if the device is not found, has no UUID, or the vendor has no env-var mechanism.
///
/// Blocks on subprocesses (nvidia-smi, sysfs reads); call via `tokio::task::spawn_blocking`.
pub fn resolve_gpu_device_env(gpu_device: &str) -> Option<(String, String)> {
    let gpus = detect_gpu_devices();
    resolve_gpu_device_env_from(gpu_device, &gpus)
}

/// Inject the GPU isolation env var onto a backend Command.
/// No-op if `gpu_device` is None or cannot be resolved to a UUID.
pub fn inject_gpu_env(cmd: &mut impl crate::process::BackendCommand, gpu_device: &Option<String>) {
    if let Some(device) = gpu_device {
        if let Some((name, value)) = resolve_gpu_device_env(device) {
            cmd.env(&name, &value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── vendor_env_var tests ──────────────────────────────────────────

    #[test]
    fn test_vendor_env_var_amd() {
        let result = vendor_env_var("amd", "rocm-1234");
        assert_eq!(
            result,
            Some(("ROCR_VISIBLE_DEVICES".to_string(), "rocm-1234".to_string()))
        );
    }

    #[test]
    fn test_vendor_env_var_nvidia() {
        let result = vendor_env_var("nvidia", "GPU-abc123");
        assert_eq!(
            result,
            Some(("CUDA_VISIBLE_DEVICES".to_string(), "GPU-abc123".to_string()))
        );
    }

    #[test]
    fn test_vendor_env_var_vulkan() {
        let result = vendor_env_var("vulkan", "uuid");
        assert_eq!(result, None);
    }

    #[test]
    fn test_vendor_env_var_unknown() {
        let result = vendor_env_var("metal", "uuid");
        assert_eq!(result, None);
    }

    // ── resolve_gpu_device_env_from tests ─────────────────────────────

    fn build_test_gpu(device_id: &str, vendor: &str, uuid: Option<&str>) -> GpuDeviceStats {
        GpuDeviceStats {
            device_id: device_id.to_string(),
            vendor: vendor.to_string(),
            name: "".to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
            pci_bus: None,
            uuid: uuid.map(String::from),
        }
    }

    #[test]
    fn test_resolve_gpu_device_env_from_found_amd() {
        let gpus = vec![build_test_gpu("GPU1", "amd", Some("rocm-uuid"))];
        let result = resolve_gpu_device_env_from("GPU1", &gpus);
        assert_eq!(
            result,
            Some(("ROCR_VISIBLE_DEVICES".to_string(), "rocm-uuid".to_string()))
        );
    }

    #[test]
    fn test_resolve_gpu_device_env_from_found_nvidia() {
        let gpus = vec![build_test_gpu("GPU0", "nvidia", Some("GPU-uuid"))];
        let result = resolve_gpu_device_env_from("GPU0", &gpus);
        assert_eq!(
            result,
            Some(("CUDA_VISIBLE_DEVICES".to_string(), "GPU-uuid".to_string()))
        );
    }

    #[test]
    fn test_resolve_gpu_device_env_from_not_found() {
        let gpus = vec![build_test_gpu("GPU0", "nvidia", Some("GPU-uuid"))];
        let result = resolve_gpu_device_env_from("GPU99", &gpus);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_gpu_device_env_from_no_uuid() {
        let gpus = vec![build_test_gpu("GPU0", "nvidia", None)];
        let result = resolve_gpu_device_env_from("GPU0", &gpus);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_gpu_device_env_from_empty_string() {
        let gpus = vec![build_test_gpu("GPU0", "nvidia", Some("GPU-uuid"))];
        let result = resolve_gpu_device_env_from("", &gpus);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_gpu_device_env_from_whitespace() {
        let gpus = vec![build_test_gpu("GPU0", "nvidia", Some("GPU-uuid"))];
        let result = resolve_gpu_device_env_from("  ", &gpus);
        assert_eq!(result, None);
    }
}
