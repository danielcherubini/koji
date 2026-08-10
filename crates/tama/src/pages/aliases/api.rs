use super::types::{Alias, ModelOption};
use crate::utils::{delete_request, get_request, handle_response, post_request, put_request};

/// Fetch all aliases from the backend.
pub async fn fetch_aliases() -> Result<Vec<Alias>, String> {
    let resp = get_request("/tama/v1/aliases")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Fetch available models for the dropdown selector.
/// Reuses the models list endpoint to get model IDs and names.
pub async fn fetch_models() -> Result<Vec<ModelOption>, String> {
    let resp = get_request("/tama/v1/models")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if handle_response(&resp) {
        return Err("unauthorized".into());
    }

    // The models endpoint returns a JSON object with a "models" array
    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let models = data
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let id = entry.get("id").and_then(|v| v.as_i64())?;
                    // Prefer display_name, then api_name, then repo_id
                    let label = entry
                        .get("display_name")
                        .and_then(|v| v.as_str())
                        .or_else(|| entry.get("api_name").and_then(|v| v.as_str()))
                        .or_else(|| entry.get("repo_id").and_then(|v| v.as_str()))
                        .unwrap_or("Unknown")
                        .to_string();
                    Some(ModelOption { id, label })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

/// Create a new alias.
pub async fn create_alias(name: &str, model_id: i64, description: &str) -> Result<Alias, String> {
    let body = serde_json::json!({
        "name": name,
        "model_id": model_id,
        "description": if description.is_empty() { serde_json::Value::Null } else { serde_json::json!(description) }
    });

    let resp = post_request("/tama/v1/aliases")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Update an existing alias.
pub async fn update_alias(
    id: i64,
    name: Option<&str>,
    model_id: Option<i64>,
    description: Option<&str>,
    enabled: Option<bool>,
) -> Result<Alias, String> {
    let mut body = serde_json::Map::new();
    if let Some(n) = name {
        body.insert("name".into(), serde_json::json!(n));
    }
    if let Some(m) = model_id {
        body.insert("model_id".into(), serde_json::json!(m));
    }
    if let Some(d) = description {
        body.insert(
            "description".into(),
            if d.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::json!(d)
            },
        );
    }
    if let Some(e) = enabled {
        body.insert("enabled".into(), serde_json::json!(e));
    }

    let resp = put_request(&format!("/tama/v1/aliases/{}", id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Delete an alias by id.
pub async fn delete_alias(id: i64) -> Result<(), String> {
    let resp = delete_request(&format!("/tama/v1/aliases/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if handle_response(&resp) {
        return Err("unauthorized".into());
    }
    Ok(())
}
