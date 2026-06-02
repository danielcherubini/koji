/// v20 — Rebuild backend_installations with gpu_variant (FK_OFF)
pub const MIGRATION: (i32, bool, &str) = (
    20,
    true,
    r#"
        -- Rebuild backend_installations with gpu_variant column and
        -- UNIQUE(name, gpu_variant, version) constraint. This allows
        -- multiple GPU variants (cpu, vulkan, cuda, rocm, metal) to
        -- coexist for the same backend name.
        --
        -- Uses DROP + RENAME pattern (FK_OFF_MIGRATIONS) because we
        -- need to change the UNIQUE constraint, which requires recreating
        -- the table.

        CREATE TABLE backend_installations_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            backend_type TEXT NOT NULL,
            version TEXT NOT NULL,
            path TEXT NOT NULL,
            installed_at INTEGER NOT NULL,
            gpu_type TEXT,
            gpu_variant TEXT NOT NULL DEFAULT 'cpu',
            source TEXT,
            is_active INTEGER NOT NULL DEFAULT 0,
            UNIQUE(name, gpu_variant, version)
        );

        INSERT INTO backend_installations_new (
            id, name, backend_type, version, path, installed_at,
            gpu_type, gpu_variant, source, is_active
        )
        SELECT
            id, name, backend_type, version, path, installed_at,
            gpu_type, 'cpu', source, is_active
        FROM backend_installations;

        DROP TABLE backend_installations;
        ALTER TABLE backend_installations_new RENAME TO backend_installations;
        CREATE INDEX idx_backend_installations_name ON backend_installations(name);
        CREATE INDEX idx_backend_installations_name_variant ON backend_installations(name, gpu_variant);
    "#,
);
