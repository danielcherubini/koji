//! llama-bench integration for benchmarking GGUF files directly.
//!
//! Wraps the llama-bench binary from llama.cpp's tools/ directory. Runs raw
//! inference benchmarks without spawning a server.
//!
//! Split into:
//! - [`discovery`] — binary lookup and GPU-type inference (pure filesystem logic).
//! - [`args`] — CLI-argument construction from [`LlamaBenchConfig`].
//! - [`parse`] — JSON parsing of llama-bench's `-o json` output.
//!
//! This module's `mod.rs` keeps only the public surface ([`LlamaBenchConfig`],
//! [`find_llama_bench`], [`run_llama_bench`]) plus the async orchestrator that
//! ties the pieces together.

mod args;
mod discovery;
mod parse;

pub use discovery::find_llama_bench;

use crate::bench::{BenchConfig, BenchReport, ModelInfo};
use crate::config::Config;
use crate::installations::ProgressSink;
use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::Command;

/// Configuration for llama-bench specific parameters.
#[derive(Debug, Clone)]
pub struct LlamaBenchConfig {
    /// Prompt sizes to test (maps to -p)
    pub pp_sizes: Vec<u32>,
    /// Generation lengths to test (maps to -n)
    pub tg_sizes: Vec<u32>,
    /// Number of measurement runs (maps to -r)
    pub runs: u32,
    /// Warmup runs (handled by wrapper, not llama-bench itself)
    pub warmup: u32,
    /// Thread counts to test. None = auto-detect from system.
    pub threads: Option<Vec<u32>>,
    /// GPU layer range for sweet-spot sweep.
    /// Some("0-99+1") maps to --n-gpu-layers 0-99+1.
    /// None = use all layers (default).
    pub ngl_range: Option<String>,
    /// Optional context size override (maps to --fit-ctx)
    pub ctx_override: Option<u32>,
    /// Logical batch size (maps to -b). Sweep by comma-separating.
    pub batch_sizes: Vec<u32>,
    /// Physical micro-batch size (maps to -ub). Sweep by comma-separating.
    pub ubatch_sizes: Vec<u32>,
    /// KV cache type applied to BOTH -ctk and -ctv.
    /// Mismatched K/V quant falls back to CPU attention on most builds, so we
    /// only expose a single matched-pair value (e.g. "f16", "q8_0", "q4_0").
    pub kv_cache_type: Option<String>,
    /// Depth sweep (maps to -d). Tokens pre-filled into KV cache before timing.
    /// Critical for evaluating KV-cache quantization at non-trivial context.
    pub depth: Vec<u32>,
    /// Flash attention toggle (maps to -fa 0|1). None = llama-bench default.
    pub flash_attn: Option<bool>,
}

/// Run a benchmark using llama-bench and return the report.
///
/// `quant` is an optional quant label (e.g. "Q6_K") that overrides the model
/// config's default quant when resolving the GGUF file path.
/// `backend_name` is an optional override — if provided, llama-bench is
/// resolved from that backend's installation path instead of the model's
/// configured backend.
/// `gpu_variant` is an optional override for the GPU variant used to resolve
/// the backend binary path. When `None`, falls back to the model config's
/// own gpu_variant (preserving Auto behavior).
///
/// This function is designed to be called from a background job — it streams
/// progress via the provided ProgressSink.
pub async fn run_llama_bench(
    config: &Config,
    model_id: &str,
    quant: Option<&str>,
    backend_name: Option<&str>,
    gpu_variant: Option<crate::gpu::GpuVariant>,
    bench_config: &LlamaBenchConfig,
    progress: &dyn ProgressSink,
) -> Result<BenchReport> {
    let db_dir = Config::config_dir()?;
    run_llama_bench_with_dir(
        config,
        &db_dir,
        model_id,
        quant,
        backend_name,
        gpu_variant,
        bench_config,
        progress,
    )
    .await
}

/// Internal implementation of [`run_llama_bench`] that takes an explicit database directory.
///
/// This is the extraction point used by tests — the public function resolves
/// `Config::config_dir()` and delegates here.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_llama_bench_with_dir(
    config: &Config,
    db_dir: &std::path::Path,
    model_id: &str,
    quant: Option<&str>,
    backend_name: Option<&str>,
    gpu_variant: Option<crate::gpu::GpuVariant>,
    bench_config: &LlamaBenchConfig,
    progress: &dyn ProgressSink,
) -> Result<BenchReport> {
    use crate::db::OpenResult;

    let OpenResult { conn, .. } = crate::db::open(db_dir)?;
    let model_configs = crate::db::load_model_configs(&conn)?;

    // If model_id is an integer db_id, resolve it to the config key first.
    let resolved_id = if let Ok(db_id) = model_id.parse::<i64>() {
        model_configs
            .iter()
            .find(|(_, mc)| mc.db_id == Some(db_id))
            .map(|(key, _)| key.as_str())
            .unwrap_or(model_id)
    } else {
        model_id
    };

    let (model_config, _backend_config) = config
        .resolve_backend(&model_configs, resolved_id)
        .context("Failed to resolve server config for benchmark")?;

    let model_path = resolve_model_path(config, db_dir, &conn, &model_configs, resolved_id, quant)?;

    let target_backend = backend_name.unwrap_or(&model_config.backend);
    // CRITICAL: gpu_variant from the request takes priority; fall back to the
    // model config's own gpu_variant (preserving Auto behavior when None).
    let variant = gpu_variant.as_ref().or(model_config.gpu_variant.as_ref());
    let manager = crate::installations::InstallationManager::open(db_dir)?;
    let backend_path = config.resolve_backend_path(target_backend, variant, &manager)?;

    let bench_binary = discovery::find_llama_bench(&backend_path).context(format!(
        "llama-bench not found for backend '{}'. Install llama.cpp from source or set LLAMA_BENCH_PATH",
        target_backend
    ))?;

    // Get llama-bench version for reporting (best-effort).
    let _version_output = Command::new(&bench_binary)
        .arg("--version")
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    progress.log(&format!("Using llama-bench: {}", bench_binary.display()));
    progress.log(&format!("Model: {} ({})", model_id, model_path.display()));

    let args = args::build_args(&model_path, bench_config);

    progress.log(&format!(
        "Running: {} {}",
        bench_binary.display(),
        args.join(" ")
    ));

    let start_time = std::time::Instant::now();

    let output = Command::new(&bench_binary)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute llama-bench")?;

    let _duration = start_time.elapsed();

    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        for line in stderr.lines() {
            if !line.trim().is_empty() {
                progress.log(line);
            }
        }
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "llama-bench exited with error (code {}): {}",
            output.status,
            stderr
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let summaries = parse::parse_bench_json(&stdout)?;

    // Prefer the human-friendly display name stored on the model config.
    // Fall back to the HF repo id, then the API name, then the raw model_id
    // (which is the db_id when called from the web UI — ugly but at least
    // identifies the row).
    let display_name = model_configs
        .get(resolved_id)
        .and_then(|mc| {
            mc.display_name
                .clone()
                .or_else(|| mc.api_name.clone())
                .or_else(|| mc.model.clone())
        })
        .unwrap_or_else(|| model_id.to_string());

    let model_info = ModelInfo {
        name: display_name,
        model_id: model_config.model.clone(),
        quant: quant.map(String::from).or(model_config.quant.clone()),
        backend: model_config.backend.clone(),
        gpu_variant: discovery::detect_gpu_variant_label(&backend_path),
        context_length: bench_config.ctx_override.or(model_config.context_length),
        gpu_layers: None,
    };

    let report = BenchReport {
        model_info,
        config: BenchConfig {
            pp_sizes: bench_config.pp_sizes.clone(),
            tg_sizes: bench_config.tg_sizes.clone(),
            runs: bench_config.runs,
            warmup: bench_config.warmup,
            ctx_override: bench_config.ctx_override,
            batch_sizes: bench_config.batch_sizes.clone(),
            ubatch_sizes: bench_config.ubatch_sizes.clone(),
            kv_cache_type: bench_config.kv_cache_type.clone(),
            depth: bench_config.depth.clone(),
            flash_attn: bench_config.flash_attn,
        },
        summaries,
        load_time_ms: 0.0,
        vram: crate::gpu::query_vram(),
    };

    // Stream the full report to the client via the progress sink. The frontend
    // uses this to render the header card (model / backend / GPU / VRAM) plus
    // the per-test results table — so we serialize the whole report, not just
    // `summaries`.
    if let Ok(report_json) = serde_json::to_string(&report) {
        progress.result(&report_json);
    }

    Ok(report)
}

/// Resolve the on-disk GGUF path for a model config.
///
/// `quant_override` takes priority over `mc.quant` when resolving the target file.
/// Falls back to the legacy `<db_dir>/models/` location if the configured
/// `models_dir` doesn't hold the file.
fn resolve_model_path(
    config: &Config,
    db_dir: &std::path::Path,
    conn: &rusqlite::Connection,
    model_configs: &std::collections::HashMap<String, crate::config::ModelConfig>,
    resolved_id: &str,
    quant_override: Option<&str>,
) -> Result<std::path::PathBuf> {
    let mc = model_configs
        .get(resolved_id)
        .with_context(|| format!("Model config '{}' not found", resolved_id))?;
    let rec_id = mc.db_id.context("Model config has no db_id")?;
    let record = crate::db::queries::get_model_config(conn, rec_id)?
        .with_context(|| format!("Model config record (id={}) not found in database", rec_id))?;
    let files = crate::db::queries::get_model_files(conn, record.id)?;

    // Resolve the target filename: prefer quant_override, then mc.quant from config,
    // falling back to the first .gguf if quants map is empty (legacy configs).
    let first_gguf = files
        .iter()
        .find(|f| f.filename.ends_with(".gguf"))
        .map(|f| f.filename.clone());

    let target_filename = quant_override
        .or(mc.quant.as_deref())
        .and_then(|quant_label| mc.quants.get(quant_label).map(|qe| qe.file.clone()))
        .or(first_gguf)
        .context("No model file found for this config")?;

    let model_file = files
        .into_iter()
        .find(|f| f.filename == target_filename)
        .context("Resolved model file not found in database")?;

    let model_data_dir = config.models_dir()?;
    let candidate = model_data_dir
        .join(&record.repo_id)
        .join(&model_file.filename);
    if candidate.exists() {
        return Ok(candidate);
    }

    let legacy = db_dir.join("models");
    let legacy_candidate = legacy.join(&record.repo_id).join(&model_file.filename);
    if legacy_candidate.exists() {
        return Ok(legacy_candidate);
    }

    bail!(
        "Model file not found: {} (searched {:?} and {:?})",
        model_file.filename,
        candidate,
        legacy_candidate
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LogLevel, ModelConfig};
    use crate::db::queries::{insert_installation, upsert_general, upsert_installation_config};
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, MutexGuard};

    /// A simple progress sink that captures log lines and result JSON.
    struct CaptureSink {
        logs: Mutex<Vec<String>>,
        results: Mutex<Vec<String>>,
    }

    /// Guard to serialize env var tests without needing serial_test.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    impl CaptureSink {
        fn new() -> Self {
            Self {
                logs: Mutex::new(Vec::new()),
                results: Mutex::new(Vec::new()),
            }
        }

        pub fn logs(&self) -> MutexGuard<'_, Vec<String>> {
            self.logs.lock().unwrap()
        }

        pub fn results(&self) -> MutexGuard<'_, Vec<String>> {
            self.results.lock().unwrap()
        }
    }

    impl ProgressSink for CaptureSink {
        fn log(&self, line: &str) {
            self.logs.lock().unwrap().push(line.to_string());
        }
        fn result(&self, json: &str) {
            self.results.lock().unwrap().push(json.to_string());
        }
    }

    /// Helper to set up a minimal test database with a backend config,
    /// an installation record, and a model config + file entry.
    fn seed_test_db(temp_dir: &tempfile::TempDir) -> anyhow::Result<(std::path::PathBuf, String)> {
        let db_path = temp_dir.path().join("tama.db");

        // Open the database and run migrations
        let conn = rusqlite::Connection::open(&db_path)?;
        crate::db::migrations::run(&conn)?;

        // Seed defaults so Config::from_db works
        crate::db::queries::seed_defaults(&conn)?;

        // Set models_dir to point to our temp dir's models subdirectory
        let models_dir = temp_dir.path().join("models");
        upsert_general(
            &conn,
            &LogLevel::Info,
            Some(models_dir.to_string_lossy().as_ref()),
            None, // logs_dir
            None, // hf_token
            60,   // update_check_interval
        )?;

        // 1. Insert a backend config (llama_cpp, cpu)
        upsert_installation_config(
            &conn,
            "",
            "llama_cpp",
            "cpu",
            &[],
            &[],
            Some("http://localhost:8080/health"),
        )?;

        // 2. Create a fake llama-server binary in the temp dir
        let backend_dir = temp_dir
            .path()
            .join("backends")
            .join("llama_cpp")
            .join("cpu");
        std::fs::create_dir_all(&backend_dir)?;
        let fake_server = backend_dir.join("llama-server");
        std::fs::write(&fake_server, "#!/bin/sh\necho 'fake llama-server'\nexit 0")?;
        let mut perms = std::fs::metadata(&fake_server)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_server, perms)?;

        // 3. Insert a backend installation record pointing to the fake binary
        insert_installation(
            &conn,
            &crate::db::queries::InstallationRecord {
                id: 0,
                name: "llama_cpp".to_string(),
                backend_type: "llama_cpp".to_string(),
                version: "test".to_string(),
                path: fake_server.to_string_lossy().to_string(),
                installed_at: 1000,
                gpu_variant: "cpu".to_string(),
                source: None,
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )?;

        // 4. Insert a model config using the high-level API
        let mut quants = BTreeMap::new();
        quants.insert(
            "Q4_K_M".to_string(),
            crate::config::QuantEntry {
                file: "test-model-Q4_K_M.gguf".to_string(),
                kind: crate::config::QuantKind::from_filename("test-model-Q4_K_M.gguf"),
                size_bytes: Some(4_294_967_296),
                context_length: None,
            },
        );

        let model_config = ModelConfig {
            backend: "llama_cpp".to_string(),
            gpu_variant: None,
            gpu_device: None,
            args: vec![],
            sampling: None,
            model: Some("test/test-model".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            mtp_model: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: Some(4096),
            num_parallel: Some(1),
            kv_unified: false,
            profile: None,
            api_name: Some("test-model".to_string()),
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            quants,
            modalities: None,
            display_name: Some("Test Model".to_string()),
            db_id: None,
            spec_decoding: Default::default(),
            n_batch: None,
            n_ubatch: None,
            vllm: Default::default(),
            provider_name: None,
        };

        let config_key = "test--test-model";
        crate::db::save_model_config(&conn, config_key, &model_config)?;

        // 5. Insert a model file record
        crate::db::queries::upsert_model_file(
            &conn,
            1, // model_id
            "test/test-model",
            "test-model-Q4_K_M.gguf",
            Some("Q4_K_M"),
            None,                // lfs_oid
            Some(4_294_967_296), // size_bytes
        )?;

        let config_dir = temp_dir.path().to_path_buf();

        Ok((config_dir, "test-model-Q4_K_M.gguf".to_string()))
    }

    /// Verifies that `run_llama_bench_with_dir` executes llama-bench via a stub
    /// script, parses the JSON output, and streams progress through the sink.
    #[tokio::test]
    async fn test_run_llama_bench_with_stub_binary() {
        let _env_guard = ENV_GUARD.lock().unwrap();

        // Clean slate for env vars
        std::env::remove_var("LLAMA_BENCH_PATH");

        let temp_dir = tempfile::tempdir().unwrap();

        // Seed the database with backend + model entries
        let (config_dir, gguf_filename) = seed_test_db(&temp_dir).unwrap();

        // Create models directory with a dummy GGUF file
        let models_dir = config_dir.join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let model_dir = models_dir.join("test/test-model");
        std::fs::create_dir_all(&model_dir).unwrap();
        let gguf_path = model_dir.join(&gguf_filename);
        std::fs::write(&gguf_path, vec![0u8; 1024]).unwrap();

        // Write a stub llama-bench script that outputs valid JSON
        let stub_script = temp_dir.path().join("stub-llama-bench");
        std::fs::write(
            &stub_script,
            r#"#!/bin/sh
echo '[{"n_prompt": 512, "n_gen": 0, "avg_ts": 5120.5, "stddev_ts": 42.3}, {"n_prompt": 0, "n_gen": 128, "avg_ts": 1000.0, "stddev_ts": 15.5}]'
exit 0
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&stub_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub_script, perms).unwrap();

        // Set LLAMA_BENCH_PATH
        std::env::set_var("LLAMA_BENCH_PATH", stub_script.to_string_lossy().as_ref());

        // Load Config from the seeded database
        let config = Config::load_from(&config_dir.join("tama.db")).unwrap();

        // Create a bench config with PP and TG tests
        let bench_config = LlamaBenchConfig {
            pp_sizes: vec![512],
            tg_sizes: vec![128],
            runs: 1,
            warmup: 0,
            threads: None,
            ngl_range: None,
            ctx_override: Some(4096),
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        };

        let sink = std::sync::Arc::new(CaptureSink::new());

        // Drop the env guard before calling the async function
        drop(_env_guard);

        // Call the function under test (model_id must be the config key format)
        let result = run_llama_bench_with_dir(
            &config,
            &config_dir,
            "test--test-model",
            None,
            None,
            None,
            &bench_config,
            &*sink,
        )
        .await;

        // Restore env
        std::env::remove_var("LLAMA_BENCH_PATH");

        // Assert the benchmark succeeded
        assert!(
            result.is_ok(),
            "run_llama_bench_with_dir should succeed: {:?}",
            result.err()
        );
        let report = result.unwrap();

        // Assert 2 summaries (one PP, one TG)
        assert_eq!(report.summaries.len(), 2);
        assert_eq!(report.summaries[0].test_name, "pp512");
        assert!((report.summaries[0].pp_mean - 5120.5).abs() < 0.01);
        assert_eq!(report.summaries[1].test_name, "tg128");
        assert!((report.summaries[1].tg_mean - 1000.0).abs() < 0.01);

        // Assert progress sink captured logs
        assert!(
            !sink.logs().is_empty(),
            "ProgressSink should have received log lines"
        );

        // Assert the report was serialized and sent via result()
        assert!(
            !sink.results().is_empty(),
            "ProgressSink should have received a result"
        );
    }

    /// Verifies that when llama-bench exits with a non-zero status, the error
    /// message contains the stderr output.
    #[tokio::test]
    async fn test_run_llama_bench_stub_failure_surfaces_stderr() {
        let _env_guard = ENV_GUARD.lock().unwrap();

        std::env::remove_var("LLAMA_BENCH_PATH");

        let temp_dir = tempfile::tempdir().unwrap();

        // Seed the database
        let (config_dir, gguf_filename) = seed_test_db(&temp_dir).unwrap();

        // Create models directory with a dummy GGUF file (needed for path resolution)
        let models_dir = config_dir.join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let model_dir = models_dir.join("test/test-model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join(&gguf_filename), vec![0u8; 1024]).unwrap();

        // Write a stub that exits with error and prints to stderr
        let stub_script = temp_dir.path().join("stub-llama-bench-fail");
        std::fs::write(
            &stub_script,
            r#"#!/bin/sh
echo "llama-bench crashed: out of memory" >&2
exit 1
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&stub_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub_script, perms).unwrap();

        std::env::set_var("LLAMA_BENCH_PATH", stub_script.to_string_lossy().as_ref());

        let config_dir = temp_dir.path().to_path_buf();
        let config = Config::load_from(&config_dir.join("tama.db")).unwrap();

        let bench_config = LlamaBenchConfig {
            pp_sizes: vec![512],
            tg_sizes: vec![128],
            runs: 1,
            warmup: 0,
            threads: None,
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        };

        let sink = std::sync::Arc::new(CaptureSink::new());

        // Drop the env guard before calling the async function
        drop(_env_guard);

        let result = run_llama_bench_with_dir(
            &config,
            &config_dir,
            "test--test-model",
            None,
            None,
            None,
            &bench_config,
            &*sink,
        )
        .await;

        std::env::remove_var("LLAMA_BENCH_PATH");

        assert!(result.is_err(), "Should return Err when llama-bench fails");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("llama-bench exited with error"),
            "Error should mention 'llama-bench exited with error', got: {}",
            err_msg
        );
    }
}
