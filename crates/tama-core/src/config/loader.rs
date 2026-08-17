use super::types::{CompactionConfig, Config, General, LangfuseConfig, Lifecycle, ProxyConfig};
use crate::profiles::Profile;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

impl Config {
    /// Base directory for all tama data.
    /// Linux: `~/.config/tama`
    ///
    /// On first run after the rename from `kronk` to `tama`, this function
    /// also performs a one-time auto-migration of the legacy `kronk` data
    /// directory to the new `tama` location (including renaming `kronk.db`
    /// to `tama.db`).
    pub fn base_dir() -> Result<PathBuf> {
        let proj = directories::ProjectDirs::from("", "", "tama")
            .context("Failed to determine config directory")?;
        // config_dir() on Linux = ~/.config/tama which is already the base
        let base = proj.config_dir().to_path_buf();

        Ok(base)
    }

    pub fn config_dir() -> Result<PathBuf> {
        Self::base_dir()
    }

    /// Resolve the logs directory path.
    /// Uses `general.logs_dir` if set, otherwise defaults to `<base_dir>/logs/`.
    /// On Linux this is `~/.config/tama/logs/`.
    pub fn logs_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.general.logs_dir {
            Ok(PathBuf::from(dir))
        } else {
            Ok(Self::base_dir()?.join("logs"))
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let backends = HashMap::new();

        // Built-in sampling templates for all profiles
        let mut sampling_templates = HashMap::new();
        for (_, _, profile) in Profile::all() {
            let params = match profile {
                Profile::Coding => crate::profiles::SamplingParams {
                    temperature: Some(0.3),
                    top_p: Some(0.9),
                    top_k: Some(50),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.1),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
                Profile::Chat => crate::profiles::SamplingParams {
                    temperature: Some(0.7),
                    top_p: Some(0.95),
                    top_k: Some(40),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.0),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
                Profile::Analysis => crate::profiles::SamplingParams {
                    temperature: Some(0.3),
                    top_p: Some(0.9),
                    top_k: Some(20),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.0),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
                Profile::Creative => crate::profiles::SamplingParams {
                    temperature: Some(0.9),
                    top_p: Some(0.95),
                    top_k: Some(50),
                    min_p: Some(0.02),
                    presence_penalty: Some(0.0),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
            };
            sampling_templates.insert(profile.to_string(), params);
        }

        Config {
            general: General::default(),
            backends,
            lifecycle: Lifecycle::default(),
            proxy: ProxyConfig::default(),
            compaction: CompactionConfig::default(),
            langfuse: LangfuseConfig::default(),
            sampling_templates,
        }
    }
}
