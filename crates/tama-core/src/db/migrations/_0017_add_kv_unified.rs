/// v17 — Add kv_unified to model_configs
pub const MIGRATION: (i32, bool, &str) = (
    17,
    false,
    r#"
        ALTER TABLE model_configs ADD COLUMN kv_unified INTEGER NOT NULL DEFAULT 0;
        UPDATE model_configs SET kv_unified = 1 WHERE num_parallel IS NULL OR num_parallel <= 1;
    "#,
);
