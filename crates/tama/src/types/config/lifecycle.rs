//! Backend lifecycle configuration (WASM mirror).

use serde::{Deserialize, Serialize};

use crate::core_shared::RestartPolicy as CoreRestartPolicy;

/// Backend lifecycle configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lifecycle {
    #[serde(default = "default_restart_policy")]
    pub restart_policy: CoreRestartPolicy,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
    #[serde(default = "default_restart_delay_ms")]
    pub restart_delay_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    #[serde(default = "default_health_check_timeout_ms")]
    pub health_check_timeout_ms: u64,
    #[serde(default = "default_health_check_retries")]
    pub health_check_retries: u32,
}

/// Default helper functions for Lifecycle fields.
fn default_restart_policy() -> CoreRestartPolicy {
    CoreRestartPolicy::Always
}

fn default_max_restarts() -> u32 {
    10
}

fn default_restart_delay_ms() -> u64 {
    3000
}

fn default_health_check_interval_ms() -> u64 {
    5000
}

fn default_health_check_timeout_ms() -> u64 {
    30000
}

fn default_health_check_retries() -> u32 {
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifecycle_serialization() {
        let lifecycle = Lifecycle {
            restart_policy: CoreRestartPolicy::Always,
            max_restarts: 3,
            restart_delay_ms: 5000,
            health_check_interval_ms: 10000,
            health_check_timeout_ms: 5000,
            health_check_retries: 2,
        };

        let json = serde_json::to_string(&lifecycle).unwrap();
        let deserialized: Lifecycle = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.restart_policy, CoreRestartPolicy::Always);
        assert_eq!(deserialized.max_restarts, 3);
    }
}
