//! Repository layer — domain-level database operations for the API layer.
//!
//! Transitional (plan-190): the model/alias/provider query methods moved to
//! Postgres in Task 5, the pull queue/tamad queries moved to Postgres in
//! Task 7, and the benchmark queries moved to Postgres in Task 8 — API
//! handlers now use `WebState.db_pool` directly with the async `db::queries`
//! functions. This module is deleted in Task 9.

use rusqlite::Connection;
use std::path::Path;

/// Domain-level database access for API handlers.
///
/// Wraps a SQLite connection. Benchmark access moved to the Postgres
/// `db::queries` functions in Task 8; model/alias/provider and
/// pull queue/tamad access is pool-based as well (plan-190 Tasks 5 and 7).
#[derive(Debug)]
pub struct Repository {
    pub(crate) conn: Connection,
}

// Manual Clone impl: rusqlite::Connection is not Clone, but
// WebState derives Clone and held Option<Arc<Mutex<Repository>>>.
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

    /// Check if a model config exists by its integer id.
    pub fn model_exists(&self, id: i64) -> anyhow::Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM model_configs WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// The model/alias CRUD tests moved to the Postgres harness (plan-190 Task 5)
// and the benchmark CRUD tests moved to the Postgres harness (plan-190
// Task 8): crates/tama-core/src/db/queries/benchmark_queries.rs.

#[cfg(test)]
mod tests {
    use super::*;

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
