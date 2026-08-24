//! Model registry state: loaded backends, model configs, and aliases.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

/// Caches describing which models exist and which are currently loaded.
///
/// The lifecycle truth for *running* models is the live wire rows
/// ([`crate::proxy::rows`]), aggregated from each tamad's 1 Hz process
/// snapshot — this registry holds no per-model cache. What
/// remains is configuration (model configs, aliases) plus the proxy-owned
/// LRU access-time map that drives idle-timeout / LRU eviction (a write-side
/// only: reads of *state* come from the rows).
#[derive(Clone, Default)]
pub(crate) struct RegistryState {
    /// model_name → last request/access Instant, proxy-owned (LRU / idle
    /// decision). Survives the mirror deletion as a lightweight write-side.
    pub(crate) last_accessed: Arc<RwLock<HashMap<String, Instant>>>,
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

    /// Record that `backend_name` was just accessed (LRU / idle tracking).
    pub(crate) async fn update_last_accessed(&self, backend_name: &str) {
        let mut lru = self.last_accessed.write().await;
        lru.insert(backend_name.to_string(), Instant::now());
    }

    /// Return the proxy-owned last-access Instant for a backend, if any.
    pub(crate) async fn last_accessed_time(&self, backend_name: &str) -> Option<Instant> {
        let lru = self.last_accessed.read().await;
        lru.get(backend_name).copied()
    }

    /// Drop the per-key last-access entry for `backend_name` (LRU / idle
    /// bookkeeping). Called by every unload path (the proxy's own
    /// `ProxyState::unload_model` and the management-API unload handler)
    /// so an unloaded model never keeps a dead entry until its next
    /// access. Idempotent: a missing key is a no-op.
    pub(crate) async fn prune_last_accessed(&self, backend_name: &str) {
        self.last_accessed.write().await.remove(backend_name);
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `prune_last_accessed` removes the per-key last-access entry so an
    /// unloaded model does not keep a dead LRU/idle entry until its next
    /// access. Both unload paths funnel through this (the proxy's own
    /// `ProxyState::unload_model` and the management-API unload handler).
    #[tokio::test]
    async fn test_prune_last_accessed_removes_entry() {
        let registry = RegistryState::new();
        registry.update_last_accessed("owner--model").await;
        assert!(
            registry.last_accessed_time("owner--model").await.is_some(),
            "precondition: the access entry is present"
        );

        registry.prune_last_accessed("owner--model").await;

        assert!(
            registry.last_accessed_time("owner--model").await.is_none(),
            "the unloaded model's entry must be gone"
        );
        // A repeat prune (double-unload, or a model that never accessed
        // the proxy) must be a harmless no-op.
        registry.prune_last_accessed("owner--model").await;
        registry.prune_last_accessed("never-seen").await;
    }
}
