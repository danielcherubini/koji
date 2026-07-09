use super::types::{ApiKey, CreateKeyResponse};
use crate::utils::{delete_request, extract_and_store_csrf_token, get_request, post_request};

/// Fetch all API keys from the backend.
pub async fn fetch_keys() -> Result<Vec<ApiKey>, String> {
    let resp = get_request("/tama/v1/keys")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    extract_and_store_csrf_token(&resp);
    resp.json().await.map_err(|e| e.to_string())
}

/// Create a new API key. Returns the response including the plaintext key.
pub async fn create_key(
    name: &str,
    scopes: &[String],
    expires_at: Option<String>,
) -> Result<CreateKeyResponse, String> {
    let body = serde_json::json!({
        "name": name,
        "scopes": scopes,
        "expires_at": expires_at,  // None serializes as null — backend accepts this
    });

    let resp = post_request("/tama/v1/keys")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}

/// Update an API key's scopes.
pub async fn update_key_scopes(id: i64, scopes: &[String]) -> Result<ApiKey, String> {
    let body = serde_json::json!({
        "scopes": scopes,
    });

    // gloo-net provides `Request::patch()` — use it directly with CSRF.
    let token = crate::utils::get_csrf_token().unwrap_or_default();
    let resp = gloo_net::http::Request::patch(&format!("/tama/v1/keys/{}", id))
        .header("Content-Type", "application/json")
        .header("X-CSRF-Token", &token)
        .body(serde_json::to_string(&body).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    extract_and_store_csrf_token(&resp);
    resp.json().await.map_err(|e| e.to_string())
}

/// Revoke an API key (soft delete).
pub async fn revoke_key(id: i64) -> Result<(), String> {
    let resp = delete_request(&format!("/tama/v1/keys/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    // Check HTTP status — 204 No Content is expected success.
    // 404 means key not found, 4xx/5xx means server error.
    let status = resp.status();
    if status != 204 {
        return Err(format!("revoke failed with HTTP {}", status));
    }
    Ok(())
}
