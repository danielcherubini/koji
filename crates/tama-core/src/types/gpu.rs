use serde::{Deserialize, Serialize};

/// GPU vendor identifier.
///
/// `PartialOrd`/`Ord` derive is used for stable sort ordering of GPU devices
/// in `SystemMetrics` (Amd < Nvidia).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Amd,
    #[default]
    Nvidia,
}

impl GpuVendor {
    /// Convert the vendor to its string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nvidia => "nvidia",
            Self::Amd => "amd",
        }
    }

    /// Try to create a `GpuVendor` from its string representation.
    ///
    /// Accepts `"nvidia"` and `"amd"` (case-insensitive).
    pub fn try_from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "nvidia" => Some(Self::Nvidia),
            "amd" => Some(Self::Amd),
            _ => None,
        }
    }
}

/// Lifecycle state of a model's backend.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelState {
    /// No model is loaded on this backend.
    #[default]
    Idle,
    /// The backend is currently starting up.
    #[serde(alias = "loading")]
    Starting,
    /// The backend is ready and accepting requests.
    Ready,
    /// The backend is unloading.
    Unloading,
    /// The backend has failed to load or crashed.
    Failed,
}

impl ModelState {
    /// Convert the state to its string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Unloading => "unloading",
            Self::Failed => "failed",
        }
    }

    /// Parse a model state from a string value.
    ///
    /// Accepts `"idle"`, `"starting"` (with alias `"loading"`), `"ready"`,
    /// `"unloading"`, and `"failed"`. Case-insensitive.
    pub fn from_str_fallback(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "idle" => Self::Idle,
            "starting" | "loading" => Self::Starting,
            "ready" => Self::Ready,
            "unloading" => Self::Unloading,
            "failed" => Self::Failed,
            _ => Self::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_state_serializes_starting() {
        let json = serde_json::to_string(&ModelState::Starting).unwrap();
        assert_eq!(json, "\"starting\"");
    }

    #[test]
    fn test_model_state_deserializes_loading_as_starting() {
        let state: ModelState = serde_json::from_str("\"loading\"").unwrap();
        assert_eq!(state, ModelState::Starting);
    }

    #[test]
    fn test_model_state_serializes_and_deserializes_starting() {
        let json = serde_json::to_string(&ModelState::Starting).unwrap();
        let state: ModelState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, ModelState::Starting);
    }
}
