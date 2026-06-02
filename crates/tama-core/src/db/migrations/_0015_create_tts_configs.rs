/// v15 — Create tts_configs table
pub const MIGRATION: (i32, bool, &str) = (
    15,
    false,
    r#"
        CREATE TABLE tts_configs (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            engine       TEXT NOT NULL UNIQUE COLLATE NOCASE,  -- TTS engine name (e.g., 'kokoro')
            default_voice TEXT,                                -- e.g., 'af_sky'
            speed        REAL   NOT NULL DEFAULT 1.0,          -- 0.5 to 2.0
            format       TEXT   NOT NULL DEFAULT 'mp3',        -- mp3, wav, ogg
            enabled      INTEGER NOT NULL DEFAULT 1,
            created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
    "#,
);
