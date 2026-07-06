/// v33 — Add default_env column to backend_configs
pub const MIGRATION: (i32, bool, &str) = (
    33,
    false,
    r#"
        ALTER TABLE backend_configs ADD COLUMN default_env TEXT;
    "#,
);
