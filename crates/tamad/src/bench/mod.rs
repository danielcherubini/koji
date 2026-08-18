//! Benchmark execution on the tamad host (plan-191 Task 8).
//!
//! Benchmarks (llama-bench, spec, MTP) measure the *tamad's* hardware, so
//! the runners execute here as jobs. The proxy ships a
//! [`RunBenchmarkRequest`] with the model file and backend binary as
//! paths RELATIVE to this host's roots (`models_dir` / `install`); the
//! model metadata the runners need for their report travels in
//! `config_json` (the proxy serialized it from the central DB — the
//! tamad holds no database, invariant 2).
//!
//! [`run_benchmark`] resolves the host paths, reports progress through
//! the [`JobHandle`], and returns the serialized report JSON (the same
//! structs the proxy persists into benchmark history).

mod llama_bench;
mod llama_cli_mtp;
mod llama_cli_spec;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;

use crate::bench::llama_bench::run_llama_bench_resolved;
use crate::bench::llama_cli_mtp::run_mtp_bench;
use crate::bench::llama_cli_spec::run_spec_bench;
use tama_core::bench::llama_bench::LlamaBenchConfigJson;
use tama_core::bench::llama_cli_mtp::MtpBenchConfig;
use tama_core::bench::llama_cli_spec::SpecBenchConfig;
use tama_core::installations::ProgressSink;
use tama_core::tamad::RunBenchmarkRequest;

use crate::jobs::JobHandle;
use crate::state::TamadState;

/// Benchmark job kind string (the `JobEvent.kind` wire value).
pub const KIND: &str = "benchmark";

/// Wire values accepted in `RunBenchmarkRequest.kind`.
pub const KIND_LLAMA_BENCH: &str = "llama_bench";
pub const KIND_SPEC: &str = "spec";
pub const KIND_MTP: &str = "mtp";

/// Future returned by a [`BenchExecutor::run`] call (borrowing the bench).
pub type BenchRunFuture<'a> = Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

/// A benchmark request with all paths resolved onto this host's disks.
#[derive(Debug, Clone)]
pub struct ResolvedBench {
    /// Model display identity for the request (proxy-provided).
    pub model_name: String,
    /// The kind: "llama_bench" | "spec" | "mtp".
    pub kind: String,
    /// The model file on this host (`state.models_dir` + `model_path_rel`).
    pub model_path: std::path::PathBuf,
    /// The backend (llama-server) binary on this host
    /// (`state.install_dir()` + `binary_path_rel`).
    pub binary_path: std::path::PathBuf,
    /// The proxy-serialized per-kind config (untouched).
    pub config_json: String,
}

/// Executor for benchmark runs (plan-191 Task 8).
///
/// Dependency-injection seam: production uses [`TamadBenchExecutor`] (the
/// real `tama_core::bench` runners executing their subprocesses on this
/// host); unit tests use a stub that returns a scripted result JSON.
pub trait BenchExecutor: Send + Sync {
    /// Execute the resolved benchmark, streaming progress through `sink`.
    ///
    /// `Ok` carries the serialized report JSON (the persistence struct for
    /// the kind); `Err` fails the benchmark job with the error message.
    fn run<'a>(
        &'a self,
        bench: &'a ResolvedBench,
        sink: std::sync::Arc<dyn ProgressSink>,
    ) -> BenchRunFuture<'a>;
}

/// ProgressSink bridging runner output lines to the job handle.
///
/// Benchmark progress is message-only (progress 0) — the runners report
/// phase lines, not fractions. The sink's `result` callback is a no-op:
/// the runner's returned report is the job's result JSON.
struct JobProgressSink {
    handle: JobHandle,
}

impl ProgressSink for JobProgressSink {
    fn log(&self, line: &str) {
        if !line.trim().is_empty() {
            self.handle.report(0, line);
        }
    }

    fn result(&self, _json: &str) {}
}

/// Production executor: the real `tama_core::bench` runners.
///
/// - `llama_bench` → the DB-free core [`run_llama_bench_resolved`] (the
///   llama-bench binary is discovered relative to the backend binary);
///   the proxy's envelope supplies the report's `ModelInfo`.
/// - `spec` / `mtp` → `run_spec_bench` / `run_mtp_bench` (already
///   DB-free); `model_path` is overwritten with the resolved host path
///   and the backend binary is passed as the binary override.
pub struct TamadBenchExecutor;

fn parse_config_json<T: DeserializeOwned>(config_json: &str, kind: &str) -> Result<T> {
    serde_json::from_str(config_json)
        .with_context(|| format!("invalid {kind} config_json: {config_json}"))
}

impl BenchExecutor for TamadBenchExecutor {
    fn run<'a>(
        &'a self,
        bench: &'a ResolvedBench,
        sink: std::sync::Arc<dyn ProgressSink>,
    ) -> BenchRunFuture<'a> {
        Box::pin(async move {
            match bench.kind.as_str() {
                KIND_LLAMA_BENCH => {
                    let envelope: LlamaBenchConfigJson =
                        parse_config_json(&bench.config_json, "llama_bench")?;

                    let mut report = run_llama_bench_resolved(
                        &bench.model_path,
                        &bench.binary_path,
                        None,
                        &envelope.bench,
                        &*sink,
                    )
                    .await?;

                    // The core derives ModelInfo from paths alone;
                    // overlay the proxy-resolved metadata (the tamad has
                    // no DB). The GPU label stays core-derived (binary
                    // path heuristic on this host).
                    report.model_info.name = envelope.model_info.name;
                    report.model_info.model_id = envelope.model_info.model_id;
                    report.model_info.quant = envelope.model_info.quant;
                    report.model_info.backend = envelope.model_info.backend;
                    report.model_info.context_length = envelope.model_info.context_length;
                    report.model_info.gpu_layers = envelope.model_info.gpu_layers;

                    let json =
                        serde_json::to_string(&report).context("serializing llama_bench report")?;
                    Ok(json)
                }
                KIND_SPEC => {
                    let mut config: SpecBenchConfig =
                        parse_config_json(&bench.config_json, "spec")?;
                    // The proxy serialized its own model path; re-anchor
                    // it to this host's models dir.
                    config.model_path = bench.model_path.clone();

                    let mut result =
                        run_spec_bench(&config, Some(bench.binary_path.clone()), sink).await?;
                    // This host samples its own VRAM (ADR-0010): the proxy
                    // persists what the execution host reports.
                    result.vram = crate::gpu::vram::query_vram();
                    let json = serde_json::to_string(&result).context("serializing spec result")?;
                    Ok(json)
                }
                KIND_MTP => {
                    let mut config: MtpBenchConfig = parse_config_json(&bench.config_json, "mtp")?;
                    config.model_path = bench.model_path.clone();

                    let mut result =
                        run_mtp_bench(&config, Some(bench.binary_path.clone()), sink).await?;
                    result.vram = crate::gpu::vram::query_vram();
                    let json = serde_json::to_string(&result).context("serializing mtp result")?;
                    Ok(json)
                }
                other => bail!("unknown benchmark kind '{other}'"),
            }
        })
    }
}

/// Resolve a `RunBenchmarkRequest` against this host's roots.
///
/// - `model_path_rel` → `state.models_dir` (the model file must exist here)
/// - `binary_path_rel` → `state.install_dir()` (the llama-server binary
///   must exist here)
///
/// Missing files fail with an actionable host-path error (the jobs
/// registry marks the job failed with this message).
pub fn resolve_req(req: &RunBenchmarkRequest, state: &TamadState) -> Result<ResolvedBench> {
    if !matches!(req.kind.as_str(), KIND_LLAMA_BENCH | KIND_SPEC | KIND_MTP) {
        bail!(
            "unknown benchmark kind '{}' (expected llama_bench, spec, or mtp)",
            req.kind
        );
    }
    if req.model_path_rel.trim().is_empty() {
        bail!("model_path_rel must not be empty");
    }
    if req.binary_path_rel.trim().is_empty() {
        bail!("binary_path_rel must not be empty");
    }

    let model_path = state.models_dir.join(&req.model_path_rel);
    if !model_path.is_file() {
        bail!("model not found on this host: {}", model_path.display());
    }

    let binary_path = state.install_dir().join(&req.binary_path_rel);
    if !binary_path.is_file() {
        bail!("binary not found on this host: {}", binary_path.display());
    }

    Ok(ResolvedBench {
        model_name: req.model_name.clone(),
        kind: req.kind.clone(),
        model_path,
        binary_path,
        config_json: req.config_json.clone(),
    })
}

/// Validate `config_json` against `kind` without executing (fast,
/// actionable pre-validation for the `RunBenchmark` RPC).
pub fn validate_config_json(kind: &str, config_json: &str) -> Result<()> {
    match kind {
        KIND_LLAMA_BENCH => {
            parse_config_json::<LlamaBenchConfigJson>(config_json, "llama_bench").map(|_| ())
        }
        KIND_SPEC => parse_config_json::<SpecBenchConfig>(config_json, "spec").map(|_| ()),
        KIND_MTP => parse_config_json::<MtpBenchConfig>(config_json, "mtp").map(|_| ()),
        other => bail!("unknown benchmark kind '{other}' (expected llama_bench, spec, or mtp)"),
    }
}

/// Run a benchmark job to completion: resolve the host paths, execute via
/// `executor` (progress through the job handle), and return the result
/// JSON. Any error fails the job with its message.
pub async fn run_benchmark(
    req: &RunBenchmarkRequest,
    state: &TamadState,
    handle: JobHandle,
    executor: &dyn BenchExecutor,
) -> Result<String> {
    let bench = resolve_req(req, state)?;
    handle.report(
        0,
        &format!(
            "benchmark {} '{}' (model {})",
            bench.kind,
            bench.model_name,
            bench.model_path.display()
        ),
    );

    let sink = Arc::new(JobProgressSink {
        handle: handle.clone(),
    });
    let json = executor.run(&bench, sink).await?;
    handle.report(0, "benchmark finished");
    Ok(json)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// Find an available port by binding to port 0.
///
/// Used by the spec/MTP runners to pick an ephemeral listen port for the
/// spawned llama-server (moved from `tama_core::bench` in plan-191 Task 10 —
/// only runners use it now).
pub async fn find_available_port() -> anyhow::Result<u16> {
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static PORT_SALT: AtomicUsize = AtomicUsize::new(20000);
    let base = PORT_SALT.fetch_add(1, Ordering::Relaxed) as u16;
    for i in 0..50u16 {
        let port = base.wrapping_add(i);
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::jobs::{JobRegistry, STATUS_FAILED, STATUS_SUCCEEDED};
    use tama_core::bench::{BenchReport, ModelInfo};

    use anyhow::anyhow;

    /// Stub executor: captures the resolved bench, returns a scripted
    /// result JSON (or a scripted failure).
    #[derive(Clone)]
    struct StubExecutor {
        json: String,
        error: Option<String>,
        captured: Arc<Mutex<Option<ResolvedBench>>>,
        gate: Option<Arc<tokio::sync::Notify>>,
    }

    impl BenchExecutor for StubExecutor {
        fn run<'a>(
            &'a self,
            bench: &'a ResolvedBench,
            _sink: Arc<dyn ProgressSink>,
        ) -> BenchRunFuture<'a> {
            let captured = Arc::clone(&self.captured);
            let error = self.error.clone();
            let json = self.json.clone();
            let gate = self.gate.clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.notified().await;
                }
                *captured.lock().unwrap() = Some(bench.clone());
                match error {
                    Some(e) => Err(anyhow!(e)),
                    None => Ok(json),
                }
            })
        }
    }

    /// A minimal valid `BenchReport` JSON (the persistence struct the
    /// proxy deserializes the job result into).
    const MIN_REPORT_JSON: &str = r#"{
        "model_info": {"name": "m", "model_id": "org/m", "quant": "Q4_K_M",
                        "backend": "llama_cpp", "gpu_variant": "CPU",
                        "context_length": 4096, "gpu_layers": null},
        "config": {"pp_sizes": [64], "tg_sizes": [16], "runs": 1, "warmup": 0,
                   "ctx_override": null, "batch_sizes": [], "ubatch_sizes": [],
                   "kv_cache_type": null, "depth": [], "flash_attn": null},
        "summaries": [
            {"test_name": "pp64", "prompt_tokens": 64, "gen_tokens": 0,
             "pp_mean": 6400.0, "pp_stddev": 0.0, "tg_mean": 0.0, "tg_stddev": 0.0,
             "ttft_mean": 0.0, "ttft_stddev": 0.0, "total_mean": 0.0, "total_stddev": 0.0,
             "n_depth": null, "n_batch": null, "n_ubatch": null,
             "type_k": null, "type_v": null, "flash_attn": null,
             "n_threads": null, "n_gpu_layers": null}
        ],
        "load_time_ms": 0.0,
        "vram": null
    }"#;

    fn req(
        model_path_rel: &str,
        binary_path_rel: &str,
        kind: &str,
        config_json: &str,
    ) -> RunBenchmarkRequest {
        RunBenchmarkRequest {
            model_name: "Test Model".to_string(),
            kind: kind.to_string(),
            config_json: config_json.to_string(),
            model_path_rel: model_path_rel.to_string(),
            binary_path_rel: binary_path_rel.to_string(),
        }
    }

    /// Create the model file + backend binary under the tamad roots.
    fn seed_host_files(state: &TamadState) -> (String, String) {
        let model_rel = "org/m/model-Q4_K_M.gguf";
        let model_path = state.models_dir.join(model_rel);
        std::fs::create_dir_all(model_path.parent().unwrap()).unwrap();
        std::fs::write(&model_path, vec![0u8; 32]).unwrap();

        let binary_rel = "llama_cpp/cpu/v1/llama-server";
        let binary_path = state.install_dir().join(binary_rel);
        std::fs::create_dir_all(binary_path.parent().unwrap()).unwrap();
        std::fs::write(&binary_path, b"#!/bin/sh\n").unwrap();

        (model_rel.to_string(), binary_rel.to_string())
    }

    /// Poll the registry until the job is terminal.
    async fn wait_terminal(registry: &Arc<JobRegistry>, id: &str) -> crate::jobs::Job {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let job = registry.get(id).expect("job must exist");
            if job.is_terminal() {
                return job;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "job did not reach a terminal state"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Path resolution joins the tamad roots; missing model/binary/unknown
    /// kind all fail with actionable messages.
    #[test]
    fn test_resolve_req_paths_and_errors() {
        let (state, _dir) = crate::server::test_support::test_state();
        let (model_rel, binary_rel) = seed_host_files(&state);

        let Ok(resolved) = resolve_req(
            &req(&model_rel, &binary_rel, KIND_LLAMA_BENCH, "{}"),
            &state,
        ) else {
            panic!("resolution must succeed when both files exist");
        };
        assert_eq!(resolved.model_path, state.models_dir.join(&model_rel));
        assert_eq!(resolved.binary_path, state.install_dir().join(&binary_rel));
        assert_eq!(resolved.model_name, "Test Model");
        assert_eq!(resolved.kind, KIND_LLAMA_BENCH);

        let err = resolve_req(&req("nope/m.gguf", &binary_rel, KIND_SPEC, "{}"), &state)
            .unwrap_err()
            .to_string();
        assert!(err.contains("model not found on this host"), "got: {err}");

        let err = resolve_req(
            &req(&model_rel, "nope/llama-server", KIND_MTP, "{}"),
            &state,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("binary not found on this host"), "got: {err}");

        let err = resolve_req(&req(&model_rel, &binary_rel, "docker", "{}"), &state)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown benchmark kind"), "got: {err}");
    }

    /// Wiring: the stub executor sees the resolved host paths, and its
    /// result JSON round-trips through the persistence struct.
    #[tokio::test]
    async fn test_run_benchmark_wiring_roundtrip() {
        let (state, _dir) = crate::server::test_support::test_state();
        let (model_rel, binary_rel) = seed_host_files(&state);
        let captured = Arc::new(Mutex::new(None));
        let stub = StubExecutor {
            json: MIN_REPORT_JSON.to_string(),
            error: None,
            captured: Arc::clone(&captured),
            gate: None,
        };

        let registry = JobRegistry::new();
        let state_for_run = Arc::clone(&state);
        let model_rel_for_run = model_rel.clone();
        let binary_rel_for_run = binary_rel.clone();
        let id = registry
            .start(KIND, move |handle| {
                let stub = stub.clone();
                Box::pin(async move {
                    run_benchmark(
                        &req(
                            &model_rel_for_run,
                            &binary_rel_for_run,
                            KIND_LLAMA_BENCH,
                            "{}",
                        ),
                        &state_for_run,
                        handle,
                        &stub,
                    )
                    .await
                })
            })
            .await;

        let job = wait_terminal(&registry, &id).await;
        assert_eq!(job.status, STATUS_SUCCEEDED, "error: {:?}", job.error);
        let result_json = job.result_json.expect("terminal result JSON");
        // The result must be a valid, parseable BenchReport (the proxy
        // deserializes it into the persistence struct).
        let report: BenchReport =
            serde_json::from_str(&result_json).expect("result must parse as BenchReport");
        assert_eq!(report.model_info.quant.as_deref(), Some("Q4_K_M"));
        assert_eq!(report.summaries.len(), 1);

        // The executor saw the host-resolved paths.
        let resolved = captured.lock().unwrap().take().expect("executor ran");
        assert_eq!(resolved.model_path, state.models_dir.join(&model_rel));
        assert_eq!(resolved.binary_path, state.install_dir().join(&binary_rel));
    }

    /// Missing model file on this host → the job fails with the host path
    /// in its error.
    #[tokio::test]
    async fn test_run_benchmark_missing_model_fails_job() {
        let (state, _dir) = crate::server::test_support::test_state();
        let (_model_rel, binary_rel) = seed_host_files(&state);

        let stub = StubExecutor {
            json: MIN_REPORT_JSON.to_string(),
            error: None,
            captured: Arc::new(Mutex::new(None)),
            gate: None,
        };
        let registry = JobRegistry::new();
        let id = registry
            .start(KIND, move |handle| {
                let stub = stub.clone();
                Box::pin(async move {
                    run_benchmark(
                        &req("missing/ghost.gguf", &binary_rel, KIND_SPEC, "{}"),
                        &state,
                        handle,
                        &stub,
                    )
                    .await
                })
            })
            .await;

        let job = wait_terminal(&registry, &id).await;
        assert_eq!(job.status, STATUS_FAILED);
        let err = job.error.unwrap_or_default();
        assert!(
            err.contains("model not found on this host: ") && err.contains("missing/ghost.gguf"),
            "got: {err}"
        );
    }

    /// Missing backend binary on this host → the job fails with the host
    /// path in its error.
    #[tokio::test]
    async fn test_run_benchmark_missing_binary_fails_job() {
        let (state, _dir) = crate::server::test_support::test_state();
        let (model_rel, _binary_rel) = seed_host_files(&state);

        let stub = StubExecutor {
            json: MIN_REPORT_JSON.to_string(),
            error: None,
            captured: Arc::new(Mutex::new(None)),
            gate: None,
        };
        let registry = JobRegistry::new();
        let id = registry
            .start(KIND, move |handle| {
                let stub = stub.clone();
                Box::pin(async move {
                    run_benchmark(
                        &req(&model_rel, "missing/llama-server", KIND_MTP, "{}"),
                        &state,
                        handle,
                        &stub,
                    )
                    .await
                })
            })
            .await;

        let job = wait_terminal(&registry, &id).await;
        assert_eq!(job.status, STATUS_FAILED);
        let err = job.error.unwrap_or_default();
        assert!(
            err.contains("binary not found on this host: ") && err.contains("missing/llama-server"),
            "got: {err}"
        );
    }

    /// The production executor rejects a malformed llama_bench config_json
    /// before running anything.
    #[tokio::test]
    async fn test_tamad_executor_rejects_bad_config_json() {
        let (state, _dir) = crate::server::test_support::test_state();
        let (model_rel, binary_rel) = seed_host_files(&state);

        let handle_registry = JobRegistry::new();
        let id = handle_registry
            .start(KIND, move |handle| {
                Box::pin(async move {
                    run_benchmark(
                        &req(&model_rel, &binary_rel, KIND_LLAMA_BENCH, "not-json"),
                        &state,
                        handle,
                        &TamadBenchExecutor,
                    )
                    .await
                })
            })
            .await;

        let job = wait_terminal(&handle_registry, &id).await;
        assert_eq!(job.status, STATUS_FAILED);
        let err = job.error.unwrap_or_default();
        assert!(
            err.contains("invalid llama_bench config_json"),
            "got: {err}"
        );
    }

    /// The production executor runs the REAL llama-bench subprocess path
    /// on this host: a fake `llama-bench` next to the backend binary
    /// (discovery step 2), the proxy envelope supplies the ModelInfo, and
    /// the result round-trips as a `BenchReport` with the host-derived
    /// GPU label.
    #[tokio::test]
    async fn test_tamad_executor_llama_bench_real_subprocess() {
        let (state, _dir) = crate::server::test_support::test_state();
        let (model_rel, binary_rel) = seed_host_files(&state);

        // Fake llama-bench next to the backend binary (find_llama_bench
        // step 2). It emits a minimal valid report.
        let backend_dir = state.install_dir().join("llama_cpp/cpu/v1");
        let fake_bench = backend_dir.join("llama-bench");
        std::fs::write(
            &fake_bench,
            r#"#!/bin/sh
echo '[{"n_prompt": 64, "n_gen": 0, "avg_ts": 6400.5, "stddev_ts": 7.0}, {"n_prompt": 0, "n_gen": 16, "avg_ts": 88.25, "stddev_ts": 1.25}]'
exit 0
"#,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake_bench).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_bench, perms).unwrap();

        // The proxy-style envelope: bench knobs + model metadata.
        let envelope = LlamaBenchConfigJson {
            bench: tama_core::bench::llama_bench::LlamaBenchConfig {
                pp_sizes: vec![64],
                tg_sizes: vec![16],
                runs: 1,
                warmup: 0,
                threads: None,
                ngl_range: None,
                ctx_override: Some(512),
                batch_sizes: vec![],
                ubatch_sizes: vec![],
                kv_cache_type: None,
                depth: vec![],
                flash_attn: None,
            },
            model_info: ModelInfo {
                name: "E2E Model".to_string(),
                model_id: Some("org/m".to_string()),
                quant: Some("Q4_K_M".to_string()),
                backend: "llama_cpp".to_string(),
                gpu_variant: String::new(), // host derives the real label
                context_length: Some(4096),
                gpu_layers: None,
            },
        };
        let config_json = serde_json::to_string(&envelope).expect("envelope serializes");

        let registry = JobRegistry::new();
        let state_for_run = Arc::clone(&state);
        let id = registry
            .start(KIND, move |handle| {
                Box::pin(async move {
                    run_benchmark(
                        &req(&model_rel, &binary_rel, KIND_LLAMA_BENCH, &config_json),
                        &state_for_run,
                        handle,
                        &TamadBenchExecutor,
                    )
                    .await
                })
            })
            .await;

        let job = wait_terminal(&registry, &id).await;
        assert_eq!(job.status, STATUS_SUCCEEDED, "error: {:?}", job.error);
        let report: BenchReport =
            serde_json::from_str(job.result_json.as_deref().expect("result JSON"))
                .expect("result must parse as BenchReport");

        // The fake binary's numbers made it through the real core.
        assert_eq!(report.summaries.len(), 2);
        assert_eq!(report.summaries[0].test_name, "pp64");
        assert!((report.summaries[0].pp_mean - 6400.5).abs() < 0.01);
        assert_eq!(report.summaries[1].test_name, "tg16");
        assert!((report.summaries[1].tg_mean - 88.25).abs() < 0.01);

        // Proxy envelope metadata overlaid; host-derived label kept.
        assert_eq!(report.model_info.name, "E2E Model");
        assert_eq!(report.model_info.model_id.as_deref(), Some("org/m"));
        assert_eq!(report.model_info.quant.as_deref(), Some("Q4_K_M"));
        assert_eq!(report.model_info.gpu_variant, "CPU");
    }
}
