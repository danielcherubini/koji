/// v13 — Create benchmarks table
pub const MIGRATION: (i32, bool, &str) = (
    13,
    false,
    r#"
        -- Stores benchmark results for comparison over time.
        CREATE TABLE IF NOT EXISTS benchmarks (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at          INTEGER NOT NULL,           -- Unix timestamp (seconds)
            model_id            TEXT NOT NULL,              -- Model config key (e.g. "qwen7b")
            display_name        TEXT,                       -- Model display name
            quant               TEXT,                       -- Quantization label (e.g. "Q4_K_M")
            backend             TEXT NOT NULL,              -- Backend type (e.g. "llama_cpp")
            engine              TEXT NOT NULL DEFAULT 'llama_bench',
            pp_sizes            TEXT NOT NULL,              -- JSON array, e.g. "[512,1024]"
            tg_sizes            TEXT NOT NULL,              -- JSON array, e.g. "[128,256]"
            threads             TEXT,                       -- JSON array or null
            ngl_range           TEXT,                       -- GPU layers range or null
            runs                INTEGER NOT NULL DEFAULT 3,
            warmup              INTEGER NOT NULL DEFAULT 1,
            results             TEXT NOT NULL,              -- JSON array of BenchSummary objects
            load_time_ms        REAL,                       -- Model load time in ms
            vram_used_mib       INTEGER,                    -- VRAM used at benchmark time
            vram_total_mib      INTEGER,                    -- Total VRAM
            duration_seconds    REAL,                       -- How long the benchmark took
            status              TEXT NOT NULL DEFAULT 'success'
        );
        CREATE INDEX IF NOT EXISTS idx_benchmarks_model_id ON benchmarks(model_id);
        CREATE INDEX IF NOT EXISTS idx_benchmarks_created_at ON benchmarks(created_at DESC);
    "#,
);
