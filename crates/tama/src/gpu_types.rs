//! Mirror types from tama-core that can be used from WASM.
//!
//! These match the serde serialization of tama-core types so the frontend
//! can deserialize SSE payloads and config JSON without depending on
//! tama-core (which is unavailable in WASM builds).

use serde::{Deserialize, Serialize};

// ── GPU types ────────────────────────────────────────────────────────────────

/// Mirror of `tama_core::gpu::GpuVendor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Nvidia,
    Amd,
}

/// Mirror of `tama_core::gpu::ModelState`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModelState {
    #[default]
    Idle,
    Loading,
    Ready,
    Unloading,
    Failed,
}

impl ModelState {
    /// Convert the state to its string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Unloading => "unloading",
            Self::Failed => "failed",
        }
    }
}

// ── Config enums ─────────────────────────────────────────────────────────────

/// Mirror of `tama_core::config::LogLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Parse from string (case-insensitive), returning default for unknowns.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "debug" => Self::Debug,
            "info" => Self::Info,
            "warn" => Self::Warn,
            "error" => Self::Error,
            _ => Self::default(),
        }
    }

    /// Convert to string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Mirror of `tama_core::config::RestartPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    #[default]
    Always,
    OnFailure,
}

impl RestartPolicy {
    /// Parse from string (case-insensitive), returning default for unknowns.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "always" => Self::Always,
            "on-failure" => Self::OnFailure,
            _ => Self::default(),
        }
    }

    /// Convert to string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::OnFailure => "on-failure",
        }
    }
}

/// Mirror of `tama_core::config::CompactionDevice`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CompactionDevice {
    #[default]
    Cpu,
    Cuda,
    CudaDevice(u32),
    Mps,
}

impl Serialize for CompactionDevice {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for CompactionDevice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_str(&s))
    }
}

impl CompactionDevice {
    /// Parse from string, returning default for unknowns.
    pub fn from_str(s: &str) -> Self {
        match s {
            "cpu" => Self::Cpu,
            "cuda" => Self::Cuda,
            "mps" => Self::Mps,
            s if s.starts_with("cuda:") => {
                if let Some(idx) = s.strip_prefix("cuda:") {
                    if let Ok(n) = idx.parse::<u32>() {
                        return Self::CudaDevice(n);
                    }
                }
                Self::Cuda
            }
            _ => Self::default(),
        }
    }

    /// Convert to string representation (allocates for CudaDevice).
    pub fn as_str(&self) -> String {
        match self {
            Self::Cpu => "cpu".to_string(),
            Self::Cuda => "cuda".to_string(),
            Self::CudaDevice(idx) => format!("cuda:{idx}"),
            Self::Mps => "mps".to_string(),
        }
    }
}
