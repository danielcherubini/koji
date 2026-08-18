//! Spec-decoding benchmark runner (plan-191 Tasks 8 and 10).
//!
//! Spawns `llama-server` (baseline + per-config spec-decode servers) on
//! this host and measures TG throughput. Moved from
//! `tama_core::bench::llama_cli_spec` (ADR-0010). Shared types and the
//! sweep-matrix builder/validation stay in `tama_core::bench::llama_cli_spec`.

pub(crate) mod discovery;

pub(crate) mod server;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use tama_core::bench::llama_cli_spec::{
    build_sweep_matrix, format_config_label, SpecBenchConfig, SpecBenchResult, SpecEntry, SpecType,
    SweepConfig,
};
use tama_core::bench::mean_stddev;
use tama_core::installations::ProgressSink;

/// Execute benchmark runs against a running llama-server.
///
/// Makes `config.runs` completion requests and returns timing stats.
/// Parses draft acceptance rate from server stderr statistics.
/// If a run produces an impossibly fast result (>10x baseline), it is logged
/// and re-run (up to 3 retries) before being accepted.
async fn execute_server_runs(
    handle: &server::ServerHandle,
    sweep_cfg: &SweepConfig,
    bench_cfg: &SpecBenchConfig,
    baseline_mean: f64,
    progress: Arc<dyn ProgressSink>,
) -> SpecEntry {
    let label = format_config_label(sweep_cfg);
    let prompt = tama_core::bench::build_prompt(512);
    let mut timings = Vec::new();
    const MAX_WILD_RETRIES: u32 = 3;

    for run in 1..=bench_cfg.runs {
        let mut retries = 0;
        loop {
            progress.log(&format!(
                "[{}] run {}/{}{}",
                label,
                run,
                bench_cfg.runs,
                if retries > 0 {
                    format!(" (retry {})", retries)
                } else {
                    String::new()
                }
            ));

            match handle.complete(&prompt, bench_cfg.gen_tokens).await {
                Ok(tokens_per_sec) => {
                    // Check for impossibly fast result (>10x baseline)
                    if baseline_mean > 0.0 && tokens_per_sec > baseline_mean * 10.0 {
                        progress.log(&format!(
                            "[{}] run {} wild result: {:.2} tokens/s is {:.0}x baseline ({:.2}) — discarding",
                            label, run, tokens_per_sec, tokens_per_sec / baseline_mean, baseline_mean
                        ));
                        if retries >= MAX_WILD_RETRIES {
                            progress.log(&format!(
                                "[{}] run {} accepted after {} retries (may be outlier)",
                                label, run, MAX_WILD_RETRIES
                            ));
                            timings.push(tokens_per_sec);
                            break;
                        }
                        retries += 1;
                        continue;
                    }
                    timings.push(tokens_per_sec);
                    break;
                }
                Err(e) => {
                    progress.log(&format!("[{}] run {} failed: {}", label, run, e));
                    return SpecEntry {
                        spec_type: sweep_cfg.spec_type.as_str().to_string(),
                        draft_max: sweep_cfg.draft_max,
                        ngram_n: sweep_cfg.ngram_n,
                        ngram_m: sweep_cfg.ngram_m,
                        ngram_min: sweep_cfg.ngram_min,
                        ngram_max: sweep_cfg.ngram_max,
                        tg_ts_mean: 0.0,
                        tg_ts_stddev: 0.0,
                        delta_pct: 0.0,
                        acceptance_rate: None,
                        status: "failed".to_string(),
                        error: Some(e.to_string()),
                    };
                }
            }
        }
    }

    let (mean, stddev) = mean_stddev(&timings);
    let acceptance_rate = handle.parse_acceptance_rate().await;
    progress.log(&format!(
        "[{}] completed: {:.2} ± {:.2} tokens/s (acceptance: {:?})",
        label, mean, stddev, acceptance_rate
    ));

    SpecEntry {
        spec_type: sweep_cfg.spec_type.as_str().to_string(),
        draft_max: sweep_cfg.draft_max,
        ngram_n: sweep_cfg.ngram_n,
        ngram_m: sweep_cfg.ngram_m,
        ngram_min: sweep_cfg.ngram_min,
        ngram_max: sweep_cfg.ngram_max,
        tg_ts_mean: mean,
        tg_ts_stddev: stddev,
        delta_pct: 0.0,
        acceptance_rate,
        status: "success".to_string(),
        error: None,
    }
}

/// Spawn a server for a single config, execute benchmark runs, and return results.
///
/// Each config gets its own server to ensure correct parameters (ngram params
/// and draft_max are server startup flags that can't be changed mid-session).
async fn run_single_config(
    binary: &Path,
    cfg: &SweepConfig,
    bench_cfg: &SpecBenchConfig,
    baseline_mean: f64,
    progress: Arc<dyn ProgressSink>,
) -> SpecEntry {
    let label = format_config_label(cfg);

    let port = match crate::bench::find_available_port().await {
        Ok(p) => p,
        Err(e) => {
            progress.log(&format!("Failed to find available port: {}", e));
            return SpecEntry {
                spec_type: cfg.spec_type.as_str().to_string(),
                draft_max: cfg.draft_max,
                ngram_n: cfg.ngram_n,
                ngram_m: cfg.ngram_m,
                ngram_min: cfg.ngram_min,
                ngram_max: cfg.ngram_max,
                tg_ts_mean: 0.0,
                tg_ts_stddev: 0.0,
                delta_pct: 0.0,
                acceptance_rate: None,
                status: "failed".to_string(),
                error: Some(format!("Port allocation failed: {}", e)),
            };
        }
    };

    // draft_min/draft_max are not used for ngram-mod (draft length controlled by n-min/n-max)
    let use_draft_bounds = !matches!(cfg.spec_type, SpecType::NgramMod);
    let draft_max_val = use_draft_bounds.then_some(cfg.draft_max);
    let draft_min_val = use_draft_bounds.then_some((cfg.draft_max / 2).max(1));

    let server_args = server::ServerArgs {
        binary: binary.to_path_buf(),
        model_path: bench_cfg.model_path.clone(),
        port,
        ngl: bench_cfg.ngl,
        flash_attn: bench_cfg.flash_attn,
        spec_type: Some(cfg.spec_type),
        spec_ngram_n: cfg.ngram_n,
        spec_ngram_m: cfg.ngram_m,
        spec_ngram_min_hits: (bench_cfg.ngram_min_hits > 1).then_some(bench_cfg.ngram_min_hits),
        spec_ngram_min: cfg.ngram_min,
        spec_ngram_max: cfg.ngram_max,
        draft_max: draft_max_val,
        draft_min: draft_min_val,
        spec_draft_ngl: None,
        context_size: None,
    };

    let arg_vec = server_args.to_args();
    progress.log(&format!(
        "Starting llama-server on port {} ({})",
        port, label
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
                "Failed to start llama-server for {}: {}",
                label, e
            ));
            return SpecEntry {
                spec_type: cfg.spec_type.as_str().to_string(),
                draft_max: cfg.draft_max,
                ngram_n: cfg.ngram_n,
                ngram_m: cfg.ngram_m,
                ngram_min: cfg.ngram_min,
                ngram_max: cfg.ngram_max,
                tg_ts_mean: 0.0,
                tg_ts_stddev: 0.0,
                delta_pct: 0.0,
                acceptance_rate: None,
                status: "failed".to_string(),
                error: Some(format!("Server start failed: {}", e)),
            };
        }
    };

    progress.log(&format!("llama-server ready on port {} ({})", port, label));

    execute_server_runs(&handle, cfg, bench_cfg, baseline_mean, progress.clone()).await
}

/// Run a speculative decoding benchmark sweep using llama-server.
///
/// Spawns one `llama-server` per spec-type group (since spec-type is a server
/// startup flag). Each server handles all draft-max variants for its type.
///
/// # Arguments
/// - `config`: benchmark configuration specifying model, spec types, sweep dimensions.
/// - `binary_override`: optional path to the `llama-server` binary. If `None`, uses
///   discovery to find it alongside the backend's `llama-server` binary.
/// - `progress`: progress sink for streaming status updates.
///
/// # Returns
/// A [`SpecBenchResult`] with baseline timing and one entry per sweep configuration.
pub async fn run_spec_bench(
    config: &SpecBenchConfig,
    binary_override: Option<PathBuf>,
    progress: Arc<dyn ProgressSink>,
) -> Result<SpecBenchResult> {
    // Step 1: Discover or use provided llama-server binary.
    let backend_dir = config
        .model_path
        .parent()
        .unwrap_or(std::path::Path::new(""));
    let binary = if let Some(bp) = binary_override {
        if !bp.exists() {
            bail!(
                "Provided llama-server path does not exist: {}",
                bp.display()
            );
        }
        bp
    } else {
        discovery::find_llama_server(backend_dir)
            .context("llama-server not found. Set LLAMA_SERVER_PATH or ensure llama-server is in the backend directory.")?
    };

    progress.log(&format!("Using llama-server: {}", binary.display()));
    progress.log(&format!(
        "Model: {} (gen_tokens={}, runs={})",
        config.model_path.display(),
        config.gen_tokens,
        config.runs,
    ));

    // Step 2: Run baseline (no spec-decoding) on a dedicated server.
    progress.log("Starting baseline server (no speculative decoding)...");
    let baseline_port = crate::bench::find_available_port().await?;
    let baseline_args = server::ServerArgs {
        binary: binary.clone(),
        model_path: config.model_path.clone(),
        port: baseline_port,
        ngl: config.ngl,
        flash_attn: config.flash_attn,
        spec_type: None,
        spec_ngram_n: None,
        spec_ngram_m: None,
        spec_ngram_min_hits: None,
        spec_ngram_min: None,
        spec_ngram_max: None,
        draft_max: None,
        draft_min: None,
        spec_draft_ngl: None,
        context_size: None,
    };
    progress.log(&format!(
        "llama-server {} {}",
        binary.display(),
        baseline_args.to_args().join(" ")
    ));

    let timeout_secs = std::env::var("LLAMA_SERVER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300);

    let baseline_handle = server::spawn_server(&baseline_args, timeout_secs)
        .await
        .with_context(|| "Failed to start baseline llama-server")?;

    progress.log(&format!("Baseline server ready on port {}", baseline_port));

    let mut baseline_timings = Vec::new();
    let prompt = tama_core::bench::build_prompt(512);

    for run in 1..=config.runs {
        progress.log(&format!("[baseline] run {}/{}", run, config.runs));
        match baseline_handle.complete(&prompt, config.gen_tokens).await {
            Ok(ts) => {
                baseline_timings.push(ts);
            }
            Err(e) => {
                bail!(
                    "Baseline run {} failed: {}. Cannot continue without baseline.",
                    run,
                    e
                );
            }
        }
    }

    let (baseline_mean, baseline_stddev) = mean_stddev(&baseline_timings);
    progress.log(&format!(
        "Baseline TG t/s: {:.2} ± {:.2}",
        baseline_mean, baseline_stddev
    ));

    if baseline_mean == 0.0 {
        bail!("Baseline mean is 0.0 — benchmark data may be invalid.");
    }

    // Step 3: Build sweep matrix.
    // Drop the baseline server now so GPU memory is available for spec-type servers.
    drop(baseline_handle);
    // Brief pause to let GPU memory fully free before spawning new servers.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let sweep_matrix = build_sweep_matrix(config).context("Failed to build sweep matrix")?;
    progress.log(&format!(
        "Sweep matrix: {} configurations across {} spec-types",
        sweep_matrix.len(),
        config.spec_types.len()
    ));

    // Step 4: Execute each config with its own server.
    // Each config gets a dedicated server because ngram params and draft_max
    // are server startup flags that can't be changed mid-session.
    let mut all_entries = Vec::new();
    let mut oom_detected = false;

    for cfg in &sweep_matrix {
        if oom_detected {
            progress.log(&format!(
                "[{}] skipping due to prior OOM",
                cfg.spec_type.as_str()
            ));
            all_entries.push(SpecEntry {
                spec_type: cfg.spec_type.as_str().to_string(),
                draft_max: cfg.draft_max,
                ngram_n: cfg.ngram_n,
                ngram_m: cfg.ngram_m,
                ngram_min: cfg.ngram_min,
                ngram_max: cfg.ngram_max,
                tg_ts_mean: 0.0,
                tg_ts_stddev: 0.0,
                delta_pct: 0.0,
                acceptance_rate: None,
                status: "skipped_oom".to_string(),
                error: Some("Skipped due to OOM in earlier config".to_string()),
            });
            continue;
        }

        let mut entry =
            run_single_config(&binary, cfg, config, baseline_mean, progress.clone()).await;

        // Brief pause between configs to let GPU memory be freed
        // before the next server starts loading the model.
        tokio::time::sleep(Duration::from_secs(2)).await;

        if entry.status == "skipped_oom" {
            oom_detected = true;
        }

        // Compute delta vs baseline.
        if entry.tg_ts_mean > 0.0 && baseline_mean > 0.0 {
            entry.delta_pct = ((entry.tg_ts_mean - baseline_mean) / baseline_mean) * 100.0;
        }

        all_entries.push(entry);
    }

    Ok(SpecBenchResult {
        baseline_tg_ts: baseline_mean,
        baseline_tg_stddev: baseline_stddev,
        entries: all_entries,
        vram: None, /* filled at dispatch */
    })
}
