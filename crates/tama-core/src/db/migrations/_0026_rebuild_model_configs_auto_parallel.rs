/// v26 — Rebuild model_configs to allow num_parallel=0 (FK_OFF)
pub const MIGRATION: (i32, bool, &str) = (
    26,
    true,
    r#"
        -- Rebuild model_configs to allow num_parallel=0 (auto/not set).
        -- Previously CHECK(num_parallel >= 1) prevented 0, which meant
        -- users couldn't explicitly set -np 1 (0 was needed for "auto").
        --
        -- Uses DROP + RENAME pattern (FK_OFF_MIGRATIONS) because we need
        -- to change the CHECK constraint, which requires recreating the table.

        CREATE TABLE model_configs_new (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_id       TEXT NOT NULL UNIQUE COLLATE NOCASE,
            display_name  TEXT,
            backend       TEXT NOT NULL DEFAULT 'llama_cpp',
            gpu_variant   TEXT,
            enabled       INTEGER NOT NULL DEFAULT 1,
            selected_quant  TEXT,
            selected_mmproj TEXT,
            context_length  INTEGER,
            num_parallel    INTEGER DEFAULT 0 CHECK(num_parallel >= 0),
            kv_unified      INTEGER NOT NULL DEFAULT 0,
            gpu_layers      INTEGER,
            cache_type_k    TEXT,
            cache_type_v    TEXT,
            port            INTEGER,
            args            TEXT,
            sampling        TEXT,
            modalities      TEXT,
            profile         TEXT,
            api_name        TEXT,
            health_check    TEXT,
            hf_format       TEXT,
            hf_base_model   TEXT,
            hf_pipeline_tag TEXT,
            hf_total_params TEXT,
            hf_active_params TEXT,
            hf_architecture_type TEXT,
            hf_context_length INTEGER,
            hf_num_layers   INTEGER,
            hf_last_modified TEXT,
            spec_decoding   TEXT,
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        INSERT INTO model_configs_new (
            id, repo_id, display_name, backend, gpu_variant, enabled,
            selected_quant, selected_mmproj, context_length, num_parallel,
            kv_unified, gpu_layers, cache_type_k, cache_type_v, port, args,
            sampling, modalities, profile, api_name, health_check,
            hf_format, hf_base_model, hf_pipeline_tag, hf_total_params,
            hf_active_params, hf_architecture_type, hf_context_length,
            hf_num_layers, hf_last_modified, spec_decoding,
            created_at, updated_at
        )
        SELECT
            id, repo_id, display_name, backend, gpu_variant, enabled,
            selected_quant, selected_mmproj, context_length, num_parallel,
            kv_unified, gpu_layers, cache_type_k, cache_type_v, port, args,
            sampling, modalities, profile, api_name, health_check,
            hf_format, hf_base_model, hf_pipeline_tag, hf_total_params,
            hf_active_params, hf_architecture_type, hf_context_length,
            hf_num_layers, hf_last_modified, spec_decoding,
            created_at, updated_at
        FROM model_configs;

        DROP TABLE model_configs;
        ALTER TABLE model_configs_new RENAME TO model_configs;
    "#,
);
