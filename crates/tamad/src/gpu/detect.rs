//! Host GPU detection that shells out to vendor tooling (plan-191 Task 10).
//!
//! Moved from `tama_core::gpu::detect` — ADR-0010: only the tamad (which
//! owns the hardware) may probe `nvcc`/`nvidia-smi`/`rocminfo`. The pure
//! `GpuVariant` type, serde impls, and `parse_rocminfo_gfx_names` parser
//! stay in `tama_core::gpu` (shared by the proxy's DB/config code).

use tama_core::gpu::parse_rocminfo_gfx_names;

/// Detect AMD GPU architectures suitable for `-DAMDGPU_TARGETS=...`.
///
/// Honors `TAMA_AMDGPU_TARGETS` as an override (accepts `;` or `,` as
/// separators; whitespace trimmed; empty entries dropped). Otherwise runs
/// `rocminfo` and parses `Name:\s+gfx[0-9a-f]+` lines. Returns the
/// deduplicated list in first-seen order. Returns an empty `Vec` if
/// detection fails, `rocminfo` is unavailable, or no gfx entries are found.
///
/// This function is Linux-oriented but compiles on all platforms — on
/// non-Linux hosts it returns `Vec::new()` unless the env override is set.
pub fn detect_amdgpu_targets() -> Vec<String> {
    if let Ok(raw) = std::env::var("TAMA_AMDGPU_TARGETS") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed
                .split([',', ';'])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    let output = match std::process::Command::new("rocminfo").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_rocminfo_gfx_names(&stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_detect_amdgpu_targets_env_override_semicolons() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TAMA_AMDGPU_TARGETS");
        std::env::set_var("TAMA_AMDGPU_TARGETS", "gfx1100;gfx1201");
        let result = detect_amdgpu_targets();
        std::env::remove_var("TAMA_AMDGPU_TARGETS");
        assert_eq!(result, vec!["gfx1100", "gfx1201"]);
    }

    #[test]
    fn test_detect_amdgpu_targets_env_override_commas_and_whitespace() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TAMA_AMDGPU_TARGETS");
        std::env::set_var("TAMA_AMDGPU_TARGETS", "  gfx942 , gfx90a ");
        let result = detect_amdgpu_targets();
        std::env::remove_var("TAMA_AMDGPU_TARGETS");
        assert_eq!(result, vec!["gfx942", "gfx90a"]);
    }

    #[test]
    fn test_detect_amdgpu_targets_env_override_empty_is_ignored() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TAMA_AMDGPU_TARGETS");
        std::env::set_var("TAMA_AMDGPU_TARGETS", "");
        let result = detect_amdgpu_targets();
        std::env::remove_var("TAMA_AMDGPU_TARGETS");
        assert!(result.is_empty() || result.iter().all(|s| s.starts_with("gfx")));
    }

    #[test]
    fn test_detect_amdgpu_targets_env_override_single_value() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("TAMA_AMDGPU_TARGETS");
        std::env::set_var("TAMA_AMDGPU_TARGETS", "gfx1100");
        let result = detect_amdgpu_targets();
        std::env::remove_var("TAMA_AMDGPU_TARGETS");
        assert_eq!(result, vec!["gfx1100"]);
    }

    #[test]
    fn test_detect_cuda_version_nvcc_parsing() {
        // Simulate nvcc output parsing
        let sample = "nvcc: NVIDIA (R) Cuda compiler driver\n\
                       Copyright (c) 2005-2024 NVIDIA Corporation\n\
                       Built on Thu_Mar_28_02:18:24_PDT_2024\n\
                       Cuda compilation tools, release 12.4, V12.4.131\n\
                       Build cuda_12.4.r12.4/compiler_84907967_0";

        let mut version = None;
        for line in sample.lines() {
            if let Some(pos) = line.find("release ") {
                let after = &line[pos + 8..];
                if let Some(v) = after.split(',').next() {
                    let v = v.trim();
                    if !v.is_empty() {
                        version = Some(v.to_string());
                    }
                }
            }
        }
        assert_eq!(version, Some("12.4".to_string()));
    }

    #[test]
    fn test_detect_cuda_version_nvcc_parsing_v13() {
        let sample = "Cuda compilation tools, release 13.1, V13.1.105";
        let mut version = None;
        for line in sample.lines() {
            if let Some(pos) = line.find("release ") {
                let after = &line[pos + 8..];
                if let Some(v) = after.split(',').next() {
                    let v = v.trim();
                    if !v.is_empty() {
                        version = Some(v.to_string());
                    }
                }
            }
        }
        assert_eq!(version, Some("13.1".to_string()));
    }

    #[test]
    fn test_detect_cuda_version_nvidia_smi_parsing() {
        let sample =
            "| NVIDIA-SMI 550.54.14    Driver Version: 550.54.14    CUDA Version: 12.4     |";
        let mut version = None;
        for line in sample.lines() {
            if let Some(pos) = line.find("CUDA Version:") {
                let after = &line[pos + 13..];
                if let Some(v) = after.split_whitespace().next() {
                    if !v.is_empty() {
                        version = Some(v.to_string());
                    }
                }
            }
        }
        assert_eq!(version, Some("12.4".to_string()));
    }
}
