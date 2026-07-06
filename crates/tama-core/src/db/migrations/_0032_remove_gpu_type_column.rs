//! Migration 32: Remove `gpu_type` column from `backend_installations`.
//!
//! The `gpu_type` column (storing JSON-serialized GpuType) is redundant with
//! `gpu_variant` (storing the folder name like "cpu", "cuda", "vulkan").
//! This migration drops the `gpu_type` column to clean up the schema.
//!
//! The `gpu_variant` column is preserved as it holds the canonical domain term.

pub const MIGRATION: (i32, bool, &str) = (
    32,
    false,
    "ALTER TABLE backend_installations DROP COLUMN gpu_type;",
);

#[cfg(test)]
mod tests {
    use crate::db::open_in_memory;

    #[test]
    fn test_migration_removes_gpu_type_column() {
        let open_result = open_in_memory().unwrap();
        let conn = &open_result.conn;

        // After running all migrations (including 0032), gpu_type should be gone
        let mut stmt = conn
            .prepare("PRAGMA table_info(backend_installations)")
            .expect("prepare should succeed");
        let columns: Vec<String> = stmt
            .query_map([], |row: &rusqlite::Row| Ok(row.get::<_, String>(1)?))
            .expect("query should succeed")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect should succeed");

        let columns: Vec<&str> = columns.iter().map(|s| s.as_str()).collect();
        assert!(
            !columns.contains(&"gpu_type"),
            "gpu_type column should have been dropped by migration 0032"
        );
        assert!(
            columns.contains(&"gpu_variant"),
            "gpu_variant column should still exist after migration 0032"
        );
    }
}
