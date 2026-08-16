//! Repository layer — domain-level database operations for the API layer.
//!
//! This module provides a `Repository` struct that wraps SQLite connections
//! and exposes domain-level methods. API handlers call repository methods
//! instead of raw queries, and receive the canonical DB record types from
//! `db::queries` directly.
//!
//! Transitional (plan-190): the model/alias/provider query methods moved to
//! Postgres in Task 5 — API handlers now use `WebState.db_pool` directly with
//! the async `db::queries` functions. The methods remaining here (benchmarks,
//! pull queue, tamads) stay on SQLite until Tasks 7-8 and this module is
//! deleted in Task 9.

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
    /// Suite identifier for grouping related benchmark runs.
    pub suite_id: Option<String>,
}

// ── Repository ───────────────────────────────────────────────────────────────

/// Domain-level database access for API handlers.
///
/// Wraps a SQLite connection and provides high-level methods for
/// benchmarks, pull queue, and update checks. Model/alias/provider access is
/// pool-based via `db::queries` (plan-190 Task 5).
#[derive(Debug)]
pub struct Repository {
    pub(crate) conn: Connection,
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
            suite_id: params.suite_id.as_deref(),
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

    // ── Tamad CRUD ───────────────────────────────────────────────────────

    /// Insert a new tamad connection.
    pub fn insert_tamad(
        &self,
        id: &str,
        name: &str,
        url: &str,
        protocol: &str,
        token: Option<&str>,
    ) -> anyhow::Result<()> {
        queries::insert_tamad(&self.conn, id, name, url, protocol, token)
            .with_context(|| format!("Failed to insert tamad '{}'", name))
    }

    /// Get a tamad connection by id.
    pub fn get_tamad(&self, id: &str) -> anyhow::Result<Option<crate::providers::TamadConnection>> {
        queries::get_tamad(&self.conn, id).with_context(|| "Failed to get tamad")
    }

    /// List all tamad connections.
    pub fn list_tamads(&self) -> anyhow::Result<Vec<crate::providers::TamadConnection>> {
        queries::list_tamads(&self.conn).with_context(|| "Failed to list tamads")
    }

    /// Update a tamad connection's url and/or token.
    pub fn update_tamad(&self, id: &str, url: &str, token: Option<&str>) -> anyhow::Result<()> {
        queries::update_tamad(&self.conn, id, url, token)
            .with_context(|| format!("Failed to update tamad '{}'", id))
    }

    /// Delete a tamad connection by id. Returns true if a row was deleted.
    pub fn delete_tamad(&self, id: &str) -> anyhow::Result<bool> {
        queries::delete_tamad(&self.conn, id)
            .with_context(|| format!("Failed to delete tamad '{}'", id))
    }

    /// Update only the status of a tamad connection.
    pub fn update_tamad_status(&self, id: &str, status: &str) -> anyhow::Result<()> {
        queries::update_tamad_status(&self.conn, id, status)
            .with_context(|| format!("Failed to update tamad status '{}'", id))
    }
}

// Re-export internal query types used by the Repository
use crate::db::queries;

// ── Tests ────────────────────────────────────────────────────────────────────
//
// The model/alias CRUD tests moved to the Postgres harness (plan-190 Task 5):
// crates/tama-core/tests/{model_config_queries,alias_queries,provider_queries}.rs

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: open an in-memory database and return a Repository.
    fn test_repo() -> Repository {
        let open_result = crate::db::open_in_memory().unwrap();
        Repository {
            conn: open_result.conn,
        }
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
            suite_id: None,
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
}
