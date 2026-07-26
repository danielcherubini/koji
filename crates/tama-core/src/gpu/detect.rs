use super::vram::VramInfo;

/// GPU type / backend variant.
///
/// The canonical string representation (used by `Display`, `FromStr`, and
/// serde) is the `gpu_variant` folder name — e.g. `"cuda"`, `"rocm"`,
/// `"vulkan"`. Variant payloads (`version` on `Cuda`/`RocM`) do **not**
/// survive a string round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuType {
    Cuda { version: String },
    Vulkan,
    Metal,
    RocM { version: String },
    CpuOnly,
    Custom,
}

#[derive(Debug, Clone)]
pub struct BuildPrerequisites {
    pub os: String,
    pub arch: String,
    pub cmake_available: bool,
    pub compiler_available: bool,
    pub git_available: bool,
}

/// A suggested context size with metadata.
#[derive(Debug, Clone)]
pub struct ContextSuggestion {
    /// Context length in tokens
    pub context_length: u32,
    /// Human-readable label
    pub label: String,
    /// Whether this fits in available VRAM
    pub fits: bool,
    /// Estimated VRAM needed for KV cache at this context (MiB)
    pub kv_cache_mib: u64,
}

impl GpuType {
    /// Returns the folder name used for this GPU variant.
    /// e.g. "cpu", "vulkan", "metal", "cuda", "rocm", "custom"
    pub fn variant_folder(&self) -> &str {
        match self {
            GpuType::CpuOnly => "cpu",
            GpuType::Vulkan => "vulkan",
            GpuType::Metal => "metal",
            GpuType::Cuda { .. } => "cuda",
            GpuType::RocM { .. } => "rocm",
            GpuType::Custom => "custom",
        }
    }
}

impl std::fmt::Display for GpuType {
    /// Format as the canonical `gpu_variant` string (e.g. `"cuda"`, `"rocm"`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.variant_folder())
    }
}

impl std::str::FromStr for GpuType {
    type Err = anyhow::Error;

    /// Parse a gpu_variant string, case-insensitively
    /// (`"cpu"`, `"cuda"`, `"vulkan"`, `"rocm"`, `"metal"`, `"custom"`).
    ///
    /// **Note:** variant payloads (`version` on `Cuda`/`RocM`) are always
    /// set to an empty string after parsing — they do not survive a
    /// string round-trip.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "cpu" => Ok(GpuType::CpuOnly),
            "cuda" => Ok(GpuType::Cuda {
                version: String::new(),
            }),
            "vulkan" => Ok(GpuType::Vulkan),
            "metal" => Ok(GpuType::Metal),
            "rocm" => Ok(GpuType::RocM {
                version: String::new(),
            }),
            "custom" => Ok(GpuType::Custom),
            other => Err(anyhow::anyhow!(
                "unknown gpu_variant '{}'; expected one of: cpu, cuda, vulkan, rocm, metal, custom",
                other
            )),
        }
    }
}

impl serde::Serialize for GpuType {
    /// Serialize as the canonical `gpu_variant` string.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.variant_folder())
    }
}

impl<'de> serde::Deserialize<'de> for GpuType {
    /// Deserialize from a `gpu_variant` string.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        <GpuType as std::str::FromStr>::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Default CUDA version used when auto-detection fails.
pub const DEFAULT_CUDA_VERSION: &str = "12.4";

/// Detect the installed CUDA toolkit version.
///
/// Tries `nvcc --version` first (most reliable), then falls back to
/// `nvidia-smi` driver-reported CUDA version. Returns `None` if neither
/// is available.
pub fn detect_cuda_version() -> Option<String> {
    // Try nvcc first — this reports the actual toolkit version
    if let Some(ver) = detect_cuda_version_nvcc() {
        return Some(ver);
    }
    // Fall back to nvidia-smi — reports the max supported CUDA version
    detect_cuda_version_nvidia_smi()
}

/// Parse CUDA version from `nvcc --version` output.
fn detect_cuda_version_nvcc() -> Option<String> {
    let output = std::process::Command::new("nvcc")
        .arg("--version")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // nvcc output contains a line like: "Cuda compilation tools, release 12.4, V12.4.131"
    // or "Cuda compilation tools, release 13.1, V13.1.105"
    for line in stdout.lines() {
        if let Some(pos) = line.find("release ") {
            let after = &line[pos + 8..];
            // Take "12.4" from "12.4, V12.4.131"
            let version = after.split(',').next()?.trim();
            if !version.is_empty() {
                return Some(version.to_string());
            }
        }
    }
    None
}

/// Parse CUDA version from `nvidia-smi` output.
fn detect_cuda_version_nvidia_smi() -> Option<String> {
    let output = std::process::Command::new("nvidia-smi").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // nvidia-smi header contains: "CUDA Version: 12.4"
    for line in stdout.lines() {
        if let Some(pos) = line.find("CUDA Version:") {
            let after = &line[pos + 13..];
            let version = after.split_whitespace().next()?;
            if !version.is_empty() {
                return Some(version.to_string());
            }
        }
    }
    None
}

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

/// Parse `rocminfo` stdout and extract unique `gfxNNNN` architecture names.
///
/// Returns the deduplicated list in first-seen order. Ignores CPU `Name:`
/// lines and any other `Name:` entries that don't match `gfx<hex+>`.
pub fn parse_rocminfo_gfx_names(stdout: &str) -> Vec<String> {
    use std::collections::HashSet;

    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        let rest = match trimmed.strip_prefix("Name:") {
            Some(r) => r,
            None => continue,
        };
        let token = match rest.split_whitespace().next() {
            Some(t) => t,
            None => continue,
        };
        let digits = match token.strip_prefix("gfx") {
            Some(d) => d,
            None => continue,
        };
        if digits.is_empty()
            || !digits
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            continue;
        }
        if seen.insert(token.to_string()) {
            result.push(token.to_string());
        }
    }

    result
}

/// Detect build prerequisites (OS, arch, cmake, compiler, git).
pub fn detect_build_prerequisites() -> BuildPrerequisites {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();

    let cmake_available = std::process::Command::new("cmake")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let git_available = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let compiler_available = {
        // Try g++ first (C++ compiler needed for llama.cpp), then c++
        std::process::Command::new("g++")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
            || std::process::Command::new("c++")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
    };

    BuildPrerequisites {
        os,
        arch,
        cmake_available,
        compiler_available,
        git_available,
    }
}

/// Suggest context sizes based on available VRAM and model size.
///
/// Uses a simple heuristic: KV cache grows roughly linearly with context.
/// For a ~7-9B model, KV cache at FP16 is ~0.5 GiB per 8K context.
/// With quantized KV (Q4/Q8), it's 2-4x smaller.
///
/// `model_size_bytes`: size of the GGUF file on disk
/// `vram`: GPU VRAM info (if available)
pub fn suggest_context_sizes(
    model_size_bytes: u64,
    vram: Option<&VramInfo>,
) -> Vec<ContextSuggestion> {
    // Estimate param count from model size (very rough)
    // Q4_K_M: ~0.6 bytes/param, Q8_0: ~1.1 bytes/param, FP16: ~2 bytes/param
    // Use 0.8 as middle ground for "average quant"
    let est_params_b = model_size_bytes as f64 / 0.8 / 1_000_000_000.0;

    // KV cache sizing (empirical from llama.cpp benchmarks):
    // A 7B model with FP16 KV cache uses ~256 MiB per 4K context.
    // That's ~64 MiB per 1K context for 7B params.
    // With Q4_0 KV cache it's ~4x less, but we estimate for FP16 (safe default).
    let mib_per_1k_ctx = 64.0 * (est_params_b / 7.0);

    let context_tiers: Vec<(u32, &str)> = vec![
        (2048, "2K (minimal)"),
        (4096, "4K (small)"),
        (8192, "8K (standard)"),
        (16384, "16K"),
        (32768, "32K"),
        (65536, "64K"),
        (100000, "100K"),
        (131072, "128K (max for most models)"),
    ];

    let available_for_kv = match vram {
        Some(v) => {
            let model_mib = model_size_bytes / (1024 * 1024);
            // Reserve model size + 512 MiB overhead for compute buffers
            v.total_mib.saturating_sub(model_mib).saturating_sub(512)
        }
        None => u64::MAX, // No GPU info — mark everything as "fits" but we don't know
    };

    context_tiers
        .into_iter()
        .map(|(ctx, desc)| {
            let kv_mib = (mib_per_1k_ctx * (ctx as f64 / 1024.0)) as u64;
            let fits = kv_mib <= available_for_kv;
            ContextSuggestion {
                context_length: ctx,
                label: format!(
                    "{} — ~{} MiB KV cache{}",
                    desc,
                    kv_mib,
                    if fits { "" } else { "  [may not fit]" }
                ),
                fits,
                kv_cache_mib: kv_mib,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_parse_rocminfo_gfx_names_single_gpu() {
        let sample = "\
*******                  Agent 1                  *******
  Name:                    AMD Ryzen 9 7950X 16-Core Processor
  Uuid:                    CPU-XX
*******                  Agent 2                  *******
  Name:                    gfx1201
  Uuid:                    GPU-XX
";
        assert_eq!(parse_rocminfo_gfx_names(sample), vec!["gfx1201"]);
    }

    #[test]
    fn test_parse_rocminfo_gfx_names_multi_gpu_dedup() {
        let sample = "\
  Name:                    gfx1100
  Name:                    gfx1201
  Name:                    AMD Ryzen 9
  Name:                    gfx1100
";
        assert_eq!(parse_rocminfo_gfx_names(sample), vec!["gfx1100", "gfx1201"]);
    }

    #[test]
    fn test_parse_rocminfo_gfx_names_no_match() {
        let sample = "\
  Name:                    AMD Ryzen 9 7950X 16-Core Processor
  Name:                    Intel Core i9
";
        assert!(parse_rocminfo_gfx_names(sample).is_empty());
    }

    #[test]
    fn test_parse_rocminfo_gfx_names_tolerates_trailing_whitespace() {
        let sample = "  Name:   gfx942   \n";
        assert_eq!(parse_rocminfo_gfx_names(sample), vec!["gfx942"]);
    }

    #[test]
    fn test_parse_rocminfo_gfx_names_empty() {
        assert!(parse_rocminfo_gfx_names("").is_empty());
    }

    #[test]
    fn test_parse_rocminfo_gfx_names_only_whitespace() {
        assert!(parse_rocminfo_gfx_names("   \n\n  ").is_empty());
    }

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
    fn test_detect_build_prerequisites() {
        let caps = detect_build_prerequisites();
        assert!(!caps.os.is_empty());
        assert!(!caps.arch.is_empty());
        // No gpu field — that's the point
    }

    #[test]
    fn test_detect_cuda_version_nvcc_parsing() {
        // Simulate nvcc output parsing
        let sample = "nvcc: NVIDIA (R) Cuda compiler driver\n\
                       Copyright (c) 2005-2024 NVIDIA Corporation\n\
                       Built on Thu_Mar_28_02:18:24_PDT_2024\n\
                       Cuda compilation tools, release 12.4, V12.4.131\n\
                       Build cuda_12.4.r12.4/compiler.34097967_0";

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

    #[test]
    fn test_variant_folder_all_variants() {
        assert_eq!(GpuType::CpuOnly.variant_folder(), "cpu");
        assert_eq!(GpuType::Vulkan.variant_folder(), "vulkan");
        assert_eq!(GpuType::Metal.variant_folder(), "metal");
        assert_eq!(
            GpuType::Cuda {
                version: "12.4".to_string()
            }
            .variant_folder(),
            "cuda"
        );
        assert_eq!(
            GpuType::RocM {
                version: "6.0".to_string()
            }
            .variant_folder(),
            "rocm"
        );
        assert_eq!(GpuType::Custom.variant_folder(), "custom");
    }

    #[test]
    fn test_default_cuda_version_is_set() {
        assert!(!DEFAULT_CUDA_VERSION.is_empty());
    }

    #[test]
    fn test_default_cuda_version_format() {
        // Should be a valid version string like "12.4"
        assert!(DEFAULT_CUDA_VERSION
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.'));
    }

    #[test]
    fn test_suggest_context_sizes_no_gpu() {
        // 5.6 GB model, no GPU info
        let suggestions = suggest_context_sizes(5_600_000_000, None);
        assert!(!suggestions.is_empty());
        // All should be marked as fits since we don't know
        assert!(suggestions.iter().all(|s| s.fits));
    }

    #[test]
    fn test_suggest_context_sizes_with_gpu() {
        let vram = VramInfo {
            used_mib: 0,
            total_mib: 8192, // 8 GiB GPU
        };
        // 5.6 GB Q4 model — leaves ~2.3 GiB for KV after model + overhead
        let suggestions = suggest_context_sizes(5_600_000_000, Some(&vram));
        assert!(!suggestions.is_empty());
        // Small contexts should fit
        assert!(suggestions[0].fits); // 2K
        assert!(suggestions[1].fits); // 4K
                                      // 32K just barely fits (2048 MiB KV < 2339 MiB available)
        let ctx_32k = suggestions
            .iter()
            .find(|s| s.context_length == 32768)
            .unwrap();
        assert!(ctx_32k.fits);
        // 64K+ should not fit (~4 GiB KV > 2.3 GiB available)
        let ctx_64k = suggestions
            .iter()
            .find(|s| s.context_length == 65536)
            .unwrap();
        assert!(!ctx_64k.fits);
        // 128K definitely won't fit
        let last = suggestions.last().unwrap();
        assert!(!last.fits);
    }

    #[test]
    fn test_suggest_context_sizes_large_gpu() {
        let vram = VramInfo {
            used_mib: 0,
            total_mib: 24576, // 24 GiB GPU
        };
        // Small model on big GPU — more contexts should fit
        let suggestions = suggest_context_sizes(5_600_000_000, Some(&vram));
        let fitting: Vec<_> = suggestions.iter().filter(|s| s.fits).collect();
        // 24 GiB - 5.3 GiB model - 0.5 GiB overhead = ~18 GiB for KV
        // 8K = ~512 MiB, 16K = ~1GiB, 32K = ~2GiB — all should fit
        assert!(fitting.len() >= 4);
    }

    #[test]
    fn test_suggest_context_sizes_empty_model() {
        let vram = VramInfo {
            used_mib: 0,
            total_mib: 8192,
        };
        let suggestions = suggest_context_sizes(0, Some(&vram));
        assert!(!suggestions.is_empty());
        // With no model, all contexts should fit
        assert!(suggestions.iter().all(|s| s.fits));
    }

    #[test]
    fn test_suggest_context_sizes_very_large_model() {
        // 24 GB model on 8 GB GPU — nothing should fit
        let vram = VramInfo {
            used_mib: 0,
            total_mib: 8192,
        };
        let suggestions = suggest_context_sizes(24_000_000_000, Some(&vram));
        assert!(suggestions.iter().all(|s| !s.fits));
    }

    #[test]
    fn test_suggest_context_sizes_no_gpu_all_fits() {
        let suggestions = suggest_context_sizes(1_000_000_000, None);
        // Without GPU info, all should be marked as fits
        assert!(suggestions.iter().all(|s| s.fits));
    }

    // -----------------------------------------------------------------------
    // FromStr / Display / string serde tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_from_str_all_variants() {
        assert_eq!("cpu".parse::<GpuType>().unwrap(), GpuType::CpuOnly);
        assert_eq!(
            "cuda".parse::<GpuType>().unwrap(),
            GpuType::Cuda {
                version: String::new()
            }
        );
        assert_eq!("vulkan".parse::<GpuType>().unwrap(), GpuType::Vulkan);
        assert_eq!("metal".parse::<GpuType>().unwrap(), GpuType::Metal);
        assert_eq!(
            "rocm".parse::<GpuType>().unwrap(),
            GpuType::RocM {
                version: String::new()
            }
        );
        assert_eq!("custom".parse::<GpuType>().unwrap(), GpuType::Custom);
    }

    #[test]
    fn test_from_str_case_insensitive() {
        assert_eq!(
            "CUDA".parse::<GpuType>().unwrap(),
            GpuType::Cuda {
                version: String::new()
            }
        );
        assert_eq!(
            "Rocm".parse::<GpuType>().unwrap(),
            GpuType::RocM {
                version: String::new()
            }
        );
        assert_eq!(
            "ROCM".parse::<GpuType>().unwrap(),
            GpuType::RocM {
                version: String::new()
            }
        );
        assert_eq!("Cpu".parse::<GpuType>().unwrap(), GpuType::CpuOnly);
    }

    #[test]
    fn test_from_str_unknown_rejected() {
        assert!("tpu".parse::<GpuType>().is_err());
        assert!("".parse::<GpuType>().is_err());
        assert!("gpu".parse::<GpuType>().is_err());
    }

    #[test]
    fn test_display_matches_variant_folder() {
        let variants = [
            GpuType::CpuOnly,
            GpuType::Vulkan,
            GpuType::Metal,
            GpuType::Cuda {
                version: "12.4".into(),
            },
            GpuType::RocM {
                version: "6.0".into(),
            },
            GpuType::Custom,
        ];
        for v in &variants {
            assert_eq!(format!("{}", v), v.variant_folder());
        }
    }

    #[test]
    fn test_string_roundtrip_canonical() {
        let variants = [
            GpuType::CpuOnly,
            GpuType::Cuda {
                version: String::new(),
            },
            GpuType::Vulkan,
            GpuType::Metal,
            GpuType::RocM {
                version: String::new(),
            },
            GpuType::Custom,
        ];
        for v in &variants {
            let s = v.to_string();
            assert_eq!(s.parse::<GpuType>().unwrap(), *v);
        }
    }

    #[test]
    fn test_serde_string_form() {
        // Serialize — version payloads do not survive
        assert_eq!(
            serde_json::to_string(&GpuType::Cuda {
                version: "12.4".into()
            })
            .unwrap(),
            r#""cuda""#
        );
        // Deserialize
        assert_eq!(
            serde_json::from_str::<GpuType>(r#""vulkan""#).unwrap(),
            GpuType::Vulkan
        );
        // Rejection
        assert!(serde_json::from_str::<GpuType>(r#""tpu""#).is_err());
    }
}
