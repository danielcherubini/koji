//! MTP (multi-token prediction) benchmark runner (plan-191 Tasks 8 and 10).
//!
//! Spawns `llama-server` with the MTP draft active on this host. Moved from
//! `tama_core::bench::llama_cli_mtp` (ADR-0010). The result types stay in
//! `tama_core::bench::llama_cli_mtp` (the proxy persists the report).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::time::sleep;

use crate::bench::llama_cli_spec::discovery::find_llama_server;
use crate::bench::llama_cli_spec::server::{self, ServerArgs, ServerHandle};
use tama_core::bench::llama_cli_mtp::{
    MtpAggregate, MtpBenchConfig, MtpBenchResult, MtpPromptResult, MTP_PROMPTS,
};
use tama_core::bench::llama_cli_spec::SpecType;
use tama_core::installations::ProgressSink;

/// Run a single prompt against a running server and return the result.
async fn run_single_prompt(
    handle: &ServerHandle,
    model_name: &str,
    prompt_name: &str,
    prompt_text: &str,
    draft_max: u32,
    progress: &Arc<dyn ProgressSink>,
) -> MtpPromptResult {
    let messages = vec![("user", prompt_text)];
    let start = std::time::Instant::now();

    match handle.chat_complete(model_name, &messages, 192).await {
        Ok(timing) => {
            let wall_s = start.elapsed().as_secs_f64();
            let accept_rate = if timing.draft_n > 0 {
                Some(timing.draft_n_accepted as f64 / timing.draft_n as f64)
            } else {
                None
            };
            MtpPromptResult {
                draft_max,
                name: prompt_name.to_string(),
                wall_s,
                predicted_n: timing.predicted_n,
                draft_n: timing.draft_n,
                draft_n_accepted: timing.draft_n_accepted,
                accept_rate,
                predicted_per_second: timing.predicted_per_second,
                error: None,
            }
        }
        Err(e) => {
            let wall_s = start.elapsed().as_secs_f64();
            progress.log(&format!(
                "[draft_max={}] prompt '{}' failed: {}",
                draft_max, prompt_name, e
            ));
            MtpPromptResult {
                draft_max,
                name: prompt_name.to_string(),
                wall_s,
                predicted_n: 0,
                draft_n: 0,
                draft_n_accepted: 0,
                accept_rate: None,
                predicted_per_second: 0.0,
                error: Some(e.to_string()),
            }
        }
    }
}

/// Run all 9 MTP prompts against a server with the given draft-n-max config.
async fn run_prompts_for_config(
    binary: &Path,
    config: &MtpBenchConfig,
    draft_max: u32,
    model_name: &str,
    progress: Arc<dyn ProgressSink>,
) -> Vec<MtpPromptResult> {
    let port = match crate::bench::find_available_port().await {
        Ok(p) => p,
        Err(e) => {
            progress.log(&format!("Failed to find available port: {}", e));
            return MTP_PROMPTS
                .iter()
                .map(|(name, _)| MtpPromptResult {
                    draft_max,
                    name: name.to_string(),
                    wall_s: 0.0,
                    predicted_n: 0,
                    draft_n: 0,
                    draft_n_accepted: 0,
                    accept_rate: None,
                    predicted_per_second: 0.0,
                    error: Some(format!("Port allocation failed: {}", e)),
                })
                .collect();
        }
    };

    let server_args = ServerArgs {
        binary: binary.to_path_buf(),
        model_path: config.model_path.clone(),
        port,
        ngl: config.ngl,
        flash_attn: config.flash_attn,
        spec_type: Some(SpecType::DraftMtp),
        spec_ngram_n: None,
        spec_ngram_m: None,
        spec_ngram_min_hits: None,
        spec_ngram_min: None,
        spec_ngram_max: None,
        draft_max: Some(draft_max),
        draft_min: None,
        spec_draft_ngl: config.draft_ngl,
        context_size: config.context_size,
    };

    let arg_vec = server_args.to_args();
    progress.log(&format!(
        "Starting llama-server on port {} (draft-n-max={})",
        port, draft_max
    ));
    progress.log(&format!(
        "llama-server {} {}",
        binary.display(),
        arg_vec.join(" ")
    ));

    let timeout_secs = std::env::var("LLAMA_SERVER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300);

    let handle = match server::spawn_server(&server_args, timeout_secs).await {
        Ok(h) => h,
        Err(e) => {
            progress.log(&format!(
                "Failed to start llama-server for draft-n-max={}: {}",
                draft_max, e
            ));
            return MTP_PROMPTS
                .iter()
                .map(|(name, _)| MtpPromptResult {
                    draft_max,
                    name: name.to_string(),
                    wall_s: 0.0,
                    predicted_n: 0,
                    draft_n: 0,
                    draft_n_accepted: 0,
                    accept_rate: None,
                    predicted_per_second: 0.0,
                    error: Some(format!("Server start failed: {}", e)),
                })
                .collect();
        }
    };

    progress.log(&format!(
        "llama-server ready on port {} (draft-n-max={})",
        port, draft_max
    ));

    // Run all 9 prompts
    let mut results = Vec::with_capacity(MTP_PROMPTS.len());
    for (name, text) in MTP_PROMPTS {
        progress.log(&format!(
            "[draft_max={}] running prompt '{}'",
            draft_max, name
        ));
        let result = run_single_prompt(&handle, model_name, name, text, draft_max, &progress).await;
        results.push(result);
    }

    // Drop the server handle (kills the server)
    drop(handle);

    results
}

/// Run the MTP benchmark sweep.
///
/// Executes a baseline phase (draft-n-max=0) followed by a sweep phase
/// for each draft_max value > 0 in the config. Each phase spawns its own
/// llama-server instance.
///
/// # Arguments
/// - `config`: MTP benchmark configuration specifying model, draft values, etc.
/// - `binary_override`: optional path to the `llama-server` binary. If `None`, uses
///   discovery to find it alongside the model.
/// - `progress`: progress sink for streaming status updates.
///
/// # Returns
/// A [`MtpBenchResult`] with per-prompt entries and aggregate statistics.
pub async fn run_mtp_bench(
    config: &MtpBenchConfig,
    binary_override: Option<PathBuf>,
    progress: Arc<dyn ProgressSink>,
) -> Result<MtpBenchResult> {
    // Step 1: Discover or use provided llama-server binary.
    let backend_dir = config.model_path.parent().unwrap_or(Path::new(""));
    let binary = if let Some(bp) = binary_override {
        if !bp.exists() {
            bail!(
                "Provided llama-server path does not exist: {}",
                bp.display()
            );
        }
        bp
    } else {
        find_llama_server(backend_dir)
            .context("llama-server not found. Set LLAMA_SERVER_PATH or ensure llama-server is in the backend directory.")?
    };

    // Step 2: Extract model name from path.
    let model_name = config
        .model_path
        .file_stem()
        .unwrap_or(std::ffi::OsStr::new("model"))
        .to_string_lossy()
        .into_owned();

    progress.log(&format!("Using llama-server: {}", binary.display()));
    progress.log(&format!(
        "Model: {} ({})",
        config.model_path.display(),
        model_name
    ));

    // Effective defaults
    let ngl = config.ngl.or(Some(99));
    let draft_ngl = config.draft_ngl.or(Some(99));
    let effective_config = MtpBenchConfig {
        ngl,
        draft_ngl,
        ..config.clone()
    };

    // Collect all results in order
    let mut all_entries: Vec<MtpPromptResult> = Vec::new();

    // Step 3: Baseline phase (draft-n-max=0)
    progress.log("Starting baseline phase (draft-n-max=0)...");
    let baseline_results =
        run_prompts_for_config(&binary, &effective_config, 0, &model_name, progress.clone()).await;
    all_entries.extend(baseline_results);

    // 2s sleep between configs
    sleep(Duration::from_secs(2)).await;

    // Step 4: Sweep phase (draft_max > 0)
    for &draft_max in &config.draft_max_values {
        if draft_max == 0 {
            continue; // Already covered by baseline
        }
        progress.log(&format!(
            "Starting sweep phase (draft-n-max={})...",
            draft_max
        ));
        let sweep_results = run_prompts_for_config(
            &binary,
            &effective_config,
            draft_max,
            &model_name,
            progress.clone(),
        )
        .await;
        all_entries.extend(sweep_results);

        // 2s sleep between configs
        sleep(Duration::from_secs(2)).await;
    }

    // Step 5: Build aggregate
    let n_requests = all_entries.len();
    let total_predicted: u32 = all_entries.iter().map(|e| e.predicted_n).sum();
    let total_draft: u32 = all_entries.iter().map(|e| e.draft_n).sum();
    let total_draft_accepted: u32 = all_entries.iter().map(|e| e.draft_n_accepted).sum();
    let aggregate_accept_rate = if total_draft > 0 {
        total_draft_accepted as f64 / total_draft as f64
    } else {
        0.0
    };
    let wall_s_total: f64 = all_entries.iter().map(|e| e.wall_s).sum();

    let aggregate = MtpAggregate {
        n_requests,
        total_predicted,
        total_draft,
        total_draft_accepted,
        aggregate_accept_rate,
        wall_s_total,
    };

    let result = MtpBenchResult {
        entries: all_entries,
        aggregate,
        vram: None, // filled at dispatch (host VRAM)
    };

    // Step 6: Report result via progress sink
    let json = serde_json::to_string(&result).context("Failed to serialize MtpBenchResult")?;
    progress.result(&json);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::bench::llama_cli_spec::server::ChatTiming;

    /// Verifies accept_rate is None for baseline (draft_n == 0).
    #[test]
    fn test_accept_rate_baseline_none() {
        let timing = ChatTiming {
            predicted_per_second: 100.0,
            predicted_n: 100,
            draft_n: 0,
            draft_n_accepted: 0,
        };

        let accept_rate = if timing.draft_n > 0 {
            Some(timing.draft_n_accepted as f64 / timing.draft_n as f64)
        } else {
            None
        };

        assert!(accept_rate.is_none());
    }

    /// Verifies accept_rate is Some when draft_n > 0.
    #[test]
    fn test_accept_rate_with_drafts() {
        let timing = ChatTiming {
            predicted_per_second: 150.0,
            predicted_n: 100,
            draft_n: 50,
            draft_n_accepted: 25,
        };

        let accept_rate = if timing.draft_n > 0 {
            Some(timing.draft_n_accepted as f64 / timing.draft_n as f64)
        } else {
            None
        };

        assert!(accept_rate.is_some());
        assert!((accept_rate.unwrap() - 0.5).abs() < 0.001);
    }
}
