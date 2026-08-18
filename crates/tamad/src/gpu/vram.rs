/// `VramInfo` is the shared wire type — single definition in
/// `tama_core::gpu::types` (plan-191 Task 10): the proxy's DB/config code
/// and the dashboard both use it, so it stays in the shared crate.
pub use tama_core::gpu::types::VramInfo;

/// Query GPU VRAM via nvidia-smi. Returns None if no NVIDIA GPU or nvidia-smi unavailable.
pub fn query_vram() -> Option<VramInfo> {
    // Try NVIDIA first
    if let Some(info) = query_nvidia_vram() {
        return Some(info);
    }
    // Fall back to AMD sysfs
    query_amd_vram()
}

fn query_nvidia_vram() -> Option<VramInfo> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.trim().split(", ").collect();
    if parts.len() == 2 {
        let used = parts[0].trim().parse().ok()?;
        let total = parts[1].trim().parse().ok()?;
        Some(VramInfo {
            used_mib: used,
            total_mib: total,
        })
    } else {
        None
    }
}

/// Read VRAM usage from AMD AMDGPU sysfs interface.
///
/// Reads `mem_info_vram_used` and `mem_info_vram_total` (reported in bytes)
/// for the first AMD card found under `/sys/class/drm/card*/device/`.
fn query_amd_vram() -> Option<VramInfo> {
    let used_pattern = "/sys/class/drm/card*/device/mem_info_vram_used";
    let total_pattern = "/sys/class/drm/card*/device/mem_info_vram_total";

    let used_bytes: u64 = glob::glob(used_pattern)
        .ok()?
        .flatten()
        .find_map(|p| std::fs::read_to_string(p).ok()?.trim().parse().ok())?;

    let total_bytes: u64 = glob::glob(total_pattern)
        .ok()?
        .flatten()
        .find_map(|p| std::fs::read_to_string(p).ok()?.trim().parse().ok())?;

    Some(VramInfo {
        used_mib: used_bytes / (1024 * 1024),
        total_mib: total_bytes / (1024 * 1024),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vram_info_available() {
        let info = VramInfo {
            used_mib: 2000,
            total_mib: 8000,
        };
        assert_eq!(info.available_mib(), 6000);
    }

    #[test]
    fn test_vram_info_zero_available() {
        let info = VramInfo {
            used_mib: 8000,
            total_mib: 8000,
        };
        assert_eq!(info.available_mib(), 0);
    }

    #[test]
    fn test_vram_info_full() {
        let info = VramInfo {
            used_mib: 0,
            total_mib: 16384,
        };
        assert_eq!(info.available_mib(), 16384);
    }
}
