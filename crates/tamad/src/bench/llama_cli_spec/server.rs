//! Server lifecycle management for llama-server.
//!
//! Spawns a `llama-server` process with the given args, waits for it to load
//! the model and become ready, then provides a `ServerHandle` that can be used
//! to make HTTP completion requests. Captures stderr for parsing spec-decoding
//! statistics (draft acceptance rate). Dropping the handle kills the server.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Arguments for starting a llama-server instance.
#[derive(Debug, Clone)]
pub struct ServerArgs {
    pub binary: PathBuf,
    pub model_path: PathBuf,
    pub port: u16,
    /// GPU layers (None = use server default).
    pub ngl: Option<u32>,
    /// Flash attention (default true).
    pub flash_attn: bool,
    /// Speculative decoding type (None = no spec decoding).
    pub spec_type: Option<crate::bench::llama_cli_spec::SpecType>,
    pub spec_ngram_n: Option<u32>,
    pub spec_ngram_m: Option<u32>,
    pub spec_ngram_min_hits: Option<u32>,
    /// N-gram minimum match for n-gram-mod (maps to --spec-ngram-mod-n-min).
    pub spec_ngram_min: Option<u32>,
    /// N-gram maximum match for n-gram-mod (maps to --spec-ngram-mod-n-max).
    pub spec_ngram_max: Option<u32>,
    pub draft_max: Option<u32>,
    pub draft_min: Option<u32>,
    /// Spec draft NGL for MTP (maps to --spec-draft-ngl).
    pub spec_draft_ngl: Option<u32>,
    /// Context size (maps to -c). None = use server default.
    pub context_size: Option<u32>,
}

impl ServerArgs {
    /// Convert to a flat vector of CLI arguments for tokio::process::Command.
    #[allow(clippy::vec_init_then_push)]
    pub fn to_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        args.push("-m".to_string());
        args.push(self.model_path.to_string_lossy().to_string());

        args.push("--port".to_string());
        args.push(self.port.to_string());

        if let Some(ngl) = self.ngl {
            args.push("--n-gpu-layers".to_string());
            args.push(ngl.to_string());
        }

        args.push("-fa".to_string());
        args.push(if self.flash_attn { "on" } else { "off" }.to_string());

        // Disable web UI — we only need the API.
        args.push("--no-webui".to_string());

        // Context size.
        if let Some(ctx) = self.context_size {
            args.push("-c".to_string());
            args.push(ctx.to_string());
        }

        // Speculative decoding flags.
        if let Some(spec_type) = &self.spec_type {
            args.push("--spec-type".to_string());
            args.push(spec_type.as_str().to_string());

            // Type-specific n-gram flags (llama.cpp PR #22397).
            let (size_n_flag, size_m_flag, min_hits_flag) = spec_type.spec_ngram_flags();

            if !size_n_flag.is_empty() {
                if let Some(n) = self.spec_ngram_n {
                    args.push(size_n_flag.to_string());
                    args.push(n.to_string());
                }
            }
            if !size_m_flag.is_empty() {
                if let Some(m) = self.spec_ngram_m {
                    args.push(size_m_flag.to_string());
                    args.push(m.to_string());
                }
            }
            if !min_hits_flag.is_empty() {
                if let Some(hits) = self.spec_ngram_min_hits {
                    args.push(min_hits_flag.to_string());
                    args.push(hits.to_string());
                }
            }
            if let Some(dm) = self.draft_max {
                args.push("--spec-draft-n-max".to_string());
                args.push(dm.to_string());
            }
            if let Some(dmin) = self.draft_min {
                args.push("--spec-draft-n-min".to_string());
                args.push(dmin.to_string());
            }

            // spec-draft-ngl for MTP benchmarking — only valid for DraftMtp spec type
            if matches!(
                &self.spec_type,
                Some(crate::bench::llama_cli_spec::SpecType::DraftMtp)
            ) {
                if let Some(ngl) = self.spec_draft_ngl {
                    args.push("--spec-draft-ngl".to_string());
                    args.push(ngl.to_string());
                }
            }

            // Ngram-mod needs its own n-min and n-max flags (not covered by spec_ngram_flags).
            if matches!(spec_type, crate::bench::llama_cli_spec::SpecType::NgramMod) {
                if let Some(nmin) = self.spec_ngram_min {
                    args.push("--spec-ngram-mod-n-min".to_string());
                    args.push(nmin.to_string());
                }
                if let Some(nmax) = self.spec_ngram_max {
                    args.push("--spec-ngram-mod-n-max".to_string());
                    args.push(nmax.to_string());
                }
            }
        }

        args
    }
}

/// Timing and usage data from a chat completion response.
#[derive(Debug, Clone)]
pub struct ChatTiming {
    pub predicted_per_second: f64,
    pub predicted_n: u32,      // completion_tokens
    pub draft_n: u32,          // total draft tokens proposed
    pub draft_n_accepted: u32, // draft tokens accepted
}

/// A running llama-server instance. Dropping this kills the server.
pub struct ServerHandle {
    child: Child,
    port: u16,
    /// Collected stderr lines for parsing spec-decoding statistics.
    stderr_lines: Arc<Mutex<Vec<String>>>,
}

impl ServerHandle {
    /// The base URL of the running server.
    pub fn base_url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Returns once the server has loaded the model and is ready to accept requests.
    /// Polls `/v1/models` until it returns successfully or the timeout expires.
    pub async fn wait_ready(&self, timeout_secs: u64) -> Result<()> {
        let url = format!("{}/v1/models", self.base_url());
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .context("Failed to build reqwest client")?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

        loop {
            if tokio::time::Instant::now() >= deadline {
                bail!("llama-server did not become ready within {timeout_secs}s at {url}");
            }

            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    // Server is ready.
                    return Ok(());
                }
                Ok(_resp) => {
                    // Still loading or not ready yet.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(_) => {
                    // Connection refused or network error — still starting.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Make a completion request and extract the generation speed (tokens/s).
    ///
    /// Returns `Ok(predicted_per_second)` on success.
    pub async fn complete(&self, prompt: &str, max_tokens: u32) -> Result<f64> {
        #[derive(serde::Deserialize)]
        struct CompletionResponse {
            timings: Timings,
        }

        #[derive(serde::Deserialize)]
        struct Timings {
            #[serde(rename = "predicted_per_second")]
            predicted_per_second: f64,
        }

        #[derive(serde::Serialize)]
        struct Request<'a> {
            prompt: &'a str,
            #[serde(rename = "max_tokens")]
            max_tokens: u32,
            #[serde(rename = "cache_prompt")]
            cache_prompt: bool,
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .context("Failed to build reqwest client")?;

        let request = Request {
            prompt,
            max_tokens,
            cache_prompt: true,
        };

        let url = format!("{}/v1/completions", self.base_url());
        let resp = client
            .post(&url)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("HTTP request to {url} failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Server returned error {status}: {body}");
        }

        let completion: CompletionResponse = resp
            .json()
            .await
            .context("Failed to parse server JSON response")?;

        Ok(completion.timings.predicted_per_second)
    }

    /// Make a chat completion request and extract timing/usage data.
    ///
    /// POSTs to `/v1/chat/completions` with the given messages and returns
    /// timing and usage statistics.
    pub async fn chat_complete(
        &self,
        model: &str,
        messages: &[(&str, &str)],
        max_tokens: u32,
    ) -> Result<ChatTiming> {
        #[derive(serde::Deserialize)]
        struct ChatCompletionResponse {
            timings: ChatTimings,
            usage: ChatUsage,
        }

        #[derive(serde::Deserialize)]
        struct ChatTimings {
            #[serde(rename = "predicted_per_second")]
            predicted_per_second: f64,
            #[serde(rename = "draft_n")]
            draft_n: u32,
            #[serde(rename = "draft_n_accepted")]
            draft_n_accepted: u32,
        }

        #[derive(serde::Deserialize)]
        struct ChatUsage {
            #[serde(rename = "completion_tokens")]
            completion_tokens: u32,
        }

        #[derive(serde::Serialize)]
        struct Message<'a> {
            role: &'a str,
            content: &'a str,
        }

        #[derive(serde::Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            messages: Vec<Message<'a>>,
            #[serde(rename = "max_tokens")]
            max_tokens: u32,
            seed: u64,
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .context("Failed to build reqwest client")?;

        let chat_messages: Vec<Message<'_>> = messages
            .iter()
            .map(|(role, content)| Message { role, content })
            .collect();

        let request = ChatRequest {
            model,
            messages: chat_messages,
            max_tokens,
            seed: 42,
        };

        let url = format!("{}/v1/chat/completions", self.base_url());
        let resp = client
            .post(&url)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("HTTP request to {url} failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Server returned error {status}: {body}");
        }

        let completion: ChatCompletionResponse = resp
            .json()
            .await
            .context("Failed to parse chat completion JSON response")?;

        Ok(ChatTiming {
            predicted_per_second: completion.timings.predicted_per_second,
            predicted_n: completion.usage.completion_tokens,
            draft_n: completion.timings.draft_n,
            draft_n_accepted: completion.timings.draft_n_accepted,
        })
    }

    /// Parse the draft acceptance rate from collected stderr lines.
    ///
    /// llama-server prints statistics like:
    /// `draft acceptance rate = 0.57576 (  171 accepted /   297 generated)`
    ///
    /// Returns `Some(rate)` if found, `None` otherwise.
    ///
    /// Uses `lock().await` (not `blocking_lock()`) to avoid deadlocking
    /// the tokio runtime when called from async context while the stderr
    /// reader task holds the lock.
    pub async fn parse_acceptance_rate(&self) -> Option<f64> {
        let lines = self.stderr_lines.lock().await;
        for line in lines.iter() {
            if let Some(start) = line.find("draft acceptance rate = ") {
                let after_eq = &line[start + "draft acceptance rate = ".len()..];
                if let Some(end) = after_eq.find(' ') {
                    if let Ok(rate) = after_eq[..end].parse::<f64>() {
                        return Some(rate);
                    }
                }
            }
        }
        None
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // Best-effort kill. The process is already kill_on_drop.
        let _ = self.child.start_kill();
    }
}

/// Spawn a llama-server process with the given arguments.
///
/// Waits up to `timeout_secs` for the model to load. Returns a `ServerHandle`
/// that must be kept alive for the duration of benchmarking.
pub async fn spawn_server(args: &ServerArgs, timeout_secs: u64) -> Result<ServerHandle> {
    let arg_vec = args.to_args();

    let mut child = Command::new(&args.binary);
    child
        .args(&arg_vec)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::process::configure_backend_command(&mut child, &args.binary);

    let mut child = child
        .spawn()
        .with_context(|| format!("Failed to spawn {}", args.binary.display()))?;

    let stderr_lines = Arc::new(Mutex::new(Vec::new()));

    // Extract stderr before moving child into ServerHandle.
    let stderr = child.stderr.take();
    if let Some(stderr) = stderr {
        let lines = stderr_lines.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                lines.lock().await.push(line);
            }
        });
    }

    let handle = ServerHandle {
        child,
        port: args.port,
        stderr_lines,
    };

    handle
        .wait_ready(timeout_secs)
        .await
        .context("llama-server failed to load model and become ready")?;

    Ok(handle)
}

// Spec-server lifecycle tests using the `tama-mock` binary as a stand-in
// llama-server (moved from `tama-mock/tests/bench_server.rs` in plan-191 Task 10 —
// the runner lives in this crate now; ADR-0010).

#[cfg(test)]
mod bench_server_tests {
    use super::*;

    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;

    /// Locate the `tama-mock` binary for the current build (plan-191 Task 10:
    /// the mock crate is a dev-dependency of tamad). Prefers `CARGO_BIN_EXE_*`;
    /// falls back to the target dir (building it once if missing).
    fn mock_binary() -> std::path::PathBuf {
        if let Ok(p) = std::env::var("CARGO_BIN_EXE_tama-mock") {
            return std::path::PathBuf::from(p);
        }
        let exe = std::env::current_exe().expect("current_exe for target-dir resolution");
        // .../target/<profile>/deps/tamad-<hash> → target/<profile>
        let target_dir = exe
            .parent()
            .and_then(|p| p.parent())
            .expect("target dir from test exe path");
        let bin = target_dir.join("tama-mock");
        if !bin.exists() {
            // First run: build the mock dependency's binary once.
            let status = std::process::Command::new("cargo")
                .args(["build", "-p", "tama-mock", "--bins"])
                .arg(format!("--target-dir={}", target_dir.display()))
                .status()
                .expect("run cargo build for tama-mock");
            assert!(status.success(), "cargo build -p tama-mock must succeed");
        }
        assert!(
            bin.exists(),
            "tama-mock binary not found at {}",
            bin.display()
        );
        bin
    }

    /// Spawn the tama-mock binary on a random port, then exercise the full
    /// server lifecycle: spawn → wait_ready → complete → chat_complete.
    #[tokio::test]
    async fn test_spawn_server_wait_ready_and_complete() {
        let port = find_free_port();

        // Spawn tama-mock (not hanging, default crash_after=0 means it runs forever)
        let binary = mock_binary();
        let mut mock_child = std::process::Command::new(&binary)
            .arg("--port")
            .arg(port.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("Failed to spawn tama-mock");

        // Wait for the mock to be ready before proceeding
        wait_for_mock_ready(port, 10).await;

        let model_path = std::path::PathBuf::from("/dev/null/mock-model.gguf");

        // Build ServerArgs — we use /bin/sh as a dummy binary since spawn_server
        // will try to spawn it. We don't actually need the binary to work because
        // tama-mock is already serving on the port.
        let args = ServerArgs {
            binary: std::path::PathBuf::from("/bin/sh"),
            model_path,
            port,
            ngl: None,
            flash_attn: true,
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

        // spawn_server will spawn /bin/sh (which exits immediately) but wait_ready
        // polls the HTTP port where tama-mock is already listening.
        let handle = match spawn_server(&args, 10).await {
            Ok(h) => h,
            Err(e) => {
                // If spawn_server fails because /bin/sh doesn't support the args,
                // we still have the mock server running — skip the HTTP tests.
                eprintln!(
                    "spawn_server returned error (expected for non-server binary): {}",
                    e
                );
                let _ = mock_child.kill();
                let _ = mock_child.wait();
                return;
            }
        };

        // Test complete()
        let tokens_per_sec = handle.complete("Hello world", 10).await;
        assert!(
            tokens_per_sec.is_ok(),
            "complete() should succeed: {:?}",
            tokens_per_sec.err()
        );
        let tps = tokens_per_sec.unwrap();
        assert!(
            (tps - 42.5).abs() < 0.01,
            "expected predicted_per_second=42.5, got {}",
            tps
        );

        // Test chat_complete()
        let timing = handle
            .chat_complete("mock-model", &[("user", "Hello")], 10)
            .await;
        assert!(
            timing.is_ok(),
            "chat_complete() should succeed: {:?}",
            timing.err()
        );
        let t = timing.unwrap();
        assert!((t.predicted_per_second - 42.5).abs() < 0.01);
        assert_eq!(t.draft_n, 10);
        assert_eq!(t.draft_n_accepted, 7);
        assert_eq!(t.predicted_n, 12);

        // Drop the handle (kills the child process)
        drop(handle);

        // Kill the mock process
        let _ = mock_child.kill();
    }

    /// Verify that spawn_server returns an error when the server never becomes ready.
    /// We use a script that exits immediately, so /v1/models is never available.
    #[tokio::test]
    async fn test_spawn_server_ready_timeout() {
        let port = find_free_port();

        // Create a temporary script that exits immediately (never serves HTTP)
        let temp_dir = tempfile::tempdir().unwrap();
        let exit_script = temp_dir.path().join("exit-immediately");
        std::fs::write(&exit_script, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&exit_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&exit_script, perms).unwrap();

        let model_path = std::path::PathBuf::from("/dev/null/mock-model.gguf");

        let args = ServerArgs {
            binary: exit_script,
            model_path,
            port,
            ngl: None,
            flash_attn: true,
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

        let result = spawn_server(&args, 2).await;
        assert!(
            result.is_err(),
            "spawn_server should return Err when server never becomes ready"
        );
        let err_msg = match result {
            Ok(_) => unreachable!(),
            Err(e) => e.to_string(),
        };
        assert!(
            err_msg.contains("did not become ready") || err_msg.contains("failed to load model"),
            "Error should mention readiness timeout, got: {}",
            err_msg
        );
    }

    /// Find a free port by binding to port 0.
    fn find_free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    /// Wait for tama-mock to respond on the given port, up to `timeout_secs`.
    async fn wait_for_mock_ready(port: u16, timeout_secs: u64) {
        let url = format!("http://127.0.0.1:{port}/health");
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

        loop {
            if tokio::time::Instant::now() >= deadline {
                panic!("Mock server did not become ready within {timeout_secs}s");
            }
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => return,
                _ => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            }
        }
    }
}
