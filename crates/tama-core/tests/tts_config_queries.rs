//! Postgres ports of the `tts_config_queries` tests (plan-190, Task 4 —
//! TTS engine config moves to Postgres).
//!
//! These mirror the former in-file SQLite tests 1:1 against the async
//! `&PgPool` API on an isolated migrated schema. Two SQLite-isms are
//! adapted to the Postgres schema:
//! - `engine` was `COLLATE NOCASE` in SQLite; the Postgres schema uses a
//!   plain case-sensitive `UNIQUE`, so lookup is case-sensitive.
//! - `created_at`/`updated_at` are `TIMESTAMPTZ`; the empty-string
//!   timestamps from the old tests become explicit [`OffsetDateTime`] values.

mod common;

use common::with_schema;
use sqlx::types::time::OffsetDateTime;
use tama_core::db::queries::{
    delete_tts_config, get_tts_config, upsert_tts_config, TtsConfigRecord,
};

/// A fixed timestamp used for deterministic assertions.
const FIX_TS: i64 = 1_700_000_000; // 2023-11-14

fn fix_ts() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(FIX_TS).expect("valid unix timestamp")
}

/// A record with the given engine name and explicit timestamps.
fn make_record(engine: &str, voice: Option<&str>, speed: f32, format: &str) -> TtsConfigRecord {
    let ts = fix_ts();
    TtsConfigRecord {
        id: 0,
        engine: engine.to_string(),
        default_voice: voice.map(String::from),
        speed,
        format: format.to_string(),
        enabled: true,
        created_at: ts,
        updated_at: ts,
    }
}

/// Test that upsert_tts_config creates a new record and returns its id.
#[tokio::test]
async fn test_upsert_creates_new_record() {
    let guard = with_schema().await;

    let id = upsert_tts_config(
        &guard.pool,
        &make_record("kokoro", Some("af_sky"), 1.2, "mp3"),
    )
    .await
    .unwrap();
    assert_eq!(id, 1);

    guard.finish().await;
}

/// Test that get_tts_config returns the correct record.
#[tokio::test]
async fn test_get_tts_config_returns_record() {
    let guard = with_schema().await;
    upsert_tts_config(
        &guard.pool,
        &make_record("kokoro", Some("af_nicole"), 1.5, "wav"),
    )
    .await
    .unwrap();

    let found = get_tts_config(&guard.pool, "kokoro")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.engine, "kokoro");
    assert_eq!(found.speed, 1.5);
    assert_eq!(found.format, "wav");

    guard.finish().await;
}

/// Test that get_tts_config returns None for unknown engine.
#[tokio::test]
async fn test_get_tts_config_returns_none_for_unknown() {
    let guard = with_schema().await;
    let result = get_tts_config(&guard.pool, "unknown_engine").await.unwrap();
    assert!(result.is_none());

    guard.finish().await;
}

/// The Postgres schema uses a case-sensitive `UNIQUE` on `engine`
/// (SQLite's `COLLATE NOCASE` has no equivalent in the migrated schema):
/// different casings are distinct records.
#[tokio::test]
async fn test_engine_lookup_is_case_sensitive() {
    let guard = with_schema().await;
    let record = TtsConfigRecord {
        id: 0,
        engine: "Kokoro".to_string(), // Capital K
        default_voice: None,
        speed: 1.0,
        format: "mp3".to_string(),
        enabled: true,
        created_at: fix_ts(),
        updated_at: fix_ts(),
    };
    upsert_tts_config(&guard.pool, &record).await.unwrap();

    // Exact case finds it
    let found = get_tts_config(&guard.pool, "Kokoro")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.engine, "Kokoro");

    // Different case is a distinct engine in Postgres
    assert!(get_tts_config(&guard.pool, "kokoro")
        .await
        .unwrap()
        .is_none());

    guard.finish().await;
}

/// Test that upsert updates an existing record.
#[tokio::test]
async fn test_upsert_updates_existing_record() {
    let guard = with_schema().await;

    // Insert initial config
    let record1 = make_record("kokoro", Some("af_sky"), 1.0, "mp3");
    let id1 = upsert_tts_config(&guard.pool, &record1).await.unwrap();

    // Update the config
    let record2 = make_record("kokoro", Some("af_bella"), 1.5, "wav");
    let id2 = upsert_tts_config(&guard.pool, &record2).await.unwrap();

    // Same id returned (not a new record)
    assert_eq!(id1, id2);

    // Verify the update took effect
    let found = get_tts_config(&guard.pool, "kokoro")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.default_voice, Some("af_bella".to_string()));
    assert_eq!(found.speed, 1.5);

    guard.finish().await;
}

/// Test that delete_tts_config removes a record.
#[tokio::test]
async fn test_delete_tts_config() {
    let guard = with_schema().await;

    upsert_tts_config(&guard.pool, &make_record("kokoro", None, 1.0, "mp3"))
        .await
        .unwrap();

    delete_tts_config(&guard.pool, "kokoro").await.unwrap();

    let result = get_tts_config(&guard.pool, "kokoro").await.unwrap();
    assert!(result.is_none());

    guard.finish().await;
}

/// Test that deleting a non-existent engine does not error.
#[tokio::test]
async fn test_delete_nonexistent_engine() {
    let guard = with_schema().await;
    // Should not panic or error
    let result = delete_tts_config(&guard.pool, "nonexistent").await;
    assert!(result.is_ok());

    guard.finish().await;
}

/// Test that enabled field is correctly stored as boolean.
#[tokio::test]
async fn test_enabled_boolean_storage() {
    let guard = with_schema().await;

    // Insert with enabled=false
    let mut record = make_record("kokoro", None, 1.0, "mp3");
    record.enabled = false;
    upsert_tts_config(&guard.pool, &record).await.unwrap();

    let found = get_tts_config(&guard.pool, "kokoro")
        .await
        .unwrap()
        .unwrap();
    assert!(!found.enabled);

    guard.finish().await;
}

/// Test that `created_at` is stored as passed and `updated_at` is refreshed
/// to `now()` on conflict (SQLite's strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
/// equivalent in the Postgres schema is `now()`).
#[tokio::test]
async fn test_timestamps_stored_as_passed() {
    let guard = with_schema().await;

    let created = fix_ts();
    let mut record = make_record("kokoro", None, 1.0, "mp3");
    record.created_at = created;
    record.updated_at = created;
    upsert_tts_config(&guard.pool, &record).await.unwrap();

    let found = get_tts_config(&guard.pool, "kokoro")
        .await
        .unwrap()
        .unwrap();
    // The upsert function stores the record's created_at directly,
    // so the passed-in timestamp round-trips.
    assert_eq!(
        found.created_at, created,
        "created_at should be stored as passed"
    );
    assert!(
        found.updated_at >= created,
        "updated_at should be at least the initial timestamp"
    );

    // A second upsert refreshes updated_at (and does not touch created_at)
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    upsert_tts_config(
        &guard.pool,
        &make_record("kokoro", Some("af_x"), 1.0, "mp3"),
    )
    .await
    .unwrap();
    let updated = get_tts_config(&guard.pool, "kokoro")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        updated.created_at, created,
        "upsert must not change created_at"
    );
    assert!(
        updated.updated_at > found.updated_at,
        "upsert must refresh updated_at"
    );

    guard.finish().await;
}
