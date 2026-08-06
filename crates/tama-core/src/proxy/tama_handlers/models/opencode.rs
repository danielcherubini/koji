use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::utils::{build_model_entry, OpencodeModelsResponse};
use crate::proxy::handlers::models::{
    fetch_models_from_backend, find_model_in_entries, BackendModelEntry,
};
use crate::proxy::ProxyState;
use axum::extract::State;
use axum::Json;

use super::ModelCapabilities;

/// Handle listing all enabled models for OpenCode plugin discovery.
/// Returns rich metadata including context limits, modalities, and capabilities.
/// Aliases are included with the same metadata as their target model, using the alias name as `id`.
pub async fn handle_opencode_list_models(
    state: State<Arc<ProxyState>>,
) -> Json<OpencodeModelsResponse> {
    // 1. Snapshot data under locks — clone out so locks are dropped before any .await below.
    // `all_configs` is a clone of the HashMap contents, not the guard, so no explicit drop needed.
    let (loaded_models, all_configs): (HashMap<_, _>, _) = {
        let models = state.registry.models.read().await;
        let configs = state.registry.model_configs.read().await;
        // Collect (config_name, backend_url) for Ready backends
        let loaded: HashMap<_, _> = models
            .iter()
            .filter_map(|(name, ms)| {
                if let crate::proxy::BackendState::Ready { backend_url, .. } = ms {
                    Some((name.clone(), backend_url.clone()))
                } else {
                    None
                }
            })
            .collect();
        (loaded, configs.clone())
    }; // locks dropped

    // 2. Fetch capabilities (/props) and models (/v1/models) for all loaded backends concurrently.
    // Deduplicate backend URLs — multiple configs can share the same backend.
    let unique_urls: Vec<_> = loaded_models
        .values()
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // Fetch capabilities from /props for each unique URL
    let cap_futures: Vec<_> = unique_urls
        .iter()
        .map(|url| fetch_capabilities_from_backend(&state.client, url))
        .collect();
    let cap_results: Vec<(bool, bool)> = futures::future::join_all(cap_futures).await;

    // Build url -> ModelCapabilities map
    let url_cap_map: HashMap<_, _> = unique_urls
        .iter()
        .zip(cap_results)
        .map(|(url, (tc, r))| {
            (
                url.clone(),
                ModelCapabilities {
                    tool_call: tc,
                    reasoning: r,
                },
            )
        })
        .collect();

    // Fetch /v1/models from each unique backend URL
    let model_futures: Vec<_> = unique_urls
        .iter()
        .map(|url| fetch_models_from_backend(&state, url))
        .collect();
    let model_results: Vec<Vec<BackendModelEntry>> = futures::future::join_all(model_futures).await;

    // Build url -> Vec<BackendModelEntry> map
    let url_model_map: HashMap<_, _> = unique_urls.into_iter().zip(model_results).collect();

    // Build config_name -> ModelCapabilities map (for backward compat with cap_map lookups)
    let cap_map: HashMap<_, _> = loaded_models
        .iter()
        .filter_map(|(name, url)| url_cap_map.get(url).map(|caps| (name.clone(), *caps)))
        .collect();

    // 3. Build model entries with capabilities and context lengths
    let mut models: Vec<super::utils::ModelEntry> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    for (id, cfg) in all_configs.iter().filter(|(_, cfg)| cfg.enabled) {
        let caps = cap_map.get(id);

        // Look up backend context length from /v1/models response
        let backend_ctx = loaded_models
            .get(id)
            .and_then(|url| url_model_map.get(url))
            .and_then(|entries| find_model_in_entries(entries, cfg.model.as_deref()))
            .as_ref()
            .and_then(extract_context_length_from_backend_entry);

        if let Some(entry) = build_model_entry(&state, id, cfg, caps, backend_ctx).await {
            if let Some(api_id) = entry.id.as_deref() {
                seen_ids.insert(api_id.to_string());
            }
            models.push(entry);
        }
    }

    // 4. Add alias entries — inherit capabilities and context_length from target model
    let aliases = state.registry.aliases.read().await;
    for (alias_name, resolved_model) in aliases.iter() {
        if seen_ids.contains(alias_name.as_str()) {
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
            // Look up backend context length for the target config
            let backend_ctx = loaded_models
                .get(key)
                .and_then(|url| url_model_map.get(url))
                .and_then(|entries| find_model_in_entries(entries, cfg.model.as_deref()))
                .as_ref()
                .and_then(extract_context_length_from_backend_entry);

            if let Some(mut entry) = build_model_entry(&state, key, cfg, caps, backend_ctx).await {
                entry.id = Some(alias_name.clone());
                // Derive a display name from the alias slug (not from the target model).
                let alias_display = alias_name
                    .replace(['-', '_'], " ")
                    .split_whitespace()
                    .map(super::utils::capitalize_first)
                    .collect::<Vec<_>>()
                    .join(" ");
                entry.name = alias_display;
                models.push(entry);
                seen_ids.insert(alias_name.clone());
            }
        }
    }
    drop(aliases);

    Json(OpencodeModelsResponse { models })
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

/// Extract context length from a backend /v1/models entry.
/// Checks `max_model_len` (vLLM) first, then falls back to `meta.n_ctx` (llama.cpp).
fn extract_context_length_from_backend_entry(entry: &BackendModelEntry) -> Option<u32> {
    // vLLM: max_model_len
    entry
        .extra
        .get("max_model_len")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        // llama.cpp: meta.n_ctx
        .or_else(|| {
            entry
                .extra
                .get("meta")
                .and_then(|m| m.get("n_ctx"))
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
        })
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
