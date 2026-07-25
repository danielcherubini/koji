use axum::{
    body::Body,
    http::{Method, Request},
};
use tower::ServiceExt;

use super::helpers::{create_test_state, pull_router};

const PULLS_ROUTE: &str = "/tama/v1/pulls";
const CT_JSON: &str = "application/json";

/// Malformed JSON body returns 400 (axum JsonSyntaxError)
#[tokio::test]
async fn test_pull_model_malformed_json_returns_400() {
    let (state, _tmp) = create_test_state();
    let app = pull_router(state);

    // Use raw string to avoid Rust 2021 raw identifier prefix confusion
    // in the `pull` module path.
    let body_str = r#"{""#;
    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header(r#"content-type"#, CT_JSON)
        .body(Body::from(body_str))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
}

/// Missing repo_id returns 422 (JsonDataError)
#[tokio::test]
async fn test_pull_model_missing_repo_id_returns_422() {
    let (state, _tmp) = create_test_state();
    let app = pull_router(state);

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header(r#"content-type"#, CT_JSON)
        .body(Body::from("{}"))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 422);
}

/// Too many filenames (9 > max of 8) returns 400
#[tokio::test]
async fn test_pull_model_too_many_files_returns_400() {
    let (state, _tmp) = create_test_state();
    let app = pull_router(state);

    // Build the JSON body manually to avoid raw-identifier-parser issues
    // with "gguf" patterns when inside the `pull` module.
    let filenames: Vec<String> = (1..=9).map(|i| format!("f{}.gguf", i)).collect();
    let body = serde_json::json!({
        r#"repo_id"#: "test/repo",
        r#"filenames"#: filenames
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header(r#"content-type"#, CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("Too many files requested. Maximum is 8."),
        "Response: {}",
        text
    );
}

/// Too many quants (9 > max of 8) returns 400
#[tokio::test]
async fn test_pull_model_too_many_quants_returns_400() {
    let (state, _tmp) = create_test_state();
    let app = pull_router(state);

    // Build the JSON body manually to avoid raw-identifier-parser issues.
    let quants: Vec<serde_json::Value> = (1..=9)
        .map(|i| {
            serde_json::json!({
                r#"filename"#: format!("f{}.gguf", i),
                r#"quant"#: "Q4_K_M"
            })
        })
        .collect();
    let body = serde_json::json!({
        r#"repo_id"#: "test/repo",
        r#"quants"#: quants
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header(r#"content-type"#, CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("Too many quants requested"),
        "Response: {}",
        text
    );
}
