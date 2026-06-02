/// v7 — Create model_configs table and add kind column to model_files
pub const MIGRATION: (i32, bool, &str) = (
    7,
    false,
    r#"
        -- Per-repo user configuration (replaces [models] in tama.toml)
        CREATE TABLE IF NOT EXISTS model_configs (
            repo_id       TEXT PRIMARY KEY,
            display_name  TEXT,
            backend       TEXT NOT NULL DEFAULT 'llama_cpp',
            enabled       INTEGER NOT NULL DEFAULT 1,
            selected_quant  TEXT,        -- quant key (e.g. "Q4_K_M"), references model_files.quant
            selected_mmproj TEXT,        -- mmproj filename (e.g. "mmproj-F16.gguf")
            context_length  INTEGER,
            gpu_layers      INTEGER,
            port            INTEGER,
            args            TEXT,        -- JSON array of strings, e.g. '["--flash-attn"]'
            sampling        TEXT,        -- JSON object (serialised SamplingParams), nullable
            modalities      TEXT,        -- JSON object {input:[],output:[]}, nullable
            profile         TEXT,
            api_name        TEXT,
            health_check    TEXT,        -- JSON object (serialised HealthCheck), nullable
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        -- Add file kind so model files and mmproj files are distinguishable
        ALTER TABLE model_files ADD COLUMN kind TEXT NOT NULL DEFAULT 'model';
    "#,
);
