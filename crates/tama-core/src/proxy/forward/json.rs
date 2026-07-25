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
