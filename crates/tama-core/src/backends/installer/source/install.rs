use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use zip::ZipArchive;

use super::build::build_cmake_args;
use super::detect::detect_hip_env;
use super::detect::detect_rocm_lib_dir;
use super::detect::register_ldconfig_path;
use crate::backends::installer::emit;
use crate::backends::installer::extract::find_backend_binary;
use crate::backends::installer::prebuilt::prepare_target_dir;
use crate::backends::types::BackendType;
use crate::backends::InstallOptions;
use crate::backends::ProgressSink;
use crate::gpu::detect_amdgpu_targets;

/// Build and install a backend from source using git + cmake.
pub async fn install_from_source(
    options: &InstallOptions,
    version: &str,
    git_url: &str,
    commit: Option<&str>,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<PathBuf> {
    emit(
        progress,
        format!("Building from source: {} version {}", git_url, version),
    );

    prepare_target_dir(&options.target_dir, options.allow_overwrite)?;

    // Check prerequisites
    let caps = crate::gpu::detect_build_prerequisites();
    // Git is only needed for ik_llama (no releases) and commit-based builds.
    // llama.cpp uses GitHub source ZIP downloads instead.
    if !caps.git_available
        && (matches!(options.backend_type, BackendType::IkLlama) || commit.is_some())
    {
        return Err(anyhow!(
            "Git is required to build from source.\n\
             Install it: https://git-scm.com/downloads\n\
             Linux: sudo apt install git (Debian/Ubuntu) or sudo dnf install git (Fedora)"
        ));
    }
    if !caps.cmake_available {
        return Err(anyhow!(
            "CMake is required to build from source.\n\
             Install it: https://cmake.org/download/\n\
             Linux: sudo apt install cmake (Debian/Ubuntu) or sudo dnf install cmake (Fedora)"
        ));
    }
    if !caps.compiler_available {
        return Err(anyhow!(
            "C++ compiler is required to build from source.\n\
             Linux: sudo apt install build-essential"
        ));
    }

    // Use a persistent build directory inside the target dir so that debug
    // symbols in the compiled binary point to real paths (not a temp dir that
    // gets deleted). This also lets users inspect the source if a crash log
    // references a file path.
    let build_root = options.target_dir.join("build");
    let source_dir = build_root.join("source");
    let build_output = build_root.join("cmake");

    // Clean any previous build attempt
    if build_root.exists() {
        std::fs::remove_dir_all(&build_root)?;
    }
    std::fs::create_dir_all(&build_output)?;

    // Resolve "latest" using the shared check_latest_version function
    // (uses GitHub releases API, same as the CLI and update checker).
    // For ik_llama, check_latest_version returns "main@sha" which we handle below.
    let resolved_version = if version == "latest"
        && !matches!(options.backend_type, BackendType::TtsKokoro)
    {
        match crate::backends::check_latest_version(&options.backend_type, None, None).await {
            Ok(v) => {
                emit(progress, format!("Resolved 'latest' to: {}", v));
                v
            }
            Err(e) => {
                emit(
                    progress,
                    format!("Warning: Could not resolve latest version: {} — falling back to 'latest' tag", e),
                );
                version.to_string()
            }
        }
    } else {
        version.to_string()
    };

    // Fetch source — download ZIP for llama.cpp releases, git clone for ik_llama/commits
    fetch_source(
        &options.backend_type,
        &resolved_version,
        git_url,
        &source_dir,
        commit,
        progress,
    )
    .await?;

    // Configure with CMake
    configure_cmake(options, &source_dir, &build_output, progress).await?;

    // Build
    build_cmake(&build_output, progress).await?;

    // Register ROCm library path with ldconfig so the built binary can find
    // shared libraries like libhipblas.so at runtime.
    #[cfg(not(target_os = "windows"))]
    if options.gpu_variant == "rocm" {
        if let Some(lib_dir) = detect_rocm_lib_dir() {
            match register_ldconfig_path(&lib_dir, "rocm.conf") {
                Ok(()) => {
                    tracing::info!("Registered ROCm library path: {}", lib_dir);
                }
                Err(e) => {
                    emit(
                        progress,
                        format!(
                            "Warning: Could not register ROCm library path with ldconfig: {}.\n\n\
                             The built binary may fail with 'cannot open shared object file' errors.\n\
                             Fix: run as root:  echo '{lib_dir}' > /etc/ld.so.conf.d/rocm.conf && ldconfig\n\
                             Or set LD_LIBRARY_PATH:  export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:{lib_dir}",
                            e
                        ),
                    );
                }
            }
        }
    }

    // Install binary
    let result = install_binary(&build_output, options, progress).await;

    // Register the backend install directory with ldconfig so the binary can
    // find its own shared libraries (libllama.so, libllama-common.so, etc.).
    #[cfg(not(target_os = "windows"))]
    if result.is_ok() {
        let target = options
            .target_dir
            .canonicalize()
            .unwrap_or(options.target_dir.clone());
        let target_str = target.to_string_lossy().to_string();
        match register_ldconfig_path(&target_str, "tama-backend.conf") {
            Ok(()) => {
                tracing::info!("Registered backend library path: {}", target_str);
            }
            Err(e) => {
                emit(
                    progress,
                    format!(
                        "Warning: Could not register backend library path with ldconfig: {}.\n\n\
                         The built binary may fail with 'cannot open shared object file' errors.\n\
                         Fix: run as root:  echo '{}' > /etc/ld.so.conf.d/tama-backend.conf && ldconfig\n\
                         Or set LD_LIBRARY_PATH:  export LD_LIBRARY_PATH=$LD_LIBRARY_PATH:{}",
                        e, target_str, target_str
                    ),
                );
            }
        }
    }

    // Clean up build artifacts on success — the binary is installed and the
    // multi-GB build tree is no longer needed. On failure, leave it in place
    // so the source paths in any crash logs remain valid for debugging.
    if result.is_ok() {
        if let Err(e) = std::fs::remove_dir_all(&build_root) {
            tracing::warn!("Failed to clean up build directory: {}", e);
        }
    }

    result
}

/// Fetch source code for building — downloads a ZIP for llama.cpp releases,
/// falls back to git clone for ik_llama (no releases) or commit-based builds.
async fn fetch_source(
    backend_type: &BackendType,
    version: &str,
    git_url: &str,
    source_dir: &Path,
    commit: Option<&str>,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    // For llama.cpp with a release tag (e.g. "b9585"), download the source ZIP.
    // This avoids needing git and is faster for large repos.
    if matches!(backend_type, BackendType::LlamaCpp)
        && commit.is_none()
        && !version.starts_with("main@")
    {
        return download_source_zip(backend_type, version, source_dir, progress).await;
    }

    // Fall back to git clone for ik_llama, commit-based builds, or edge cases.
    clone_repository(version, git_url, source_dir, commit, progress).await
}

/// Download a GitHub source ZIP and extract it to the source directory.
async fn download_source_zip(
    backend_type: &BackendType,
    version: &str,
    source_dir: &Path,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    let repo_match = match backend_type {
        BackendType::LlamaCpp => "ggml-org/llama.cpp",
        _ => return Err(anyhow!("ZIP download not supported for this backend")),
    };

    let zip_url = format!(
        "https://github.com/{}/archive/refs/tags/{}.zip",
        repo_match, version
    );

    emit(progress, format!("Downloading source archive: {}", version));

    let client = Client::builder()
        .user_agent("tama-backend-manager")
        .timeout(std::time::Duration::from_secs(300))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()?;

    let response = client
        .get(&zip_url)
        .send()
        .await
        .with_context(|| format!("Failed to download source ZIP from {}", zip_url))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Failed to download source ZIP: HTTP {}",
            response.status()
        ));
    }

    let bytes = response
        .bytes()
        .await
        .with_context(|| "Failed to read source ZIP bytes")?;

    emit(progress, "Extracting source archive...");
    std::fs::create_dir_all(source_dir)?;

    let cursor = Cursor::new(bytes.as_ref());
    let mut archive =
        ZipArchive::new(cursor).with_context(|| "Failed to open source ZIP archive")?;

    // GitHub source ZIPs have a top-level directory like "llama.cpp-b9585/".
    // We need to extract its contents directly into source_dir.
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let entry_name = entry
            .enclosed_name()
            .ok_or_else(|| anyhow!("Invalid path in archive: {}", entry.name()))?;

        let outpath = source_dir.join(&entry_name);

        if entry.is_dir() {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut entry, &mut outfile)?;
        }
    }

    // GitHub ZIPs wrap everything in a top-level dir like "llama.cpp-b9585/".
    // Move contents out so source_dir has the source files directly.
    let children: Vec<_> = std::fs::read_dir(source_dir)?
        .filter_map(|e| e.ok())
        .collect();
    if children.len() == 1 && children[0].path().is_dir() {
        let inner = children[0].path();
        for entry in std::fs::read_dir(&inner)? {
            let entry = entry?;
            let dest = source_dir.join(entry.file_name());
            std::fs::rename(entry.path(), dest)?;
        }
        std::fs::remove_dir(&inner)?;
    }

    Ok(())
}

/// Clone a git repository, with fallback logic for "latest" and "main" tags.
///
/// When `commit` is `Some`, clones the `main` branch with a sufficient depth
/// to reach the target commit, then runs `git checkout <commit>`.
///
/// Note: "latest" resolution is done by the caller using `check_latest_version`
/// (GitHub releases API). This function just clones the given version/tag/branch.
async fn clone_repository(
    version: &str,
    git_url: &str,
    source_dir: &Path,
    commit: Option<&str>,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    // When a specific commit is requested, do a deeper clone of main then checkout.
    if let Some(commit_hash) = commit {
        emit(
            progress,
            format!(
                "Cloning repository for commit {} (depth 500)...",
                commit_hash
            ),
        );
        let clone_status = tokio::process::Command::new("git")
            .args([
                "clone",
                "--depth",
                "500",
                "--branch",
                "main",
                git_url,
                &source_dir.to_string_lossy(),
            ])
            .status()
            .await?;

        if !clone_status.success() {
            return Err(anyhow!(
                "Failed to clone repository from {} (depth 500)",
                git_url
            ));
        }

        emit(progress, format!("Checking out commit {}...", commit_hash));
        let checkout_status = tokio::process::Command::new("git")
            .args(["-C", &source_dir.to_string_lossy(), "checkout", commit_hash])
            .status()
            .await?;

        if !checkout_status.success() {
            return Err(anyhow!(
                "Failed to checkout commit {}. \
                 The commit may be older than the clone depth (500). \
                 Try a more recent commit.",
                commit_hash
            ));
        }

        emit(progress, format!("Checked out commit {}.", commit_hash));
        return Ok(());
    }

    emit(progress, "Cloning repository (shallow)...");

    // Versions like "main@abc12345" mean "clone the main branch"
    let branch = if version.starts_with("main@") {
        "main"
    } else {
        version
    };

    let clone_result = tokio::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            branch,
            git_url,
            &source_dir.to_string_lossy(),
        ])
        .status()
        .await?;

    if clone_result.success() {
        return Ok(());
    }

    // Only allow fallback to HEAD for "main" or "latest" (tags may not exist)
    if !version.starts_with("main") && version != "latest" {
        return Err(anyhow!(
            "Tag/branch '{}' not found. Only 'main' or 'latest' are allowed for fallback.\n\
             Use an explicit version tag (e.g., 'b8407') or specify --build to build from source.",
            version
        ));
    }

    // Fallback: clone without branch tag
    tracing::warn!(
        "Tag/branch '{}' not found, cloning HEAD as fallback. Use an explicit version tag or --build flag.",
        version
    );
    emit(
        progress,
        format!("Tag/branch '{}' not found, cloning HEAD...", version),
    );
    let status = tokio::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            git_url,
            &source_dir.to_string_lossy(),
        ])
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!("Failed to clone repository from {}", git_url));
    }

    Ok(())
}

/// Run CMake configuration step.
async fn configure_cmake(
    options: &InstallOptions,
    source_dir: &Path,
    build_output: &Path,
    #[allow(unused_variables)] progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    let amdgpu_targets = if options.gpu_variant == "rocm" {
        let targets = detect_amdgpu_targets();
        if targets.is_empty() {
            tracing::warn!(
                "No AMDGPU_TARGETS detected (rocminfo missing or returned no gfx entries). \
                 Falling back to llama.cpp's default target list — this may exclude newer archs. \
                 Set TAMA_AMDGPU_TARGETS=gfxNNNN to override."
            );
        } else {
            tracing::info!("Detected AMDGPU_TARGETS: {}", targets.join(";"));
        }
        targets
    } else {
        Vec::new()
    };
    let cmake_args = build_cmake_args(options, source_dir, build_output, &amdgpu_targets);

    let mut cmd = tokio::process::Command::new("cmake");
    cmd.args(&cmake_args);
    if options.gpu_variant == "rocm" {
        if let Some((hipcxx, hip_path)) = detect_hip_env() {
            tracing::info!("Using HIPCXX={}, HIP_PATH={}", hipcxx, hip_path);
            cmd.env("HIPCXX", hipcxx);
            cmd.env("HIP_PATH", hip_path);
        } else {
            tracing::warn!(
                "hipconfig not found or returned empty output. \
                 Falling back to PATH-based HIP discovery. \
                 Ensure /opt/rocm/bin is on PATH if the build fails."
            );
        }
    }
    let status = cmd.status().await?;

    if !status.success() {
        return Err(anyhow!(
            "CMake configuration failed. Check that all build dependencies are installed."
        ));
    }

    Ok(())
}

/// Run CMake build step with parallel jobs.
async fn build_cmake(build_output: &Path, progress: Option<&Arc<dyn ProgressSink>>) -> Result<()> {
    let num_jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    emit(
        progress,
        format!(
            "Building with {} parallel jobs (this may take several minutes)...",
            num_jobs
        ),
    );

    let status = tokio::process::Command::new("cmake")
        .args([
            "--build",
            &build_output.to_string_lossy(),
            "--config",
            "Release",
            "-j",
            &num_jobs.to_string(),
        ])
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!("Build failed. Check the output above for errors."));
    }

    Ok(())
}

/// Copy the built binary (and shared libs) to the target directory.
async fn install_binary(
    build_output: &Path,
    options: &InstallOptions,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<PathBuf> {
    emit(progress, "Installing binary...");
    let binary_src = find_backend_binary(build_output)?;

    std::fs::create_dir_all(&options.target_dir)?;
    let binary_name = binary_src
        .file_name()
        .ok_or_else(|| anyhow!("Could not determine binary filename"))?;
    let binary_dest = options.target_dir.join(binary_name);

    // Copy main binary
    std::fs::copy(&binary_src, &binary_dest)?;

    // Set executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&binary_dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&binary_dest, perms)?;
    }

    // Copy files from the build output's bin/ subdirectory directly into
    // the target directory (flattened, no intermediate bin/ folder).
    // This ensures all shared libraries (including versioned .so symlink chains),
    // executables, and any other runtime dependencies are available at the
    // expected location alongside the main binary.
    let bin_dir = build_output.join("bin");
    if bin_dir.is_dir() {
        fn copy_bin_contents(src_dir: &std::path::Path, dest_dir: &std::path::Path) {
            if let Ok(entries) = std::fs::read_dir(src_dir) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    let name = match entry_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    // Skip cmake build metadata and git files
                    if name == "CMakeCache.txt"
                        || name == "cmake_install.cmake"
                        || name == "CMakeDirectoryInformation.cmake"
                        || name.starts_with("CMakeFiles")
                        || name.starts_with(".git")
                    {
                        continue;
                    }
                    let dest_path = dest_dir.join(&name);
                    // Use metadata() which follows symlinks, so symlinks to files
                    // resolve as files and get copied by std::fs::copy which
                    // also follows symlinks. Versioned .so.* files in the chain
                    // are regular files (the leaf) and get copied naturally since
                    // read_dir lists every entry in the directory.
                    let meta = match std::fs::metadata(&entry_path) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    if meta.is_dir() {
                        if let Err(e) = std::fs::create_dir_all(&dest_path) {
                            tracing::warn!("Failed to create dir {}: {}", name, e);
                        }
                        copy_bin_contents(&entry_path, &dest_path);
                    } else if meta.is_file() && !dest_path.exists() {
                        if let Err(e) = std::fs::copy(&entry_path, &dest_path) {
                            tracing::warn!("Failed to copy {}: {}", name, e);
                        }
                    }
                }
            }
        }
        copy_bin_contents(&bin_dir, &options.target_dir);
    } else {
        tracing::warn!("No bin/ directory found in build output at {:?}", bin_dir);
    }

    // Shared libraries (.so files) are placed by CMake in subdirectories like
    // src/, ggml/src/, examples/mtmd/ — not in bin/. Recursively scan the
    // entire build tree for .so files and copy them to the target directory
    // so the binary can find them at runtime via the registered ldconfig path.
    fn copy_shared_libs(build_root: &std::path::Path, dest_dir: &std::path::Path) {
        fn walk(dir: &std::path::Path, dest: &std::path::Path) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    // Skip build metadata directories
                    if name == "CMakeFiles" || name == ".git" {
                        continue;
                    }
                    // Use symlink_metadata to detect symlinks vs regular files
                    let sym_meta = match std::fs::symlink_metadata(&path) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    if sym_meta.file_type().is_symlink() {
                        // Copy .so symlinks (e.g. libllama.so -> libllama.so.0)
                        if name.starts_with("lib") && name.contains(".so") {
                            let dest_path = dest.join(&name);
                            if !dest_path.exists() && !dest_path.is_symlink() {
                                if let Err(e) = std::fs::copy(&path, &dest_path) {
                                    tracing::warn!("Failed to copy symlink {}: {}", name, e);
                                }
                            }
                        }
                    } else if sym_meta.is_dir() {
                        walk(&path, dest);
                    } else if sym_meta.is_file() {
                        // Copy .so* files (shared libraries and versioned variants)
                        if name.starts_with("lib") && name.contains(".so") {
                            let dest_path = dest.join(&name);
                            if !dest_path.exists() {
                                if let Err(e) = std::fs::copy(&path, &dest_path) {
                                    tracing::warn!("Failed to copy {}: {}", name, e);
                                }
                            }
                        }
                    }
                }
            }
        }
        walk(build_root, dest_dir);
    }
    copy_shared_libs(build_output, &options.target_dir);

    // Set executable permissions on all copied executables (Unix only).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(entries) = std::fs::read_dir(&options.target_dir) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_file() {
                    if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
                        let is_shared = name.contains(".so") || name.ends_with(".dylib");
                        if !is_shared && !name.contains('.') {
                            if let Ok(mut perms) =
                                std::fs::metadata(&entry_path).map(|m| m.permissions())
                            {
                                perms.set_mode(0o755);
                                let _ = std::fs::set_permissions(&entry_path, perms);
                            }
                        }
                    }
                }
            }
        }
    }

    emit(
        progress,
        format!("Backend built and installed at: {:?}", binary_dest),
    );
    Ok(binary_dest)
}
