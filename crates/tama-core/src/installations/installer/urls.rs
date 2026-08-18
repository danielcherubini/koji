use anyhow::{anyhow, Result};

use crate::installations::types::InstallationType;

// E2E seam: when `TAMA_E2E_GITHUB_BASE` is set (integration/E2E harnesses
// only), GitHub release hosts are rewritten to that local base so a local
// server can serve mock release data + archives. Production (env unset)
// is unaffected.
fn apply_e2e_base(url: &str) -> Option<String> {
    match std::env::var("TAMA_E2E_GITHUB_BASE") {
        Ok(base) if !base.is_empty() => {
            let base = base.trim_end_matches('/');
            url.strip_prefix("https://github.com/")
                .or_else(|| url.strip_prefix("https://api.github.com/"))
                .map(|rest| format!("{}/{}", base, rest))
        }
        _ => None,
    }
}

/// Construct the GitHub release download URL for a pre-built binary.
///
/// The `gpu_variant` parameter is a folder name string (e.g. "cpu", "cuda",
/// "vulkan", "rocm") that determines which pre-built binary to download.
/// Note: The `tag` parameter is the release tag (e.g. "b8407"), kept
/// separate from any GPU version strings to avoid shadowing.
pub fn get_prebuilt_url(
    backend: &InstallationType,
    tag: &str,
    os: &str,
    arch: &str,
    gpu_variant: &str,
) -> Result<String> {
    match backend {
        InstallationType::LlamaCpp => {
            let base = format!(
                "https://github.com/ggml-org/llama.cpp/releases/download/{}/",
                tag
            );

            let filename = match (os, arch, gpu_variant) {
                // Linux - Vulkan
                ("linux", "x86_64", "vulkan") => {
                    format!("llama-{}-bin-ubuntu-vulkan-x64.tar.gz", tag)
                }
                // Linux - ROCm
                ("linux", "x86_64", "rocm") => {
                    format!("llama-{}-bin-ubuntu-rocm-7.2-x64.tar.gz", tag)
                }
                // Linux - CPU, CUDA, and all other variants
                // (llama.cpp doesn't ship Linux CUDA pre-built binaries;
                // they use the generic ubuntu-x64 build)
                ("linux", "x86_64", _) => {
                    format!("llama-{}-bin-ubuntu-x64.tar.gz", tag)
                }
                // macOS - ARM64
                ("macos", "aarch64", _) => {
                    format!("llama-{}-bin-macos-arm64.tar.gz", tag)
                }
                // macOS - x86_64
                ("macos", "x86_64", _) => {
                    format!("llama-{}-bin-macos-x64.tar.gz", tag)
                }
                _ => return Err(anyhow!("Unsupported platform: {} {}", os, arch)),
            };

            let url = format!("{}{}", base, filename);
            Ok(apply_e2e_base(&url).unwrap_or(url))
        }
        InstallationType::IkLlama => {
            Err(anyhow!(
                "ik_llama does not provide pre-built release binaries. Use --build to build from source."
            ))
        }
        InstallationType::TtsKokoro | InstallationType::Compaction => {
            Err(anyhow!(
                "Non-inference backends do not provide pre-built release binaries. Use --build to build from source."
            ))
        }
        InstallationType::Custom => {
            Err(anyhow!("Custom backends must be added manually"))
        }
        InstallationType::Docker => {
            Err(anyhow!("Docker backends do not use pre-built binaries"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// E2E seam: when `TAMA_E2E_GITHUB_BASE` is set (integration/E2E
    /// harnesses only), the release download host is rewritten to that
    /// base so a local server can serve mock release archives. Production
    /// (env unset) is unaffected.
    #[test]
    fn test_prebuilt_url_e2e_base_redirect() {
        std::env::set_var("TAMA_E2E_GITHUB_BASE", "http://127.0.0.1:8991/");
        let url = get_prebuilt_url(
            &InstallationType::LlamaCpp,
            "b8407",
            "linux",
            "x86_64",
            "cpu",
        )
        .unwrap();
        assert_eq!(
            url,
            "http://127.0.0.1:8991/ggml-org/llama.cpp/releases/download/b8407/llama-b8407-bin-ubuntu-x64.tar.gz"
        );
        std::env::remove_var("TAMA_E2E_GITHUB_BASE");

        // Unset → the real GitHub URL.
        let url = get_prebuilt_url(
            &InstallationType::LlamaCpp,
            "b8407",
            "linux",
            "x86_64",
            "cuda",
        )
        .unwrap();
        assert_eq!(
            url,
            "https://github.com/ggml-org/llama.cpp/releases/download/b8407/llama-b8407-bin-ubuntu-x64.tar.gz"
        );
    }

    #[test]
    fn test_llama_cpp_download_url_linux_cpu() {
        let url = get_prebuilt_url(
            &InstallationType::LlamaCpp,
            "b8407",
            "linux",
            "x86_64",
            "cpu",
        )
        .unwrap();

        assert_eq!(
            url,
            "https://github.com/ggml-org/llama.cpp/releases/download/b8407/llama-b8407-bin-ubuntu-x64.tar.gz"
        );
    }

    #[test]
    fn test_llama_cpp_download_url_linux_vulkan() {
        let url = get_prebuilt_url(
            &InstallationType::LlamaCpp,
            "b8407",
            "linux",
            "x86_64",
            "vulkan",
        )
        .unwrap();

        assert_eq!(
            url,
            "https://github.com/ggml-org/llama.cpp/releases/download/b8407/llama-b8407-bin-ubuntu-vulkan-x64.tar.gz"
        );
    }

    #[test]
    fn test_llama_cpp_download_url_linux_rocm() {
        let url = get_prebuilt_url(
            &InstallationType::LlamaCpp,
            "b8407",
            "linux",
            "x86_64",
            "rocm",
        )
        .unwrap();

        assert_eq!(
            url,
            "https://github.com/ggml-org/llama.cpp/releases/download/b8407/llama-b8407-bin-ubuntu-rocm-7.2-x64.tar.gz"
        );
    }

    #[test]
    fn test_ik_llama_prebuilt_not_available() {
        let result = get_prebuilt_url(&InstallationType::IkLlama, "main", "linux", "x86_64", "cpu");
        assert!(result.is_err());
    }
}
