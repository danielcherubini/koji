//! TTS configuration database query functions (Postgres, plan-190 Task 4).
//!
//! All functions are async and take a `&PgPool` — the caller owns the pool.
//!
//! Case-insensitive parity gap: v2's `tts_configs.engine` was `COLLATE NOCASE`,
//! but the Postgres `UNIQUE (engine)` is case-sensitive, so 'Kokoro' and
//! 'kokoro' are distinct rows. There are no runtime callers today; when TTS
//! config gains runtime callers, a `lower(engine)` (or citext) guard must be
//! added to restore v2 parity.

use anyhow::Result;
use sqlx::{PgPool, Row};

use super::types::TtsConfigRecord;

/// Insert or update the TTS engine configuration.
/// `updated_at` is refreshed to `now()` on conflict.
/// Returns the config id.
pub async fn upsert_tts_config(pool: &PgPool, record: &TtsConfigRecord) -> Result<i64> {
    sqlx::query(
        "INSERT INTO tts_configs (
            engine, default_voice, speed, format, enabled,
            created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7
        )
         ON CONFLICT (engine) DO UPDATE SET
             default_voice = EXCLUDED.default_voice,
             speed = EXCLUDED.speed,
             format = EXCLUDED.format,
             enabled = EXCLUDED.enabled,
             updated_at = now()",
    )
    .bind(&record.engine)
    .bind(&record.default_voice)
    .bind(record.speed as f64)
    .bind(&record.format)
    .bind(record.enabled)
    .bind(record.created_at)
    .bind(record.updated_at)
    .execute(pool)
    .await?;
    // Return the id (either existing or newly created)
    let row = sqlx::query("SELECT id FROM tts_configs WHERE engine = $1")
        .bind(&record.engine)
        .fetch_one(pool)
        .await?;
    Ok(row.get("id"))
}

/// Get the TTS configuration by engine name. Returns None if not found.
pub async fn get_tts_config(pool: &PgPool, engine: &str) -> Result<Option<TtsConfigRecord>> {
    let row = sqlx::query(
        "SELECT id, engine, default_voice, speed, format, enabled, created_at, updated_at
         FROM tts_configs WHERE engine = $1",
    )
    .bind(engine)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(TtsConfigRecord {
        id: row.get("id"),
        engine: row.get("engine"),
        default_voice: row.get("default_voice"),
        speed: row.get::<f64, _>("speed") as f32,
        format: row.get("format"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }))
}

/// Delete the TTS configuration by engine name.
pub async fn delete_tts_config(pool: &PgPool, engine: &str) -> Result<()> {
    sqlx::query("DELETE FROM tts_configs WHERE engine = $1")
        .bind(engine)
        .execute(pool)
        .await?;
    Ok(())
}
