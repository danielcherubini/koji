/// v19 — Add HuggingFace metadata columns to model_configs
pub const MIGRATION: (i32, bool, &str) = (
    19,
    false,
    r#"
        ALTER TABLE model_configs ADD COLUMN hf_format TEXT;
        ALTER TABLE model_configs ADD COLUMN hf_base_model TEXT;
        ALTER TABLE model_configs ADD COLUMN hf_pipeline_tag TEXT;
        ALTER TABLE model_configs ADD COLUMN hf_total_params TEXT;
        ALTER TABLE model_configs ADD COLUMN hf_active_params TEXT;
        ALTER TABLE model_configs ADD COLUMN hf_architecture_type TEXT;
        ALTER TABLE model_configs ADD COLUMN hf_context_length INTEGER;
        ALTER TABLE model_configs ADD COLUMN hf_num_layers INTEGER;
        ALTER TABLE model_configs ADD COLUMN hf_last_modified TEXT;
    "#,
);
