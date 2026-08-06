use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::time::Duration;

use crate::web_types::JobManager;
use tama_core::backends::ProgressSink;

// ─────────────────────────────────────────────────────────────────────────────
// Wire DTOs (tama-web only, not exposed from tama-core)
// ─────────────────────────────────────────────────────────────────────────────

/// DTO for the compaction backend card (embedded, always installed).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CompactionCardDto {
    /// Whether compaction is enabled in config.
    pub enabled: bool,
    /// Compute device (e.g. "cpu", "cuda", "mps").
    pub device: String,
    /// Fixed port or null if auto-assigned.
    pub port: Option<u16>,
    /// Whether the compaction backend is currently running (Ready in model registry).
    pub running: bool,
    /// Server URL if running (e.g. "http://127.0.0.1:18962").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendListResponse {
    pub active_job: Option<ActiveJobDto>,
    pub backends: Vec<BackendCardDto>,
    pub custom: Vec<BackendCardDto>,
    /// Docker-based backends (e.g. vLLM), kept separate from native `custom` backends.
    #[serde(default)]
    pub docker: Vec<BackendCardDto>,
    /// Backend type identifiers that are known but not currently installed.
    #[serde(default)]
    pub available: Vec<String>,
    /// Compaction backend status (embedded, always "installed").
    pub compaction: CompactionCardDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendCardDto {
    pub r#type: String,
    /// Actual DB key for the backend (used in save URLs). For native backends this equals `r#type`;
    /// for docker/custom backends it carries the actual name (e.g. "vllm").
    #[serde(default)]
    pub backend_name: String,
    pub display_name: String,
    pub installed: bool,
    /// GPU variant folder for this card (e.g. "cpu", "cuda_12", "vulkan").
    #[serde(default)]
    pub gpu_variant: String,
    /// Info for the currently active version (shown by default in the UI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<BackendInfoDto>,
    /// All installed versions of this backend.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<BackendVersionDto>,
    #[serde(skip_serializing_if = "UpdateStatusDto::is_default")]
    pub update: UpdateStatusDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes_url: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub default_env: Vec<String>,
    /// Whether the active version is currently selected for display.
    #[serde(default)]
    pub is_active: bool,
}

impl BackendCardDto {
    pub(super) fn default_uninstalled(
        type_: &str,
        display_name: &str,
        release_notes_url: Option<&str>,
        default_args: Vec<String>,
    ) -> Self {
        Self {
            r#type: type_.to_string(),
            backend_name: type_.to_string(),
            display_name: display_name.to_string(),
            installed: false,
            gpu_variant: String::new(),
            info: None,
            versions: vec![],
            update: UpdateStatusDto::default(),
            release_notes_url: release_notes_url.map(String::from),
            default_args,
            default_env: vec![],
            is_active: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendInfoDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_variant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSourceDto>,
}

impl From<tama_core::backends::BackendInfo> for BackendInfoDto {
    fn from(info: tama_core::backends::BackendInfo) -> Self {
        Self {
            name: info.name,
            version: info.version,
            path: info.path.to_string_lossy().to_string(),
            installed_at: info.installed_at,
            gpu_variant: info.gpu_variant,
            source: info.source.as_ref().map(|s| s.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BackendSourceDto {
    Prebuilt {
        version: String,
    },
    SourceCode {
        version: String,
        git_url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
}

impl From<&tama_core::backends::BackendSource> for BackendSourceDto {
    fn from(source: &tama_core::backends::BackendSource) -> Self {
        match source {
            tama_core::backends::BackendSource::Prebuilt { version } => Self::Prebuilt {
                version: version.clone(),
            },
            tama_core::backends::BackendSource::SourceCode {
                version,
                git_url,
                commit,
            } => Self::SourceCode {
                version: version.clone(),
                git_url: git_url.clone(),
                commit: commit.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct UpdateStatusDto {
    pub checked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_available: Option<bool>,
}

impl UpdateStatusDto {
    pub fn is_default(&self) -> bool {
        !self.checked
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActiveJobDto {
    pub id: String,
    pub kind: String,
    pub backend_type: String,
}

pub(super) fn job_to_active_dto(j: &crate::web_types::Job) -> ActiveJobDto {
    ActiveJobDto {
        id: j.id.clone(),
        kind: match j.kind {
            crate::web_types::JobKind::Install => "install".to_string(),
            crate::web_types::JobKind::Update => "update".to_string(),
            crate::web_types::JobKind::Restore => "restore".to_string(),
            crate::web_types::JobKind::Benchmark => "benchmark".to_string(),
        },
        backend_type: j.backend_type.as_deref().unwrap_or("").to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CapabilitiesDto {
    pub os: String,
    pub arch: String,
    pub git_available: bool,
    pub cmake_available: bool,
    pub compiler_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_cuda_version: Option<String>,
    pub supported_cuda_versions: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Request/Response DTOs for mutation API
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallRequest {
    pub backend_type: String,
    pub version: Option<String>,
    /// GPU variant for the installation (e.g. "cpu", "cuda", "vulkan", "rocm", "metal").
    pub gpu_variant: tama_core::gpu::GpuVariant,
    pub build_from_source: bool,
    pub force: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallResponse {
    pub job_id: String,
    pub kind: String,
    pub backend_type: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DeleteResponse {
    pub removed: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Docker backend registration DTOs (POST /tama/v1/backends)
// ─────────────────────────────────────────────────────────────────────────────

/// Request body for POST /tama/v1/backends — register a backend directly.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RegisterBackendRequest {
    /// Backend name (e.g. "vllm", "ollama")
    pub name: String,
    /// Backend type identifier (e.g. "docker")
    pub backend_type: String,
    /// Version string
    pub version: String,
    /// GPU variant for the installation (e.g. "cpu", "cuda"). Defaults to "cpu".
    #[serde(default = "default_cpu_variant")]
    pub gpu_variant: String,
    /// Docker configuration — required when backend_type is "docker".
    #[serde(default)]
    pub docker_config: Option<tama_core::backends::DockerConfig>,
}

fn default_cpu_variant() -> String {
    "cpu".to_string()
}

/// Response for POST /tama/v1/backends — created backend info.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RegisterBackendResponse {
    pub name: String,
    pub backend_type: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_variant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSourceDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docker_config: Option<DockerConfigDto>,
}

impl From<tama_core::backends::BackendInfo> for RegisterBackendResponse {
    fn from(info: tama_core::backends::BackendInfo) -> Self {
        Self {
            name: info.name,
            backend_type: info.backend_type.to_string(),
            version: info.version,
            path: info.path.to_string_lossy().to_string(),
            installed_at: info.installed_at,
            gpu_variant: info.gpu_variant,
            source: info.source.as_ref().map(|s| s.into()),
            docker_config: info.docker_config.as_ref().map(|d| d.into()),
        }
    }
}

/// Docker configuration DTO for API responses.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DockerConfigDto {
    pub image: String,
    pub container_port: u16,
    pub model_mount: DockerVolumeDto,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<DockerVolumeDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpus: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shm_size: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cap_adds: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_opts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_adds: Vec<String>,
}

impl From<&tama_core::backends::DockerConfig> for DockerConfigDto {
    fn from(cfg: &tama_core::backends::DockerConfig) -> Self {
        Self {
            image: cfg.image.clone(),
            container_port: cfg.container_port,
            model_mount: (&cfg.model_mount).into(),
            volumes: cfg.volumes.iter().map(|v| v.into()).collect(),
            devices: cfg.devices.clone(),
            gpus: cfg.gpus.clone(),
            shm_size: cfg.shm_size.clone(),
            cap_adds: cfg.cap_adds.clone(),
            security_opts: cfg.security_opts.clone(),
            group_adds: cfg.group_adds.clone(),
        }
    }
}

/// Docker volume DTO for API responses.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DockerVolumeDto {
    pub host_path: String,
    pub container_path: String,
    #[serde(default)]
    pub read_only: bool,
}

impl From<&tama_core::backends::DockerVolume> for DockerVolumeDto {
    fn from(vol: &tama_core::backends::DockerVolume) -> Self {
        Self {
            host_path: vol.host_path.clone(),
            container_path: vol.container_path.clone(),
            read_only: vol.read_only,
        }
    }
}

/// Version info returned by the versions endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendVersionDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_variant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSourceDto>,
    pub is_active: bool,
}

/// Response for GET /tama/v1/backends/:name/versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendVersionsResponse {
    pub versions: Vec<BackendVersionDto>,
    pub active_version: Option<String>,
}

/// Request body for POST /tama/v1/backends/:name/activate.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivateRequest {
    pub version: String,
}

/// Response for POST /tama/v1/backends/:name/activate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivateResponse {
    pub version: String,
    pub is_active: bool,
}

/// Request body for POST /tama/v1/backends/:name/source.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateSourceRequest {
    pub build_from_source: bool,
}

/// Response for POST /tama/v1/backends/:name/source.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateSourceResponse {
    pub build_from_source: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CheckUpdatesResponse {
    pub active_job: Option<ActiveJobDto>,
    pub backends: Vec<BackendCardDto>,
    pub custom: Vec<BackendCardDto>,
    /// Docker-based backends (e.g. vLLM), kept separate from native `custom` backends.
    #[serde(default)]
    pub docker: Vec<BackendCardDto>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Job snapshot DTO
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct JobSnapshotDto {
    pub id: String,
    pub kind: String,
    pub status: crate::web_types::JobStatus,
    pub backend_type: String,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Known backends lookup
// ─────────────────────────────────────────────────────────────────────────────

pub(super) const KNOWN_BACKENDS: &[(&str, &str, Option<&str>)] = &[
    (
        "llama_cpp",
        "llama.cpp",
        Some("https://github.com/ggml-org/llama.cpp/releases"),
    ),
    (
        "ik_llama",
        "ik_llama.cpp",
        Some("https://github.com/ikawrakow/ik_llama.cpp/commits/main"),
    ),
    // TTS backends — installed via HuggingFace model downloads (no GPU needed)
    (
        "tts_kokoro",
        "Kokoro TTS",
        Some("https://huggingface.co/hexgrad/Kokoro-82M"),
    ),
];

// ─────────────────────────────────────────────────────────────────────────────
// Job adapter for progress streaming
// ─────────────────────────────────────────────────────────────────────────────

pub struct JobAdapter {
    pub(super) jobs: Arc<JobManager>,
    pub(super) job: Arc<crate::web_types::Job>,
}

impl ProgressSink for JobAdapter {
    fn log(&self, line: &str) {
        let jobs = self.jobs.clone();
        let job = self.job.clone();
        let line = line.to_string();
        // ProgressSink::log is sync; we need to call async append_log.
        // Use tokio::runtime::Handle::current().spawn — installer runs inside the runtime.
        tokio::runtime::Handle::current().spawn(async move {
            jobs.append_log(&job, line).await;
        });
    }

    fn result(&self, _json: &str) {
        // Not used for backend installs/updates
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Capabilities cache
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct CapabilitiesCache {
    inner: Arc<tokio::sync::Mutex<Option<(std::time::Instant, CapabilitiesDto)>>>,
}

impl CapabilitiesCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub async fn get_or_compute(
        &self,
        detect_prereqs: fn() -> tama_core::gpu::BuildPrerequisites,
        detect_cuda: fn() -> Option<String>,
    ) -> anyhow::Result<CapabilitiesDto> {
        let now = std::time::Instant::now();
        let mut guard = self.inner.lock().await;

        // Check cache hit (5-second TTL)
        if let Some((cached_at, cached)) = &*guard {
            if now.duration_since(*cached_at) < Duration::from_secs(5) {
                return Ok(cached.clone());
            }
        }

        // Cold path: spawn_blocking to avoid blocking runtime
        let result = tokio::task::spawn_blocking(move || {
            let caps = detect_prereqs();
            let cuda = detect_cuda();
            CapabilitiesDto {
                os: caps.os,
                arch: caps.arch,
                git_available: caps.git_available,
                cmake_available: caps.cmake_available,
                compiler_available: caps.compiler_available,
                detected_cuda_version: cuda,
                supported_cuda_versions: vec![
                    "11.1".to_string(),
                    "12.4".to_string(),
                    "13.1".to_string(),
                ],
            }
        })
        .await;

        let caps = match result {
            Ok(c) => c,
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to detect capabilities: {}", e));
            }
        };

        *guard = Some((now, caps.clone()));
        Ok(caps)
    }
}

impl Default for CapabilitiesCache {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create an ActiveJobDto for testing.
    fn make_active_dto(id: &str, kind: &str, backend_type: &str) -> ActiveJobDto {
        ActiveJobDto {
            id: id.to_string(),
            kind: kind.to_string(),
            backend_type: backend_type.to_string(),
        }
    }

    // ── ActiveJobDto serialization tests ──────────────────────────────────

    #[test]
    fn test_active_job_dto_serialization() {
        let dto = make_active_dto("job-123", "install", "llama_cpp");
        let json = serde_json::to_string(&dto).unwrap();
        let deserialized: ActiveJobDto = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "job-123");
        assert_eq!(deserialized.kind, "install");
        assert_eq!(deserialized.backend_type, "llama_cpp");
    }

    #[test]
    fn test_active_job_dto_update_kind() {
        let dto = make_active_dto("job-456", "update", "ik_llama");
        assert_eq!(dto.kind, "update");
        assert_eq!(dto.backend_type, "ik_llama");
    }

    #[test]
    fn test_active_job_dto_restore_kind() {
        let dto = make_active_dto("job-789", "restore", "custom");
        assert_eq!(dto.kind, "restore");
        assert_eq!(dto.backend_type, "custom");
    }

    #[test]
    fn test_active_job_dto_empty_backend() {
        let dto = make_active_dto("job-000", "install", "");
        assert_eq!(dto.backend_type, "");
    }

    // ── CapabilitiesDto serialization tests ───────────────────────────────

    #[test]
    fn test_capabilities_dto_serialization() {
        let caps = CapabilitiesDto {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            git_available: true,
            cmake_available: true,
            compiler_available: true,
            detected_cuda_version: Some("12.4".to_string()),
            supported_cuda_versions: vec!["12.0".to_string(), "12.4".to_string()],
        };

        let json = serde_json::to_string(&caps).unwrap();
        let deserialized: CapabilitiesDto = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.os, "linux");
        assert_eq!(deserialized.arch, "x86_64");
        assert!(deserialized.git_available);
        assert!(deserialized.cmake_available);
        assert!(deserialized.compiler_available);
        assert_eq!(deserialized.detected_cuda_version, Some("12.4".to_string()));
        assert_eq!(deserialized.supported_cuda_versions.len(), 2);
    }

    #[test]
    fn test_capabilities_dto_minimal() {
        let caps = CapabilitiesDto {
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            git_available: false,
            cmake_available: false,
            compiler_available: false,
            detected_cuda_version: None,
            supported_cuda_versions: vec![],
        };

        let json = serde_json::to_string(&caps).unwrap();
        let deserialized: CapabilitiesDto = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.os, "macos");
        assert!(!deserialized.git_available);
    }

    // ── InstallRequest gpu_variant tests ──────────────────────────────────

    #[test]
    fn test_install_request_accepts_known_variant() {
        let req: InstallRequest = serde_json::from_str(
            r#"{"backend_type":"llama_cpp","version":null,"gpu_variant":"cuda","build_from_source":false,"force":false}"#,
        )
        .unwrap();
        assert!(matches!(
            req.gpu_variant,
            tama_core::gpu::GpuVariant::Cuda { .. }
        ));
    }

    #[test]
    fn test_install_request_rejects_unknown_variant() {
        let result: Result<InstallRequest, _> = serde_json::from_str(
            r#"{"backend_type":"llama_cpp","version":null,"gpu_variant":"tpu","build_from_source":false,"force":false}"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_install_request_variant_case_insensitive() {
        let req: InstallRequest = serde_json::from_str(
            r#"{"backend_type":"llama_cpp","version":null,"gpu_variant":"CUDA","build_from_source":false,"force":false}"#,
        )
        .unwrap();
        assert!(matches!(
            req.gpu_variant,
            tama_core::gpu::GpuVariant::Cuda { .. }
        ));
    }

    // ── BackendCardDto tests ────────────────────────────────────────────────

    #[test]
    fn test_default_uninstalled_sets_backend_name() {
        let card = BackendCardDto::default_uninstalled(
            "llama_cpp",
            "llama.cpp",
            Some("https://example.com"),
            vec!["--threads 4".to_string()],
        );
        assert_eq!(card.backend_name, "llama_cpp");
        assert_eq!(card.r#type, "llama_cpp");
        assert!(!card.installed);
    }

    #[test]
    fn test_backend_card_dto_deserialize_without_backend_name() {
        // backend_name has `#[serde(default)]`, so omitting it should succeed
        let json = r#"{"type":"llama_cpp","display_name":"llama.cpp","installed":true,"update":{"checked":false},"default_args":["--threads 4"]}"#;
        let card: BackendCardDto = serde_json::from_str(json).unwrap();
        assert_eq!(card.backend_name, "");
        assert_eq!(card.r#type, "llama_cpp");
    }
}
