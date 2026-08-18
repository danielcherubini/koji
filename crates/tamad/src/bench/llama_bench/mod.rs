//! llama-bench runner (plan-191 Tasks 8 and 10).
//!
//! Executes the `llama-bench` binary on this host. Moved from
//! `tama_core::bench::llama_bench` — ADR-0010: benchmarks measure tamad
//! hardware and run there. The config types (`LlamaBenchConfig`,
//! `LlamaBenchConfigJson`) stay in `tama_core::bench::llama_bench` (the
//! proxy serializes them into `RunBenchmarkRequest.config_json`).

mod args;
mod discovery;
mod parse;

use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::Command;

use tama_core::bench::llama_bench::LlamaBenchConfig;
use tama_core::bench::{BenchConfig, BenchReport, ModelInfo};
use tama_core::gpu::GpuVariant;
use tama_core::installations::ProgressSink;

/// DB-free core of a llama-bench run: execute the subprocess and assemble
/// the report from resolved paths (plan-191 Task 8).
///
/// `model_path` is the GGUF file and `binary_path` the backend's
/// llama-server binary on the execution host; the llama-bench binary is
/// discovered relative to it (or via `LLAMA_BENCH_PATH` / `PATH`). This is
/// the code path the tamad calls directly — it touches no database.
///
/// The core derives `ModelInfo` from paths alone (file-stem display name,
/// no quant/backend metadata); callers that have the central database
/// overlay those values (see [`run_llama_bench_with_dir`]). The GPU
/// variant label keeps its binary-path heuristic, and `vram` is sampled on
/// the execution host.
#[allow(clippy::too_many_arguments)]
pub async fn run_llama_bench_resolved(
    model_path: &std::path::Path,
    binary_path: &std::path::Path,
    gpu_variant: Option<GpuVariant>,
    bench_config: &LlamaBenchConfig,
    progress: &dyn ProgressSink,
) -> Result<BenchReport> {
    let bench_binary = discovery::find_llama_bench(binary_path)
        .with_context(|| {
            format!("llama-bench not found for backend binary '{}'. Install llama.cpp from source or set LLAMA_BENCH_PATH", binary_path.display())
        })?;

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
    if let Some(variant) = gpu_variant {
        progress.log(&format!("GPU variant: {}", variant.variant_folder()));
    }
    progress.log(&format!("Model: {}", model_path.display()));

    let args = args::build_args(model_path, bench_config);

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

    // Path-derived model metadata (tamad has no DB to resolve these from).
    let display_name = model_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| model_path.display().to_string());

    let model_info = ModelInfo {
        name: display_name,
        model_id: None,
        quant: None,
        backend: "llama_cpp".to_string(),
        gpu_variant: discovery::detect_gpu_variant_label(binary_path),
        context_length: bench_config.ctx_override,
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
        vram: crate::gpu::vram::query_vram(),
    };

    // Stream the full report to the client via the progress sink. The
    // frontend uses this to render the header card (model / backend / GPU /
    // VRAM) plus the per-test results table — so we serialize the whole
    // report, not just `summaries`.
    if let Ok(report_json) = serde_json::to_string(&report) {
        progress.result(&report_json);
    }

    Ok(report)
}

#[cfg(test)]
mod tests {}
