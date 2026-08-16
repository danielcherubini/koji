use super::check::{CheckResponse, UpdateCheckDto, UpdatesListResponse};
use crate::api::error::tests::assert_error_shape;
use axum::body::Body;
use axum::http::Request;
use std::sync::Arc;
use tama_core::config::Config;
use tama_core::proxy::ProxyState;
use tower::ServiceExt;

// ── UpdateCheckDto serialization tests ────────────────────────────────

#[test]
fn test_update_check_dto_serialization() {
    let dto = UpdateCheckDto {
        item_type: "backend".to_string(),
        item_id: "llama-cpp".to_string(),
        variant: Some("cpu".to_string()),
        repo_id: None,
        display_name: Some("Llama CPP".to_string()),
        current_version: Some("1.0.0".to_string()),
        latest_version: Some("1.1.0".to_string()),
        update_available: true,
        status: "update_available".to_string(),
        error_message: None,
        details_json: None,
        checked_at: 1700000000,
    };

    let json = serde_json::to_string(&dto).unwrap();
    let deserialized: UpdateCheckDto = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.item_type, "backend");
    assert_eq!(deserialized.item_id, "llama-cpp");
    assert_eq!(deserialized.display_name, Some("Llama CPP".to_string()));
    assert_eq!(deserialized.current_version, Some("1.0.0".to_string()));
    assert_eq!(deserialized.latest_version, Some("1.1.0".to_string()));
    assert!(deserialized.update_available);
    assert_eq!(deserialized.status, "update_available");
}

#[test]
fn test_update_check_dto_model_type() {
    let dto = UpdateCheckDto {
        item_type: "model".to_string(),
        item_id: "123".to_string(),
        variant: None,
        repo_id: Some("unsloth/Qwen3.6-35B-A3B-GGUF".to_string()),
        display_name: Some("Qwen 3.6".to_string()),
        current_version: Some("abc123".to_string()),
        latest_version: Some("def456".to_string()),
        update_available: false,
        status: "up_to_date".to_string(),
        error_message: None,
        details_json: None,
        checked_at: 1700000000,
    };

    let json = serde_json::to_string(&dto).unwrap();
    let deserialized: UpdateCheckDto = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.item_type, "model");
    assert_eq!(
        deserialized.repo_id,
        Some("unsloth/Qwen3.6-35B-A3B-GGUF".to_string())
    );
    assert!(!deserialized.update_available);
}

#[test]
fn test_update_check_dto_with_error() {
    let dto = UpdateCheckDto {
        item_type: "backend".to_string(),
        item_id: "custom-backend".to_string(),
        variant: None,
        repo_id: None,
        display_name: None,
        current_version: Some("1.0.0".to_string()),
        latest_version: None,
        update_available: false,
        status: "error".to_string(),
        error_message: Some("API rate limited".to_string()),
        details_json: None,
        checked_at: 1700000000,
    };

    let json = serde_json::to_string(&dto).unwrap();
    let deserialized: UpdateCheckDto = serde_json::from_str(&json).unwrap();

    assert_eq!(
        deserialized.error_message,
        Some("API rate limited".to_string())
    );
    assert_eq!(deserialized.status, "error");
}

#[test]
fn test_update_check_dto_with_details_json() {
    let details = serde_json::json!({
        "repo_id": "test/repo",
        "commit_sha": "abc123",
        "file_count": 3
    });
    let dto = UpdateCheckDto {
        item_type: "model".to_string(),
        item_id: "456".to_string(),
        variant: None,
        repo_id: Some("test/repo".to_string()),
        display_name: None,
        current_version: Some("abc123".to_string()),
        latest_version: Some("def456".to_string()),
        update_available: true,
        status: "update_available".to_string(),
        error_message: None,
        details_json: Some(details.clone()),
        checked_at: 1700000000,
    };

    let json = serde_json::to_string(&dto).unwrap();
    let deserialized: UpdateCheckDto = serde_json::from_str(&json).unwrap();

    assert!(deserialized.details_json.is_some());
    let details_val = deserialized.details_json.unwrap();
    assert_eq!(details_val["file_count"], 3);
}

// ── UpdatesListResponse serialization tests ───────────────────────────

#[test]
fn test_updates_list_response_serialization() {
    let response = UpdatesListResponse {
        backends: vec![UpdateCheckDto {
            item_type: "backend".to_string(),
            item_id: "llama-cpp".to_string(),
            variant: Some("cpu".to_string()),
            repo_id: None,
            display_name: None,
            current_version: Some("1.0.0".to_string()),
            latest_version: Some("1.1.0".to_string()),
            update_available: true,
            status: "update_available".to_string(),
            error_message: None,
            details_json: None,
            checked_at: 1700000000,
        }],
        models: vec![UpdateCheckDto {
            item_type: "model".to_string(),
            item_id: "1".to_string(),
            variant: None,
            repo_id: Some("test/model".to_string()),
            display_name: None,
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "no_prior_record".to_string(),
            error_message: None,
            details_json: None,
            checked_at: 1700000000,
        }],
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: UpdatesListResponse = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.backends.len(), 1);
    assert_eq!(deserialized.models.len(), 1);
    assert_eq!(deserialized.backends[0].item_type, "backend");
    assert_eq!(deserialized.models[0].item_type, "model");
}

#[test]
fn test_updates_list_response_empty() {
    let response = UpdatesListResponse {
        backends: vec![],
        models: vec![],
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: UpdatesListResponse = serde_json::from_str(&json).unwrap();

    assert!(deserialized.backends.is_empty());
    assert!(deserialized.models.is_empty());
}

// ── CheckResponse serialization tests ─────────────────────────────────

#[test]
fn test_check_response_serialization() {
    let response = CheckResponse {
        triggered: true,
        message: "Update check started".to_string(),
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: CheckResponse = serde_json::from_str(&json).unwrap();

    assert!(deserialized.triggered);
    assert_eq!(deserialized.message, "Update check started");
}

#[test]
fn test_check_response_serialization_false() {
    let response = CheckResponse {
        triggered: false,
        message: "No changes".to_string(),
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: CheckResponse = serde_json::from_str(&json).unwrap();

    assert!(!deserialized.triggered);
}

// ── Error shape tests ────────────────────────────────────────────────

/// POST /tama/v1/updates/check/bogus/123 — invalid item_type should return
/// 400 with canonical error shape and type=ValidationError.
#[tokio::test]
async fn test_check_item_for_update_invalid_item_type_error_shape() {
    let config = Config::default();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(ProxyState::new(
        config,
        Some(tmp_dir.path().to_path_buf()),
        None,
    ));

    let web_state = Arc::new(crate::web_types::WebState {
        jobs: Some(Arc::new(crate::web_types::JobManager::new())),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        repository: None,
        db_pool: None,
    });

    let router = crate::router::build_web_routes(web_state.clone())
        .with_state(state)
        .layer(axum::extract::Extension(web_state.as_ref().clone()));

    let csrf_token = "test-csrf-token-12345";
    let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/updates/check/bogus/123")
        .header(axum::http::header::COOKIE, cookie_header.as_str())
        .header("X-CSRF-Token", csrf_token)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.expect("request should complete");

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "check_item_for_update should return 400 for invalid item_type"
    );

    let detail = assert_error_shape(resp).await;
    assert_eq!(
        detail.r#type,
        Some("ValidationError".to_string()),
        "invalid item_type should return ValidationError type"
    );
}
