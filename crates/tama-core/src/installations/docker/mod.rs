//! Docker backend configuration types and image management utilities.
//!
//! `DockerConfig` describes how to run a backend inside a Docker container — image,
//! ports, volumes, devices, GPU flags, and optional runtime tweaks like shared memory
//! size or capability additions. It is stored as JSON in the `docker_config` column of
//! `backend_installations`.
//!
//! `DockerVolume` represents a single bind mount (`-v`) used by the container.
//!
//! The `image` sub-module provides functions for checking Docker availability,
//! inspecting images, and pulling images with progress streaming.

pub mod image;
pub mod reconcile;
pub mod runner;

// Re-export key functions for use by lifecycle module.
pub use image::{docker_available, is_image_present, pull_image};
pub use runner::{
    inspect_container, remove_container, resolve_group_gids, resolve_volumes,
    rewrite_args_for_container, spawn_container, stop_container,
};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Default container port for docker backends.
fn default_container_port() -> u16 {
    8000
}

/// Configuration for running a backend inside a Docker container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Container image (e.g. "stilldeadcode/vllm-radiance:0.5.8")
    pub image: String,
    /// Port the backend listens on inside the container. Default: 8000.
    #[serde(default = "default_container_port")]
    pub container_port: u16,
    /// Model storage volume mount.
    pub model_mount: DockerVolume,
    /// Additional bind mounts.
    #[serde(default)]
    pub volumes: Vec<DockerVolume>,
    /// Linux devices to expose (e.g. "/dev/nvidia0").
    #[serde(default)]
    pub devices: Vec<String>,
    /// GPU flags passed to `docker run` (e.g. "all" or "nvidia.com/gpu=0").
    #[serde(default)]
    pub gpus: Option<String>,
    /// Shared memory size (e.g. "2G").
    #[serde(default)]
    pub shm_size: Option<String>,
    /// Linux capabilities to add (e.g. "SYS_PTRACE").
    #[serde(default)]
    pub cap_adds: Vec<String>,
    /// Security options (e.g. "no-new-privileges").
    #[serde(default)]
    pub security_opts: Vec<String>,
    /// Supplementary GIDs (e.g. "296" for docker group).
    #[serde(default)]
    pub group_adds: Vec<String>,
}

/// A bind mount volume for Docker containers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerVolume {
    /// Host path — supports the "{{MODEL_DIR}}" template variable.
    pub host_path: String,
    /// Container path (must be absolute).
    pub container_path: String,
    /// Whether the mount is read-only. Default: false.
    #[serde(default)]
    pub read_only: bool,
}

impl DockerConfig {
    /// Validate the configuration and return an error if any field is invalid.
    pub fn validate(&self) -> Result<()> {
        // image must be non-empty
        if self.image.is_empty() {
            return Err(anyhow!("Docker image must not be empty"));
        }

        // Validate image reference format
        validate_image_ref(&self.image)?;

        // container_port must be in valid range (u16 max is 65535)
        if self.container_port < 1 {
            return Err(anyhow!(
                "container_port must be between 1 and 65535, got {}",
                self.container_port
            ));
        }

        // Validate gpus field — value should not start with "--" (common mistake)
        if let Some(ref gpus) = self.gpus {
            if gpus.starts_with("--") {
                return Err(anyhow!(
                    "gpus value must be the flag argument only (e.g. \"all\"), not a full flag like \"--gpus all\""
                ));
            }
        }

        // model_mount.container_path must be absolute
        if !self.model_mount.container_path.starts_with('/') {
            return Err(anyhow!(
                "model_mount.container_path must be an absolute path starting with '/', got '{}'",
                self.model_mount.container_path
            ));
        }

        // All volumes must have absolute container paths
        for (i, vol) in self.volumes.iter().enumerate() {
            if !vol.container_path.starts_with('/') {
                return Err(anyhow!(
                    "volumes[{}].container_path must be an absolute path starting with '/', got '{}'",
                    i,
                    vol.container_path
                ));
            }
        }

        Ok(())
    }
}

/// Validate a Docker image reference string.
///
/// Rules:
/// - Must not be empty (checked by caller)
/// - If it contains `@`, validate as digest ref (name@sha256:hex...)
/// - If it contains `:` after the last `/`, validate as name:tag
/// - Tagless images are accepted (implicit :latest)
fn validate_image_ref(image: &str) -> Result<()> {
    // Reject image refs starting with `-` — prevents flag injection
    // since `spawn_container` does `cmd.arg(&config.image)` in flag position.
    if image.starts_with('-') {
        return Err(anyhow!(
            "Image reference must not start with '-', got '{}'",
            image
        ));
    }

    // Reject image refs containing whitespace — invalid and suspicious
    if image.chars().any(|c| c.is_whitespace()) {
        return Err(anyhow!(
            "Image reference must not contain whitespace, got '{}'",
            image
        ));
    }

    // Check for digest reference
    if let Some(at_pos) = image.find('@') {
        let digest = &image[at_pos + 1..];
        // Digest must be sha256:hex
        if !digest.starts_with("sha256:") {
            return Err(anyhow!(
                "Image digest must use sha256: prefix, got '{}'",
                digest
            ));
        }
        let hex_part = &digest[7..];
        if hex_part.len() != 64 || !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(anyhow!(
                "Invalid sha256 digest length (expected 64 hex chars), got '{}'",
                hex_part.len()
            ));
        }
    }

    // Check for tag after last `/` (or at end if no `/`)
    let name_part = image.rfind('/').map_or(image, |pos| &image[pos + 1..]);
    if let Some(colon_pos) = name_part.find(':') {
        let tag = &name_part[colon_pos + 1..];
        if tag.is_empty() {
            return Err(anyhow!("Image tag must not be empty"));
        }
        // Tags can contain alphanumeric, _, -, .
        if !tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return Err(anyhow!("Image tag contains invalid characters: '{}'", tag));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> DockerConfig {
        DockerConfig {
            image: "stilldeadcode/vllm-radiance:0.5.8".to_string(),
            container_port: 8000,
            model_mount: DockerVolume {
                host_path: "/models".to_string(),
                container_path: "/models".to_string(),
                read_only: false,
            },
            volumes: vec![DockerVolume {
                host_path: "/data".to_string(),
                container_path: "/data".to_string(),
                read_only: true,
            }],
            devices: vec!["/dev/nvidia0".to_string()],
            gpus: Some("all".to_string()),
            shm_size: Some("2G".to_string()),
            cap_adds: vec!["SYS_PTRACE".to_string()],
            security_opts: vec!["no-new-privileges".to_string()],
            group_adds: vec!["296".to_string()],
        }
    }

    // ─── Serde round-trip tests ──────────────────────────────────

    #[test]
    fn test_docker_config_serde_roundtrip() {
        let config = sample_config();
        let json = serde_json::to_string(&config).expect("serialize failed");
        let parsed: DockerConfig = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(parsed.image, config.image);
        assert_eq!(parsed.container_port, config.container_port);
        assert_eq!(parsed.model_mount.host_path, config.model_mount.host_path);
        assert_eq!(
            parsed.model_mount.container_path,
            config.model_mount.container_path
        );
        assert_eq!(parsed.volumes.len(), config.volumes.len());
        assert_eq!(parsed.devices, config.devices);
        assert_eq!(parsed.gpus, config.gpus);
        assert_eq!(parsed.shm_size, config.shm_size);
        assert_eq!(parsed.cap_adds, config.cap_adds);
        assert_eq!(parsed.security_opts, config.security_opts);
        assert_eq!(parsed.group_adds, config.group_adds);
    }

    #[test]
    fn test_docker_config_defaults_applied() {
        // Serialize a minimal config (only required fields)
        let minimal = DockerConfig {
            image: "myimage".to_string(),
            container_port: default_container_port(),
            model_mount: DockerVolume {
                host_path: "/models".to_string(),
                container_path: "/models".to_string(),
                read_only: false,
            },
            volumes: vec![],
            devices: vec![],
            gpus: None,
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };

        let json = serde_json::to_string(&minimal).expect("serialize failed");

        // Deserialize and verify defaults
        let parsed: DockerConfig = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(parsed.container_port, 8000);
        assert!(parsed.volumes.is_empty());
        assert!(parsed.devices.is_empty());
        assert!(parsed.gpus.is_none());
        assert!(parsed.shm_size.is_none());
        assert!(parsed.cap_adds.is_empty());
        assert!(parsed.security_opts.is_empty());
        assert!(parsed.group_adds.is_empty());
    }

    #[test]
    fn test_docker_volume_serde_roundtrip() {
        let vol = DockerVolume {
            host_path: "/host/data".to_string(),
            container_path: "/data".to_string(),
            read_only: true,
        };
        let json = serde_json::to_string(&vol).expect("serialize failed");
        let parsed: DockerVolume = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(parsed.host_path, vol.host_path);
        assert_eq!(parsed.container_path, vol.container_path);
        assert!(parsed.read_only);
    }

    #[test]
    fn test_docker_volume_default_read_only() {
        let vol = DockerVolume {
            host_path: "/host".to_string(),
            container_path: "/container".to_string(),
            read_only: false,
        };
        let json = serde_json::to_string(&vol).expect("serialize failed");
        let parsed: DockerVolume = serde_json::from_str(&json).expect("deserialize failed");
        assert!(!parsed.read_only);
    }

    // ─── Validation tests ────────────────────────────────────────

    #[test]
    fn test_validate_empty_image() {
        let config = DockerConfig {
            image: String::new(),
            container_port: 8000,
            model_mount: DockerVolume {
                host_path: "/models".to_string(),
                container_path: "/models".to_string(),
                read_only: false,
            },
            volumes: vec![],
            devices: vec![],
            gpus: None,
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_non_absolute_model_mount_path() {
        let config = DockerConfig {
            image: "myimage:1.0".to_string(),
            container_port: 8000,
            model_mount: DockerVolume {
                host_path: "/models".to_string(),
                container_path: "models".to_string(), // not absolute
                read_only: false,
            },
            volumes: vec![],
            devices: vec![],
            gpus: None,
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("absolute path"));
    }

    #[test]
    fn test_validate_non_absolute_volume_path() {
        let config = DockerConfig {
            image: "myimage:1.0".to_string(),
            container_port: 8000,
            model_mount: DockerVolume {
                host_path: "/models".to_string(),
                container_path: "/models".to_string(),
                read_only: false,
            },
            volumes: vec![DockerVolume {
                host_path: "/data".to_string(),
                container_path: "data".to_string(), // not absolute
                read_only: false,
            }],
            devices: vec![],
            gpus: None,
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("absolute path"));
    }

    #[test]
    fn test_validate_valid_config() {
        let config = sample_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_container_port_zero() {
        let config = DockerConfig {
            image: "myimage".to_string(),
            container_port: 0,
            model_mount: DockerVolume {
                host_path: "/models".to_string(),
                container_path: "/models".to_string(),
                read_only: false,
            },
            volumes: vec![],
            devices: vec![],
            gpus: None,
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_container_port_max_valid() {
        let config = DockerConfig {
            image: "myimage".to_string(),
            container_port: u16::MAX,
            model_mount: DockerVolume {
                host_path: "/models".to_string(),
                container_path: "/models".to_string(),
                read_only: false,
            },
            volumes: vec![],
            devices: vec![],
            gpus: None,
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };
        assert!(config.validate().is_ok());
    }

    // ─── Image reference validation tests ────────────────────────

    #[test]
    fn test_validate_image_ref_with_tag() {
        assert!(validate_image_ref("myimage:1.0").is_ok());
        assert!(validate_image_ref("registry.example.com/myimage:v2.3").is_ok());
    }

    #[test]
    fn test_validate_image_ref_tagless_accepted() {
        // Tagless images are accepted (implicit :latest)
        assert!(validate_image_ref("myimage").is_ok());
        assert!(validate_image_ref("registry.example.com/myimage").is_ok());
    }

    #[test]
    fn test_validate_image_ref_with_digest() {
        let digest = "sha256:".to_string() + &"a".repeat(64);
        assert!(validate_image_ref(&format!("myimage@{}", digest)).is_ok());
    }

    #[test]
    fn test_validate_image_ref_invalid_digest_prefix() {
        assert!(validate_image_ref("myimage@md5:abc123").is_err());
    }

    #[test]
    fn test_validate_image_ref_empty_tag() {
        assert!(validate_image_ref("myimage:").is_err());
    }

    #[test]
    fn test_validate_image_ref_invalid_tag_chars() {
        assert!(validate_image_ref("myimage:v@1.0").is_err());
    }

    #[test]
    fn test_validate_image_ref_leading_dash_rejected() {
        // Leading `-` would be interpreted as a docker flag, not an image ref.
        assert!(validate_image_ref("-network=host").is_err());
        assert!(validate_image_ref("--privileged").is_err());
        assert!(validate_image_ref("-v").is_err());
    }

    #[test]
    fn test_validate_image_ref_whitespace_rejected() {
        // Whitespace in image refs is invalid and suspicious.
        assert!(validate_image_ref("my image").is_err());
        assert!(validate_image_ref("my\timage").is_err());
        assert!(validate_image_ref("my image:latest").is_err());
    }

    #[test]
    fn test_validate_gpus_flag_format_rejected() {
        // Common mistake: users put "--gpus all" instead of just "all".
        let config = DockerConfig {
            image: "myimage".to_string(),
            container_port: 8000,
            model_mount: DockerVolume {
                host_path: "/models".to_string(),
                container_path: "/models".to_string(),
                read_only: false,
            },
            volumes: vec![],
            devices: vec![],
            gpus: Some("--gpus all".to_string()),
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("--gpus"));
    }

    #[test]
    fn test_default_container_port() {
        assert_eq!(default_container_port(), 8000);
    }
}
