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

    /// Load config from the default SQLite database.
    ///
    /// If `config.toml` exists in the config directory, it is migrated to the
    /// SQLite database in a single pass (backends, models, and global config),
    /// then renamed to `config.toml.migrated`.
    ///
    /// If no TOML exists and the DB is empty, defaults are seeded.
    pub fn load() -> Result<Self> {
        let config_dir = Self::config_dir()?;
        let db_path = config_dir.join("tama.db");

        // Run one-time TOML → DB migration if config.toml exists.
        // The migration is idempotent (checks app_general row), so concurrent
        // callers are safe — the second will skip.
        if config_dir.join("config.toml").exists() {
            crate::db::backfill::migrate_toml_to_db(&config_dir, &db_path)?;
        }

        // Load from DB
        Self::from_db(&db_path)
    }

    /// Load config from an explicit SQLite database path.
    ///
    /// Used by `tama web` CLI handler and tests which need to load from a
    /// non-standard DB location.
    pub fn load_from(db_path: &std::path::Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create config directory")?;
        }

        let config = Self::from_db(db_path)?;
        Ok(config)
    }

    /// Save config to the default SQLite database.
    pub fn save(&self) -> Result<()> {
        let config_dir = Self::config_dir()?;
        let db_path = config_dir.join("tama.db");
        self.to_db(&db_path)
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
