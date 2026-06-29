/// v31 — Create app config tables (app_general, app_proxy, app_supervisor, app_compaction, sampling_templates)
pub const MIGRATION: (i32, bool, &str) = (
    31,
    false,
    r#"
        CREATE TABLE IF NOT EXISTS app_general (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            log_level TEXT NOT NULL DEFAULT 'info',
            models_dir TEXT,
            logs_dir TEXT,
            hf_token TEXT,
            update_check_interval INTEGER NOT NULL DEFAULT 12
        );

        CREATE TABLE IF NOT EXISTS app_proxy (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            host TEXT NOT NULL DEFAULT '0.0.0.0',
            port INTEGER NOT NULL DEFAULT 11434,
            auto_unload INTEGER NOT NULL DEFAULT 0,
            idle_timeout_secs INTEGER NOT NULL DEFAULT 300,
            startup_timeout_secs INTEGER NOT NULL DEFAULT 120,
            circuit_breaker_threshold INTEGER NOT NULL DEFAULT 3,
            circuit_breaker_cooldown_seconds INTEGER NOT NULL DEFAULT 60,
            metrics_retention_secs INTEGER NOT NULL DEFAULT 86400,
            download_queue_poll_interval_secs INTEGER NOT NULL DEFAULT 2,
            max_loaded_models INTEGER NOT NULL DEFAULT 1,
            authenticator_url TEXT,
            authenticator_skip_paths TEXT NOT NULL DEFAULT '["/health","/metrics"]'
        );

        CREATE TABLE IF NOT EXISTS app_supervisor (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            restart_policy TEXT NOT NULL DEFAULT 'always',
            max_restarts INTEGER NOT NULL DEFAULT 10,
            restart_delay_ms INTEGER NOT NULL DEFAULT 3000,
            health_check_interval_ms INTEGER NOT NULL DEFAULT 5000,
            health_check_timeout_ms INTEGER NOT NULL DEFAULT 30000,
            health_check_retries INTEGER NOT NULL DEFAULT 3
        );

        CREATE TABLE IF NOT EXISTS app_compaction (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            enabled INTEGER NOT NULL DEFAULT 0,
            server_path TEXT,
            device TEXT NOT NULL DEFAULT 'cpu',
            port INTEGER,
            request_timeout_ms INTEGER NOT NULL DEFAULT 30000
        );

        CREATE TABLE IF NOT EXISTS sampling_templates (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL,
            temperature REAL,
            top_k INTEGER,
            top_p REAL,
            min_p REAL,
            presence_penalty REAL,
            frequency_penalty REAL,
            repeat_penalty REAL
        );
    "#,
);
