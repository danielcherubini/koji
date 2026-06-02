/// v14 — Add num_parallel to model_configs
pub const MIGRATION: (i32, bool, &str) = (
    14,
    false,
    r#"
        ALTER TABLE model_configs ADD COLUMN num_parallel INTEGER NOT NULL DEFAULT 1 CHECK(num_parallel >= 1);
    "#,
);
