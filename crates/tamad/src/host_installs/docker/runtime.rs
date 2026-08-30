//! Container runtime selection for docker-backed backends.
//!
//! tama launches docker-backed engines (e.g. vllm-radiance) through the
//! `docker` CLI today, but podman is CLI-compatible for every flag the
//! runner emits (`run -d --name --label -p -v --device --shm-size
//! --cap-add --security-opt --group-add -e`, `image inspect`, `pull`,
//! `stop`, `inspect`, `rm -f`, `ps -a --filter`, `logs -f -t`) and its
//! `inspect` JSON parses into the existing [`DockerInspect`] shape
//! unchanged. A host that runs podman (common where dockerd-in-LXC is
//! blocked — e.g. AppArmor profile loading) can therefore point tama at
//! podman without any per-model change: the `DockerConfig` shipped by the
//! proxy is runtime-agnostic.
//!
//! The selector is a tamad CLI flag (`--container-runtime podman`),
//! defaulting to `docker` for backward compatibility. It is a *host*
//! property, resolved once at startup and threaded into every call site
//! as the binary name — mirrors the existing handle-less "dumb executor"
//! design (ADR-0010, plan-191).

use std::fmt;
use std::str::FromStr;

/// Which container CLI the tamad uses to spawn docker-backed engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContainerRuntime {
    #[default]
    Docker,
    Podman,
}

impl ContainerRuntime {
    /// The CLI binary name for this runtime.
    pub fn command(self) -> &'static str {
        match self {
            ContainerRuntime::Docker => "docker",
            ContainerRuntime::Podman => "podman",
        }
    }
}

impl fmt::Display for ContainerRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.command())
    }
}

impl FromStr for ContainerRuntime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "docker" => Ok(ContainerRuntime::Docker),
            "podman" => Ok(ContainerRuntime::Podman),
            other => Err(format!(
                "unknown container runtime '{other}' (expected 'docker' or 'podman')"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_defaults_and_case() {
        assert_eq!(
            "docker".parse::<ContainerRuntime>().unwrap(),
            ContainerRuntime::Docker
        );
        assert_eq!(
            "podman".parse::<ContainerRuntime>().unwrap(),
            ContainerRuntime::Podman
        );
        assert_eq!(
            "PODMAN".parse::<ContainerRuntime>().unwrap(),
            ContainerRuntime::Podman
        );
        assert_eq!(ContainerRuntime::default(), ContainerRuntime::Docker);
        assert_eq!(ContainerRuntime::Docker.command(), "docker");
        assert_eq!(ContainerRuntime::Podman.command(), "podman");
    }

    #[test]
    fn test_parse_rejects_unknown() {
        assert!("containerd".parse::<ContainerRuntime>().is_err());
        assert!("".parse::<ContainerRuntime>().is_err());
    }
}
