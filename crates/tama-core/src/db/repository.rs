//! Repository layer — domain-level database operations for the API layer.
//!
//! This module provides a `Repository` struct that wraps SQLite connections
//! and exposes domain-level methods. API handlers call repository methods
//! instead of raw queries, and receive DTO types instead of DB record types.
//!
//! DB record types (`ModelConfigRecord`, `ModelFileRecord`, etc.) are
//! `pub(crate)` — they are implementation details of the DB layer.

use anyhow::Context;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ── DTO types ────────────────────────────────────────────────────────────────

/// Data Transfer Object for a model configuration.
/// Replaces `ModelConfigRecord` in the API layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfigDto {
    pub id: i64,
    pub repo_id: String,
    pub display_name: Option<String>,
    pub backend: String,
    pub gpu_variant: Option<String>,
    pub gpu_device: Option<String>,
    pub enabled: bool,
    pub selected_quant: Option<String>,
    pub selected_mmproj: Option<String>,
    pub selected_mtp_model: Option<String>,
    pub context_length: Option<u32>,
    pub num_parallel: Option<u32>,
    pub kv_unified: bool,
    pub gpu_layers: Option<u32>,
    pub cache_type_k: Option<String>,
    pub cache_type_v: Option<String>,
    pub port: Option<u16>,
    pub args: Option<String>,
    pub sampling: Option<String>,
    pub modalities: Option<String>,
    pub profile: Option<String>,
    pub api_name: Option<String>,
    pub health_check: Option<String>,
    pub hf_format: Option<String>,
    pub hf_base_model: Option<String>,
    pub hf_pipeline_tag: Option<String>,
    pub hf_total_params: Option<String>,
    pub hf_active_params: Option<String>,
    pub hf_architecture_type: Option<String>,
    pub hf_context_length: Option<u32>,
    pub hf_num_layers: Option<u32>,
    pub hf_last_modified: Option<String>,
    pub spec_decoding: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Data Transfer Object for a model file.
/// Replaces `ModelFileRecord` in the API layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFileDto {
    pub id: i64,
    pub model_id: i64,
    pub repo_id: String,
    pub filename: String,
    pub quant: Option<String>,
    pub lfs_oid: Option<String>,
    pub size_bytes: Option<i64>,
    pub downloaded_at: String,
    pub last_verified_at: Option<String>,
    pub verified_ok: Option<bool>,
    pub verify_error: Option<String>,
}

/// Data Transfer Object for a model alias.
/// Reuses the existing `AliasResponse` shape from the alias queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasDto {
    pub id: i64,
    pub name: String,
    pub model_id: i64,
    pub model_name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Parameters for inserting a benchmark result.
/// Replaces `BenchmarkInsertParams` in the API layer.
#[derive(Debug, Clone)]
pub struct BenchmarkParams {
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub engine: String,
    pub pp_sizes_json: String,
    pub tg_sizes_json: String,
    pub threads_json: Option<String>,
    pub ngl_range: Option<String>,
    pub runs: u32,
    pub warmup: u32,
    pub results_json: String,
    pub load_time_ms: Option<f64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub duration_seconds: f64,
    pub status: String,
    pub benchmark_type: Option<String>,
}

/// Data Transfer Object for a benchmark result.
/// Replaces `BenchmarkRow` in the API layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkDto {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub engine: String,
    pub pp_sizes: String,
    pub tg_sizes: String,
    pub threads: Option<String>,
    pub ngl_range: Option<String>,
    pub runs: u32,
    pub warmup: u32,
    pub results: String,
    pub load_time_ms: Option<f64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub duration_seconds: f64,
    pub status: String,
    pub benchmark_type: Option<String>,
}

/// Data Transfer Object for a download queue item.
/// Replaces `DownloadQueueItem` in the API layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadQueueDto {
    pub id: i64,
    pub job_id: String,
    pub repo_id: String,
    pub filename: String,
    pub display_name: Option<String>,
    pub status: String,
    pub bytes_downloaded: i64,
    pub total_bytes: Option<i64>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub queued_at: String,
    pub kind: String,
    pub quant: Option<String>,
    pub context_length: Option<u32>,
}

/// Data Transfer Object for an update check record.
/// Replaces `UpdateCheckRecord` in the API layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckDto {
    pub item_type: String,
    pub item_id: String,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub status: String,
    pub error_message: Option<String>,
    pub details_json: Option<String>,
    pub checked_at: i64,
}

// ── Repository ───────────────────────────────────────────────────────────────

/// Domain-level database access for API handlers.
///
/// Wraps a SQLite connection and provides high-level methods for
/// model configs, aliases, benchmarks, download queue, and update checks.
pub struct Repository {
    conn: Connection,
}

impl Repository {
    /// Open a repository at the given config directory.
    pub fn open(config_dir: &Path) -> anyhow::Result<Self> {
        let open_result = crate::db::open(config_dir)?;
        Ok(Self {
            conn: open_result.conn,
        })
    }

    /// Returns a reference to the underlying SQLite connection.
    ///
    /// This is a permanent escape hatch for callers that need raw access.
    #[allow(dead_code)]
    pub fn conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    /// Check if a model config exists by its integer id.
    pub fn model_exists(&self, id: i64) -> anyhow::Result<bool> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM model_configs WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .with_context(|| format!("Failed to check model config existence id={}", id))?;
        Ok(count > 0)
    }

    /// Get an active download queue item by repo_id and filename.
    ///
    /// Returns `None` if no active (queued/running/verifying) item exists
    /// for the given repo_id and filename combination.
    pub fn get_active_download_by_filename(
        &self,
        repo_id: &str,
        filename: &str,
    ) -> anyhow::Result<Option<DownloadQueueDto>> {
        let item = queries::get_active_item_by_repo_filename(&self.conn, repo_id, filename)
            .with_context(|| {
                format!(
                    "Failed to get active download for repo_id={} filename={}",
                    repo_id, filename
                )
            })?;
        Ok(item.map(queue_item_to_dto))
    }

    /// Load all model configs as a `HashMap<config_key, ModelConfig>`.
    ///
    /// Returns the raw `ModelConfig` type (not DTOs), suitable for benchmark
    /// operations that need config fields like `quants`, `api_name`, etc.
    pub fn load_model_configs_for_benchmarks(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, crate::config::ModelConfig>> {
        crate::db::load_model_configs(&self.conn)
            .with_context(|| "Failed to load model configs for benchmarks")
    }

    // ── Model Config ────────────────────────────────────────────────────

    /// Get a model configuration by its integer id.
    pub fn get_model_config(&self, id: i64) -> anyhow::Result<Option<ModelConfigDto>> {
        let record = queries::get_model_config(&self.conn, id)
            .with_context(|| format!("Failed to get model config id={}", id))?;
        Ok(record.map(record_to_dto))
    }

    /// Get a model configuration by repo_id.
    pub fn get_model_config_by_repo_id(
        &self,
        repo_id: &str,
    ) -> anyhow::Result<Option<ModelConfigDto>> {
        let record = queries::get_model_config_by_repo_id(&self.conn, repo_id)
            .with_context(|| format!("Failed to get model config by repo_id={}", repo_id))?;
        Ok(record.map(record_to_dto))
    }

    /// Get all files for a model config by its id.
    pub fn get_model_files(&self, config_id: i64) -> anyhow::Result<Vec<ModelFileDto>> {
        let records = queries::get_model_files(&self.conn, config_id)
            .with_context(|| format!("Failed to get model files for config id={}", config_id))?;
        Ok(records.into_iter().map(file_record_to_dto).collect())
    }

    /// Load all model configs as a HashMap<config_key, ModelConfigDto>.
    /// config_key = repo_id.to_lowercase().replace('/', "--").
    pub fn load_model_configs(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, ModelConfigDto>> {
        let records = queries::get_all_model_configs(&self.conn)?;
        let mut configs = std::collections::HashMap::new();
        for record in records {
            let config_key = record.repo_id.to_lowercase().replace('/', "--");
            configs.insert(config_key, record_to_dto(record));
        }
        Ok(configs)
    }

    // ── Aliases ─────────────────────────────────────────────────────────

    /// Load all aliases with resolved model names.
    pub fn get_all_aliases(&self) -> anyhow::Result<Vec<AliasDto>> {
        let aliases = queries::get_all_aliases(&self.conn)?;
        Ok(aliases.into_iter().map(alias_response_to_dto).collect())
    }

    /// Get a single alias by id.
    pub fn get_alias_by_id(&self, id: i64) -> anyhow::Result<Option<AliasDto>> {
        let alias = queries::get_alias_by_id(&self.conn, id)?;
        Ok(alias.map(alias_response_to_dto))
    }

    /// Insert a new alias. Returns the new row's id.
    pub fn insert_alias(
        &self,
        name: &str,
        model_id: i64,
        description: Option<&str>,
    ) -> anyhow::Result<i64> {
        let id = queries::insert_alias(&self.conn, name, model_id, description)?;
        Ok(id)
    }

    /// Update an existing alias.
    pub fn update_alias(
        &self,
        id: i64,
        name: Option<&str>,
        model_id: Option<i64>,
        description: Option<Option<&str>>,
        enabled: Option<bool>,
    ) -> anyhow::Result<()> {
        queries::update_alias(&self.conn, id, name, model_id, description, enabled)?;
        Ok(())
    }

    /// Delete an alias by id.
    pub fn delete_alias(&self, id: i64) -> anyhow::Result<()> {
        queries::delete_alias(&self.conn, id)?;
        Ok(())
    }

    // ── Benchmarks ──────────────────────────────────────────────────────

    /// Insert a benchmark result. Returns the new row id.
    pub fn insert_benchmark(&self, params: &BenchmarkParams) -> anyhow::Result<i64> {
        let insert_params = queries::BenchmarkInsertParams {
            model_id: &params.model_id,
            display_name: params.display_name.as_deref(),
            quant: params.quant.as_deref(),
            backend: &params.backend,
            engine: &params.engine,
            pp_sizes_json: &params.pp_sizes_json,
            tg_sizes_json: &params.tg_sizes_json,
            threads_json: params.threads_json.as_deref(),
            ngl_range: params.ngl_range.as_deref(),
            runs: params.runs,
            warmup: params.warmup,
            results_json: &params.results_json,
            load_time_ms: params.load_time_ms,
            vram_used_mib: params.vram_used_mib,
            vram_total_mib: params.vram_total_mib,
            duration_seconds: params.duration_seconds,
            status: &params.status,
            benchmark_type: params.benchmark_type.as_deref(),
        };
        let id = queries::insert_benchmark(&self.conn, &insert_params)?;
        Ok(id)
    }

    /// List all benchmarks ordered by created_at DESC.
    pub fn list_benchmarks(&self) -> anyhow::Result<Vec<BenchmarkDto>> {
        let rows = queries::list_benchmarks(&self.conn)?;
        Ok(rows.into_iter().map(benchmark_row_to_dto).collect())
    }

    /// Delete a benchmark by id.
    pub fn delete_benchmark(&self, id: i64) -> anyhow::Result<()> {
        queries::delete_benchmark(&self.conn, id)?;
        Ok(())
    }

    // ── Download Queue ──────────────────────────────────────────────────

    /// Get a download queue item by job_id.
    pub fn get_download_queue_item(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<DownloadQueueDto>> {
        let item = queries::get_item_by_job_id(&self.conn, job_id)?;
        Ok(item.map(queue_item_to_dto))
    }

    // ── Update Checks ───────────────────────────────────────────────────

    /// Get all update check records.
    pub fn get_all_update_checks(&self) -> anyhow::Result<Vec<UpdateCheckDto>> {
        let records = queries::get_all_update_checks(&self.conn)?;
        Ok(records
            .into_iter()
            .map(update_check_record_to_dto)
            .collect())
    }

    /// Delete an update check by item_type and item_id.
    pub fn delete_update_check(&self, item_type: &str, item_id: &str) -> anyhow::Result<()> {
        queries::delete_update_check(&self.conn, item_type, item_id)?;
        Ok(())
    }

    /// Delete update check records matching a SQL LIKE pattern.
    pub fn delete_update_checks_by_pattern(
        &self,
        item_type: &str,
        item_id_pattern: &str,
    ) -> anyhow::Result<()> {
        queries::delete_update_checks_by_pattern(&self.conn, item_type, item_id_pattern)?;
        Ok(())
    }
}

// ── Conversion helpers ───────────────────────────────────────────────────────

fn record_to_dto(record: queries::ModelConfigRecord) -> ModelConfigDto {
    ModelConfigDto {
        id: record.id,
        repo_id: record.repo_id,
        display_name: record.display_name,
        backend: record.backend,
        gpu_variant: record.gpu_variant,
        gpu_device: record.gpu_device,
        enabled: record.enabled,
        selected_quant: record.selected_quant,
        selected_mmproj: record.selected_mmproj,
        selected_mtp_model: record.selected_mtp_model,
        context_length: record.context_length,
        num_parallel: record.num_parallel,
        kv_unified: record.kv_unified,
        gpu_layers: record.gpu_layers,
        cache_type_k: record.cache_type_k,
        cache_type_v: record.cache_type_v,
        port: record.port,
        args: record.args,
        sampling: record.sampling,
        modalities: record.modalities,
        profile: record.profile,
        api_name: record.api_name,
        health_check: record.health_check,
        hf_format: record.hf_format,
        hf_base_model: record.hf_base_model,
        hf_pipeline_tag: record.hf_pipeline_tag,
        hf_total_params: record.hf_total_params,
        hf_active_params: record.hf_active_params,
        hf_architecture_type: record.hf_architecture_type,
        hf_context_length: record.hf_context_length,
        hf_num_layers: record.hf_num_layers,
        hf_last_modified: record.hf_last_modified,
        spec_decoding: record.spec_decoding,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn file_record_to_dto(record: queries::ModelFileRecord) -> ModelFileDto {
    ModelFileDto {
        id: record.id,
        model_id: record.model_id,
        repo_id: record.repo_id,
        filename: record.filename,
        quant: record.quant,
        lfs_oid: record.lfs_oid,
        size_bytes: record.size_bytes,
        downloaded_at: record.downloaded_at,
        last_verified_at: record.last_verified_at,
        verified_ok: record.verified_ok,
        verify_error: record.verify_error,
    }
}

fn alias_response_to_dto(alias: queries::AliasResponse) -> AliasDto {
    AliasDto {
        id: alias.id,
        name: alias.name,
        model_id: alias.model_id,
        model_name: alias.model_name,
        description: alias.description,
        enabled: alias.enabled,
        created_at: alias.created_at,
        updated_at: alias.updated_at,
    }
}

fn benchmark_row_to_dto(row: queries::BenchmarkRow) -> BenchmarkDto {
    BenchmarkDto {
        id: row.id,
        created_at: row.created_at,
        model_id: row.model_id,
        display_name: row.display_name,
        quant: row.quant,
        backend: row.backend,
        engine: row.engine,
        pp_sizes: row.pp_sizes,
        tg_sizes: row.tg_sizes,
        threads: row.threads,
        ngl_range: row.ngl_range,
        runs: row.runs,
        warmup: row.warmup,
        results: row.results,
        load_time_ms: row.load_time_ms,
        vram_used_mib: row.vram_used_mib,
        vram_total_mib: row.vram_total_mib,
        duration_seconds: row.duration_seconds,
        status: row.status,
        benchmark_type: row.benchmark_type,
    }
}

fn queue_item_to_dto(item: queries::DownloadQueueItem) -> DownloadQueueDto {
    DownloadQueueDto {
        id: item.id,
        job_id: item.job_id,
        repo_id: item.repo_id,
        filename: item.filename,
        display_name: item.display_name,
        status: item.status,
        bytes_downloaded: item.bytes_downloaded,
        total_bytes: item.total_bytes,
        error_message: item.error_message,
        started_at: item.started_at,
        completed_at: item.completed_at,
        queued_at: item.queued_at,
        kind: item.kind,
        quant: item.quant,
        context_length: item.context_length,
    }
}

fn update_check_record_to_dto(record: queries::UpdateCheckRecord) -> UpdateCheckDto {
    UpdateCheckDto {
        item_type: record.item_type,
        item_id: record.item_id,
        current_version: record.current_version,
        latest_version: record.latest_version,
        update_available: record.update_available,
        status: record.status,
        error_message: record.error_message,
        details_json: record.details_json,
        checked_at: record.checked_at,
    }
}

// Re-export internal query types used by the Repository
use crate::db::queries;
