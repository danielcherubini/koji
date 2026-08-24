//! Model registry state: loaded backends, model configs, and aliases.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use crate::proxy::types::BackendState;

/// Caches describing which models exist and which are currently loaded.
#[derive(Clone, Default)]
pub(crate) struct RegistryState {
    /// model_name → BackendState for all known backends (loaded, starting, failed, unloading).
    pub(crate) models: Arc<RwLock<HashMap<String, BackendState>>>,
    /// model_name → ModelConfig for all configured models.
    pub(crate) model_configs: Arc<RwLock<HashMap<String, crate::config::ModelConfig>>>,
    /// alias_name → resolved model name (api_name or repo_id).
    /// Only enabled aliases are cached. Populated from DB on init and reload.
    pub(crate) aliases: Arc<RwLock<HashMap<String, String>>>,
}

impl RegistryState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Get the state of a loaded model (backend).
    pub(crate) async fn get_model_state(&self, backend_name: &str) -> Option<BackendState> {
        let models = self.models.read().await;
        models.get(backend_name).cloned()
    }

    /// Update the last accessed time for a backend.
    pub(crate) async fn update_last_accessed(&self, backend_name: &str) {
        let mut models = self.models.write().await;
        if let Some(state) = models.get_mut(backend_name) {
            match state {
                BackendState::Starting { last_accessed, .. } => {
                    *last_accessed = Instant::now();
                }
                BackendState::Ready { last_accessed, .. } => {
                    *last_accessed = Instant::now();
                }
                BackendState::Unloading { last_accessed, .. } => {
                    *last_accessed = Instant::now();
                }
                BackendState::Failed { .. } => {}
            }
        }
    }

    /// Return the names of all loaded backends that are TTS (text-to-speech) backends.
    /// - If `name` is an alias → returns the resolved model name (api_name or repo_id)
    /// - If `name` is not an alias → returns `name` unchanged (pass-through)
    pub(crate) async fn resolve_alias(&self, name: &str) -> String {
        let aliases = self.aliases.read().await;
        if let Some(resolved) = aliases.get(name) {
            return resolved.clone();
        }
        name.to_string()
    }

    /// Replace the in-memory model configs with newly loaded data.
    pub(crate) async fn reload_model_configs(
        &self,
        configs: HashMap<String, crate::config::ModelConfig>,
    ) {
        *self.model_configs.write().await = configs;
    }

    /// Replace the in-memory alias map with newly loaded pairs.
    ///
    /// Populates the in-memory alias map with enabled aliases only.
    /// Disabled aliases are filtered out by the DB query.
    pub(crate) async fn reload_aliases(&self, pairs: Vec<(String, String)>) {
        let mut aliases = self.aliases.write().await;
        *aliases = pairs.into_iter().collect();
    }

    /// Return the names of all loaded backends that are TTS (text-to-speech) backends.
    pub(crate) async fn tts_backend_names(&self) -> Vec<String> {
        self.models
            .read()
            .await
            .iter()
            .filter(|(_, ms)| ms.is_tts_backend())
            .map(|(name, _)| name.clone())
            .collect()
    }
}
