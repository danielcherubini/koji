pub const MIGRATION: super::Migration = (
    41,
    false,
    r#"ALTER TABLE model_configs ADD COLUMN n_batch INTEGER;
       ALTER TABLE model_configs ADD COLUMN n_ubatch INTEGER;"#,
);
