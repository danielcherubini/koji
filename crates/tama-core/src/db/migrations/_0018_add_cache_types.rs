/// v18 — Add cache_type_k and cache_type_v to model_configs
pub const MIGRATION: (i32, bool, &str) = (
    18,
    false,
    r#"
        ALTER TABLE model_configs ADD COLUMN cache_type_k TEXT;
        ALTER TABLE model_configs ADD COLUMN cache_type_v TEXT;
    "#,
);
