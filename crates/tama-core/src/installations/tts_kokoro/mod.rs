pub mod download;
pub mod paths;

use super::{
    get_backend_install_path, InstallationInfo, InstallationManager, InstallationSource,
    InstallationType, ProgressSink,
};
use anyhow::Context;

/// Install the Kokoro TTS backend: clone repo, create venv, install deps, download model.
pub async fn install_tts_kokoro(
    manager: InstallationManager,
    progress: Box<dyn ProgressSink>,
) -> anyhow::Result<()> {
    let p = std::sync::Arc::from(progress);

    // Compute the versioned base directory for installation
    let backends_dir = crate::installations::backends_dir()?;
    let version = paths::KOKORO_FASTAPI_TAG.to_string();
    let base_dir =
        get_backend_install_path(&backends_dir, &InstallationType::TtsKokoro, "cpu", &version);

    // Run the full Kokoro-FastAPI installation pipeline
    download::install_kokoro_fastapi(&base_dir, &p).await?;

    // Register in the backend registry — path points to base_dir (parent of
    // install_dir and venv). This allows safe_remove_installation to remove
    // the entire versioned directory including both venv and repo.
    let info = InstallationInfo {
        name: "tts_kokoro".to_string(),
        backend_type: InstallationType::TtsKokoro,
        version: version.clone(),
        path: paths::base_dir(&base_dir),
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64),
        gpu_variant: "cpu".to_string(),
        source: Some(InstallationSource::SourceCode {
            version,
            git_url: paths::KOKORO_FASTAPI_URL.to_string(),
            commit: None,
        }),
        docker_config: None,
    };

    manager
        .add_installation(&info)
        .with_context(|| "Failed to register Kokoro backend")?;

    Ok(())
}
