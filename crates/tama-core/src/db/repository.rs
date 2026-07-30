//! Repository layer — domain-level database operations for the API layer.
//!
//! This module provides a `Repository` struct that wraps SQLite connections
//! and exposes domain-level methods. API handlers call repository methods
//! instead of raw queries, and receive the canonical DB record types from
//! `db::queries` directly.

use anyhow::Context;
use rusqlite::Connection;
use std::path::Path;

// ── Params type ──────────────────────────────────────────────────────────────

/// Parameters for inserting a benchmark result.
#[derive(Debug, Clone)]
pub struct BenchmarkParams {
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub engine: String,
    pub pp_sizes_json: String,
    pub tg_sizes_json: String,
    pub threads_json: Option<String>,
    pub ngl_range: Option<String>,
    pub runs: u32,
    pub warmup: u32,
    pub results_json: String,
    pub load_time_ms: Option<f64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub duration_seconds: f64,
    pub status: String,
    pub benchmark_type: Option<String>,
}

// ── Repository ───────────────────────────────────────────────────────────────

/// Domain-level database access for API handlers.
///
/// Wraps a SQLite connection and provides high-level methods for
/// model configs, aliases, benchmarks, pull queue, and update checks.
#[derive(Debug)]
pub struct Repository {
    conn: Connection,
}

// Manual Clone impl: rusqlite::Connection is not Clone, but
// WebState derives Clone and holds Option<Arc<Mutex<Repository>>>.
// Arc::clone() does not call Repository::clone(), so this is never
// actually invoked — it exists only to satisfy the derive(Clone) bound.
impl Clone for Repository {
    fn clone(&self) -> Self {
        panic!("Repository cannot be cloned; wrap in Arc<Mutex<Repository>> instead")
    }
}

impl Repository {
    /// Open a repository at the given config directory.
    pub fn open(config_dir: &Path) -> anyhow::Result<Self> {
        let open_result = crate::db::open(config_dir)?;
        Ok(Self {
            conn: open_result.conn,
        })
    }

    /// Open an in-memory repository for testing.
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let open_result = crate::db::open_in_memory()?;
        Ok(Self {
            conn: open_result.conn,
        })
    }

    /// Check if a model config exists by its integer id.
    pub fn model_exists(&self, id: i64) -> anyhow::Result<bool> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM model_configs WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .with_context(|| format!("Failed to check model config existence id={}", id))?;
        Ok(count > 0)
    }

    /// Get an active pull queue item by repo_id and filename.
    ///
    /// Returns `None` if no active (queued/running/verifying) item exists
    /// for the given repo_id and filename combination.
    pub fn get_active_pull_by_filename(
        &self,
        repo_id: &str,
        filename: &str,
    ) -> anyhow::Result<Option<queries::PullQueueItem>> {
        queries::get_active_item_by_repo_filename(&self.conn, repo_id, filename).with_context(
            || {
                format!(
                    "Failed to get active pull for repo_id={} filename={}",
                    repo_id, filename
                )
            },
        )
    }

    /// Load all model configs as a `HashMap<config_key, ModelConfig>`.
    ///
    /// Returns the raw `ModelConfig` type (not DTOs), suitable for benchmark
    /// operations that need config fields like `quants`, `api_name`, etc.
    pub fn load_model_configs_for_benchmarks(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, crate::config::ModelConfig>> {
        crate::db::load_model_configs(&self.conn)
            .with_context(|| "Failed to load model configs for benchmarks")
    }

    // ── Model Config ────────────────────────────────────────────────────

    /// Get a model configuration by its integer id.
    pub fn get_model_config(&self, id: i64) -> anyhow::Result<Option<queries::ModelConfigRecord>> {
        queries::get_model_config(&self.conn, id)
            .with_context(|| format!("Failed to get model config id={}", id))
    }

    /// Get a model configuration by repo_id.
    pub fn get_model_config_by_repo_id(
        &self,
        repo_id: &str,
    ) -> anyhow::Result<Option<queries::ModelConfigRecord>> {
        queries::get_model_config_by_repo_id(&self.conn, repo_id)
            .with_context(|| format!("Failed to get model config by repo_id={}", repo_id))
    }

    /// Get all files for a model config by its id.
    pub fn get_model_files(&self, config_id: i64) -> anyhow::Result<Vec<queries::ModelFileRecord>> {
        queries::get_model_files(&self.conn, config_id)
            .with_context(|| format!("Failed to get model files for config id={}", config_id))
    }

    /// Load all model configs as a HashMap<config_key, ModelConfigRecord>.
    /// config_key is derived via `crate::models::ConfigKey::from_repo_id`.
    pub fn load_model_configs(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, queries::ModelConfigRecord>> {
        let records = queries::get_all_model_configs(&self.conn)?;
        let mut configs = std::collections::HashMap::new();
        for record in records {
            let config_key = crate::models::ConfigKey::from_repo_id(&record.repo_id).to_string();
            configs.insert(config_key, record);
        }
        Ok(configs)
    }

    // ── Aliases ─────────────────────────────────────────────────────────

    /// Load all aliases with resolved model names.
    pub fn get_all_aliases(&self) -> anyhow::Result<Vec<queries::AliasResponse>> {
        queries::get_all_aliases(&self.conn)
    }

    /// Get a single alias by id.
    pub fn get_alias_by_id(&self, id: i64) -> anyhow::Result<Option<queries::AliasResponse>> {
        queries::get_alias_by_id(&self.conn, id)
    }

    /// Insert a new alias. Returns the new row's id.
    pub fn insert_alias(
        &self,
        name: &str,
        model_id: i64,
        description: Option<&str>,
    ) -> anyhow::Result<i64> {
        let id = queries::insert_alias(&self.conn, name, model_id, description)?;
        Ok(id)
    }

    /// Update an existing alias.
    pub fn update_alias(&self, id: i64, update: queries::AliasUpdate) -> anyhow::Result<()> {
        queries::update_alias(&self.conn, id, update)?;
        Ok(())
    }

    /// Delete an alias by id.
    pub fn delete_alias(&self, id: i64) -> anyhow::Result<()> {
        queries::delete_alias(&self.conn, id)?;
        Ok(())
    }

    // ── Benchmarks ──────────────────────────────────────────────────────

    /// Insert a benchmark result. Returns the new row id.
    pub fn insert_benchmark(&self, params: &BenchmarkParams) -> anyhow::Result<i64> {
        let insert_params = queries::BenchmarkInsertParams {
            model_id: &params.model_id,
            display_name: params.display_name.as_deref(),
            quant: params.quant.as_deref(),
            backend: &params.backend,
            engine: &params.engine,
            pp_sizes_json: &params.pp_sizes_json,
            tg_sizes_json: &params.tg_sizes_json,
            threads_json: params.threads_json.as_deref(),
            ngl_range: params.ngl_range.as_deref(),
            runs: params.runs,
            warmup: params.warmup,
            results_json: &params.results_json,
            load_time_ms: params.load_time_ms,
            vram_used_mib: params.vram_used_mib,
            vram_total_mib: params.vram_total_mib,
            duration_seconds: params.duration_seconds,
            status: &params.status,
            benchmark_type: params.benchmark_type.as_deref(),
        };
        let id = queries::insert_benchmark(&self.conn, &insert_params)?;
        Ok(id)
    }

    /// List all benchmarks ordered by created_at DESC.
    pub fn list_benchmarks(&self) -> anyhow::Result<Vec<queries::BenchmarkRow>> {
        queries::list_benchmarks(&self.conn)
    }

    /// Delete a benchmark by id.
    pub fn delete_benchmark(&self, id: i64) -> anyhow::Result<()> {
        queries::delete_benchmark(&self.conn, id)?;
        Ok(())
    }

    // ── Download Queue ──────────────────────────────────────────────────

    // ── Update Checks ───────────────────────────────────────────────────

    /// Get all update check records.
    pub fn get_all_update_checks(&self) -> anyhow::Result<Vec<queries::UpdateCheckRecord>> {
        queries::get_all_update_checks(&self.conn)
    }

    /// Delete an update check by item_type and item_id.
    pub fn delete_update_check(&self, item_type: &str, item_id: &str) -> anyhow::Result<()> {
        queries::delete_update_check(&self.conn, item_type, item_id)?;
        Ok(())
    }

    /// Delete update check records matching a SQL LIKE pattern.
    pub fn delete_update_checks_by_pattern(
        &self,
        item_type: &str,
        item_id_pattern: &str,
    ) -> anyhow::Result<()> {
        queries::delete_update_checks_by_pattern(&self.conn, item_type, item_id_pattern)?;
        Ok(())
    }

    /// Delete all update check records for a backend name, covering every
    /// gpu_variant (`name:%`) plus the legacy variant-less row (`name`).
    pub fn delete_update_checks_for_backend(&self, name: &str) -> anyhow::Result<()> {
        queries::delete_update_checks_for_backend(&self.conn, name)?;
        Ok(())
    }

    // ── Model writes ── Model-domain write methods absorbed from ModelManager.
    // These are the subset the `tama` API layer needs; queue/active-model/lifecycle
    // methods stay on ModelManager for tama-core-internal use only.

    /// Convenience method to save a ModelConfig as a DB record.
    ///
    /// Converts config_key to repo_id, converts ModelConfig → ModelConfigRecord,
    /// sets api_name default, and upserts. Returns the model id.
    pub fn save_model_config(
        &self,
        config_key: &str,
        mc: &crate::config::ModelConfig,
    ) -> anyhow::Result<i64> {
        let repo_id = crate::models::config_key_to_repo_id(config_key);
        let mut record = mc.to_db_record(&repo_id);
        if record.api_name.as_deref().is_none_or(str::is_empty) {
            record.api_name = Some(repo_id.clone());
        }
        queries::upsert_model_config(&self.conn, &record)
    }

    /// Get all stored file records for a model.
    pub fn get_files(&self, model_id: i64) -> anyhow::Result<Vec<queries::ModelFileRecord>> {
        queries::get_model_files(&self.conn, model_id)
            .with_context(|| format!("Failed to get model files for id={}", model_id))
    }

    /// Insert or update a model file record.
    pub fn upsert_file(
        &self,
        model_id: i64,
        repo_id: &str,
        filename: &str,
        quant: Option<&str>,
        lfs_oid: Option<&str>,
        size_bytes: Option<i64>,
    ) -> anyhow::Result<()> {
        queries::upsert_model_file(
            &self.conn, model_id, repo_id, filename, quant, lfs_oid, size_bytes,
        )
    }

    /// Delete a single model file record by (model_id, filename).
    pub fn delete_file(&self, model_id: i64, filename: &str) -> anyhow::Result<()> {
        queries::delete_model_file(&self.conn, model_id, filename).with_context(|| {
            format!(
                "Failed to delete model file for id={} filename={}",
                model_id, filename
            )
        })
    }

    /// Insert or update the pull record for a model.
    pub fn upsert_pull(
        &self,
        model_id: i64,
        repo_id: &str,
        commit_sha: &str,
    ) -> anyhow::Result<()> {
        queries::upsert_model_pull(&self.conn, model_id, repo_id, commit_sha)
    }

    /// Get the stored pull record for a model. Returns None if never pulled.
    pub fn get_pull(&self, model_id: i64) -> anyhow::Result<Option<queries::ModelPullRecord>> {
        queries::get_model_pull(&self.conn, model_id)
            .with_context(|| format!("Failed to get pull record for id={}", model_id))
    }

    /// Delete the model configuration by id. CASCADE deletes model_pulls and model_files.
    pub fn delete_config(&self, id: i64) -> anyhow::Result<()> {
        queries::delete_model_config(&self.conn, id)
            .with_context(|| format!("Failed to delete model config for id={}", id))
    }

    /// Update the verification columns for a single file.
    pub fn update_verification(
        &self,
        model_id: i64,
        filename: &str,
        verified_ok: Option<bool>,
        verify_error: Option<&str>,
    ) -> anyhow::Result<()> {
        queries::update_verification(&self.conn, model_id, filename, verified_ok, verify_error)
    }
}

// Re-export internal query types used by the Repository
use crate::db::queries;

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, OpenResult};

    /// Helper: open an in-memory database and return a Repository.
    fn test_repo() -> Repository {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        Repository { conn }
    }

    /// Helper: insert a model config record and return its id.
    fn insert_model_config(
        conn: &Connection,
        repo_id: &str,
        display_name: Option<&str>,
        backend: &str,
    ) -> i64 {
        let record = queries::ModelConfigRecord {
            id: 0,
            repo_id: repo_id.to_string(),
            display_name: display_name.map(String::from),
            backend: backend.to_string(),
            gpu_variant: None,
            gpu_device: None,
            enabled: true,
            selected_quant: None,
            selected_mmproj: None,
            selected_mtp_model: None,
            context_length: None,
            num_parallel: None,
            kv_unified: false,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: Some(repo_id.to_string()),
            health_check: None,
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            spec_decoding: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
            n_batch: None,
            n_ubatch: None,
        };
        queries::upsert_model_config(conn, &record).unwrap()
    }

    // ── Alias CRUD ─────────────────────────────────────────────────────────

    #[test]
    fn test_alias_crud_round_trip() {
        use rusqlite::params;
        let repo = test_repo();

        // Insert a model config first (alias references it via FK)
        let model_id: i64 = repo.conn.query_row(
            "INSERT INTO model_configs (repo_id, backend, api_name) VALUES (?1, ?2, ?3) RETURNING id",
            params!["test-org/test-model", "llama_cpp", "test"],
            |row| row.get(0),
        ).unwrap();

        // Act: insert alias
        let alias_id = repo
            .insert_alias("test-alias", model_id, Some("A test alias"))
            .unwrap();
        assert!(alias_id > 0);

        // Assert: retrieve by id
        let alias = repo.get_alias_by_id(alias_id).unwrap().unwrap();
        assert_eq!(alias.name, "test-alias");
        assert_eq!(alias.model_id, model_id);
        assert_eq!(alias.description, Some("A test alias".to_string()));

        // Act: update alias (use same model_id since model_id+1 doesn't exist)
        repo.update_alias(
            alias_id,
            queries::AliasUpdate {
                name: Some("renamed"),
                model_id: Some(model_id),
                ..Default::default()
            },
        )
        .unwrap();

        // Assert: updated values
        let alias = repo.get_alias_by_id(alias_id).unwrap().unwrap();
        assert_eq!(alias.name, "renamed");
        assert_eq!(alias.model_id, model_id); // unchanged model_id
        assert_eq!(alias.description, Some("A test alias".to_string())); // unchanged

        // Assert: visible in get_all
        let all = repo.get_all_aliases().unwrap();
        assert!(all.iter().any(|a| a.id == alias_id));

        // Act: delete
        repo.delete_alias(alias_id).unwrap();

        // Assert: gone
        assert!(repo.get_alias_by_id(alias_id).unwrap().is_none());
    }

    #[test]
    fn test_alias_not_found() {
        let repo = test_repo();
        let result = repo.get_alias_by_id(999).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_all_aliases_empty() {
        let repo = test_repo();
        let aliases = repo.get_all_aliases().unwrap();
        assert!(aliases.is_empty());
    }

    // ── Benchmark CRUD ─────────────────────────────────────────────────────

    #[test]
    fn test_benchmark_crud_round_trip() {
        let repo = test_repo();

        let params = BenchmarkParams {
            model_id: "test-model".to_string(),
            display_name: Some("Test Model".to_string()),
            quant: Some("Q4_K_M".to_string()),
            backend: "llama_cpp".to_string(),
            engine: "llama_bench".to_string(),
            pp_sizes_json: "[512,1024]".to_string(),
            tg_sizes_json: "[128,256]".to_string(),
            threads_json: Some("[8]".to_string()),
            ngl_range: None,
            runs: 3,
            warmup: 1,
            results_json: "[{\"pp\":100}]".to_string(),
            load_time_ms: Some(1500.0),
            vram_used_mib: Some(4096),
            vram_total_mib: Some(8192),
            duration_seconds: 30.5,
            status: "success".to_string(),
            benchmark_type: Some("baseline".to_string()),
        };

        // Insert
        let id = repo.insert_benchmark(&params).unwrap();
        assert!(id > 0);

        // List
        let benchmarks = repo.list_benchmarks().unwrap();
        assert_eq!(benchmarks.len(), 1);
        let b = &benchmarks[0];
        assert_eq!(b.id, id);
        assert_eq!(b.model_id, "test-model");
        assert_eq!(b.backend, "llama_cpp");
        assert_eq!(b.quant, Some("Q4_K_M".to_string()));
        assert_eq!(b.runs, 3);
        assert_eq!(b.duration_seconds, 30.5);

        // Delete
        repo.delete_benchmark(id).unwrap();

        // Verify gone
        let benchmarks = repo.list_benchmarks().unwrap();
        assert!(benchmarks.is_empty());
    }

    #[test]
    fn test_list_benchmarks_empty() {
        let repo = test_repo();
        let benchmarks = repo.list_benchmarks().unwrap();
        assert!(benchmarks.is_empty());
    }

    // ── Update Checks ──────────────────────────────────────────────────────

    #[test]
    fn test_delete_update_check() {
        let repo = test_repo();

        // Insert directly via queries
        queries::upsert_update_check(
            &repo.conn,
            queries::UpdateCheckParams {
                item_type: "backend",
                item_id: "llama-cpp",
                current_version: Some("v0.1.0"),
                latest_version: Some("v0.2.0"),
                update_available: true,
                status: "update_available",
                error_message: None,
                details_json: None,
                checked_at: 1713168000,
            },
        )
        .unwrap();

        // Verify present
        let checks = repo.get_all_update_checks().unwrap();
        assert_eq!(checks.len(), 1);

        // Delete
        repo.delete_update_check("backend", "llama-cpp").unwrap();

        // Verify gone
        let checks = repo.get_all_update_checks().unwrap();
        assert!(checks.is_empty());
    }

    #[test]
    fn test_delete_update_checks_by_pattern() {
        let repo = test_repo();

        // Insert multiple records
        queries::upsert_update_check(
            &repo.conn,
            queries::UpdateCheckParams {
                item_type: "backend",
                item_id: "llama_cpp:cpu",
                current_version: None,
                latest_version: None,
                update_available: false,
                status: "unknown",
                error_message: None,
                details_json: None,
                checked_at: 1000,
            },
        )
        .unwrap();

        queries::upsert_update_check(
            &repo.conn,
            queries::UpdateCheckParams {
                item_type: "backend",
                item_id: "llama_cpp:cuda",
                current_version: None,
                latest_version: None,
                update_available: false,
                status: "unknown",
                error_message: None,
                details_json: None,
                checked_at: 1001,
            },
        )
        .unwrap();

        queries::upsert_update_check(
            &repo.conn,
            queries::UpdateCheckParams {
                item_type: "backend",
                item_id: "vulkan:cpu",
                current_version: None,
                latest_version: None,
                update_available: false,
                status: "unknown",
                error_message: None,
                details_json: None,
                checked_at: 1002,
            },
        )
        .unwrap();

        // Delete all llama_cpp variants
        repo.delete_update_checks_by_pattern("backend", "llama_cpp:%")
            .unwrap();

        // Verify llama_cpp records are gone
        let checks = repo.get_all_update_checks().unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].item_id, "vulkan:cpu");
    }

    // ── Model Configs ──────────────────────────────────────────────────────

    #[test]
    fn test_load_model_configs_empty() {
        let repo = test_repo();
        let configs = repo.load_model_configs().unwrap();
        assert!(configs.is_empty());
    }

    #[test]
    fn test_load_model_configs_returns_inserted() {
        let repo = test_repo();

        // Insert via queries (Repository has no insert method)
        let id = insert_model_config(
            &repo.conn,
            "test-org/test-model",
            Some("Test Model"),
            "llama_cpp",
        );

        // Load via Repository
        let configs = repo.load_model_configs().unwrap();
        assert_eq!(configs.len(), 1);

        let key = "test-org--test-model";
        let config = configs.get(key).unwrap();
        assert_eq!(config.id, id);
        assert_eq!(config.repo_id, "test-org/test-model");
        assert_eq!(config.display_name, Some("Test Model".to_string()));
        assert_eq!(config.backend, "llama_cpp");
        assert!(config.enabled);
    }

    #[test]
    fn test_load_model_configs_multiple() {
        let repo = test_repo();

        insert_model_config(&repo.conn, "org/model-a", Some("Model A"), "llama_cpp");
        insert_model_config(&repo.conn, "org/model-b", Some("Model B"), "vulkan");

        let configs = repo.load_model_configs().unwrap();
        assert_eq!(configs.len(), 2);
        assert!(configs.contains_key("org--model-a"));
        assert!(configs.contains_key("org--model-b"));
    }

    // ── Model Files ────────────────────────────────────────────────────────

    #[test]
    fn test_get_model_files_empty_for_unknown_config() {
        let repo = test_repo();
        let files = repo.get_model_files(999).unwrap();
        assert!(files.is_empty());
    }

    // ── Repository::open() ─────────────────────────────────────────────────

    #[test]
    fn test_repository_open_and_model_exists() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let repo = Repository::open(dir.path()).unwrap();

        // Fresh DB: no models exist
        assert!(!repo.model_exists(1).unwrap());
        assert!(!repo.model_exists(0).unwrap());
    }

    // ── Model writes (absorbed from ModelManager) ──────────────────────────

    #[test]
    fn test_save_model_config_round_trip() {
        let repo = test_repo();
        let mc = crate::config::ModelConfig::default();
        let id = repo.save_model_config("owner--repo", &mc).unwrap();
        assert!(id > 0);
        let record = repo.get_model_config(id).unwrap().unwrap();
        assert_eq!(record.repo_id, "owner/repo");
        assert_eq!(record.api_name.as_deref(), Some("owner/repo"));
    }

    #[test]
    fn test_upsert_and_get_pull() {
        let repo = test_repo();
        let id = insert_model_config(&repo.conn, "owner/repo", Some("Test"), "llama_cpp");
        repo.upsert_pull(id, "owner/repo", "abc123").unwrap();
        let pull = repo.get_pull(id).unwrap().unwrap();
        assert_eq!(pull.commit_sha, "abc123");
    }

    #[test]
    fn test_upsert_file_and_delete_file() {
        let repo = test_repo();
        let id = insert_model_config(&repo.conn, "owner/repo", Some("Test"), "llama_cpp");
        repo.upsert_file(
            id,
            "owner/repo",
            "m-q4.gguf",
            Some("Q4_K_M"),
            None,
            Some(123),
        )
        .unwrap();
        let files = repo.get_files(id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "m-q4.gguf");
        repo.delete_file(id, "m-q4.gguf").unwrap();
        assert!(repo.get_files(id).unwrap().is_empty());
    }

    #[test]
    fn test_delete_config_cascades() {
        let repo = test_repo();
        let id = insert_model_config(&repo.conn, "owner/repo", Some("Test"), "llama_cpp");
        repo.upsert_file(
            id,
            "owner/repo",
            "m-q4.gguf",
            Some("Q4_K_M"),
            None,
            Some(123),
        )
        .unwrap();
        assert!(repo.get_model_config(id).unwrap().is_some());
        repo.delete_config(id).unwrap();
        assert!(repo.get_model_config(id).unwrap().is_none());
        assert!(repo.get_files(id).unwrap().is_empty());
    }
}
