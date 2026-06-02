/// v8 — Rebuild model tables with integer PK and FK references
pub const MIGRATION: (i32, bool, &str) = (
    8,
    false,
    r#"
        -- Destructive migration: drop and recreate model tables with integer PK.
        -- All tables now use `model_id INTEGER REFERENCES models(id)` instead of repo_id.
        DROP TABLE IF EXISTS model_configs;
        DROP TABLE IF EXISTS model_files;
        DROP TABLE IF EXISTS model_pulls;

        -- Per-repo user configuration (replaces [models] in tama.toml)
        CREATE TABLE model_configs (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_id       TEXT NOT NULL UNIQUE,           -- HF repo name, e.g. "unsloth/gemma-4-26B-A4B-it-GGUF"
            display_name  TEXT,
            backend       TEXT NOT NULL DEFAULT 'llama_cpp',
            enabled       INTEGER NOT NULL DEFAULT 1,
            selected_quant  TEXT,                          -- active quant key (e.g. "Q4_K_M")
            selected_mmproj TEXT,                          -- mmproj filename (e.g. "mmproj-F16.gguf")
            context_length  INTEGER,
            gpu_layers      INTEGER,
            port            INTEGER,
            args            TEXT,                          -- JSON array of strings
            sampling        TEXT,                          -- JSON object (serialised SamplingParams)
            modalities      TEXT,                          -- JSON object {input:[],output:[]}
            profile         TEXT,
            api_name        TEXT,
            health_check    TEXT,                          -- JSON object (serialised HealthCheck)
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        -- Tracks HuggingFace repo state at time of pull
        CREATE TABLE model_pulls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            model_id   INTEGER NOT NULL REFERENCES model_configs(id) ON DELETE CASCADE,
            repo_id    TEXT NOT NULL,                              -- cached for convenience
            commit_sha TEXT NOT NULL,                              -- HF repo HEAD commit hash
            pulled_at  TEXT NOT NULL                               -- ISO 8601 timestamp
        );

        -- Tracks per-file metadata for downloaded GGUFs
        CREATE TABLE model_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            model_id   INTEGER NOT NULL REFERENCES model_configs(id) ON DELETE CASCADE,
            repo_id    TEXT NOT NULL,                              -- cached for convenience
            filename   TEXT NOT NULL,                              -- e.g. "gemma-4-26B-Q4_K_M.gguf"
            quant      TEXT,                                       -- e.g. "Q4_K_M"
            lfs_oid    TEXT,                                       -- LFS SHA256 content hash
            size_bytes INTEGER,                                    -- file size in bytes
            downloaded_at     TEXT NOT NULL,                       -- ISO 8601 timestamp
            last_verified_at   TEXT,                                 -- ISO 8601, cleared on hash change
            verified_ok       INTEGER,                               -- 1=ok, 0=mismatch, NULL=never verified
            verify_error      TEXT,                                  -- short error message
            kind TEXT NOT NULL DEFAULT 'model'                      -- 'model' or 'mmproj'
        );
        CREATE UNIQUE INDEX idx_model_files_model_id_filename ON model_files(model_id, filename);
    "#,
);
