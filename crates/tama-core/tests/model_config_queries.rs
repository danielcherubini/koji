//! Postgres harness tests for `model_config_queries` case-insensitive
//! `repo_id` uniqueness (plan-190 review fix).
//!
//! v2 SQLite used `COLLATE NOCASE` on `repo_id`, so 'Owner/Repo' and
//! 'owner/repo' were the same row. The Postgres schema enforces the same
//! uniqueness with a unique expression index on `lower(repo_id)`; the
//! upsert's `ON CONFLICT` targets that index so even concurrent upserts
//! with different casing cannot create duplicate rows.

mod common;

use common::with_schema;
use sqlx::Row;
use tama_core::db::queries::{upsert_model_config, ModelConfigRecord};

/// A minimal model-config record with the given repo_id.
fn make_record(repo_id: &str) -> ModelConfigRecord {
    ModelConfigRecord {
        id: 0,
        repo_id: repo_id.to_string(),
        display_name: Some("Test Model".to_string()),
        backend: "llama_cpp".into(),
        gpu_variant: None,
        gpu_device: None,
        enabled: true,
        selected_quant: Some("Q4_K_M".into()),
        selected_mmproj: None,
        selected_mtp_model: None,
        context_length: None,
        num_parallel: Some(1),
        kv_unified: false,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        port: None,
        args: None,
        sampling: None,
        modalities: None,
        profile: None,
        api_name: None,
        health_check: None,
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        spec_decoding: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        n_batch: None,
        n_ubatch: None,
        vllm_config: None,
        provider_name: None,
        reasoning_levels: None,
    }
}

/// Count the rows in `model_configs` matching `lower(repo_id) = lower($1)`.
async fn count_rows(pool: &sqlx::PgPool, repo_id: &str) -> i64 {
    let row =
        sqlx::query("SELECT COUNT(*) AS n FROM model_configs WHERE lower(repo_id) = lower($1)")
            .bind(repo_id)
            .fetch_one(pool)
            .await
            .expect("count model_configs rows");
    row.get::<i64, _>("n")
}

/// The raw upsert statement (same shape as the query builder's output):
/// `ON CONFLICT (lower(repo_id))` must route to the case-insensitive
/// unique index, so two different-casing inserts collapse into one row.
const RAW_UPSERT: &str = "INSERT INTO model_configs (repo_id, display_name, backend) \
     VALUES ($1, $2, $3) ON CONFLICT (lower(repo_id)) DO UPDATE SET \
     display_name = EXCLUDED.display_name";

/// Sequential: inserting 'Owner/Repo' then upserting 'Owner/Repo' again
/// (identical casing) updates the same row instead of creating a duplicate.
#[tokio::test]
async fn test_upsert_same_casing_updates_same_row() {
    let guard = with_schema().await;

    sqlx::query(RAW_UPSERT)
        .bind("Owner/Repo")
        .bind("First")
        .bind("llama_cpp")
        .execute(&guard.pool)
        .await
        .expect("insert Owner/Repo");

    // Same casing, same repo_id: must route to the lower(repo_id) arbiter
    // and update the row — NOT raise duplicate-key on the (now removed)
    // plain repo_id unique index.
    sqlx::query(RAW_UPSERT)
        .bind("Owner/Repo")
        .bind("Second")
        .bind("llama_cpp")
        .execute(&guard.pool)
        .await
        .expect("upsert Owner/Repo again (same case)");

    assert_eq!(
        count_rows(&guard.pool, "Owner/Repo").await,
        1,
        "same-case upsert must not create a duplicate row"
    );

    let (display_name,): (String,) =
        sqlx::query_as("SELECT display_name FROM model_configs WHERE repo_id = 'Owner/Repo'")
            .fetch_one(&guard.pool)
            .await
            .expect("read stored row");
    assert_eq!(
        display_name, "Second",
        "same-case upsert must update the existing row"
    );

    guard.finish().await;
}

/// Sequential: inserting 'Owner/Repo' then upserting 'owner/repo' updates
/// the same row instead of creating a duplicate.
#[tokio::test]
async fn test_upsert_different_casing_updates_same_row() {
    let guard = with_schema().await;

    sqlx::query(RAW_UPSERT)
        .bind("Owner/Repo")
        .bind("First")
        .bind("llama_cpp")
        .execute(&guard.pool)
        .await
        .expect("insert Owner/Repo");

    sqlx::query(RAW_UPSERT)
        .bind("owner/repo")
        .bind("Second")
        .bind("llama_cpp")
        .execute(&guard.pool)
        .await
        .expect("upsert owner/repo");

    assert_eq!(
        count_rows(&guard.pool, "owner/repo").await,
        1,
        "different-casing upsert must not create a duplicate row"
    );

    // The conflicting row keeps its stored case (v2 parity).
    let (display_name,): (String,) =
        sqlx::query_as("SELECT display_name FROM model_configs WHERE repo_id = 'Owner/Repo'")
            .fetch_one(&guard.pool)
            .await
            .expect("read stored row");
    assert_eq!(
        display_name, "Second",
        "upsert must update the existing row"
    );

    guard.finish().await;
}

/// The public API: upserting a repo_id that differs only in case returns
/// the existing row's id and updates it in place.
#[tokio::test]
async fn test_api_upsert_case_insensitive_same_id() {
    let guard = with_schema().await;

    let id1 = upsert_model_config(&guard.pool, &make_record("Owner/Repo"))
        .await
        .expect("upsert Owner/Repo");
    let id2 = upsert_model_config(&guard.pool, &make_record("owner/repo"))
        .await
        .expect("upsert owner/repo");

    assert_eq!(id1, id2, "case-insensitive upsert must return the same id");
    assert_eq!(
        count_rows(&guard.pool, "owner/repo").await,
        1,
        "API upsert must not create a case-variant duplicate"
    );

    guard.finish().await;
}

/// The public API with the IDENTICAL repo_id twice (same case): the common
/// model refresh / re-registration path. The upsert's `ON CONFLICT
/// (lower(repo_id))` must update the existing row — not raise a duplicate-
/// key error on a redundant case-sensitive unique index.
#[tokio::test]
async fn test_api_upsert_same_casing_updates_same_row() {
    let guard = with_schema().await;

    let id1 = upsert_model_config(&guard.pool, &make_record("Owner/Repo"))
        .await
        .expect("first upsert Owner/Repo");
    let id2 = upsert_model_config(&guard.pool, &make_record("Owner/Repo"))
        .await
        .expect("second upsert Owner/Repo (same case) must not error");

    assert_eq!(id1, id2, "same-case re-upsert must return the same id");
    assert_eq!(
        count_rows(&guard.pool, "Owner/Repo").await,
        1,
        "same-case re-upsert must not create a duplicate row"
    );

    // The stored row reflects the second upsert's record.
    let (display_name,): (String,) =
        sqlx::query_as("SELECT display_name FROM model_configs WHERE id = $1")
            .bind(id2)
            .fetch_one(&guard.pool)
            .await
            .expect("read stored row");
    assert_eq!(
        display_name, "Test Model",
        "same-case re-upsert must update the existing row"
    );

    guard.finish().await;
}

/// Concurrent upserts with different casing both miss the pre-check; the
/// unique `lower(repo_id)` index must still collapse them into one row
/// (v2's `COLLATE NOCASE` unique index provided this guarantee).
#[tokio::test]
async fn test_concurrent_upserts_different_casing_no_duplicate() {
    let guard = with_schema().await;

    let rec_a = make_record("Owner/Repo");
    let rec_b = make_record("owner/repo");
    let (id_a, id_b) = tokio::join!(
        upsert_model_config(&guard.pool, &rec_a),
        upsert_model_config(&guard.pool, &rec_b)
    );

    let id_a = id_a.expect("upsert A");
    let id_b = id_b.expect("upsert B");
    assert_eq!(
        id_a, id_b,
        "concurrent case-variant upserts must resolve to the same row"
    );
    assert_eq!(
        count_rows(&guard.pool, "owner/repo").await,
        1,
        "concurrent case-variant upserts must not create a duplicate row"
    );

    guard.finish().await;
}
