//! Model rename functionality for ProxyState.

use anyhow::Result;

use crate::proxy::types::ProxyState;

impl ProxyState {
    /// Rename a model in the configuration and in-memory state.
    ///
    /// Logic:
    /// - Validates that `new_name` is not empty and differs from `old_name`
    /// - Takes a write lock on `self.config`:
    ///   - Checks `config.models` contains `old_name`
    ///   - Checks `config.models` does NOT contain `new_name` (error: "name already taken")
    ///   - Removes the entry at `old_name`, inserts at `new_name`
    ///   - Attempts `config.save()`
    ///   - If save fails: rollback — remove `new_name`, re-insert at `old_name`, return error
    /// - Writes nothing else: pure in-cache bookkeeping (no registry ops).
    ///   - If `old_name` exists in the map, removes and re-inserts at `new_name`
    pub async fn rename_model(&self, old_name: &str, new_name: &str) -> Result<()> {
        // Validate inputs
        if new_name.is_empty() {
            anyhow::bail!("new name cannot be empty");
        }
        if old_name == new_name {
            anyhow::bail!("old name and new name must differ");
        }

        // Lock config and model configs and perform rename
        let _config = self.config.write().await;
        let mut model_configs = self.registry.model_configs.write().await;

        // Check old name exists
        if !model_configs.contains_key(old_name) {
            anyhow::bail!("model '{}' does not exist", old_name);
        }

        // Check new name doesn't exist
        if model_configs.contains_key(new_name) {
            anyhow::bail!("model name '{}' already taken", new_name);
        }

        // Remove old entry
        let old_config = model_configs.remove(old_name).unwrap();

        // Insert new entry
        model_configs.insert(new_name.to_string(), old_config.clone());

        // Attempt to save config to DB instead of TOML
        let mgr = self.model_mgr();
        if let Some(mc) = model_configs.get(new_name) {
            let mc = mc.clone();
            if let Err(e) = mgr.save_model_config(new_name, &mc).await {
                tracing::error!(name = %new_name, error = %e, "Failed to save renamed model config to DB");
                // We don't rollback here because the in-memory state is updated,
                // and DB update is best-effort.
            } else {
                // Successfully saved new name, now remove the old config entry to avoid orphans
                // Convert old_name (double-dash config key) to repo_id, then look up model_id
                let old_repo_id = crate::models::config_key_to_repo_id(old_name);
                if let Some(record) = mgr.get_config_by_repo_id(&old_repo_id).await.ok().flatten() {
                    if let Err(e) = mgr.delete_config(record.id).await {
                        tracing::error!(name = %old_name, error = %e, "Failed to delete old model config after rename");
                    }
                }
            }
        }

        drop(_config);
        drop(model_configs);

        // Migrate the LRU access-time entry from old name to new name (the
        // model mirror is gone, plan-193 T5c).
        {
            let mut lru = self.registry.last_accessed.write().await;
            if let Some(ts) = lru.remove(old_name) {
                lru.insert(new_name.to_string(), ts);
            }
        }

        // Migrate inference_stats entry from old name to new name
        self.metrics.modify_inference_stats(|map| {
            if let Some(stats) = map.remove(old_name) {
                map.insert(new_name.to_string(), stats);
            }
        });

        Ok(())
    }
}
