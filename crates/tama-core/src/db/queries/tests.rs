//! Tests for database query functions.

use super::*;
use crate::db::{open_in_memory, OpenResult};

#[test]
fn test_upsert_and_get_update_check() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let item_type = "backend";
    let item_id = "llama-cpp";
    let now = 1713168000; // 2024-04-15

    // Insert
    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type,
            item_id,
            current_version: Some("v1.0.0"),
            latest_version: Some("v1.1.0"),
            update_available: true,
            status: "update_available",
            error_message: None,
            details_json: None,
            checked_at: now,
        },
    )
    .unwrap();

    let record = get_update_check(&conn, item_type, item_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.item_type, item_type);
    assert_eq!(record.item_id, item_id);
    assert_eq!(record.current_version.unwrap(), "v1.0.0");
    assert_eq!(record.latest_version.unwrap(), "v1.1.0");
    assert!(record.update_available);
    assert_eq!(record.status, "update_available");
    assert_eq!(record.checked_at, now);

    // Upsert (Update)
    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type,
            item_id,
            current_version: Some("v1.1.0"),
            latest_version: Some("v1.1.0"),
            update_available: false,
            status: "up_to_date",
            error_message: None,
            details_json: None,
            checked_at: now + 100,
        },
    )
    .unwrap();

    let updated = get_update_check(&conn, item_type, item_id)
        .unwrap()
        .unwrap();
    assert_eq!(updated.current_version.unwrap(), "v1.1.0");
    assert!(!updated.update_available);
    assert_eq!(updated.status, "up_to_date");
    assert_eq!(updated.checked_at, now + 100);
}

#[test]
fn test_get_all_update_checks() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let now = 1713168000;

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "b1",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: now,
        },
    )
    .unwrap();

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "model",
            item_id: "m1",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: now,
        },
    )
    .unwrap();

    let all = get_all_update_checks(&conn).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_delete_update_check() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let item_type = "backend";
    let item_id = "b1";

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type,
            item_id,
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 12345,
        },
    )
    .unwrap();

    delete_update_check(&conn, item_type, item_id).unwrap();
    let record = get_update_check(&conn, item_type, item_id).unwrap();
    assert!(record.is_none());
}

#[test]
fn test_get_oldest_check_time() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    assert_eq!(get_oldest_check_time(&conn).unwrap(), None);

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "b1",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 2000,
        },
    )
    .unwrap();

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "b2",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1000,
        },
    )
    .unwrap();

    assert_eq!(get_oldest_check_time(&conn).unwrap(), Some(1000));
}

#[test]
fn test_delete_update_checks_by_pattern() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    // Insert records for multiple backends with variant-style item_ids
    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "llama_cpp:cpu",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1000,
        },
    )
    .unwrap();

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "llama_cpp:cuda",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1001,
        },
    )
    .unwrap();

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "ik_llama:cpu",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1002,
        },
    )
    .unwrap();

    // Delete all llama_cpp variants using LIKE pattern
    let pattern = "llama_cpp:%";
    delete_update_checks_by_pattern(&conn, "backend", pattern).unwrap();

    // Verify llama_cpp records are gone
    assert!(get_update_check(&conn, "backend", "llama_cpp:cpu")
        .unwrap()
        .is_none());
    assert!(get_update_check(&conn, "backend", "llama_cpp:cuda")
        .unwrap()
        .is_none());

    // Verify ik_llama record is unaffected
    assert!(get_update_check(&conn, "backend", "ik_llama:cpu")
        .unwrap()
        .is_some());

    // Edge case: pattern that matches nothing should not error
    delete_update_checks_by_pattern(&conn, "backend", "nonexistent:%").unwrap();
}

#[test]
fn test_delete_update_checks_by_pattern_escapes_underscore() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    // Insert a record with underscore in the name
    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "my_backend:cpu",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1000,
        },
    )
    .unwrap();

    // Insert a similar record that should NOT match
    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "myXbackend:cpu",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1001,
        },
    )
    .unwrap();

    // Escape the underscore so it matches literally, not as wildcard
    let escaped_name = "my_backend"
        .replace('\\', "\\\\")
        .replace('_', "\\_")
        .replace('%', "\\%");
    let pattern = format!("{}:%", escaped_name);
    delete_update_checks_by_pattern(&conn, "backend", &pattern).unwrap();

    // my_backend:cpu should be deleted
    assert!(get_update_check(&conn, "backend", "my_backend:cpu")
        .unwrap()
        .is_none());

    // myXbackend:cpu should NOT be deleted (underscore was escaped)
    assert!(get_update_check(&conn, "backend", "myXbackend:cpu")
        .unwrap()
        .is_some());
}

#[test]
fn test_upsert_and_get_model_config() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let record = ModelConfigRecord {
        id: 0, // auto-assigned
        repo_id: "test-repo".to_string(),
        display_name: Some("Test Model".to_string()),
        backend: "llama_cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        enabled: true,
        selected_quant: Some("Q4_K_M".to_string()),
        selected_mmproj: Some("mmproj-f16.gguf".to_string()),
        selected_mtp_model: Some("mtp-F16.gguf".to_string()),
        context_length: Some(4096),
        num_parallel: Some(1),
        kv_unified: false,
        gpu_layers: Some(32),
        cache_type_k: Some("q8_0".to_string()),
        cache_type_v: Some("q4_0".to_string()),
        port: Some(8080),
        args: Some(r#"["--flash-attn"]"#.to_string()),
        sampling: Some(r#"{"temp": 0.7}"#.to_string()),
        modalities: Some(r#"{ "input": ["text"], "output": ["text"] }"#.to_string()),
        profile: Some("default".to_string()),
        api_name: Some("test-api".to_string()),
        health_check: Some(r#"{"path": "/health"}"#.to_string()),
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
        created_at: "2024-04-15T12:00:00Z".to_string(),
        updated_at: "2024-04-15T12:00:00Z".to_string(),
    };

    upsert_model_config(&conn, &record).unwrap();

    // Look up by repo_id to get auto-assigned id
    let by_repo = get_model_config_by_repo_id(&conn, "test-repo")
        .unwrap()
        .unwrap();
    let model_id = by_repo.id;

    let retrieved = get_model_config(&conn, model_id).unwrap().unwrap();
    assert_eq!(retrieved.repo_id, record.repo_id);
    assert_eq!(retrieved.display_name, record.display_name);
    assert_eq!(retrieved.backend, record.backend);
    assert_eq!(retrieved.enabled, record.enabled);
    assert_eq!(retrieved.selected_quant, record.selected_quant);
    assert_eq!(retrieved.selected_mmproj, record.selected_mmproj);
    assert_eq!(retrieved.context_length, record.context_length);
    assert_eq!(retrieved.kv_unified, record.kv_unified);
    assert_eq!(retrieved.gpu_layers, record.gpu_layers);
    assert_eq!(retrieved.cache_type_k, record.cache_type_k);
    assert_eq!(retrieved.cache_type_v, record.cache_type_v);
    assert_eq!(retrieved.port, record.port);
    assert_eq!(retrieved.args, record.args);
    assert_eq!(retrieved.sampling, record.sampling);
    assert_eq!(retrieved.modalities, record.modalities);
    assert_eq!(retrieved.profile, record.profile);
    assert_eq!(retrieved.api_name, record.api_name);
    assert_eq!(retrieved.health_check, record.health_check);
    assert_eq!(retrieved.created_at, record.created_at);
    // updated_at will be different because upsert_model_config updates it via strftime
}

/// `mtp_model` must round-trip through the DB record: a value set in
/// `ModelConfig` survives `to_db_record` + `upsert_model_config` + read-back.
#[test]
fn test_mtp_model_db_round_trip() {
    use crate::config::ModelConfig;

    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let mc = ModelConfig {
        backend: "llama_cpp".to_string(),
        model: Some("owner/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mtp_model: Some("mtp-F16.gguf".to_string()),
        ..Default::default()
    };

    let record = mc.to_db_record("owner/repo");
    assert_eq!(record.selected_mtp_model.as_deref(), Some("mtp-F16.gguf"));

    upsert_model_config(&conn, &record).unwrap();

    let fetched = get_model_config_by_repo_id(&conn, "owner/repo")
        .unwrap()
        .unwrap();
    assert_eq!(fetched.selected_mtp_model.as_deref(), Some("mtp-F16.gguf"));

    // And ModelConfig::from_db_record should rehydrate the mtp_model field.
    let round_tripped = ModelConfig::from_db_record(&fetched);
    assert_eq!(round_tripped.mtp_model.as_deref(), Some("mtp-F16.gguf"));
}

/// `mtp_model` round-trips through TOML serialization on ModelConfig.
#[test]
fn test_mtp_model_toml_round_trip() {
    use crate::config::ModelConfig;

    let mc = ModelConfig {
        backend: "llama_cpp".to_string(),
        mtp_model: Some("mtp-F16.gguf".to_string()),
        ..Default::default()
    };

    let toml_str = toml::to_string_pretty(&mc).unwrap();
    let loaded: ModelConfig = toml::from_str(&toml_str).unwrap();

    assert_eq!(loaded.mtp_model.as_deref(), Some("mtp-F16.gguf"));
}

#[test]
fn test_get_all_model_configs() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let rec1 = ModelConfigRecord {
        id: 0,
        repo_id: "repo1".to_string(),
        display_name: None,
        backend: "llama_cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        enabled: true,
        selected_quant: None,
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
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
    };
    let rec2 = ModelConfigRecord {
        id: 0,
        repo_id: "repo2".to_string(),
        display_name: None,
        backend: "llama_cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        enabled: true,
        selected_quant: None,
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
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
    };

    upsert_model_config(&conn, &rec1).unwrap();
    upsert_model_config(&conn, &rec2).unwrap();

    let all = get_all_model_configs(&conn).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_delete_model_config() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let record = ModelConfigRecord {
        id: 0, // auto-assigned
        repo_id: "test-repo".to_string(),
        display_name: None,
        backend: "llama_cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        enabled: true,
        selected_quant: None,
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
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
    };

    upsert_model_config(&conn, &record).unwrap();
    let by_repo = get_model_config_by_repo_id(&conn, "test-repo")
        .unwrap()
        .unwrap();
    let model_id = by_repo.id;
    assert!(get_model_config(&conn, model_id).unwrap().is_some());

    delete_model_config(&conn, model_id).unwrap();
    assert!(get_model_config(&conn, model_id).unwrap().is_none());
}

/// `delete_update_checks_for_backend` removes all variant rows (LIKE `name:%`)
/// and the legacy bare-name row, while leaving other backends untouched.
#[test]
fn test_delete_update_checks_for_backend() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    // Insert four records: two variants + one legacy + one unrelated
    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "llama_cpp:cpu",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1000,
        },
    )
    .unwrap();

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "llama_cpp:vulkan",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1001,
        },
    )
    .unwrap();

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "llama_cpp",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1002,
        },
    )
    .unwrap();

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "other:cpu",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1003,
        },
    )
    .unwrap();

    // Act: delete all update checks for "llama_cpp"
    delete_update_checks_for_backend(&conn, "llama_cpp").unwrap();

    // Assert: llama_cpp variants and legacy row are gone
    assert!(get_update_check(&conn, "backend", "llama_cpp:cpu")
        .unwrap()
        .is_none());
    assert!(get_update_check(&conn, "backend", "llama_cpp:vulkan")
        .unwrap()
        .is_none());
    assert!(get_update_check(&conn, "backend", "llama_cpp")
        .unwrap()
        .is_none());

    // Assert: other backend is untouched
    assert!(get_update_check(&conn, "backend", "other:cpu")
        .unwrap()
        .is_some());
}

/// `delete_update_checks_for_backend` correctly escapes SQL LIKE metacharacters.
#[test]
fn test_delete_update_checks_for_backend_escapes() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    // Insert records with underscore in name — one should match, one should not
    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "my_backend:cpu",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1000,
        },
    )
    .unwrap();

    upsert_update_check(
        &conn,
        super::update_check_queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "myXbackend:cpu",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: 1001,
        },
    )
    .unwrap();

    // Act: delete for "my_backend" — the underscore should be escaped
    delete_update_checks_for_backend(&conn, "my_backend").unwrap();

    // Assert: my_backend:cpu is gone, myXbackend:cpu survives
    assert!(get_update_check(&conn, "backend", "my_backend:cpu")
        .unwrap()
        .is_none());
    assert!(get_update_check(&conn, "backend", "myXbackend:cpu")
        .unwrap()
        .is_some());
}

#[test]
fn test_count_active_keys() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    assert_eq!(count_active_keys(&conn).unwrap(), 0);

    // One active key
    conn.execute(
        "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, created_at, expires_at) \
         VALUES ('a', 'tama_aaa', 'h1', '[\"inference\"]', 'test', '2026-01-01T00:00:00Z', NULL)",
        [],
    )
    .unwrap();
    assert_eq!(count_active_keys(&conn).unwrap(), 1);

    // One revoked key — must NOT be counted
    conn.execute(
        "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, created_at, revoked_at) \
         VALUES ('b', 'tama_bbb', 'h2', '[\"inference\"]', 'test', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z')",
        [],
    )
    .unwrap();
    assert_eq!(count_active_keys(&conn).unwrap(), 1);

    // One expired key — must NOT be counted
    conn.execute(
        "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, created_at, expires_at) \
         VALUES ('c', 'tama_ccc', 'h3', '[\"inference\"]', 'test', '2026-01-01T00:00:00Z', '2020-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    assert_eq!(count_active_keys(&conn).unwrap(), 1);
}
