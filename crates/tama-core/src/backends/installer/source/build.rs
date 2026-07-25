use std::path::Path;

use crate::backends::InstallOptions;

/// Build the CMake argument list for the configure step.
///
/// Extracted for testability — callers can verify flags without invoking cmake.
pub(crate) fn build_cmake_args(
    options: &InstallOptions,
    source_dir: &Path,
    build_output: &Path,
    amdgpu_targets: &[String],
) -> Vec<String> {
    let mut cmake_args = vec![
        "-B".to_string(),
        build_output.to_string_lossy().to_string(),
        "-S".to_string(),
        source_dir.to_string_lossy().to_string(),
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
        // Build all libraries (libggml, libllama, libllama-common, etc.) as
        // static archives so the final binary is self-contained. Without this,
        // llama.cpp produces a chain of .so files (libggml.so → libggml-base.so
        // → libggml-hip.so, libllama-common.so → libllama-cli-impl.so) that
        // must be findable at runtime via RPATH/LD_LIBRARY_PATH — fragile and
        // causes "cannot open shared object" / version-mismatch crashes when
        // stale .so files are picked up. Static linking eliminates all of that.
        "-DBUILD_SHARED_LIBS=OFF".to_string(),
        // Use mold as the linker instead of GNU ld. mold is 5-10x faster on
        // the link step (saves ~30-60s on a full llama.cpp build) and uses
        // much less memory, which matters because the final link of
        // llama-server with all the GGML backends can OOM on systems with
        // limited RAM. Falls back to system linker if mold is not installed.
        "-DCMAKE_EXE_LINKER_FLAGS=-fuse-ld=mold".to_string(),
        "-DCMAKE_SHARED_LINKER_FLAGS=-fuse-ld=mold".to_string(),
        // Set RUNPATH to $ORIGIN so the binary looks for shared libraries
        // in its own directory. Belt-and-suspenders with static linking —
        // no .so files are produced, but if any third-party code adds a
        // .so dependency later, the binary will still find it.
        "-DCMAKE_BUILD_RPATH=$ORIGIN".to_string(),
        // Ensure the build RPATH is used (don't replace with install RPATH)
        "-DCMAKE_BUILD_WITH_INSTALL_RPATH=ON".to_string(),
    ];

    // Add GPU-specific flags based on gpu_variant (e.g. "cuda", "vulkan", "rocm", "metal")
    match options.gpu_variant.as_str() {
        "cuda" => {
            cmake_args.push("-DGGML_CUDA=ON".to_string());
        }
        "vulkan" => {
            cmake_args.push("-DGGML_VULKAN=ON".to_string());
        }
        "metal" => {
            cmake_args.push("-DGGML_METAL=ON".to_string());
        }
        "rocm" => {
            cmake_args.push("-DGGML_HIP=ON".to_string());
            cmake_args.push("-DGGML_HIP_ROCWMMA_FATTN=ON".to_string());
            cmake_args.push("-DGGML_CUDA_FA_ALL_QUANTS=ON".to_string());
            // Note: `-DGGML_BACKEND_DL=ON` was removed because it conflicts
            // with GGML_NATIVE (ON by default). llama.cpp cmake hard-stops
            // when both are set. Keeping GGML_NATIVE for CPU optimizations.
            // Note: `-DLLAMA_CURL=ON` was deprecated upstream and is now
            // silently ignored (emits a cmake warning). curl support is
            // handled implicitly by current llama.cpp builds, so we do
            // not pass the flag.
            if !amdgpu_targets.is_empty() {
                cmake_args.push(format!("-DAMDGPU_TARGETS={}", amdgpu_targets.join(";")));
            }
        }
        // "cpu", "custom", or any other variant — no GPU flags
        _ => {}
    }

    // Explicitly enable all IQK FlashAttention KV cache quant types for ik_llama.
    // This defaults to ON in current ik_llama.cpp main, but we set it explicitly
    // to guard against any future default change. Without it, sub-q8_0 KV cache
    // types cause NaN crashes on hybrid Mamba/attention models (e.g. Qwen3.5).
    // Note: this is GGML_IQK_FA_ALL_QUANTS (CPU IQK kernels), distinct from
    // GGML_CUDA_FA_ALL_QUANTS (CUDA FlashAttention kernels, defaults OFF).
    if matches!(
        options.backend_type,
        crate::backends::types::BackendType::IkLlama
    ) {
        cmake_args.push("-DGGML_IQK_FA_ALL_QUANTS=ON".to_string());
    }

    cmake_args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::installer::source::detect;
    use crate::backends::types::{BackendSource, BackendType};
    use std::path::PathBuf;

    fn make_options(backend_type: BackendType, gpu_variant: &str) -> InstallOptions {
        InstallOptions {
            backend_type,
            source: BackendSource::SourceCode {
                version: "main".to_string(),
                git_url: "https://example.com/repo.git".to_string(),
                commit: None,
            },
            target_dir: PathBuf::from("/tmp/test"),
            gpu_variant: gpu_variant.to_string(),
            allow_overwrite: false,
        }
    }

    /// All builds must set CMAKE_BUILD_RPATH to $ORIGIN so the binary looks
    /// for shared libraries in its own directory after installation.
    #[test]
    fn test_all_builds_include_rpath_origin() {
        let opts = make_options(BackendType::LlamaCpp, "cpu");
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(
            args.contains(&"-DCMAKE_BUILD_RPATH=$ORIGIN".to_string()),
            "All builds must include -DCMAKE_BUILD_RPATH=$ORIGIN, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DCMAKE_BUILD_WITH_INSTALL_RPATH=ON".to_string()),
            "All builds must include -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON, got: {:?}",
            args
        );
    }

    /// All builds must set BUILD_SHARED_LIBS=OFF so llama.cpp produces a
    /// single self-contained binary instead of a chain of .so files
    /// (libggml.so, libllama-common.so, libllama-cli-impl.so, etc.) that
    /// must be findable at runtime. Without this, stale .so files can be
    /// picked up and cause "cannot open shared object" or version-mismatch
    /// crashes.
    #[test]
    fn test_all_builds_use_static_libraries() {
        for variant in ["cpu", "cuda", "vulkan", "rocm", "metal"] {
            let opts = make_options(BackendType::LlamaCpp, variant);
            let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
            assert!(
                args.contains(&"-DBUILD_SHARED_LIBS=OFF".to_string()),
                "{variant} build must include -DBUILD_SHARED_LIBS=OFF, got: {:?}",
                args
            );
        }
    }

    /// All builds must use mold as the linker. mold is 5-10x faster than
    /// GNU ld on the link step and uses much less memory, which matters
    /// because linking llama-server with all GGML backends can OOM on
    /// systems with limited RAM. Falls back to system linker if mold is
    /// not installed (cmake is tolerant of missing -fuse-ld= values).
    #[test]
    fn test_all_builds_use_mold_linker() {
        for variant in ["cpu", "cuda", "vulkan", "rocm", "metal"] {
            let opts = make_options(BackendType::LlamaCpp, variant);
            let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
            assert!(
                args.contains(&"-DCMAKE_EXE_LINKER_FLAGS=-fuse-ld=mold".to_string()),
                "{variant} build must use mold linker for executables, got: {:?}",
                args
            );
            assert!(
                args.contains(&"-DCMAKE_SHARED_LINKER_FLAGS=-fuse-ld=mold".to_string()),
                "{variant} build must use mold linker for shared libs, got: {:?}",
                args
            );
        }
    }

    /// ik_llama source builds must explicitly set GGML_IQK_FA_ALL_QUANTS=ON.
    /// It defaults to ON in current ik_llama.cpp main, but we set it explicitly
    /// to guard against any future default change. Without it, sub-q8_0 KV cache
    /// causes NaN crashes on hybrid Mamba/attention models (e.g. Qwen3.5).
    #[test]
    fn test_ik_llama_includes_iqk_fa_all_quants() {
        let opts = make_options(BackendType::IkLlama, "cpu");
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(
            args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()),
            "ik_llama build must include -DGGML_IQK_FA_ALL_QUANTS=ON, got: {:?}",
            args
        );
    }

    /// llama.cpp builds must NOT include the ik_llama-specific flag.
    #[test]
    fn test_llama_cpp_excludes_iqk_fa_all_quants() {
        let opts = make_options(BackendType::LlamaCpp, "cpu");
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(
            !args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()),
            "llama.cpp build must not include -DGGML_IQK_FA_ALL_QUANTS=ON"
        );
    }

    /// ik_llama + CUDA should have both the CUDA flag and the quants flag.
    #[test]
    fn test_ik_llama_cuda_includes_both_flags() {
        let opts = make_options(BackendType::IkLlama, "cuda");
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(args.contains(&"-DGGML_CUDA=ON".to_string()));
        assert!(args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()));
    }

    /// ROCm source builds must emit the full ROCm flag set.
    #[test]
    fn test_rocm_emits_full_flag_set() {
        let opts = make_options(BackendType::LlamaCpp, "rocm");
        let args = build_cmake_args(
            &opts,
            Path::new("/src"),
            Path::new("/build"),
            &["gfx1201".to_string()],
        );
        assert!(
            args.contains(&"-DGGML_HIP=ON".to_string()),
            "ROCm build must include -DGGML_HIP=ON, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DGGML_HIP_ROCWMMA_FATTN=ON".to_string()),
            "ROCm build must include -DGGML_HIP_ROCWMMA_FATTN=ON, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DGGML_CUDA_FA_ALL_QUANTS=ON".to_string()),
            "ROCm build must include -DGGML_CUDA_FA_ALL_QUANTS=ON, got: {:?}",
            args
        );
        assert!(
            !args.iter().any(|a| a.starts_with("-DGGML_BACKEND_DL=")),
            "ROCm build must NOT include -DGGML_BACKEND_DL= (conflicts with GGML_NATIVE), got: {:?}",
            args
        );
        assert!(
            !args.iter().any(|a| a.starts_with("-DLLAMA_CURL=")),
            "ROCm build must NOT include -DLLAMA_CURL= (deprecated upstream), got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DAMDGPU_TARGETS=gfx1201".to_string()),
            "ROCm build must include -DAMDGPU_TARGETS=gfx1201, got: {:?}",
            args
        );
    }

    /// Multiple AMDGPU targets are joined with semicolons (CMake list separator).
    #[test]
    fn test_rocm_multi_target_joined_with_semicolons() {
        let opts = make_options(BackendType::LlamaCpp, "rocm");
        let args = build_cmake_args(
            &opts,
            Path::new("/src"),
            Path::new("/build"),
            &["gfx1100".to_string(), "gfx1201".to_string()],
        );
        assert!(
            args.contains(&"-DAMDGPU_TARGETS=gfx1100;gfx1201".to_string()),
            "ROCm build must join targets with ';', got: {:?}",
            args
        );
    }

    /// When no AMDGPU targets are detected, the AMDGPU_TARGETS flag is omitted
    /// (fall back to llama.cpp's default list), but other ROCm flags remain.
    #[test]
    fn test_rocm_no_targets_omits_amdgpu_targets_flag() {
        let opts = make_options(BackendType::LlamaCpp, "rocm");
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(
            !args.iter().any(|a| a.starts_with("-DAMDGPU_TARGETS=")),
            "Empty targets must omit -DAMDGPU_TARGETS=, got: {:?}",
            args
        );
        assert!(args.contains(&"-DGGML_HIP=ON".to_string()));
        assert!(args.contains(&"-DGGML_HIP_ROCWMMA_FATTN=ON".to_string()));
        assert!(args.contains(&"-DGGML_CUDA_FA_ALL_QUANTS=ON".to_string()));
        assert!(
            !args.iter().any(|a| a.starts_with("-DGGML_BACKEND_DL=")),
            "ROCm build must NOT include -DGGML_BACKEND_DL= (conflicts with GGML_NATIVE), got: {:?}",
            args
        );
        assert!(
            !args.iter().any(|a| a.starts_with("-DLLAMA_CURL=")),
            "ROCm build must NOT include -DLLAMA_CURL= (deprecated upstream), got: {:?}",
            args
        );
    }

    /// Non-ROCm GPU types must never emit ROCm flags, even if amdgpu_targets
    /// is accidentally populated by the caller.
    #[test]
    fn test_non_rocm_never_emits_rocm_flags() {
        let opts = make_options(BackendType::LlamaCpp, "cuda");
        let args = build_cmake_args(
            &opts,
            Path::new("/src"),
            Path::new("/build"),
            &["gfx1201".to_string()],
        );
        assert!(!args.contains(&"-DGGML_HIP=ON".to_string()));
        assert!(!args.contains(&"-DGGML_HIP_ROCWMMA_FATTN=ON".to_string()));
        assert!(
            !args.iter().any(|a| a.starts_with("-DAMDGPU_TARGETS=")),
            "non-ROCm build must not emit -DAMDGPU_TARGETS=, got: {:?}",
            args
        );
    }

    /// ik_llama + ROCm must include both the ik_llama-specific IQK flag and
    /// the ROCm-specific rocWMMA FlashAttention flag.
    #[test]
    fn test_ik_llama_rocm_includes_both_iqk_and_rocwmma() {
        let opts = make_options(BackendType::IkLlama, "rocm");
        let args = build_cmake_args(
            &opts,
            Path::new("/src"),
            Path::new("/build"),
            &["gfx942".to_string()],
        );
        assert!(args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()));
        assert!(args.contains(&"-DGGML_HIP_ROCWMMA_FATTN=ON".to_string()));
    }

    #[test]
    fn test_hip_env_from_hipconfig_output_happy_path() {
        let result = detect::hip_env_from_hipconfig_output("/opt/rocm/llvm/bin\n", "/opt/rocm\n");
        assert_eq!(
            result,
            Some((
                "/opt/rocm/llvm/bin/clang".to_string(),
                "/opt/rocm".to_string()
            ))
        );
    }

    #[test]
    fn test_hip_env_from_hipconfig_output_empty_stdout_returns_none() {
        assert_eq!(detect::hip_env_from_hipconfig_output("", "/opt/rocm"), None);
        assert_eq!(
            detect::hip_env_from_hipconfig_output("/opt/rocm/llvm/bin", "   "),
            None
        );
    }

    #[test]
    fn test_hip_env_from_hipconfig_output_trims_whitespace() {
        let result =
            detect::hip_env_from_hipconfig_output("  /opt/rocm/llvm/bin  \n", "\t/opt/rocm\t\n");
        assert_eq!(
            result,
            Some((
                "/opt/rocm/llvm/bin/clang".to_string(),
                "/opt/rocm".to_string()
            ))
        );
    }
}
