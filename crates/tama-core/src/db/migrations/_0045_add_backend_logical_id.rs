//! Migration v45: introduce a stable logical identity for backends.
//!
//! The prior model keyed backend config (`backend_configs`), model backends
//! (`model_configs.backend`), and runtime state (`active_models.backend`) by
//! the backend's *editable name*. Renaming a backend (`backend_installations.name`)
//! silently orphaned every dependent row — e.g. the `backend_configs` default
//! args/env all disappeared the moment a user renamed "vllm" to "radiance".
//!
//! The `backend_installations.id` cannot be used as the join key either: installing
//! a new version does `INSERT OR REPLACE` keyed on `(name, gpu_variant, version)`,
//! which deletes + re-inserts the row and therefore issues a *new id*.
//!
//! This migration adds a stable `logical_id` (a UUID assigned once per logical
//! backend name and preserved across renames and version upgrades) to
//! `backend_installations`, and adds stable-key columns to `backend_configs`,
//! `model_configs`, and `active_models`. The actual UUID assignment + backfill
//! happens in Rust (`db::backfill_backend_logical_ids`) because SQLite cannot
//! generate per-name-stable UUIDs.
//!
//! `backend_configs` is rebuilt so its uniqueness is scoped to
//! `(logical_id, gpu_variant)` instead of the editable `(name, gpu_variant)`.

pub const MIGRATION: (i32, bool, &str) = (
    45,
    false, // no table referenced by FK is dropped (backend_configs has no inbound FKs)
    r#"
        ALTER TABLE backend_installations ADD COLUMN logical_id TEXT;

        CREATE TABLE backend_configs_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            logical_id TEXT,
            name TEXT NOT NULL,
            gpu_variant TEXT NOT NULL DEFAULT 'cpu',
            default_args TEXT,
            health_check_url TEXT,
            default_env TEXT,
            UNIQUE(logical_id, gpu_variant)
        );

        INSERT INTO backend_configs_new
            (id, logical_id, name, gpu_variant, default_args, health_check_url, default_env)
        SELECT id, NULL, name, gpu_variant, default_args, health_check_url, default_env
        FROM backend_configs;

        DROP TABLE backend_configs;
        ALTER TABLE backend_configs_new RENAME TO backend_configs;

        CREATE INDEX idx_backend_configs_name_variant ON backend_configs(name, gpu_variant);
        CREATE INDEX idx_backend_configs_logical_variant ON backend_configs(logical_id, gpu_variant);

        ALTER TABLE model_configs ADD COLUMN backend_id TEXT;
        ALTER TABLE active_models ADD COLUMN backend_id TEXT;
    "#,
);

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn has_column(conn: &Connection, table: &str, col: &str) -> bool {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                (table, col),
                |r| r.get(0),
            )
            .unwrap();
        n > 0
    }

    #[test]
    fn test_migration_v45_adds_logical_id_columns() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::run_up_to(&conn, 44).unwrap();

        // Not present before
        assert!(!has_column(&conn, "backend_installations", "logical_id"));
        assert!(!has_column(&conn, "backend_configs", "logical_id"));
        assert!(!has_column(&conn, "model_configs", "backend_id"));
        assert!(!has_column(&conn, "active_models", "backend_id"));

        super::super::run_up_to(&conn, 45).unwrap();

        assert!(has_column(&conn, "backend_installations", "logical_id"));
        assert!(has_column(&conn, "backend_configs", "logical_id"));
        assert!(has_column(&conn, "model_configs", "backend_id"));
        assert!(has_column(&conn, "active_models", "backend_id"));
    }

    #[test]
    fn test_migration_v45_preserves_backend_configs_rows() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::run_up_to(&conn, 44).unwrap();

        // Insert a backend_config row against the v44 schema (has no logical_id)
        conn.execute(
            "INSERT INTO backend_configs (id, name, gpu_variant, default_args, default_env, health_check_url)
             VALUES (1, 'vllm', 'rocm', '[\"--flag\"]', '[\"A=1\"]', 'http://x/health')",
            [],
        )
        .unwrap();

        super::super::run_up_to(&conn, 45).unwrap();

        // Row still exists with all data intact
        let (name, args, env, health): (String, Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT name, default_args, default_env, health_check_url FROM backend_configs WHERE name='vllm'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(name, "vllm");
        assert_eq!(args.as_deref(), Some("[\"--flag\"]"));
        assert_eq!(env.as_deref(), Some("[\"A=1\"]"));
        assert_eq!(health.as_deref(), Some("http://x/health"));
        // logical_id is NULL until the Rust backfill runs
        let has_null: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM backend_configs WHERE name='vllm' AND logical_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_null, 1);
    }
}
