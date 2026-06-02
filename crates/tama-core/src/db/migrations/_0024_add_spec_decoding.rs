/// v24 — Add spec_decoding to model_configs
pub const MIGRATION: (i32, bool, &str) = (
    24,
    false,
    r#"
        ALTER TABLE model_configs ADD COLUMN spec_decoding TEXT;
    "#,
);
