use anyhow::{anyhow, Context, Result};
use std::sync::Arc;

use crate::installations::types::{InstallationInfo, InstallationSource};

/// A single backend option for UI dropdowns (e.g. model editor backend selector).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct InstallationOption {
    pub name: String,
    #[serde(default)]
    pub variant: Option<String>,
    pub label: String,
}

/// Centralized backend data access over the shared Postgres pool.
///
/// All methods are async (plan-190 Task 8); `PgPool` is internally
/// reference-counted and safe to share across tasks.
#[derive(Clone)]
pub struct InstallationManager {
    pool: Arc<sqlx::PgPool>,
}

impl InstallationManager {
    /// Create a manager on the shared Postgres pool.
    ///
    /// The one-shot legacy backend file migration
    /// (`crate::installations::migration::migrate_legacy_backends`) runs at
    /// startup (main.rs), not here.
    pub fn new(pool: Arc<sqlx::PgPool>) -> Self {
        Self { pool }
    }

    // ── Config (backend_configs table) ──────────────────────────

    /// Get config for a backend name + gpu_variant pair.
    pub async fn get_config(
        &self,
        name: &str,
        gpu_variant: &str,
    ) -> Result<Option<crate::db::queries::InstallationConfigRecord>> {
        crate::db::queries::get_installation_config(&self.pool, name, gpu_variant).await
    }

    /// Insert or update config keyed by the backend's stable logical_id.
    /// Returns the row's integer id.
    pub async fn save_config(
        &self,
        name: &str,
        gpu_variant: &str,
        default_args: &[String],
        default_env: &[String],
        health_check_url: Option<&str>,
    ) -> Result<i64> {
        let logical_id = self.resolve_logical_id(name).await?;
        crate::db::queries::upsert_installation_config(
            &self.pool,
            &logical_id,
            name,
            gpu_variant,
            default_args,
            default_env,
            health_check_url,
        )
        .await
    }

    /// List all backend config rows.
    pub async fn list_configs(&self) -> Result<Vec<crate::db::queries::InstallationConfigRecord>> {
        crate::db::queries::list_installation_configs(&self.pool).await
    }

    // ── Resolution ───────────────────────────────────────────────

    /// Get default_args for a backend + variant from backend_configs.
    /// Returns empty vec if no config exists.
    pub async fn get_default_args(&self, name: &str, gpu_variant: &str) -> Vec<String> {
        crate::db::queries::get_installation_config(&self.pool, name, gpu_variant)
            .await
            .ok()
            .flatten()
            .map(|c| c.default_args)
            .unwrap_or_default()
    }

    /// Get default_env for a backend + variant from backend_configs.
    /// Returns empty vec if no config exists.
    pub async fn get_default_env(&self, name: &str, gpu_variant: &str) -> Vec<String> {
        crate::db::queries::get_installation_config(&self.pool, name, gpu_variant)
            .await
            .ok()
            .flatten()
            .map(|c| c.default_env)
            .unwrap_or_default()
    }

    /// Get health_check_url from backend_configs.
    pub async fn get_health_check_url(&self, name: &str, gpu_variant: &str) -> Option<String> {
        crate::db::queries::get_installation_config(&self.pool, name, gpu_variant)
            .await
            .ok()
            .flatten()
            .and_then(|c| c.health_check_url)
    }

    // ── Discovery ───────────────────────────────────────────────

    /// Return backend options for UI dropdowns (name, variant, label).
    /// Discovers from active installations in `provider_installations` table.
    pub async fn available_installations(&self) -> Result<Vec<InstallationOption>> {
        let active = crate::db::queries::list_active_installations(&self.pool).await?;
        let mut seen = std::collections::HashSet::new();
        let mut options = Vec::new();
        for record in &active {
            let key = (record.name.clone(), record.gpu_variant.clone());
            if seen.insert(key.clone()) {
                options.push(InstallationOption {
                    name: key.0.clone(),
                    variant: Some(key.1.clone()),
                    label: if key.1 == "cpu" {
                        key.0.clone()
                    } else {
                        format!("{} ({})", key.0, key.1)
                    },
                });
            }
        }
        Ok(options)
    }

    // ── Installation (provider_installations table) ─────────────

    /// Add a new backend installation, marking it as the active version.
    /// Delegates to `insert_installation` which handles the
    /// `ON CONFLICT (name, gpu_variant, version)` upsert and deactivates
    /// other versions of the same (name, gpu_variant).
    pub async fn add_installation(&self, info: &InstallationInfo) -> Result<()> {
        let record = Self::info_to_record(info)?;
        crate::db::queries::insert_installation(&self.pool, &record)
            .await
            .with_context(|| format!("Failed to insert backend '{}'", info.name))
    }

    /// Get the active installation for a name + variant.
    pub async fn get_active(
        &self,
        name: &str,
        gpu_variant: &str,
    ) -> Result<Option<InstallationInfo>> {
        let record =
            crate::db::queries::get_active_installation(&self.pool, name, gpu_variant).await?;
        match record {
            Some(r) => Ok(Some(Self::record_to_info(r)?)),
            None => Ok(None),
        }
    }

    /// List all active backend installations (one per name+variant).
    pub async fn list_active(&self) -> Result<Vec<InstallationInfo>> {
        let records = crate::db::queries::list_active_installations(&self.pool).await?;
        records.into_iter().map(Self::record_to_info).collect()
    }

    /// Atomically rename a backend across every table that carries its display
    /// name. The stable `logical_id` join keys are preserved, so `backend_configs`
    /// (keyed on `logical_id`) survives the rename intact. Returns `false` if
    /// `old_name` has no installation (backend not found).
    pub async fn rename(&self, old_name: &str, new_name: &str) -> Result<bool> {
        crate::db::queries::rename_installation(&self.pool, old_name, new_name).await
    }

    /// List all versions of a backend.
    /// If `gpu_variant` is Some, filters to that variant. If None, returns all variants.
    /// Returns None if no versions exist for this name.
    pub async fn list_versions(
        &self,
        name: &str,
        gpu_variant: Option<&str>,
    ) -> Result<Option<Vec<InstallationInfo>>> {
        let records =
            crate::db::queries::list_installation_versions(&self.pool, name, gpu_variant).await?;
        if records.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                records
                    .into_iter()
                    .map(Self::record_to_info)
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
    }

    /// Get a specific installation by (name, gpu_variant, version).
    pub async fn get_by_version(
        &self,
        name: &str,
        gpu_variant: &str,
        version: &str,
    ) -> Result<Option<InstallationInfo>> {
        let record =
            crate::db::queries::get_installation_by_version(&self.pool, name, gpu_variant, version)
                .await?;
        match record {
            Some(r) => Ok(Some(Self::record_to_info(r)?)),
            None => Ok(None),
        }
    }

    /// Activate a specific version for a name + variant.
    /// Deactivates all other versions of the same (name, gpu_variant).
    /// Returns true if the version was found and activated.
    pub async fn activate(&self, name: &str, gpu_variant: &str, version: &str) -> Result<bool> {
        crate::db::queries::activate_installation_version(&self.pool, name, gpu_variant, version)
            .await
    }

    /// Update an existing backend to a new version (convenience).
    /// Reads the current active installation, builds a new InstallationInfo with
    /// updated version/path/source, and calls add_installation.
    pub async fn update_version(
        &self,
        name: &str,
        gpu_variant: &str,
        new_version: String,
        new_path: std::path::PathBuf,
        new_source: Option<InstallationSource>,
    ) -> Result<()> {
        let existing = self
            .get_active(name, gpu_variant)
            .await?
            .ok_or_else(|| anyhow!("Backend '{}' variant '{}' not found", name, gpu_variant))?;
        let updated = InstallationInfo {
            name: existing.name,
            backend_type: existing.backend_type,
            version: new_version,
            path: new_path,
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
            gpu_variant: existing.gpu_variant,
            source: new_source,
            docker_config: existing.docker_config,
        };
        self.add_installation(&updated).await
    }

    /// Delete a specific (name, gpu_variant, version) installation row.
    /// If the deleted version was active, re-activates the newest remaining version.
    pub async fn remove_version(&self, name: &str, gpu_variant: &str, version: &str) -> Result<()> {
        // Check if the target version exists before deleting
        let existing =
            crate::db::queries::get_installation_by_version(&self.pool, name, gpu_variant, version)
                .await?;
        let was_active = existing.as_ref().map(|r| r.is_active).unwrap_or(false);

        crate::db::queries::delete_installation(&self.pool, name, gpu_variant, version).await?;

        // If we deleted the active version, activate the newest remaining one
        if was_active {
            let remaining =
                crate::db::queries::list_installation_versions(&self.pool, name, Some(gpu_variant))
                    .await?;
            if let Some(newest) = remaining.first() {
                crate::db::queries::activate_installation_version(
                    &self.pool,
                    name,
                    gpu_variant,
                    &newest.version,
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Delete all versions of a backend.
    /// If `gpu_variant` is Some, only deletes that variant.
    /// If None, deletes all variants.
    pub async fn delete_all_versions(&self, name: &str, gpu_variant: Option<&str>) -> Result<()> {
        crate::db::queries::delete_all_installation_versions(&self.pool, name, gpu_variant).await
    }

    /// Update the build method (source vs prebuilt) for an active backend installation.
    ///
    /// When switching to source build (`build_from_source = true`):
    /// - If the existing source is `SourceCode`, preserves the existing `git_url` and `commit`.
    /// - Otherwise, uses the default git URL for the backend type.
    ///
    /// When switching to prebuilt (`build_from_source = false`):
    /// - Sets source to `Prebuilt` with the current version.
    pub async fn update_build_method(
        &self,
        name: &str,
        gpu_variant: &str,
        build_from_source: bool,
    ) -> Result<()> {
        let backend_info = self
            .get_active(name, gpu_variant)
            .await?
            .ok_or_else(|| anyhow!("Backend '{}' variant '{}' not found", name, gpu_variant))?;

        let new_source = if build_from_source {
            match &backend_info.source {
                Some(InstallationSource::SourceCode {
                    git_url, commit, ..
                }) => InstallationSource::SourceCode {
                    version: backend_info.version.clone(),
                    git_url: git_url.clone(),
                    commit: commit.clone(),
                },
                _ => InstallationSource::SourceCode {
                    version: backend_info.version.clone(),
                    git_url: backend_info.backend_type.default_git_url().to_string(),
                    commit: None,
                },
            }
        } else {
            InstallationSource::Prebuilt {
                version: backend_info.version.clone(),
            }
        };

        let source_json =
            serde_json::to_string(&new_source).context("Failed to serialize InstallationSource")?;

        crate::db::queries::update_installation_source(&self.pool, name, gpu_variant, &source_json)
            .await
    }

    // ── Private helpers ──────────────────────────────────────

    /// Resolve the stable `logical_id` for a backend `name` from the active
    /// installation, falling back to the empty string if no matching installation
    /// exists (legacy rows that only carry a name). Propagates DB errors so
    /// callers don't silently save config under an empty key.
    async fn resolve_logical_id(&self, name: &str) -> Result<String> {
        Ok(
            crate::db::queries::get_installation_logical_id(&self.pool, name)
                .await?
                .unwrap_or_default(),
        )
    }

    fn info_to_record(info: &InstallationInfo) -> Result<crate::db::queries::InstallationRecord> {
        let source_json = info
            .source
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("Failed to serialize source")?;
        let docker_config_json = info
            .docker_config
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("Failed to serialize docker_config")?;
        Ok(crate::db::queries::InstallationRecord {
            id: 0,
            name: info.name.clone(),
            backend_type: info.backend_type.to_string(),
            version: info.version.clone(),
            path: info.path.to_string_lossy().to_string(),
            installed_at: info.installed_at,
            gpu_variant: info.gpu_variant.clone(),
            source: source_json,
            is_active: true,
            docker_config: docker_config_json,
            logical_id: String::new(),
        })
    }

    fn record_to_info(record: crate::db::queries::InstallationRecord) -> Result<InstallationInfo> {
        let source = record
            .source
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .context("Failed to deserialize source")?;
        let docker_config = record
            .docker_config
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .context("Failed to deserialize docker_config")?;
        Ok(InstallationInfo {
            name: record.name,
            backend_type: record.backend_type.parse().map_err(|e| {
                anyhow::anyhow!(
                    "Invalid backend_type '{}' in database: {}",
                    record.backend_type,
                    e
                )
            })?,
            version: record.version,
            path: std::path::PathBuf::from(record.path),
            installed_at: record.installed_at,
            gpu_variant: record.gpu_variant,
            source,
            docker_config,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{insert_installation, InstallationRecord};
    use crate::installations::types::InstallationType;
    use crate::testing::postgres::with_schema;

    async fn insert_active_backend(
        pool: &sqlx::PgPool,
        name: &str,
        gpu_variant: &str,
        version: &str,
    ) -> Result<()> {
        insert_installation(
            pool,
            &InstallationRecord {
                id: 0,
                name: name.to_string(),
                backend_type: "llama_cpp".to_string(),
                version: version.to_string(),
                path: "/tmp/test/llama-server".to_string(),
                installed_at: 0,
                gpu_variant: gpu_variant.to_string(),
                source: None,
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )
        .await
    }

    #[tokio::test]
    async fn test_new_creates_instance() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        // Should be able to call methods without panic
        let configs = manager.list_configs().await.unwrap();
        assert!(configs.is_empty());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_save_and_get_config_roundtrip() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let args = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        let env = vec!["RADV_PERFTEST=nogttspill".to_string()];
        let id = manager
            .save_config(
                "llama_cpp",
                "cpu",
                &args,
                &env,
                Some("http://localhost:8080/health"),
            )
            .await
            .unwrap();
        assert_eq!(id, 1);

        let record = manager
            .get_config("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.name, "llama_cpp");
        assert_eq!(record.gpu_variant, "cpu");
        assert_eq!(record.default_args, args);
        assert_eq!(record.default_env, env);
        assert_eq!(
            record.health_check_url,
            Some("http://localhost:8080/health".to_string())
        );
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_save_config_updates_existing() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        // Insert initial row
        let id1 = manager
            .save_config(
                "llama_cpp",
                "cpu",
                &["-fa 1".to_string()],
                &[],
                Some("http://localhost:8080/health"),
            )
            .await
            .unwrap();

        // Upsert with different values
        let id2 = manager
            .save_config(
                "llama_cpp",
                "cpu",
                &["-fa 1".to_string(), "-b 2048".to_string()],
                &["FOO=bar".to_string()],
                Some("http://localhost:9090/health"),
            )
            .await
            .unwrap();

        // ID should be the same (updated, not re-inserted)
        assert_eq!(id1, id2);

        let record = manager
            .get_config("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.default_args, vec!["-fa 1", "-b 2048"]);
        assert_eq!(record.default_env, vec!["FOO=bar"]);
        assert_eq!(
            record.health_check_url,
            Some("http://localhost:9090/health".to_string())
        );
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_get_config_returns_none_for_missing() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));
        let result = manager.get_config("nonexistent", "cpu").await.unwrap();
        assert!(result.is_none());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_list_configs_returns_all() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        manager
            .save_config(
                "llama_cpp",
                "cpu",
                &["-fa 1".to_string()],
                &[],
                Some("http://localhost:8080/health"),
            )
            .await
            .unwrap();
        manager
            .save_config("llama_cpp", "vulkan", &[], &[], None)
            .await
            .unwrap();
        manager
            .save_config("ik_llama", "cpu", &[], &[], None)
            .await
            .unwrap();

        let configs = manager.list_configs().await.unwrap();
        assert_eq!(configs.len(), 3);

        let cpu = configs
            .iter()
            .find(|c| c.name == "llama_cpp" && c.gpu_variant == "cpu")
            .unwrap();
        assert_eq!(cpu.default_args, vec!["-fa 1"]);

        let vulkan = configs
            .iter()
            .find(|c| c.name == "llama_cpp" && c.gpu_variant == "vulkan")
            .unwrap();
        assert!(vulkan.default_args.is_empty());
        assert!(vulkan.health_check_url.is_none());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_available_installations_returns_options() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        insert_active_backend(&guard.pool, "llama_cpp", "cpu", "b8407")
            .await
            .unwrap();
        insert_active_backend(&guard.pool, "ik_llama", "cuda", "main")
            .await
            .unwrap();

        let options = manager.available_installations().await.unwrap();
        assert_eq!(options.len(), 2);

        let llama = options.iter().find(|o| o.name == "llama_cpp").unwrap();
        assert_eq!(llama.variant, Some("cpu".to_string()));
        assert_eq!(llama.label, "llama_cpp");

        let ik = options.iter().find(|o| o.name == "ik_llama").unwrap();
        assert_eq!(ik.variant, Some("cuda".to_string()));
        assert_eq!(ik.label, "ik_llama (cuda)");
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_available_installations_groups_by_variant() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        // Insert multiple versions of the same backend+variant
        insert_active_backend(&guard.pool, "llama_cpp", "cpu", "b8407")
            .await
            .unwrap();
        insert_active_backend(&guard.pool, "llama_cpp", "cpu", "b9000")
            .await
            .unwrap();
        insert_active_backend(&guard.pool, "llama_cpp", "cuda", "b8407")
            .await
            .unwrap();

        let options = manager.available_installations().await.unwrap();
        // Should have 2 options: one for cpu, one for cuda (not 3)
        assert_eq!(options.len(), 2);

        let cpu_opt = options
            .iter()
            .find(|o| o.variant == Some("cpu".to_string()))
            .unwrap();
        assert_eq!(cpu_opt.name, "llama_cpp");

        let cuda_opt = options
            .iter()
            .find(|o| o.variant == Some("cuda".to_string()))
            .unwrap();
        assert_eq!(cuda_opt.name, "llama_cpp");
        guard.finish().await;
    }

    // ── Installation tests ───────────────────────────────────────────

    fn make_backend_info(name: &str, version: &str) -> InstallationInfo {
        InstallationInfo {
            name: name.to_string(),
            backend_type: InstallationType::LlamaCpp,
            version: version.to_string(),
            path: std::path::PathBuf::from(format!("/path/to/{}", name)),
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            gpu_variant: "cpu".to_string(),
            source: None,
            docker_config: None,
        }
    }

    #[tokio::test]
    async fn test_add_and_get_installation() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let info = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info).await.unwrap();

        let result = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.name, "llama_cpp");
        assert_eq!(result.version, "b8407");
        assert_eq!(result.backend_type, InstallationType::LlamaCpp);
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_add_installation_replaces_old() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let info1 = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info1).await.unwrap();

        // Add a new version — should deactivate old one
        let info2 = InstallationInfo {
            version: "b9000".to_string(),
            installed_at: info1.installed_at + 100,
            ..info1.clone()
        };
        manager.add_installation(&info2).await.unwrap();

        // Active should be the new version
        let active = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.version, "b9000");

        // Old version should still exist but not be active
        let old = manager
            .get_by_version("llama_cpp", "cpu", "b8407")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old.version, "b8407");
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_list_active_returns_all() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        manager
            .add_installation(&make_backend_info("llama_cpp", "b8407"))
            .await
            .unwrap();
        let ik_info = InstallationInfo {
            name: "ik_llama".to_string(),
            backend_type: InstallationType::IkLlama,
            gpu_variant: "cuda".to_string(),
            ..make_backend_info("ik_llama", "main")
        };
        manager.add_installation(&ik_info).await.unwrap();

        let active = manager.list_active().await.unwrap();
        assert_eq!(active.len(), 2);

        let llama = active.iter().find(|b| b.name == "llama_cpp").unwrap();
        assert_eq!(llama.version, "b8407");

        let ik = active.iter().find(|b| b.name == "ik_llama").unwrap();
        assert_eq!(ik.version, "main");
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_list_versions_by_variant() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let info1 = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info1).await.unwrap();

        let info2 = InstallationInfo {
            version: "b9000".to_string(),
            installed_at: info1.installed_at + 100,
            ..info1.clone()
        };
        manager.add_installation(&info2).await.unwrap();

        let info_cuda = InstallationInfo {
            name: "llama_cpp".to_string(),
            backend_type: InstallationType::LlamaCpp,
            version: "b8407".to_string(),
            path: std::path::PathBuf::from("/path/to/llama_cpp"),
            installed_at: info1.installed_at,
            gpu_variant: "cuda".to_string(),
            source: None,
            docker_config: None,
        };
        manager.add_installation(&info_cuda).await.unwrap();

        // Filter by cpu variant
        let versions = manager
            .list_versions("llama_cpp", Some("cpu"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(versions.len(), 2);

        // Filter by cuda variant
        let versions = manager
            .list_versions("llama_cpp", Some("cuda"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].gpu_variant, "cuda");

        // No variant filter — should return all
        let all_versions = manager
            .list_versions("llama_cpp", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(all_versions.len(), 3);

        // Unknown backend returns None
        assert!(manager
            .list_versions("nonexistent", None)
            .await
            .unwrap()
            .is_none());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_activate_switches_active() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let info1 = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info1).await.unwrap();

        let info2 = InstallationInfo {
            version: "b9000".to_string(),
            installed_at: info1.installed_at + 100,
            ..info1.clone()
        };
        manager.add_installation(&info2).await.unwrap();

        // b9000 should be active (added last)
        let active = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.version, "b9000");

        // Activate b8407
        let result = manager.activate("llama_cpp", "cpu", "b8407").await.unwrap();
        assert!(result);

        let active = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.version, "b8407");

        // Activate nonexistent version returns false
        let result = manager
            .activate("llama_cpp", "cpu", "nonexistent")
            .await
            .unwrap();
        assert!(!result);

        // Active should still be b8407
        let active = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.version, "b8407");
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_remove_version_deletes_row() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let info1 = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info1).await.unwrap();

        let info2 = InstallationInfo {
            version: "b9000".to_string(),
            installed_at: info1.installed_at + 100,
            ..info1.clone()
        };
        manager.add_installation(&info2).await.unwrap();

        // Two versions exist
        let all = manager
            .list_versions("llama_cpp", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(all.len(), 2);

        // Remove b8407
        manager
            .remove_version("llama_cpp", "cpu", "b8407")
            .await
            .unwrap();

        // Only b9000 remains
        let all = manager
            .list_versions("llama_cpp", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, "b9000");

        // b9000 should be active
        let active = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.version, "b9000");
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_delete_all_versions_with_variant() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        manager
            .add_installation(&make_backend_info("llama_cpp", "b8407"))
            .await
            .unwrap();

        let info_cuda = InstallationInfo {
            gpu_variant: "cuda".to_string(),
            ..make_backend_info("llama_cpp", "b8407")
        };
        manager.add_installation(&info_cuda).await.unwrap();

        // Delete only cpu variant
        manager
            .delete_all_versions("llama_cpp", Some("cpu"))
            .await
            .unwrap();

        // CPU variant should be gone
        assert!(manager
            .list_versions("llama_cpp", Some("cpu"))
            .await
            .unwrap()
            .is_none());

        // CUDA variant should still exist
        let versions = manager
            .list_versions("llama_cpp", Some("cuda"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(versions.len(), 1);
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_delete_all_versions_without_variant_deletes_all() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        manager
            .add_installation(&make_backend_info("llama_cpp", "b8407"))
            .await
            .unwrap();

        let info_cuda = InstallationInfo {
            gpu_variant: "cuda".to_string(),
            ..make_backend_info("llama_cpp", "b8407")
        };
        manager.add_installation(&info_cuda).await.unwrap();

        // Delete all variants
        manager
            .delete_all_versions("llama_cpp", None)
            .await
            .unwrap();

        // All versions should be gone
        assert!(manager
            .list_versions("llama_cpp", None)
            .await
            .unwrap()
            .is_none());
        guard.finish().await;
    }

    // ── Resolution tests ───────────────────────────────────────────

    #[tokio::test]
    async fn test_get_default_args_returns_args() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let args = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        manager
            .save_config("llama_cpp", "cpu", &args, &[], None)
            .await
            .unwrap();

        let result = manager.get_default_args("llama_cpp", "cpu").await;
        assert_eq!(result, args);
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_get_default_args_returns_empty_for_missing() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let result = manager.get_default_args("nonexistent", "cpu").await;
        assert!(result.is_empty());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_get_health_check_url_returns_url() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        manager
            .save_config(
                "llama_cpp",
                "cpu",
                &[],
                &[],
                Some("http://localhost:8080/health"),
            )
            .await
            .unwrap();

        let result = manager.get_health_check_url("llama_cpp", "cpu").await;
        assert_eq!(result, Some("http://localhost:8080/health".to_string()));
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_get_health_check_url_returns_none_for_missing() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let result = manager.get_health_check_url("nonexistent", "cpu").await;
        assert!(result.is_none());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_update_version_convenience() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let info = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info).await.unwrap();

        // Update to new version
        manager
            .update_version(
                "llama_cpp",
                "cpu",
                "b9000".to_string(),
                std::path::PathBuf::from("/path/to/llama_cpp_v2"),
                Some(InstallationSource::Prebuilt {
                    version: "b9000".to_string(),
                }),
            )
            .await
            .unwrap();

        let active = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.version, "b9000");
        assert_eq!(
            active.path,
            std::path::PathBuf::from("/path/to/llama_cpp_v2")
        );
        assert!(active.source.is_some());
        guard.finish().await;
    }

    // ── Update build method tests ──────────────────────────────────────

    async fn insert_active_with_source(
        pool: &sqlx::PgPool,
        name: &str,
        gpu_variant: &str,
        version: &str,
        source: Option<InstallationSource>,
    ) -> Result<()> {
        let source_json = source
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .unwrap();
        insert_installation(
            pool,
            &InstallationRecord {
                id: 0,
                name: name.to_string(),
                backend_type: "llama_cpp".to_string(),
                version: version.to_string(),
                path: "/tmp/test/llama-server".to_string(),
                installed_at: 0,
                gpu_variant: gpu_variant.to_string(),
                source: source_json,
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )
        .await
    }

    #[tokio::test]
    async fn test_update_build_method_prebuilt_to_source() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        // Insert with Prebuilt source
        insert_active_with_source(
            &guard.pool,
            "llama_cpp",
            "cpu",
            "b8407",
            Some(InstallationSource::Prebuilt {
                version: "b8407".to_string(),
            }),
        )
        .await
        .unwrap();

        // Switch to source build
        manager
            .update_build_method("llama_cpp", "cpu", true)
            .await
            .unwrap();

        let active = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        let source = active.source.unwrap();
        assert!(matches!(source, InstallationSource::SourceCode { .. }));
        if let InstallationSource::SourceCode {
            git_url, version, ..
        } = source
        {
            assert_eq!(git_url, "https://github.com/ggml-org/llama.cpp.git");
            assert_eq!(version, "b8407");
        }
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_update_build_method_source_to_prebuilt() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        // Insert with SourceCode
        insert_active_with_source(
            &guard.pool,
            "llama_cpp",
            "cpu",
            "b8407",
            Some(InstallationSource::SourceCode {
                version: "b8407".to_string(),
                git_url: "https://github.com/ggml-org/llama.cpp.git".to_string(),
                commit: None,
            }),
        )
        .await
        .unwrap();

        // Switch to prebuilt
        manager
            .update_build_method("llama_cpp", "cpu", false)
            .await
            .unwrap();

        let active = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        let source = active.source.unwrap();
        assert!(matches!(source, InstallationSource::Prebuilt { .. }));
        if let InstallationSource::Prebuilt { version } = source {
            assert_eq!(version, "b8407");
        }
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_update_build_method_not_found() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        let result = manager
            .update_build_method("nonexistent", "cpu", true)
            .await;
        assert!(result.is_err());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_update_build_method_preserves_git_url() {
        let guard = with_schema().await;
        let manager = InstallationManager::new(Arc::new(guard.pool.clone()));

        // Insert with SourceCode using a custom git URL
        let custom_url = "https://github.com/custom/llama.cpp.git".to_string();
        insert_active_with_source(
            &guard.pool,
            "llama_cpp",
            "cpu",
            "b8407",
            Some(InstallationSource::SourceCode {
                version: "b8407".to_string(),
                git_url: custom_url.clone(),
                commit: Some("abc123".to_string()),
            }),
        )
        .await
        .unwrap();

        // Re-apply source build — should preserve the custom git_url and commit
        manager
            .update_build_method("llama_cpp", "cpu", true)
            .await
            .unwrap();

        let active = manager
            .get_active("llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        let source = active.source.unwrap();
        if let InstallationSource::SourceCode {
            git_url,
            commit,
            version,
        } = source
        {
            assert_eq!(git_url, custom_url);
            assert_eq!(commit, Some("abc123".to_string()));
            assert_eq!(version, "b8407");
        } else {
            panic!("Expected SourceCode, got {:?}", source);
        }
        guard.finish().await;
    }
}
