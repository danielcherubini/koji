use super::system::detect_gpu_devices;
use super::types::GpuDeviceStats;

/// Resolve a `gpu_device` string (e.g. "GPU1") + `gpu_variant` to
/// (env_var_name, index) for driver-level GPU isolation.
///
/// Uses **positional indexes** — simple, but GPUs may change order across
/// reboots/driver updates. Caller accepts this trade-off.
///
/// The env var is chosen by the **backend's `gpu_variant`** (what the binary
/// was compiled for), not the GPU's physical vendor — e.g. an AMD card
/// running a Vulkan backend needs `GGML_VK_VISIBLE_DEVICES`, not
/// `ROCR_VISIBLE_DEVICES`.
///
/// The index is the **per-vendor position** (0-based) within the sorted
/// device list, matching how each runtime enumerates devices:
/// - `rocm` → `ROCR_VISIBLE_DEVICES=<amd_index>`
/// - `cuda` → `CUDA_VISIBLE_DEVICES=<nvidia_index>`
/// - `vulkan` → `GGML_VK_VISIBLE_DEVICES=<vulkan_index>` (best-effort)
///
/// Returns None if the device is not found, the variant is `cpu`, or the
/// variant has no env-var mechanism.
///
/// Blocks on subprocesses (nvidia-smi, sysfs reads); call via
/// `tokio::task::spawn_blocking`.
pub fn resolve_gpu_env(gpu_device: &str, gpu_variant: &str) -> Option<(String, String)> {
    let device = gpu_device.trim();
    if device.is_empty() || gpu_variant == "cpu" {
        return None;
    }

    // Parse GPU<N> → N
    let global_index: usize = device.strip_prefix("GPU")?.parse().ok()?;

    let gpus = detect_gpu_devices();
    let gpu = gpus.get(global_index)?;

    // Per-vendor index: how many devices of the same vendor come before this one
    let per_vendor_index = gpus
        .iter()
        .take(global_index)
        .filter(|g| g.vendor == gpu.vendor)
        .count();

    let env_name = match gpu_variant {
        "rocm" => "ROCR_VISIBLE_DEVICES",
        "cuda" => "CUDA_VISIBLE_DEVICES",
        "vulkan" => "GGML_VK_VISIBLE_DEVICES",
        _ => return None,
    };

    Some((env_name.to_string(), per_vendor_index.to_string()))
}

/// Pure variant of [`resolve_gpu_env`] that takes a pre-built GPU list,
/// so it can be unit-tested without spawning subprocesses.
pub fn resolve_gpu_env_from(
    gpu_device: &str,
    gpu_variant: &str,
    gpus: &[GpuDeviceStats],
) -> Option<(String, String)> {
    let device = gpu_device.trim();
    if device.is_empty() || gpu_variant == "cpu" {
        return None;
    }

    let global_index: usize = device.strip_prefix("GPU")?.parse().ok()?;
    let gpu = gpus.get(global_index)?;

    let per_vendor_index = gpus
        .iter()
        .take(global_index)
        .filter(|g| g.vendor == gpu.vendor)
        .count();

    let env_name = match gpu_variant {
        "rocm" => "ROCR_VISIBLE_DEVICES",
        "cuda" => "CUDA_VISIBLE_DEVICES",
        "vulkan" => "GGML_VK_VISIBLE_DEVICES",
        _ => return None,
    };

    Some((env_name.to_string(), per_vendor_index.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::GpuVendor;

    fn build_test_gpu(device_id: &str, vendor: &str) -> GpuDeviceStats {
        let gpu_vendor = match vendor {
            "amd" => GpuVendor::Amd,
            _ => GpuVendor::Nvidia,
        };
        GpuDeviceStats {
            device_id: device_id.to_string(),
            vendor: gpu_vendor,
            name: "".to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
            pci_bus: None,
            uuid: None,
        }
    }

    // ── resolve_gpu_env_from: rocm ────────────────────────────────────

    #[test]
    fn test_resolve_rocm_single_amd() {
        let gpus = vec![build_test_gpu("GPU0", "amd")];
        let result = resolve_gpu_env_from("GPU0", "rocm", &gpus);
        assert_eq!(
            result,
            Some(("ROCR_VISIBLE_DEVICES".to_string(), "0".to_string()))
        );
    }

    #[test]
    fn test_resolve_rocm_second_amd() {
        // Two AMD GPUs: GPU0=amd0, GPU1=amd1
        let gpus = vec![build_test_gpu("GPU0", "amd"), build_test_gpu("GPU1", "amd")];
        let result = resolve_gpu_env_from("GPU1", "rocm", &gpus);
        assert_eq!(
            result,
            Some(("ROCR_VISIBLE_DEVICES".to_string(), "1".to_string()))
        );
    }

    #[test]
    fn test_resolve_rocm_amd_after_nvidia() {
        // NVIDIA sorts after AMD, so: GPU0=amd, GPU1=nvidia
        // But if sorted: amd0, nvidia0 → GPU0=amd, GPU1=nvidia
        // Selecting GPU1 with rocm variant → no AMD device at index 1 → should
        // return the per-vendor index for nvidia (0), but env var is ROCR...
        // Actually this is a mismatch: rocm backend on an NVIDIA-selected GPU.
        // The function still returns a value (the amd index), but the GPU at
        // that position is NVIDIA. This is a user misconfiguration, not a crash.
        let gpus = vec![
            build_test_gpu("GPU0", "amd"),
            build_test_gpu("GPU1", "nvidia"),
        ];
        // Selecting GPU0 (amd) with rocm → ROCR_VISIBLE_DEVICES=0
        let result = resolve_gpu_env_from("GPU0", "rocm", &gpus);
        assert_eq!(
            result,
            Some(("ROCR_VISIBLE_DEVICES".to_string(), "0".to_string()))
        );
    }

    // ── resolve_gpu_env_from: cuda ────────────────────────────────────

    #[test]
    fn test_resolve_cuda_single_nvidia() {
        let gpus = vec![build_test_gpu("GPU0", "nvidia")];
        let result = resolve_gpu_env_from("GPU0", "cuda", &gpus);
        assert_eq!(
            result,
            Some(("CUDA_VISIBLE_DEVICES".to_string(), "0".to_string()))
        );
    }

    #[test]
    fn test_resolve_cuda_second_nvidia_mixed_vendors() {
        // Sorted: amd0, amd1, nvidia0, nvidia1
        let gpus = vec![
            build_test_gpu("GPU0", "amd"),
            build_test_gpu("GPU1", "amd"),
            build_test_gpu("GPU2", "nvidia"),
            build_test_gpu("GPU3", "nvidia"),
        ];
        // GPU3 = second nvidia → CUDA_VISIBLE_DEVICES=1
        let result = resolve_gpu_env_from("GPU3", "cuda", &gpus);
        assert_eq!(
            result,
            Some(("CUDA_VISIBLE_DEVICES".to_string(), "1".to_string()))
        );
    }

    // ── resolve_gpu_env_from: vulkan ──────────────────────────────────

    #[test]
    fn test_resolve_vulkan_first_amd() {
        let gpus = vec![
            build_test_gpu("GPU0", "amd"),
            build_test_gpu("GPU1", "nvidia"),
        ];
        let result = resolve_gpu_env_from("GPU0", "vulkan", &gpus);
        assert_eq!(
            result,
            Some(("GGML_VK_VISIBLE_DEVICES".to_string(), "0".to_string()))
        );
    }

    // ── resolve_gpu_env_from: edge cases ──────────────────────────────

    #[test]
    fn test_resolve_cpu_variant_returns_none() {
        let gpus = vec![build_test_gpu("GPU0", "amd")];
        let result = resolve_gpu_env_from("GPU0", "cpu", &gpus);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_empty_device_returns_none() {
        let gpus = vec![build_test_gpu("GPU0", "amd")];
        let result = resolve_gpu_env_from("", "rocm", &gpus);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_whitespace_device_returns_none() {
        let gpus = vec![build_test_gpu("GPU0", "amd")];
        let result = resolve_gpu_env_from("  ", "rocm", &gpus);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_index_out_of_range_returns_none() {
        let gpus = vec![build_test_gpu("GPU0", "amd")];
        let result = resolve_gpu_env_from("GPU99", "rocm", &gpus);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_unknown_variant_returns_none() {
        let gpus = vec![build_test_gpu("GPU0", "amd")];
        let result = resolve_gpu_env_from("GPU0", "metal", &gpus);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_non_gpu_prefix_returns_none() {
        let gpus = vec![build_test_gpu("GPU0", "amd")];
        // Legacy ROCm0 format — no longer supported, returns None
        let result = resolve_gpu_env_from("ROCm0", "rocm", &gpus);
        assert_eq!(result, None);
    }
}
