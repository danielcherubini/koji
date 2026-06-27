use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::time::Duration;

use super::types::ModelResponse;
use crate::proxy::process::{force_kill_process_group, is_process_group_alive, kill_process_group};
use crate::proxy::ModelState;
use crate::proxy::ProxyState;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, Default)]
struct ModelCapabilities {
    tool_call: bool,
    reasoning: bool,
}

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

/// Resolve an incoming `:id` path param to the internal config_key.
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
async fn resolve_model_id(state: &ProxyState, raw: &str) -> String {
    if let Ok(id) = raw.parse::<i64>() {
        let configs = state.model_configs.read().await;
        if let Some((key, _)) = configs.iter().find(|(_, c)| c.db_id == Some(id)) {
            return key.clone();
        }
    }
    if raw.contains('/') {
        return raw.to_lowercase().replace('/', "--");
    }
    raw.to_string()
}

/// Handle listing all configured models (Tama management API).
pub async fn handle_tama_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let models = state.build_status_response().await;
    let models_obj = models.get("models").and_then(|v| v.as_object());

    let result: Vec<serde_json::Value> = models_obj
        .into_iter()
        .flat_map(|models_obj| {
            models_obj.iter().filter_map(|(_key, model)| {
                model
                    .as_object()
                    .and_then(|model| serde_json::to_value(model).ok())
            })
        })
        .collect();

    Json(serde_json::json!({
        "models": result
    }))
}

/// Handle getting a single model's state (Tama management API).
pub async fn handle_tama_get_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;
    // Check if already loaded (by server name or model name)
    let model_state = state.get_model_state(&model_id).await;

    if let Some(ms) = model_state {
        let owned_by = ms.backend();
        let created = match ms.load_time() {
            Some(load_time) => load_time
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::ZERO)
                .as_secs(),
            None => 0,
        };
        return Json(serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": created,
            "owned_by": owned_by,
            "ready": ms.is_ready()
        }))
        .into_response();
    }

    // Check if it's a configured (but not loaded) model
    let model_configs = state.model_configs.read().await;
    let config = state.config.read().await;
    let servers = config.resolve_servers_for_model(&model_configs, &model_id);
    if let Some((config_name, server_cfg, _)) = servers.first() {
        if server_cfg.enabled {
            return Json(serde_json::json!({
                "id": config_name,
                "object": "model",
                "created": 0,
                "owned_by": server_cfg.backend,
                "ready": false
            }))
            .into_response();
        }
    }

    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "message": "Model not found",
                "type": "NotFoundError"
            }
        })),
    )
        .into_response()
}

/// Handle loading a model (Tama management API).
pub async fn handle_tama_load_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;
    let _ = state.evict_lru_if_needed().await;
    match state.load_model(&model_id, None).await {
        Ok(server_name) => {
            let model_state = state.get_model_state(&server_name).await;
            let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
            Json(ModelResponse {
                id: model_id,
                loaded,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Failed to load model: {}", e),
                    "type": "LoadModelError"
                }
            })),
        )
            .into_response(),
    }
}

/// Handle cancelling a loading model (Tama management API).
///
/// Kills a loading backend process and returns the model to idle. The handler
/// uses a read-write lock double-check pattern to avoid races with the
/// load_model path.
///
/// There is a narrow race window where load_model's health check succeeds after
/// cancel removes the entry, and load_model then calls mgr.insert_active()
/// unconditionally. A future fix would add a re-check in load_model before
/// insert_active under the write lock.
pub async fn handle_tama_cancel_load(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;

    // ── Step b: read lock — initial check ──────────────────────────────
    let (server_name, pid) = {
        let models = state.models.read().await;
        let entry = match models.get(&model_id) {
            Some(e) => e,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is not currently loading",
                            "type": "ModelNotLoadingError"
                        }
                    })),
                )
                    .into_response();
            }
        };

        match entry {
            ModelState::Starting { backend_pid, .. } => (model_id.clone(), *backend_pid),
            ModelState::Ready { .. } => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is already loaded",
                            "type": "ModelAlreadyLoadedError"
                        }
                    })),
                )
                    .into_response();
            }
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is not currently loading",
                            "type": "ModelNotLoadingError"
                        }
                    })),
                )
                    .into_response();
            }
        }
    }; // read lock dropped

    // ── Step c+d: write lock — re-validate and remove ──────────────────
    {
        let mut models = state.models.write().await;
        match models.get(&server_name) {
            Some(ModelState::Starting { .. }) => {
                models.remove(&server_name);
            }
            Some(ModelState::Ready { .. }) => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is already loaded",
                            "type": "ModelAlreadyLoadedError"
                        }
                    })),
                )
                    .into_response();
            }
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is not currently loading",
                            "type": "ModelNotLoadingError"
                        }
                    })),
                )
                    .into_response();
            }
        }
    } // write lock dropped

    // ── Step f: kill process group ─────────────────────────────────────
    if pid > 0 {
        // First attempt: SIGTERM
        if let Err(e) = kill_process_group(pid).await {
            warn!("Cancel kill failed for '{}': {}", server_name, e);
        }

        // Poll up to 2s for the group to die (every 250ms)
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !is_process_group_alive(pid) {
                break;
            }
        }

        // Escalate: SIGKILL if still alive
        if is_process_group_alive(pid) {
            if let Err(e) = force_kill_process_group(pid).await {
                warn!("Cancel force kill failed for '{}': {}", server_name, e);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    // ── Step g: clean up DB ────────────────────────────────────────────
    if let Some(mgr) = state.model_mgr() {
        if let Err(e) = mgr.remove_active(&server_name) {
            warn!("Failed to remove active entry for '{}': {}", server_name, e);
        }
    }

    // ── Step h: log ────────────────────────────────────────────────────
    info!("Model '{}' cancel completed", server_name);

    // ── Step i: return response ────────────────────────────────────────
    Json(ModelResponse {
        id: model_id,
        loaded: false,
    })
    .into_response()
}

/// Handle unloading a model (Tama management API).
pub async fn handle_tama_unload_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;
    // Get the server name for this model
    let server_name = state.get_available_server_for_model(&model_id).await;

    match server_name {
        Some(server_name) => {
            // Unload the model
            match state.unload_model(&server_name).await {
                Ok(_) => {
                    let model_state = state.get_model_state(&model_id).await;
                    let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
                    Json(ModelResponse {
                        id: model_id,
                        loaded,
                    })
                    .into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Failed to unload model: {}", e),
                            "type": "UnloadModelError"
                        }
                    })),
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Model not configured or not loaded",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response(),
    }
}

/// Build a model JSON entry from a config entry.
async fn build_model_entry(
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
                    .map(capitalize_first)
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
fn extract_capabilities(body: &[u8]) -> (bool, bool) {
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
async fn fetch_capabilities_from_backend(
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;
    use std::time::Instant;

    use super::*;
    use crate::config::{Config, ModelConfig, ModelModalities};
    use axum::body::Body;
    use axum::extract::Request;
    use axum::Router;
    use tower::ServiceExt;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;

    /// Valid response with supports_tool_calls: true → (true, false)
    #[test]
    fn test_extract_capabilities_tool_calls_true() {
        let body = r#"{
            "chat_template_caps": {
                "supports_tool_calls": true
            }
        }"#;
        let (tool_call, reasoning) = extract_capabilities(body.as_bytes());
        assert!(tool_call, "tool_call should be true");
        assert!(!reasoning, "reasoning should default to false");
    }

    /// Valid response with supports_preserve_reasoning: true → (true, true)
    #[test]
    fn test_extract_capabilities_preserve_reasoning_true() {
        let body = r#"{
            "chat_template_caps": {
                "supports_tool_calls": true,
                "supports_preserve_reasoning": true
            }
        }"#;
        let (tool_call, reasoning) = extract_capabilities(body.as_bytes());
        assert!(tool_call);
        assert!(
            reasoning,
            "reasoning should be true from supports_preserve_reasoning"
        );
    }

    /// Valid response with reasoning_format: "xml" → (true, true)
    #[test]
    fn test_extract_capabilities_reasoning_format_xml() {
        let body = r#"{
            "default_generation_settings": {
                "params": {
                    "reasoning_format": "xml"
                }
            }
        }"#;
        let (tool_call, reasoning) = extract_capabilities(body.as_bytes());
        assert!(tool_call, "tool_call should default to true");
        assert!(
            reasoning,
            "reasoning should be true from reasoning_format != none"
        );
    }

    /// Missing chat_template_caps → (true, false) defaults
    #[test]
    fn test_extract_capabilities_missing_chat_template_caps() {
        let body = r#"{}"#;
        let (tool_call, reasoning) = extract_capabilities(body.as_bytes());
        assert!(tool_call, "tool_call should default to true");
        assert!(!reasoning, "reasoning should default to false");
    }

    /// Invalid JSON → (true, false) defaults
    #[test]
    fn test_extract_capabilities_invalid_json() {
        let body = b"not json at all";
        let (tool_call, reasoning) = extract_capabilities(body);
        assert!(tool_call, "tool_call should default to true on parse error");
        assert!(
            !reasoning,
            "reasoning should default to false on parse error"
        );
    }

    /// Empty body → (true, false) defaults
    #[test]
    fn test_extract_capabilities_empty_body() {
        let body = b"";
        let (tool_call, reasoning) = extract_capabilities(body);
        assert!(tool_call, "tool_call should default to true on empty body");
        assert!(
            !reasoning,
            "reasoning should default to false on empty body"
        );
    }

    // ── Integration tests for capability fields ──────────────────────────

    /// Helper: create a ProxyState with a single model config.
    async fn create_state_with_model(model_cfg: ModelConfig) -> Arc<ProxyState> {
        let config = Config::default();
        let state = ProxyState::new(config, None);
        let mut mc = state.model_configs.write().await;
        mc.insert("test-model".to_string(), model_cfg);
        drop(mc);
        Arc::new(state)
    }

    /// Helper: build the router and call handle_opencode_list_models.
    async fn call_list_models(state: Arc<ProxyState>) -> serde_json::Value {
        let app = Router::new()
            .route(
                "/v1/opencode/models",
                axum::routing::get(handle_opencode_list_models),
            )
            .with_state(state);

        let request = Request::get("/v1/opencode/models")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Loaded model: capabilities from /props appear in opencode response.
    #[tokio::test]
    async fn test_loaded_model_capabilities_from_props() {
        let mock_server = MockServer::start().await;

        // Mock /props with tool_call: true, reasoning: true
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "chat_template_caps": {
                    "supports_tool_calls": true,
                    "supports_preserve_reasoning": true
                }
            })))
            .mount(&mock_server)
            .await;

        let backend_url = mock_server.uri();
        let state = crate::proxy::handlers::tests::create_state_with_two_backends(
            &backend_url,
            &backend_url,
        )
        .await;

        let result = call_list_models(state).await;
        let models = result.get("models").unwrap().as_array().unwrap();

        // model-a and model-b are loaded, model-c is not
        for model in models {
            let id = model.get("id").unwrap().as_str().unwrap();
            if id == "api-model-a" || id == "api-model-b" {
                // Loaded models: should have capabilities from /props
                assert!(
                    model.get("tool_call").unwrap().as_bool().unwrap(),
                    "tool_call should be true for loaded model {}",
                    id
                );
                assert!(
                    model.get("reasoning").unwrap().as_bool().unwrap(),
                    "reasoning should be true for loaded model {}",
                    id
                );
                assert!(
                    model.get("temperature").unwrap().as_bool().unwrap(),
                    "temperature should always be true"
                );
            } else if id == "api-model-c" {
                // Unloaded model: should have defaults
                assert!(
                    model.get("tool_call").unwrap().as_bool().unwrap(),
                    "tool_call should default to true for unloaded model"
                );
                assert!(
                    !model.get("reasoning").unwrap().as_bool().unwrap(),
                    "reasoning should default to false for unloaded model"
                );
            }
        }
    }

    /// Unloaded model: defaults tool_call: true, reasoning: false.
    #[tokio::test]
    async fn test_unloaded_model_defaults() {
        let state = create_state_with_model(ModelConfig {
            backend: "llama_cpp".to_string(),
            api_name: Some("test-unloaded".to_string()),
            model: Some("test/unloaded".to_string()),
            enabled: true,
            ..Default::default()
        })
        .await;

        let result = call_list_models(state).await;
        let models = result.get("models").unwrap().as_array().unwrap();
        assert_eq!(models.len(), 1);

        let model = &models[0];
        assert!(
            model.get("tool_call").unwrap().as_bool().unwrap(),
            "tool_call should default to true for unloaded model"
        );
        assert!(
            !model.get("reasoning").unwrap().as_bool().unwrap(),
            "reasoning should default to false for unloaded model"
        );
        assert!(
            model.get("temperature").unwrap().as_bool().unwrap(),
            "temperature should always be true"
        );
    }

    /// Model with modalities.input containing "image" → attachment: true.
    #[tokio::test]
    async fn test_model_with_image_modality_has_attachment() {
        let state = create_state_with_model(ModelConfig {
            backend: "llama_cpp".to_string(),
            api_name: Some("test-vision".to_string()),
            model: Some("test/vision".to_string()),
            enabled: true,
            modalities: Some(ModelModalities {
                input: vec!["text".to_string(), "image".to_string()],
                output: vec!["text".to_string()],
            }),
            ..Default::default()
        })
        .await;

        let result = call_list_models(state).await;
        let models = result.get("models").unwrap().as_array().unwrap();
        assert_eq!(models.len(), 1);

        let model = &models[0];
        assert!(
            model.get("attachment").unwrap().as_bool().unwrap(),
            "attachment should be true when modalities.input contains 'image'"
        );
    }

    /// Model with modalities.input: ["text"] → attachment: false.
    #[tokio::test]
    async fn test_model_text_only_modality_no_attachment() {
        let state = create_state_with_model(ModelConfig {
            backend: "llama_cpp".to_string(),
            api_name: Some("test-text".to_string()),
            model: Some("test/text".to_string()),
            enabled: true,
            modalities: Some(ModelModalities {
                input: vec!["text".to_string()],
                output: vec!["text".to_string()],
            }),
            ..Default::default()
        })
        .await;

        let result = call_list_models(state).await;
        let models = result.get("models").unwrap().as_array().unwrap();
        assert_eq!(models.len(), 1);

        let model = &models[0];
        assert!(
            !model.get("attachment").unwrap().as_bool().unwrap(),
            "attachment should be false when modalities.input only contains 'text'"
        );
    }

    /// Model with no modalities → attachment: false.
    #[tokio::test]
    async fn test_model_no_modality_no_attachment() {
        let state = create_state_with_model(ModelConfig {
            backend: "llama_cpp".to_string(),
            api_name: Some("test-nomod".to_string()),
            model: Some("test/nomod".to_string()),
            enabled: true,
            ..Default::default()
        })
        .await;

        let result = call_list_models(state).await;
        let models = result.get("models").unwrap().as_array().unwrap();
        assert_eq!(models.len(), 1);

        let model = &models[0];
        assert!(
            !model.get("attachment").unwrap().as_bool().unwrap(),
            "attachment should be false when modalities is None"
        );
    }

    /// Alias entries inherit capabilities from target model.
    #[tokio::test]
    async fn test_alias_inherits_capabilities() {
        let mock_server = MockServer::start().await;

        // Mock /props with reasoning: true
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "chat_template_caps": {
                    "supports_tool_calls": true,
                    "supports_preserve_reasoning": true
                }
            })))
            .mount(&mock_server)
            .await;

        let backend_url = mock_server.uri();
        let state = crate::proxy::handlers::tests::create_state_with_two_backends(
            &backend_url,
            &backend_url,
        )
        .await;

        // Add an alias pointing to model-a
        {
            let mut aliases = state.aliases.write().await;
            aliases.insert("my-alias".to_string(), "api-model-a".to_string());
        }

        let result = call_list_models(state).await;
        let models = result.get("models").unwrap().as_array().unwrap();

        // Find the alias entry
        let alias_entry = models
            .iter()
            .find(|m| m.get("id").unwrap().as_str().unwrap() == "my-alias")
            .expect("alias entry should exist");

        assert!(
            alias_entry.get("tool_call").unwrap().as_bool().unwrap(),
            "alias should inherit tool_call from target"
        );
        assert!(
            alias_entry.get("reasoning").unwrap().as_bool().unwrap(),
            "alias should inherit reasoning from target"
        );
        assert!(
            alias_entry.get("temperature").unwrap().as_bool().unwrap(),
            "alias should have temperature: true"
        );
    }

    /// Alias entry's name is derived from the alias slug, not the target model.
    #[tokio::test]
    async fn test_alias_name_derived_from_slug() {
        let mock_server = MockServer::start().await;

        // Mock /props
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "chat_template_caps": {
                    "supports_tool_calls": true,
                    "supports_preserve_reasoning": false
                }
            })))
            .mount(&mock_server)
            .await;

        let backend_url = mock_server.uri();
        let state = crate::proxy::handlers::tests::create_state_with_two_backends(
            &backend_url,
            &backend_url,
        )
        .await;

        // Add an alias pointing to model-a (model: "test/model-a" → name would be "Test: Model A")
        {
            let mut aliases = state.aliases.write().await;
            aliases.insert("my-awesome-alias".to_string(), "api-model-a".to_string());
        }

        let result = call_list_models(state).await;
        let models = result.get("models").unwrap().as_array().unwrap();

        // Find the alias entry
        let alias_entry = models
            .iter()
            .find(|m| m.get("id").unwrap().as_str().unwrap() == "my-awesome-alias")
            .expect("alias entry should exist");

        // The name should be derived from the alias slug, NOT from the target model.
        assert_eq!(
            alias_entry.get("name").unwrap().as_str().unwrap(),
            "My Awesome Alias",
            "Alias name should be derived from the alias slug, not the target model"
        );

        // The target model entry should still have its own name
        let target_entry = models
            .iter()
            .find(|m| m.get("id").unwrap().as_str().unwrap() == "api-model-a")
            .expect("target model entry should exist");
        assert_eq!(
            target_entry.get("name").unwrap().as_str().unwrap(),
            "Test: Model A",
            "Target model should keep its own name"
        );
    }

    // ── fetch_capabilities_from_backend HTTP failure tests ────────────────

    /// Backend returns 500 → safe defaults (true, false).
    #[tokio::test]
    async fn test_fetch_capabilities_backend_500_returns_defaults() {
        let mock_server = MockServer::start().await;

        // Mock /props returning a 500 Internal Server Error
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let (tool_call, reasoning) =
            fetch_capabilities_from_backend(&client, &mock_server.uri()).await;

        assert!(tool_call, "tool_call should default to true on 500 error");
        assert!(!reasoning, "reasoning should default to false on 500 error");
    }

    /// Backend unreachable (no mock) → safe defaults (true, false) with timeout.
    #[tokio::test]
    async fn test_fetch_capabilities_unreachable_backend_returns_defaults() {
        // Use a local address with no listener — the 3-second timeout prevents
        // hanging indefinitely.
        let unreachable_url = "http://127.0.0.1:19999";

        let client = reqwest::Client::new();
        let (tool_call, reasoning) =
            fetch_capabilities_from_backend(&client, unreachable_url).await;

        assert!(
            tool_call,
            "tool_call should default to true on unreachable backend"
        );
        assert!(
            !reasoning,
            "reasoning should default to false on unreachable backend"
        );
    }

    // ── Cancel loading model tests ──────────────────────────────────────────

    /// Cancel returns 200 for a Starting model with a PID.
    /// The fake PID (99999) has no real process group, so kill_process_group
    /// returns Ok(()) on ESRCH and is_process_group_alive returns false.
    #[tokio::test]
    async fn test_cancel_returns_200_for_starting_model() {
        let state = create_state_with_model(ModelConfig {
            backend: "llama_cpp".to_string(),
            api_name: Some("test-model".to_string()),
            model: Some("test/model".to_string()),
            enabled: true,
            ..Default::default()
        })
        .await;

        // Insert a Starting entry with a fake PID
        state.models.write().await.insert(
            "test-model".to_string(),
            ModelState::Starting {
                model_name: "test-model".into(),
                backend: "llama_cpp".into(),
                backend_url: String::new(),
                backend_pid: 99999,
                last_accessed: Instant::now(),
                start_time: Instant::now(),
                consecutive_failures: Arc::new(AtomicU32::new(0)),
                failure_timestamp: None,
            },
        );

        // Clone the Arc before moving state into the router
        let state_clone = state.clone();

        let app = Router::new()
            .route(
                "/tama/v1/models/:id/cancel",
                axum::routing::post(handle_tama_cancel_load),
            )
            .with_state(state);

        let request = Request::post("/tama/v1/models/test-model/cancel")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::OK, "Expected 200 OK");
        assert_eq!(json["loaded"], false, "Expected loaded: false");
        assert_eq!(json["id"], "test-model", "Expected id: test-model");

        // Model entry should be removed
        assert!(
            state_clone.models.read().await.get("test-model").is_none(),
            "Model entry should be removed after cancel"
        );
    }

    /// Cancel returns 409 for a Ready (already loaded) model.
    #[tokio::test]
    async fn test_cancel_returns_409_for_ready_model() {
        let state = create_state_with_model(ModelConfig {
            backend: "llama_cpp".to_string(),
            api_name: Some("test-model".to_string()),
            model: Some("test/model".to_string()),
            enabled: true,
            ..Default::default()
        })
        .await;

        // Insert a Ready entry
        state.models.write().await.insert(
            "test-model".to_string(),
            ModelState::Ready {
                model_name: "test-model".to_string(),
                backend: "llama_cpp".to_string(),
                backend_pid: 12345,
                backend_url: "http://127.0.0.1:1234".to_string(),
                load_time: std::time::SystemTime::now(),
                last_accessed: Instant::now(),
                consecutive_failures: Arc::new(AtomicU32::new(0)),
                failure_timestamp: None,
                restart_count: 0,
            },
        );

        let app = Router::new()
            .route(
                "/tama/v1/models/:id/cancel",
                axum::routing::post(handle_tama_cancel_load),
            )
            .with_state(state);

        let request = Request::post("/tama/v1/models/test-model/cancel")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "Expected 409 Conflict for Ready model"
        );
        assert_eq!(
            json["error"]["type"], "ModelAlreadyLoadedError",
            "Expected ModelAlreadyLoadedError type"
        );
    }

    /// Cancel returns 404 for a non-existing model.
    #[tokio::test]
    async fn test_cancel_returns_404_for_non_existing_model() {
        let state = create_state_with_model(ModelConfig {
            backend: "llama_cpp".to_string(),
            api_name: Some("test-model".to_string()),
            model: Some("test/model".to_string()),
            enabled: true,
            ..Default::default()
        })
        .await;

        // No model entries in the models map

        let app = Router::new()
            .route(
                "/tama/v1/models/:id/cancel",
                axum::routing::post(handle_tama_cancel_load),
            )
            .with_state(state);

        let request = Request::post("/tama/v1/models/nonexistent/cancel")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "Expected 404 Not Found for non-existing model"
        );
        assert_eq!(
            json["error"]["type"], "ModelNotLoadingError",
            "Expected ModelNotLoadingError type"
        );
    }

    /// Cancel returns 404 for a Failed model (not in a loading state).
    #[tokio::test]
    async fn test_cancel_returns_404_for_failed_model() {
        let state = create_state_with_model(ModelConfig {
            backend: "llama_cpp".to_string(),
            api_name: Some("test-model".to_string()),
            model: Some("test/model".to_string()),
            enabled: true,
            ..Default::default()
        })
        .await;

        // Insert a Failed entry
        state.models.write().await.insert(
            "test-model".to_string(),
            ModelState::Failed {
                model_name: "test-model".to_string(),
                backend: "llama_cpp".to_string(),
                error: "Some error".to_string(),
            },
        );

        let app = Router::new()
            .route(
                "/tama/v1/models/:id/cancel",
                axum::routing::post(handle_tama_cancel_load),
            )
            .with_state(state);

        let request = Request::post("/tama/v1/models/test-model/cancel")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "Expected 404 Not Found for Failed model"
        );
        assert_eq!(
            json["error"]["type"], "ModelNotLoadingError",
            "Expected ModelNotLoadingError type for Failed model"
        );
    }
}
