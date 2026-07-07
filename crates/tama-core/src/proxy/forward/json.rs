use axum::http::request::Parts;
use serde_json::Value as JsonValue;

/// Rewrite the `model` field in a JSON value. Only rewrites if model_name is provided and non-empty.
pub fn rewrite_json_model_name(mut json: JsonValue, model_name: Option<&str>) -> JsonValue {
    if let Some(name) = model_name {
        if !name.is_empty() {
            json["model"] = JsonValue::String(name.to_string());
        }
    }
    json
}

/// Build a forward request target URI from the backend URL and request path/query.
#[allow(dead_code)]
pub fn build_forward_uri(backend_url: &str, parts: &Parts) -> Option<String> {
    let path_and_query = parts.uri.path_and_query()?;
    let (path, query) = path_and_query
        .as_str()
        .split_once('?')
        .unwrap_or((path_and_query.as_str(), ""));

    let mut uri = format!("{}{}", backend_url, path);
    if !query.is_empty() {
        uri.push('?');
        uri.push_str(query);
    }
    Some(uri)
}
