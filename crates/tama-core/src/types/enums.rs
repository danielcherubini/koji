use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Restart policy for managed model processes.
///
/// Controls when the supervisor restarts a model process that exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    /// Always restart the process, regardless of exit code.
    #[default]
    Always,
    /// Only restart the process if it exits with a non-zero exit code.
    OnFailure,
}

impl RestartPolicy {
    /// Serialize this enum to its string representation for DB storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::OnFailure => "on-failure",
        }
    }

    /// Parse a restart policy from a string value.
    ///
    /// Accepts `"always"`, `"on-failure"`, and their case-insensitive variants.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "always" => Some(Self::Always),
            "on-failure" => Some(Self::OnFailure),
            _ => None,
        }
    }
}

impl FromStr for RestartPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s).ok_or_else(|| format!("invalid restart policy: {s}"))
    }
}

/// Application log level.
///
/// Maps to the corresponding `tracing::Level` for runtime logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Debug-level logging.
    Debug,
    /// Info-level logging (default).
    #[default]
    Info,
    /// Warning-level logging.
    Warn,
    /// Error-level logging.
    Error,
}

impl LogLevel {
    /// Serialize this enum to its string representation for DB storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    /// Parse a log level from a string value.
    ///
    /// Accepts `"debug"`, `"info"`, `"warn"`, `"error"`, and their
    /// case-insensitive variants.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s).ok_or_else(|| format!("invalid log level: {s}"))
    }
}

/// Compute device for the LLMLingua-2 compaction backend.
///
/// Maps to the `COMPACTION_DEVICE` environment variable passed to the
/// Python server process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CompactionDevice {
    /// CPU-only inference.
    #[default]
    Cpu,
    /// CUDA GPU inference (auto-select device 0).
    Cuda,
    /// CUDA GPU inference on a specific device (e.g. `cuda:0`, `cuda:1`).
    CudaDevice(u32),
    /// Apple Metal Performance Shaders (MPS) inference.
    Mps,
}

impl Serialize for CompactionDevice {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Cpu => serializer.serialize_str("cpu"),
            Self::Cuda => serializer.serialize_str("cuda"),
            Self::CudaDevice(idx) => serializer.serialize_str(&format!("cuda:{idx}")),
            Self::Mps => serializer.serialize_str("mps"),
        }
    }
}

impl<'de> Deserialize<'de> for CompactionDevice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid compaction device: {s}")))
    }
}

impl CompactionDevice {
    /// Serialize this enum to its string representation for DB storage.
    /// Returns `String` (not `&'static str`) because `CudaDevice(N)` requires
    /// formatting (allocates on each call).
    pub fn as_str(&self) -> String {
        match self {
            Self::Cpu => "cpu".to_string(),
            Self::Cuda => "cuda".to_string(),
            Self::CudaDevice(idx) => format!("cuda:{idx}"),
            Self::Mps => "mps".to_string(),
        }
    }

    /// Parse a compaction device from a string value.
    ///
    /// Accepts `"cpu"`, `"cuda"`, `"cuda:N"` (where N is a non-negative
    /// integer), and `"mps"`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "cpu" => Some(Self::Cpu),
            "cuda" => Some(Self::Cuda),
            "mps" => Some(Self::Mps),
            s if s.starts_with("cuda:") => {
                let idx = s.strip_prefix("cuda:")?.parse::<u32>().ok()?;
                Some(Self::CudaDevice(idx))
            }
            _ => None,
        }
    }
}

impl FromStr for CompactionDevice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s).ok_or_else(|| format!("invalid compaction device: {s}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RestartPolicy tests ────────────────────────────────────────────

    #[test]
    fn test_restart_policy_serialize_always() {
        let json = serde_json::to_string(&RestartPolicy::Always).unwrap();
        assert_eq!(json, "\"always\"");
    }

    #[test]
    fn test_restart_policy_serialize_on_failure() {
        let json = serde_json::to_string(&RestartPolicy::OnFailure).unwrap();
        assert_eq!(json, "\"on-failure\"");
    }

    #[test]
    fn test_restart_policy_deserialize_always() {
        let policy: RestartPolicy = serde_json::from_str("\"always\"").unwrap();
        assert_eq!(policy, RestartPolicy::Always);
    }

    #[test]
    fn test_restart_policy_deserialize_on_failure() {
        let policy: RestartPolicy = serde_json::from_str("\"on-failure\"").unwrap();
        assert_eq!(policy, RestartPolicy::OnFailure);
    }

    #[test]
    fn test_restart_policy_as_str() {
        assert_eq!(RestartPolicy::Always.as_str(), "always");
        assert_eq!(RestartPolicy::OnFailure.as_str(), "on-failure");
    }

    #[test]
    fn test_restart_policy_from_str() {
        assert_eq!(
            RestartPolicy::from_str("always"),
            Some(RestartPolicy::Always)
        );
        assert_eq!(
            RestartPolicy::from_str("on-failure"),
            Some(RestartPolicy::OnFailure)
        );
        assert_eq!(
            RestartPolicy::from_str("ALWAYS"),
            Some(RestartPolicy::Always)
        );
        assert_eq!(
            RestartPolicy::from_str("ON-FAILURE"),
            Some(RestartPolicy::OnFailure)
        );
        assert_eq!(RestartPolicy::from_str("never"), None);
        assert_eq!(RestartPolicy::from_str("random"), None);
    }

    #[test]
    fn test_restart_policy_default() {
        assert_eq!(RestartPolicy::default(), RestartPolicy::Always);
    }

    // ── LogLevel tests ─────────────────────────────────────────────────

    #[test]
    fn test_log_level_serialize_all() {
        assert_eq!(
            serde_json::to_string(&LogLevel::Debug).unwrap(),
            "\"debug\""
        );
        assert_eq!(serde_json::to_string(&LogLevel::Info).unwrap(), "\"info\"");
        assert_eq!(serde_json::to_string(&LogLevel::Warn).unwrap(), "\"warn\"");
        assert_eq!(
            serde_json::to_string(&LogLevel::Error).unwrap(),
            "\"error\""
        );
    }

    #[test]
    fn test_log_level_deserialize_all() {
        assert_eq!(
            serde_json::from_str::<LogLevel>("\"debug\"").unwrap(),
            LogLevel::Debug
        );
        assert_eq!(
            serde_json::from_str::<LogLevel>("\"info\"").unwrap(),
            LogLevel::Info
        );
        assert_eq!(
            serde_json::from_str::<LogLevel>("\"warn\"").unwrap(),
            LogLevel::Warn
        );
        assert_eq!(
            serde_json::from_str::<LogLevel>("\"error\"").unwrap(),
            LogLevel::Error
        );
    }

    #[test]
    fn test_log_level_as_str() {
        assert_eq!(LogLevel::Debug.as_str(), "debug");
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::Error.as_str(), "error");
    }

    #[test]
    fn test_log_level_from_str() {
        assert_eq!(LogLevel::from_str("debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::from_str("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::from_str("warn"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_str("TRACE"), None);
        assert_eq!(LogLevel::from_str("verbose"), None);
    }

    #[test]
    fn test_log_level_default() {
        assert_eq!(LogLevel::default(), LogLevel::Info);
    }

    // ── CompactionDevice tests ─────────────────────────────────────────

    #[test]
    fn test_compaction_device_serialize_cpu() {
        let json = serde_json::to_string(&CompactionDevice::Cpu).unwrap();
        assert_eq!(json, "\"cpu\"");
    }

    #[test]
    fn test_compaction_device_serialize_cuda() {
        let json = serde_json::to_string(&CompactionDevice::Cuda).unwrap();
        assert_eq!(json, "\"cuda\"");
    }

    #[test]
    fn test_compaction_device_serialize_cuda_device() {
        let json = serde_json::to_string(&CompactionDevice::CudaDevice(0)).unwrap();
        assert_eq!(json, "\"cuda:0\"");
        let json = serde_json::to_string(&CompactionDevice::CudaDevice(1)).unwrap();
        assert_eq!(json, "\"cuda:1\"");
    }

    #[test]
    fn test_compaction_device_serialize_mps() {
        let json = serde_json::to_string(&CompactionDevice::Mps).unwrap();
        assert_eq!(json, "\"mps\"");
    }

    #[test]
    fn test_compaction_device_deserialize_cpu() {
        let device: CompactionDevice = serde_json::from_str("\"cpu\"").unwrap();
        assert_eq!(device, CompactionDevice::Cpu);
    }

    #[test]
    fn test_compaction_device_deserialize_cuda() {
        let device: CompactionDevice = serde_json::from_str("\"cuda\"").unwrap();
        assert_eq!(device, CompactionDevice::Cuda);
    }

    #[test]
    fn test_compaction_device_deserialize_cuda_device() {
        let device: CompactionDevice = serde_json::from_str("\"cuda:0\"").unwrap();
        assert_eq!(device, CompactionDevice::CudaDevice(0));
        let device: CompactionDevice = serde_json::from_str("\"cuda:2\"").unwrap();
        assert_eq!(device, CompactionDevice::CudaDevice(2));
    }

    #[test]
    fn test_compaction_device_deserialize_mps() {
        let device: CompactionDevice = serde_json::from_str("\"mps\"").unwrap();
        assert_eq!(device, CompactionDevice::Mps);
    }

    #[test]
    fn test_compaction_device_as_str() {
        assert_eq!(CompactionDevice::Cpu.as_str(), "cpu");
        assert_eq!(CompactionDevice::Cuda.as_str(), "cuda");
        assert_eq!(CompactionDevice::CudaDevice(0).as_str(), "cuda:0");
        assert_eq!(CompactionDevice::CudaDevice(3).as_str(), "cuda:3");
        assert_eq!(CompactionDevice::Mps.as_str(), "mps");
    }

    #[test]
    fn test_compaction_device_from_str() {
        assert_eq!(
            CompactionDevice::from_str("cpu"),
            Some(CompactionDevice::Cpu)
        );
        assert_eq!(
            CompactionDevice::from_str("cuda"),
            Some(CompactionDevice::Cuda)
        );
        assert_eq!(
            CompactionDevice::from_str("cuda:0"),
            Some(CompactionDevice::CudaDevice(0))
        );
        assert_eq!(
            CompactionDevice::from_str("cuda:7"),
            Some(CompactionDevice::CudaDevice(7))
        );
        assert_eq!(
            CompactionDevice::from_str("mps"),
            Some(CompactionDevice::Mps)
        );
        assert_eq!(CompactionDevice::from_str("gpu"), None);
        assert_eq!(CompactionDevice::from_str("cuda:"), None);
        assert_eq!(CompactionDevice::from_str("cuda:abc"), None);
    }

    #[test]
    fn test_compaction_device_roundtrip() {
        let devices = [
            CompactionDevice::Cpu,
            CompactionDevice::Cuda,
            CompactionDevice::CudaDevice(0),
            CompactionDevice::CudaDevice(1),
            CompactionDevice::CudaDevice(4),
            CompactionDevice::Mps,
        ];
        for device in devices {
            let json = serde_json::to_string(&device).unwrap();
            let deserialized: CompactionDevice = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, device);
        }
    }

    #[test]
    fn test_compaction_device_default() {
        assert_eq!(CompactionDevice::default(), CompactionDevice::Cpu);
    }
}
