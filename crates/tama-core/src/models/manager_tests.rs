//! Tests for ModelManager.
//!
//! Model-domain tests run against a dedicated Postgres test database via
//! `crate::testing::postgres::with_schema` (plan-190).

use super::*;
use crate::config::ModelConfig;
use crate::db::queries::{ModelConfigRecord, PullLogEntry};

fn make_test_record(repo_id: &str) -> ModelConfigRecord {
    use chrono::{SecondsFormat, Utc};
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    ModelConfigRecord {
        id: 0,
        repo_id: repo_id.to_string(),
        display_name: Some("Test Model".to_string()),
        backend: "llama.cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        enabled: true,
        selected_quant: None,
        selected_mmproj: None,
        selected_mtp_model: None,
        context_length: None,
        num_parallel: None,
        kv_unified: false,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        port: None,
        args: None,
        sampling: None,
        modalities: None,
        profile: None,
        api_name: Some(repo_id.to_string()),
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
        created_at: now.clone(),
        updated_at: now,
        n_batch: None,
        n_ubatch: None,
        vllm_config: None,
        provider_name: None,
        reasoning_levels: None,
    }
}

/// Build a ModelManager on a fresh, empty Postgres schema.
async fn pool_manager() -> (crate::testing::postgres::SchemaGuard, ModelManager) {
    let guard = crate::testing::postgres::with_schema().await;
    let manager = ModelManager::new(std::sync::Arc::new(guard.pool.clone()));
    (guard, manager)
}

#[tokio::test]
async fn test_open_with_empty_schema() {
    let (guard, manager) = pool_manager().await;
    let configs = manager.get_all_configs().await.unwrap();
    assert!(configs.is_empty());
    guard.finish().await;
}

#[tokio::test]
async fn test_upsert_and_get_config() {
    let (guard, manager) = pool_manager().await;
    let record = make_test_record("owner/test-repo");
    let id = manager.upsert_config(&record).await.unwrap();
    assert_eq!(id, 1);

    let fetched = manager.get_config(id).await.unwrap().unwrap();
    assert_eq!(fetched.repo_id, "owner/test-repo");
    assert_eq!(fetched.display_name, Some("Test Model".to_string()));

    let all = manager.get_all_configs().await.unwrap();
    assert_eq!(all.len(), 1);
    guard.finish().await;
}

#[tokio::test]
async fn test_get_config_by_repo_id_missing() {
    let (guard, manager) = pool_manager().await;
    let result = manager
        .get_config_by_repo_id("nonexistent/repo")
        .await
        .unwrap();
    assert!(result.is_none());
    guard.finish().await;
}

#[tokio::test]
async fn test_enable_disable_model() {
    let (guard, manager) = pool_manager().await;

    let mc = ModelConfig {
        backend: "llama.cpp".to_string(),
        enabled: true,
        ..Default::default()
    };
    manager
        .save_model_config("owner--test-repo", &mc)
        .await
        .unwrap();

    // Disable it
    manager.disable_model("owner--test-repo").await.unwrap();
    let record = manager
        .get_config_by_repo_id("owner/test-repo")
        .await
        .unwrap()
        .unwrap();
    assert!(!record.enabled);

    // Re-enable it
    manager.enable_model("owner--test-repo").await.unwrap();
    let record = manager
        .get_config_by_repo_id("owner/test-repo")
        .await
        .unwrap()
        .unwrap();
    assert!(record.enabled);
    guard.finish().await;
}

#[tokio::test]
async fn test_rename_config() {
    let (guard, manager) = pool_manager().await;
    let record = make_test_record("owner/old-name");
    let id = manager.upsert_config(&record).await.unwrap();

    manager.rename_config(id, "owner/new-name").await.unwrap();

    // Old repo_id should return None
    let old = manager
        .get_config_by_repo_id("owner/old-name")
        .await
        .unwrap();
    assert!(old.is_none());

    // New repo_id should return the record
    let new = manager
        .get_config_by_repo_id("owner/new-name")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new.repo_id, "owner/new-name");
    assert_eq!(new.display_name, Some("Test Model".to_string()));
    guard.finish().await;
}

#[tokio::test]
async fn test_file_operations() {
    let (guard, manager) = pool_manager().await;

    // Insert a model config first (required for FK)
    let record = make_test_record("owner/test-model");
    let model_id = manager.upsert_config(&record).await.unwrap();

    // Verify no files initially
    let files = manager.get_files(model_id).await.unwrap();
    assert!(files.is_empty());

    // Upsert a file
    manager
        .upsert_file(
            model_id,
            "owner/test-model",
            "test-model.Q4_K_M.gguf",
            Some("Q4_K_M"),
            Some("sha256-abc123"),
            Some(1_000_000),
        )
        .await
        .unwrap();

    // Verify it appears in get_files
    let files = manager.get_files(model_id).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename, "test-model.Q4_K_M.gguf");
    assert_eq!(files[0].quant, Some("Q4_K_M".to_string()));
    assert_eq!(files[0].lfs_oid, Some("sha256-abc123".to_string()));
    assert_eq!(files[0].size_bytes, Some(1_000_000));

    // Verify it appears in get_all_files
    let all_files = manager.get_all_files().await.unwrap();
    assert_eq!(all_files.len(), 1);

    // Update verification
    manager
        .update_verification(model_id, "test-model.Q4_K_M.gguf", Some(true), None)
        .await
        .unwrap();

    let files = manager.get_files(model_id).await.unwrap();
    assert_eq!(files[0].verified_ok, Some(true));

    // Delete the file
    manager
        .delete_file(model_id, "test-model.Q4_K_M.gguf")
        .await
        .unwrap();

    let files = manager.get_files(model_id).await.unwrap();
    assert!(files.is_empty());
    guard.finish().await;
}

#[tokio::test]
async fn test_pull_operations() {
    let (guard, manager) = pool_manager().await;

    // Insert a model config
    let record = make_test_record("owner/test-model");
    let model_id = manager.upsert_config(&record).await.unwrap();

    // No pull record initially
    let pull = manager.get_pull(model_id).await.unwrap();
    assert!(pull.is_none());

    // Upsert a pull record
    manager
        .upsert_pull(model_id, "owner/test-model", "abc123def456")
        .await
        .unwrap();

    // Verify pull record
    let pull = manager.get_pull(model_id).await.unwrap().unwrap();
    assert_eq!(pull.model_id, model_id);
    assert_eq!(pull.repo_id, "owner/test-model");
    assert_eq!(pull.commit_sha, "abc123def456");
    guard.finish().await;
}

#[tokio::test]
async fn test_log_pull() {
    let (guard, manager) = pool_manager().await;

    let entry = PullLogEntry {
        repo_id: "owner/test-model".to_string(),
        filename: "test.gguf".to_string(),
        started_at: "2025-01-01T00:00:00Z".to_string(),
        completed_at: Some("2025-01-01T00:01:00Z".to_string()),
        size_bytes: Some(5_000_000),
        duration_ms: Some(60_000),
        success: true,
        error_message: None,
    };

    manager.log_pull(&entry).await.unwrap();
    guard.finish().await;
}

#[tokio::test]
async fn test_active_model_operations() {
    let (guard, manager) = pool_manager().await;

    // Initially empty
    let active = manager.get_active().await.unwrap();
    assert!(active.is_empty());

    // Insert an active record
    manager
        .insert_active(
            "server1",
            "model.gguf",
            "llama.cpp",
            1234,
            8080,
            "http://127.0.0.1:8080",
        )
        .await
        .unwrap();

    // Verify it appears
    let active = manager.get_active().await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].server_name, "server1");
    assert_eq!(active[0].model_name, "model.gguf");
    assert_eq!(active[0].backend, "llama.cpp");
    assert_eq!(active[0].pid, 1234);
    assert_eq!(active[0].port, 8080);

    // Rename the active record
    manager
        .rename_active("server1", "server1-renamed")
        .await
        .unwrap();
    let active = manager.get_active().await.unwrap();
    assert_eq!(active[0].server_name, "server1-renamed");

    // Remove the active record
    manager.remove_active("server1-renamed").await.unwrap();
    let active = manager.get_active().await.unwrap();
    assert!(active.is_empty());
    guard.finish().await;
}

#[tokio::test]
async fn test_pull_queue_operations() {
    let (guard, manager) = pool_manager().await;

    // Insert a queue item
    let id = manager
        .queue_insert(
            "pull-abc123",
            "owner/test-model",
            "test-model.Q4_K_M.gguf",
            Some("Test Model Q4"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .await
        .unwrap();
    assert!(id > 0);

    // Get queued item
    let item = manager.queue_get_queued().await.unwrap().unwrap();
    assert_eq!(item.job_id, "pull-abc123");
    assert_eq!(item.status, "queued");
    assert_eq!(item.kind, "model");
    assert_eq!(item.quant, Some("Q4_K_M".to_string()));
    assert_eq!(item.context_length, Some(4096));

    // Update status to running
    manager
        .queue_update_status("pull-abc123", "running", 500, Some(1000), None)
        .await
        .unwrap();

    // Get by job_id
    let item = manager
        .queue_get_by_job_id("pull-abc123")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.status, "running");
    assert_eq!(item.bytes_pulled, 500);

    // Get active items
    let active = manager.queue_get_active().await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].job_id, "pull-abc123");

    // Complete the item
    manager
        .queue_update_status("pull-abc123", "completed", 1000, Some(1000), None)
        .await
        .unwrap();

    // Should appear in history now
    let history = manager.queue_get_history(10, 0).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, "completed");

    // Should no longer be in active
    let active = manager.queue_get_active().await.unwrap();
    assert!(active.is_empty());

    guard.finish().await;
}

#[tokio::test]
async fn test_queue_cancel() {
    let (guard, manager) = pool_manager().await;

    manager
        .queue_insert(
            "pull-cancel1",
            "owner/test",
            "test.gguf",
            None,
            "model",
            None,
            None,
        )
        .await
        .unwrap();

    manager.queue_cancel("pull-cancel1").await.unwrap();

    let item = manager
        .queue_get_by_job_id("pull-cancel1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.status, "cancelled");
    assert!(item.completed_at.is_some());

    guard.finish().await;
}

#[tokio::test]
async fn test_save_model_config_convenience() {
    let (guard, manager) = pool_manager().await;

    let mc = ModelConfig {
        backend: "llama.cpp".to_string(),
        display_name: Some("My Model".to_string()),
        enabled: true,
        ..Default::default()
    };
    let id = manager
        .save_model_config("owner--my-model", &mc)
        .await
        .unwrap();
    assert_eq!(id, 1);

    let record = manager.get_config(id).await.unwrap().unwrap();
    assert_eq!(record.repo_id, "owner/my-model");
    assert_eq!(record.backend, "llama.cpp");
    assert_eq!(record.display_name, Some("My Model".to_string()));
    assert!(record.enabled);
    assert_eq!(record.api_name, Some("owner/my-model".to_string()));
    guard.finish().await;
}
