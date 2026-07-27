use serde::{Deserialize, Serialize};

use crate::config::types::RestartPolicy;

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Backend lifecycle configuration.
pub struct Lifecycle {
    #[serde(default = "default_restart_policy")]
    pub restart_policy: RestartPolicy,
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

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            restart_policy: default_restart_policy(),
            max_restarts: default_max_restarts(),
            restart_delay_ms: default_restart_delay_ms(),
            health_check_interval_ms: default_health_check_interval_ms(),
            health_check_timeout_ms: default_health_check_timeout_ms(),
            health_check_retries: default_health_check_retries(),
        }
    }
}

fn default_restart_policy() -> RestartPolicy {
    RestartPolicy::Always
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
