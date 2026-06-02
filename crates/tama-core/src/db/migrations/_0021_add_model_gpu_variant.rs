/// v21 — Add gpu_variant to model_configs
pub const MIGRATION: (i32, bool, &str) = (
    21,
    false,
    r#"
        -- Per-model GPU variant selection (e.g. "rocm", "vulkan", "cuda").
        -- When set, overrides the global [backends.<name>].gpu_variant config.
        ALTER TABLE model_configs ADD COLUMN gpu_variant TEXT;
        CREATE INDEX IF NOT EXISTS idx_model_configs_backend_variant
            ON model_configs(backend, gpu_variant);
    "#,
);
