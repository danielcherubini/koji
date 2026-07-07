use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::utils::build_model_entry;
use crate::proxy::ProxyState;
use axum::extract::State;
use axum::Json;

use super::ModelCapabilities;

/// Handle listing all enabled models for OpenCode plugin discovery.
/// Returns rich metadata including context limits, modalities, and capabilities.
/// Aliases are included with the same metadata as their target model, using the alias name as `id`.
pub async fn handle_opencode_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    // 1. Snapshot data under locks — clone out so locks are dropped before any .await below.
    // `all_configs` is a clone of the HashMap contents, not the guard, so no explicit drop needed.
    let (loaded_models, all_configs): (HashMap<_, _>, _) = {
        let models = state.models.read().await;
        let configs = state.model_configs.read().await;
        // Collect (config_name, backend_url) for Ready backends
        let loaded: HashMap<_, _> = models
            .iter()
            .filter_map(|(name, ms)| {
                if let crate::proxy::ModelState::Ready { backend_url, .. } = ms {
                    Some((name.clone(), backend_url.clone()))
                } else {
                    None
                }
            })
            .collect();
        (loaded, configs.clone())
    }; // locks dropped

    // 2. Fetch capabilities for all loaded backends concurrently
    let futures: Vec<_> = loaded_models
        .values()
        .map(|url| fetch_capabilities_from_backend(&state.client, url))
        .collect();
    let capabilities: Vec<(bool, bool)> = futures::future::join_all(futures).await;

    // Build a map: config_name -> ModelCapabilities
    let cap_map: HashMap<_, _> = loaded_models
        .keys()
        .zip(capabilities)
        .map(|(name, (tc, r))| {
            (
                name.clone(),
                ModelCapabilities {
                    tool_call: tc,
                    reasoning: r,
                },
            )
        })
        .collect();

    // 3. Build model entries with capabilities
    let mut models: Vec<serde_json::Value> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    for (id, cfg) in all_configs.iter().filter(|(_, cfg)| cfg.enabled) {
        let caps = cap_map.get(id);
        if let Some(entry) = build_model_entry(&state, id, cfg, caps).await {
            if let Some(api_id) = entry.get("id").and_then(|v| v.as_str()) {
                seen_ids.insert(api_id.to_lowercase());
            }
            models.push(entry);
        }
    }

    // 4. Add alias entries — inherit capabilities from target model
    let aliases = state.aliases.read().await;
    for (alias_name, resolved_model) in aliases.iter() {
        if seen_ids.contains(&alias_name.to_lowercase()) {
            continue;
        }

        let resolved_lower = resolved_model.to_lowercase();
        let target_cfg = all_configs.iter().find(|(_, cfg)| {
            cfg.enabled
                && (cfg.api_name.as_ref().map(|s| s.to_lowercase()) == Some(resolved_lower.clone())
                    || cfg.model.as_ref().map(|s| s.to_lowercase()) == Some(resolved_lower.clone()))
        });

        if let Some((key, cfg)) = target_cfg {
            let caps = cap_map.get(key);
            if let Some(mut entry) = build_model_entry(&state, key, cfg, caps).await {
                entry["id"] = serde_json::json!(alias_name.to_lowercase());
                // Derive a display name from the alias slug (not from the target model).
                let alias_display = alias_name
                    .replace(['-', '_'], " ")
                    .split_whitespace()
                    .map(super::utils::capitalize_first)
                    .collect::<Vec<_>>()
                    .join(" ");
                entry["name"] = serde_json::json!(alias_display);
                models.push(entry);
                seen_ids.insert(alias_name.to_lowercase());
            }
        }
    }
    drop(aliases);

    Json(serde_json::json!({ "models": models }))
}

/// Extract capability flags from a /props response body.
/// Returns (tool_call, reasoning) tuple. Defaults to (true, false) on any error.
pub(super) fn extract_capabilities(body: &[u8]) -> (bool, bool) {
    let value = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(v) => v,
        Err(_) => return (true, false),
    };

    let mut tool_call = true; // default
    let mut reasoning = false; // default

    // Check chat_template_caps.supports_tool_calls
    if let Some(supports_tool_calls) = value
        .get("chat_template_caps")
        .and_then(|c| c.get("supports_tool_calls"))
        .and_then(|v| v.as_bool())
    {
        tool_call = supports_tool_calls;
    }

    // Check chat_template_caps.supports_preserve_reasoning
    if let Some(supports_reasoning) = value
        .get("chat_template_caps")
        .and_then(|c| c.get("supports_preserve_reasoning"))
        .and_then(|v| v.as_bool())
    {
        reasoning = supports_reasoning;
    }

    // Also check default_generation_settings.params.reasoning_format != "none"
    if let Some(reasoning_format) = value
        .get("default_generation_settings")
        .and_then(|d| d.get("params"))
        .and_then(|p| p.get("reasoning_format"))
        .and_then(|v| v.as_str())
    {
        if !reasoning_format.eq_ignore_ascii_case("none") {
            reasoning = true;
        }
    }

    (tool_call, reasoning)
}

/// Query a single backend's /props endpoint and extract capability flags.
/// Returns (tool_call, reasoning) tuple. Defaults to (true, false) on any error.
pub(super) async fn fetch_capabilities_from_backend(
    client: &reqwest::Client,
    backend_url: &str,
) -> (bool, bool) {
    let url = format!("{}/props", backend_url);
    match client
        .get(&url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
    {
        Ok(resp) => match resp.bytes().await {
            Ok(bytes) => extract_capabilities(&bytes),
            Err(_) => (true, false),
        },
        Err(_) => (true, false),
    }
}
