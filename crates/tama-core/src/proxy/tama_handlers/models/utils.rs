use crate::models::ConfigKey;
use crate::proxy::ProxyState;

use super::ModelCapabilities;

/// Capitalize the first character of a string, preserve the rest unchanged.
pub fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

/// Generate a pretty display name from an HF repo name.
/// e.g., "unsloth/Qwen3.5-35B-A3B-GGUF" -> "Unsloth: Qwen3.5 35B A3B"
/// Strips common file suffixes like "GGUF".
pub fn generate_display_name(hf_repo: &str) -> String {
    let parts: Vec<&str> = hf_repo.split('/').collect();
    let (org, model_name) = if parts.len() >= 2 {
        (parts[0], parts[1])
    } else {
        (hf_repo, hf_repo)
    };

    let model_name_processed = model_name
        .replace(['-', '_'], " ")
        .split_whitespace()
        .filter(|word| !word.eq_ignore_ascii_case("GGUF"))
        .map(capitalize_first)
        .collect::<Vec<_>>()
        .join(" ");

    format!("{}: {}", capitalize_first(org), model_name_processed)
}

/// Resolve a raw model identifier (db id, repo id, or config key) to a config_key string.
///
/// Accepts three forms (in priority order):
/// 1. Integer db_id — looked up against `config.db_id` in the in-memory map.
/// 2. Repo id with a slash (e.g. `Unsloth/Foo-GGUF`) — normalized to the
///    lowercased double-dash config_key (e.g. `unsloth--foo-gguf`).
/// 3. Anything else — returned unchanged, on the assumption it is already a
///    config_key, api_name, or model field that downstream lookups will handle.
///
/// Steps 1 and 2 both honour the case-insensitive repo_id contract established
/// by the `COLLATE NOCASE` migration on `model_configs.repo_id`: the in-memory
/// HashMap is keyed by the lowercased repo_id, so a repo id in any case
/// resolves to the same bucket.
pub(super) async fn resolve_config_key(state: &ProxyState, raw: &str) -> String {
    if let Ok(id) = raw.parse::<i64>() {
        let configs = state.model_configs.read().await;
        if let Some((key, _)) = configs.iter().find(|(_, c)| c.db_id == Some(id)) {
            return key.clone();
        }
    }
    if raw.contains('/') {
        return ConfigKey::from_repo_id(raw).to_string();
    }
    raw.to_string()
}

/// Build a model JSON entry from a config entry.
pub(super) async fn build_model_entry(
    state: &ProxyState,
    id: &str,
    cfg: &crate::config::ModelConfig,
    capabilities: Option<&ModelCapabilities>,
) -> Option<serde_json::Value> {
    // Use model field first, fall back to api_name.
    let hf_repo = cfg.model.as_deref().or(cfg.api_name.as_deref())?;

    let context_length = if let Some(ctx) = cfg.context_length {
        Some(ctx)
    } else {
        let card = state.get_model_card(id).await;
        card.and_then(|c| {
            let quant_key = cfg.quant.as_deref().unwrap_or_default();
            c.quants
                .get(quant_key)
                .and_then(|q| q.context_length)
                .or(c.model.default_context_length)
        })
    };
    let modalities = cfg.modalities.as_ref().map(|m| {
        serde_json::json!({
            "input": m.input,
            "output": m.output
        })
    });

    // Output limit: 1/8 of context window, floored at 16K and capped at 32K.
    let output_limit = context_length.map(|ctx| (ctx / 8).clamp(16384, 32768));

    // API id: prefer api_name (lowercased), fall back to model (lowercased).
    let api_id = cfg
        .api_name
        .as_ref()
        .map(|s| s.to_lowercase())
        .or_else(|| cfg.model.as_ref().map(|s| s.to_lowercase()));

    // Generate a pretty display name with org prefix.
    let parts: Vec<&str> = hf_repo.split('/').collect();
    let (org, model_name) = if parts.len() >= 2 {
        (parts[0], parts[1])
    } else {
        (hf_repo, hf_repo)
    };

    let model_name_processed = model_name
        .replace(['-', '_'], " ")
        .split_whitespace()
        .filter(|word| !word.eq_ignore_ascii_case("GGUF"))
        .map(capitalize_first)
        .collect::<Vec<_>>()
        .join(" ");

    let pretty_name = format!("{}: {}", capitalize_first(org), model_name_processed);

    let mut model_json = serde_json::json!({
        "id": api_id,
        "name": pretty_name,
        "model": cfg.model,
        "backend": cfg.backend,
        "context_length": context_length,
        "limit": {
            "context": context_length,
            "output": output_limit,
        },
        "quant": cfg.quant,
        "gpu_layers": cfg.gpu_layers,
    });

    if let Some(m) = modalities {
        model_json["modalities"] = m;
    }

    // Derive attachment from modalities
    let attachment = cfg
        .modalities
        .as_ref()
        .is_some_and(|m| m.input.iter().any(|s| s == "image"));

    // Use provided capabilities or config-derived defaults
    let (tool_call, reasoning) = capabilities
        .map(|c| (c.tool_call, c.reasoning))
        .unwrap_or((true, false));

    model_json["tool_call"] = serde_json::json!(tool_call);
    model_json["reasoning"] = serde_json::json!(reasoning);
    model_json["attachment"] = serde_json::json!(attachment);
    model_json["temperature"] = serde_json::json!(true);

    Some(model_json)
}
