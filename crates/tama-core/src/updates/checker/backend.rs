use super::{UpdateChecker, UpdateEvent};
use crate::backends::{check_latest_version, BackendType};
use crate::db;
use crate::db::queries::get_active_backend;

impl UpdateChecker {
    /// Check a single backend for updates.
    ///
    /// The `item_id` stored in the DB is `name:variant` so that multiple variants
    /// of the same backend (e.g. llama_cpp:cpu and llama_cpp:vulkan) have separate
    /// update check records and don't overwrite each other.
    pub async fn check_backend(
        &self,
        config_dir: &std::path::Path,
        backend_name: &str,
        backend_type: &BackendType,
        gpu_variant: &str,
    ) -> anyhow::Result<()> {
        // Use "name:variant" as the item_id so each variant has its own record
        let item_id = format!("{}:{}", backend_name, gpu_variant);

        // item_id is "name:variant" internally, but frontend DTO uses name-only
        #[cfg(feature = "web-ui")]
        self.emit(UpdateEvent::CheckStarted {
            item_type: "backend".to_string(),
            item_id: backend_name.to_string(),
            variant: Some(gpu_variant.to_string()),
        });

        // Sync: Get current version from DB
        let current_version = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let backend_name = backend_name.to_string();
            let gpu_variant = gpu_variant.to_string();
            move || -> anyhow::Result<Option<String>> {
                let open = db::open(&config_dir)?;
                let record = get_active_backend(&open.conn, &backend_name, &gpu_variant)?;
                Ok(record.map(|r| r.version))
            }
        })
        .await??;

        // Async: Check latest version from network
        let latest_version = match backend_type {
            BackendType::LlamaCpp | BackendType::IkLlama => {
                match check_latest_version(backend_type).await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        self.save_check_result(
                            config_dir,
                            "backend",
                            &item_id,
                            current_version.as_deref(),
                            None,
                            false,
                            "error",
                            Some(&e.to_string()),
                            None,
                        )
                        .await?;

                        #[cfg(feature = "web-ui")]
                        self.emit(UpdateEvent::CheckError {
                            item_type: "backend".to_string(),
                            item_id: backend_name.to_string(),
                            variant: Some(gpu_variant.to_string()),
                            error: e.to_string(),
                        });

                        return Ok(());
                    }
                }
            }
            BackendType::TtsKokoro | BackendType::Compaction | BackendType::Custom => None,
        };

        let update_available = latest_version
            .as_ref()
            .map(|v| current_version.as_ref().map(|c| v != c).unwrap_or(true))
            .unwrap_or(false);

        let status = if latest_version.is_none() && current_version.is_none() {
            "unknown"
        } else if update_available {
            "update_available"
        } else {
            "up_to_date"
        };

        let save_result = self
            .save_check_result(
                config_dir,
                "backend",
                &item_id,
                current_version.as_deref(),
                latest_version.as_deref(),
                update_available,
                status,
                None,
                None,
            )
            .await;

        #[cfg(feature = "web-ui")]
        if save_result.is_ok() {
            let dto = serde_json::json!({
                "item_type": "backend",
                "item_id": backend_name,
                "variant": gpu_variant,
                "current_version": current_version,
                "latest_version": latest_version,
                "update_available": update_available,
                "status": status,
                "error_message": null,
                "checked_at": chrono::Utc::now().timestamp(),
                "details_json": null,
            });
            self.emit(UpdateEvent::CheckCompleted {
                item_type: "backend".to_string(),
                item_id: backend_name.to_string(),
                variant: Some(gpu_variant.to_string()),
                dto,
            });
        } else {
            // Emit CheckError so the frontend clears the "Checking..." state
            let save_err = save_result
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            self.emit(UpdateEvent::CheckError {
                item_type: "backend".to_string(),
                item_id: backend_name.to_string(),
                variant: Some(gpu_variant.to_string()),
                error: format!("Failed to save check result: {}", save_err),
            });
        }
        save_result
    }
}
