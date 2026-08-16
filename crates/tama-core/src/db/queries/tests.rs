//! Tests for database query functions.

use super::*;
use crate::db::{open_in_memory, OpenResult};

/// The former update-check tests now live in
/// `crates/tama-core/tests/update_check_queries.rs` on the Postgres harness.

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
        n_batch: None,
        n_ubatch: None,
        vllm_config: None,
        provider_name: None,
        reasoning_levels: None,
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
        n_batch: None,
        n_ubatch: None,
        vllm_config: None,
        provider_name: None,
        reasoning_levels: None,
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
        n_batch: None,
        n_ubatch: None,
        vllm_config: None,
        provider_name: None,
        reasoning_levels: None,
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
        n_batch: None,
        n_ubatch: None,
        vllm_config: None,
        provider_name: None,
        reasoning_levels: None,
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
