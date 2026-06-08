# LLMLingua-2 Compaction Endpoint Plan

**Goal:** Add a `/v1/compaction` endpoint to the Tama proxy that compresses prompts using Microsoft's LLMLingua-2 model, reducing token counts before they hit the main LLM.

**Architecture:** A Python FastAPI subprocess (LLMLingua-2 + XLM-RoBERTa-large model) is managed by the Rust proxy. The proxy spawns the server lazily on first request, forwards HTTP requests to it, and handles fallback gracefully. Follows the Kokoro TTS subprocess pattern (spawn, health poll, reaper).

**Tech Stack:** Rust (axum proxy handler, tokio subprocess), Python (FastAPI + uvicorn + llmlingua + torch + transformers), embedded server files via `include_dir!`.

---

### Task 1: Add CompactionConfig to Config Types

**Context:**
The compaction feature needs configuration in `~/.config/tama/config.toml` under a `[compaction]` section. This task adds the config struct and defaults. The config is optional — when absent, `/v1/compaction` returns 501 Not Implemented.

**Files:**
- Modify: `crates/tama-core/src/config/types.rs`
- Modify: `crates/tama-core/src/config/defaults.rs`
- Modify: `crates/tama-core/src/config/mod.rs`

**What to implement:**

Add the following struct to `config/types.rs`:

```rust
/// Configuration for the LLMLingua-2 compaction service.
/// When absent from config.toml, compaction is disabled.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompactionConfig {
    /// Whether compaction is enabled. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the Python entrypoint (main.py). If omitted, uses embedded default.
    #[serde(default)]
    pub server_path: Option<String>,
    /// Path to the Python virtual environment. If omitted, uses system python.
    #[serde(default)]
    pub venv_path: Option<String>,
    /// Compute device: "cpu", "cuda", "cuda:0", "mps". Default: "cpu".
    #[serde(default = "default_compaction_device")]
    pub device: String,
    /// Fixed port for the compaction server. If omitted, auto-assigned via TcpListener.
    #[serde(default)]
    pub port: Option<u16>,
    /// Request timeout in milliseconds. Default: 30000 (30s).
    #[serde(default = "default_compaction_timeout_ms")]
    pub timeout_ms: u64,
}
```

Add default functions:

```rust
fn default_compaction_device() -> String {
    "cpu".to_string()
}

fn default_compaction_timeout_ms() -> u64 {
    30_000
}
```

Add `compaction` field to the `Config` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: General,
    // ... existing fields ...
    #[serde(default)]
    pub compaction: CompactionConfig,
}
```

Add `CompactionConfig` to the re-export in `config/mod.rs`:

```rust
pub use types::{
    // ... existing re-exports ...
    CompactionConfig,
};
```

Add tests in the `#[cfg(test)]` module of `types.rs`:
- `test_compaction_config_defaults` — verify all defaults
- `test_compaction_config_toml_roundtrip` — serialize to TOML, deserialize, verify
- `test_compaction_config_disabled_by_default` — verify `enabled` defaults to `false`

**Steps:**
- [ ] Write failing test `test_compaction_config_defaults` in `crates/tama-core/src/config/types.rs`
- [ ] Run `cargo test --package tama-core -- config::types::tests::test_compaction_config_defaults`
  - Did it fail with compile error (missing struct)? If it passed, investigate.
- [ ] Implement `CompactionConfig` struct in `crates/tama-core/src/config/types.rs`
- [ ] Add default functions (`default_compaction_device`, `default_compaction_timeout_ms`)
- [ ] Add `compaction` field to `Config` struct with `#[serde(default)]`
- [ ] Write `test_compaction_config_toml_roundtrip` test
- [ ] Write `test_compaction_config_disabled_by_default` test
- [ ] Run `cargo test --package tama-core -- config::types::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add CompactionConfig to config types"

**Acceptance criteria:**
- [ ] `CompactionConfig` struct exists with all 6 fields
- [ ] `Config` struct has `compaction: CompactionConfig` field with `#[serde(default)]`
- [ ] All 3 tests pass
- [ ] `cargo build --package tama-core` succeeds
- [ ] TOML round-trip preserves all field values

---

### Task 2: Create Embedded Python Compaction Server

**Context:**
The compaction server is a standalone FastAPI app that wraps LLMLingua-2. It runs as a Python subprocess spawned by Tama. The server files are embedded in the Rust binary using `include_dir!` and extracted to `~/.config/tama/compaction_server/` at runtime.

**Files:**
- Create: `crates/tama-core/src/compaction_server/mod.rs`
- Create: `crates/tama-core/src/compaction_server/server/main.py`
- Create: `crates/tama-core/src/compaction_server/server/requirements.txt`
- Modify: `crates/tama-core/Cargo.toml`

**What to implement:**

Add `include_dir` to `crates/tama-core/Cargo.toml` [dependencies]:

```toml
include_dir = { workspace = true }
```

Create `compaction_server/mod.rs`:

```rust
//! Embedded Python compaction server (LLMLingua-2).
//!
//! Server files are embedded via include_dir! and extracted to the config
//! directory on first use.

use anyhow::Context;
use std::path::PathBuf;

// include_dir resolves paths relative to CARGO_MANIFEST_DIR (crate root).
// CARGO_MANIFEST_DIR for tama-core is crates/tama-core/.
static SERVER_FILES: include_dir::Dir =
    include_dir::dir!("src/compaction_server/server");

/// Extract embedded server files to the config directory.
/// Returns the path to the extracted directory.
pub fn get_server_dir(config_dir: &std::path::Path) -> anyhow::Result<PathBuf> {
    let dest = config_dir.join("compaction_server");
    if !dest.exists() {
        std::fs::create_dir_all(&dest)
            .with_context(|| format!("Failed to create {}", dest.display()))?;
        SERVER_FILES
            .unpack(&dest)
            .with_context(|| format!("Failed to unpack server files to {}", dest.display()))?;
    }
    Ok(dest)
}

/// Get the path to the Python entrypoint.
/// Uses config.server_path if set, otherwise uses the embedded default.
pub fn get_server_entrypoint(
    config: &crate::config::CompactionConfig,
    config_dir: &std::path::Path,
) -> anyhow::Result<PathBuf> {
    if let Some(ref path) = config.server_path {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
        tracing::warn!("Configured server_path '{}' does not exist, using embedded default", p.display());
    }
    let p = get_server_dir(config_dir)?.join("main.py");
    if p.exists() {
        Ok(p)
    } else {
        Err(anyhow::anyhow!("Embedded server not found at {}", p.display()))
    }
}

/// Get the Python binary path.
/// Uses config.venv_path if set, otherwise uses system `python3`.
pub fn get_python_bin(config: &crate::config::CompactionConfig) -> PathBuf {
    if let Some(ref venv_path) = config.venv_path {
        PathBuf::from(venv_path).join("bin").join("python")
    } else {
        PathBuf::from("python3")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_python_bin_default() {
        let config = crate::config::CompactionConfig::default();
        assert_eq!(get_python_bin(&config), PathBuf::from("python3"));
    }

    #[test]
    fn test_get_python_bin_venv() {
        let config = crate::config::CompactionConfig {
            venv_path: Some("/tmp/venv".to_string()),
            ..Default::default()
        };
        assert_eq!(
            get_python_bin(&config),
            PathBuf::from("/tmp/venv/bin/python")
        );
    }

    #[test]
    fn test_get_server_entrypoint_prefers_config() {
        // When server_path exists, it should be preferred
        let config = crate::config::CompactionConfig {
            server_path: Some("/tmp/custom_server.py".to_string()),
            ..Default::default()
        };
        // Create a temp file to simulate existing path
        std::fs::write("/tmp/custom_server.py", "# test").unwrap();
        let result = get_server_entrypoint(&config, &std::path::PathBuf::from("/tmp"));
        std::fs::remove_file("/tmp/custom_server.py").ok();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from("/tmp/custom_server.py"));
    }

    #[test]
    fn test_get_server_entrypoint_falls_back_to_embedded() {
        let config = crate::config::CompactionConfig {
            server_path: Some("/nonexistent/path.py".to_string()),
            ..Default::default()
        };
        // Config path doesn't exist, should fall back to embedded
        // This will extract files to /tmp and check main.py exists
        let result = get_server_entrypoint(&config, &std::path::PathBuf::from("/tmp"));
        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("main.py"));
    }
}
```

Create `compaction_server/server/main.py`:

```python
"""LLMLingua-2 compaction server for Tama proxy."""

import os
import sys
import time
from fastapi import FastAPI
from pydantic import BaseModel, Field
from typing import Optional, List, Dict, Any, Literal, Union
from llmlingua import PromptCompressor
import warnings

warnings.filterwarnings("ignore", category=FutureWarning)

app = FastAPI(title="Tama Compaction Server")

# Model configuration
MODEL_NAME = os.environ.get(
    "COMPACTION_MODEL",
    "microsoft/llmlingua-2-xlm-roberta-large-meetingbank"
)
DEVICE = os.environ.get("COMPACTION_DEVICE", "cpu")

# Global compressor — loaded once at startup
_compressor: Optional[PromptCompressor] = None
_model_load_time: Optional[float] = None


def get_compressor() -> PromptCompressor:
    """Lazy-load the PromptCompressor on first call."""
    global _compressor, _model_load_time
    if _compressor is None:
        start = time.time()
        _compressor = PromptCompressor(
            model_name=MODEL_NAME,
            use_llmlingua2=True,
            device_map=DEVICE,
        )
        _model_load_time = time.time() - start
    return _compressor


class TextCompressRequest(BaseModel):
    """Request for raw text compression."""
    mode: Literal["text"] = "text"
    text: str
    rate: float = Field(default=0.3, ge=0.01, le=1.0)
    force_tokens: List[str] = Field(default_factory=lambda: ["\n"])
    chunk_end_tokens: List[str] = Field(default_factory=lambda: [".", "\n"])


class MessagesCompressRequest(BaseModel):
    """Request for OpenAI messages compression."""
    mode: Literal["messages"] = "messages"
    messages: List[Dict[str, Any]]
    rates: Dict[str, float] = Field(
        default_factory=lambda: {
            "system": 0.8,
            "user": 0.3,
            "assistant": 0.3,
            "default": 0.3,
        }
    )
    force_tokens: List[str] = Field(default_factory=lambda: ["\n"])
    chunk_end_tokens: List[str] = Field(default_factory=lambda: [".", "\n"])


class CompressResponse(BaseModel):
    """Response from compression."""
    compressed_text: Optional[str] = None
    compressed_messages: Optional[List[Dict[str, Any]]] = None
    original_tokens: int = 0
    compressed_tokens: int = 0
    compression_ratio: float = 1.0
    latency_ms: int = 0
    status: Literal["compressed", "skipped"] = "compressed"
    warmup: bool = False


@app.get("/health")
async def health_check():
    """Health check endpoint."""
    return {"status": "OK"}


@app.post("/compress")
async def compress(request: Union[TextCompressRequest, MessagesCompressRequest]):
    """Compress text or messages using LLMLingua-2."""
    start = time.time()
    compressor = get_compressor()
    warmup = _model_load_time is not None and (time.time() - _model_load_time) < 1.0

    if request.mode == "text":
        return _compress_text(
            compressor, request.text, request.rate,
            request.force_tokens, request.chunk_end_tokens, start, warmup
        )
    else:
        return _compress_messages(
            compressor, request.messages, request.rates,
            request.force_tokens, request.chunk_end_tokens, start, warmup
        )


def _compress_text(
    compressor, text: str, rate: float,
    force_tokens: List[str], chunk_end_tokens: List[str],
    start: float, warmup: bool
) -> CompressResponse:
    """Compress raw text."""
    try:
        result = compressor.compress_prompt_llmlingua2(
            text,
            rate=rate,
            force_tokens=force_tokens,
            chunk_end_tokens=chunk_end_tokens,
            return_word_label=False,
            drop_consecutive=True,
        )
        latency_ms = int((time.time() - start) * 1000)
        original = result.get("origin_tokens", 0)
        compressed = result.get("compressed_tokens", 0)
        ratio = original / compressed if compressed > 0 else 1.0
        return CompressResponse(
            compressed_text=result.get("compressed_prompt", text),
            original_tokens=original,
            compressed_tokens=compressed,
            compression_ratio=round(ratio, 2),
            latency_ms=latency_ms,
            status="compressed",
            warmup=warmup,
        )
    except Exception as e:
        latency_ms = int((time.time() - start) * 1000)
        return CompressResponse(
            compressed_text=text,
            original_tokens=0,
            compressed_tokens=0,
            compression_ratio=1.0,
            latency_ms=latency_ms,
            status="skipped",
        )


def _compress_messages(
    compressor, messages: List[Dict[str, Any]], rates: Dict[str, float],
    force_tokens: List[str], chunk_end_tokens: List[str],
    start: float, warmup: bool
) -> CompressResponse:
    """Compress OpenAI-style messages with per-role rates."""
    default_rate = rates.get("default", 0.3)
    compressed_messages = []
    total_original = 0
    total_compressed = 0

    for msg in messages:
        role = msg.get("role", "user")
        content = msg.get("content", "")
        rate = rates.get(role, default_rate)

        try:
            result = compressor.compress_prompt_llmlingua2(
                str(content),
                rate=rate,
                force_tokens=force_tokens,
                chunk_end_tokens=chunk_end_tokens,
                return_word_label=False,
                drop_consecutive=True,
            )
            compressed_messages.append({
                "role": role,
                "content": result.get("compressed_prompt", content),
            })
            total_original += result.get("origin_tokens", 0)
            total_compressed += result.get("compressed_tokens", 0)
        except Exception:
            compressed_messages.append({"role": role, "content": content})

    latency_ms = int((time.time() - start) * 1000)
    ratio = total_original / total_compressed if total_compressed > 0 else 1.0
    return CompressResponse(
        compressed_messages=compressed_messages,
        original_tokens=total_original,
        compressed_tokens=total_compressed,
        compression_ratio=round(ratio, 2),
        latency_ms=latency_ms,
        status="compressed",
        warmup=warmup,
    )


if __name__ == "__main__":
    import uvicorn
    port = int(os.environ.get("COMPACTION_PORT", "18962"))
    uvicorn.run(app, host="127.0.0.1", port=port)
```

Create `compaction_server/server/requirements.txt`:

```
# Requires Python >= 3.8
fastapi>=0.104.0
uvicorn>=0.24.0
llmlingua>=0.3.0
torch>=2.0.0
transformers>=4.36.0
```

Add `include_dir` to `tama-core/Cargo.toml` dependencies if not already present (it's already in workspace deps).

**Steps:**
- [ ] Create `crates/tama-core/src/compaction_server/mod.rs` with `get_server_dir`, `get_server_entrypoint`, `get_python_bin` functions
- [ ] Create `crates/tama-core/src/compaction_server/server/main.py` with FastAPI app
- [ ] Create `crates/tama-core/src/compaction_server/server/requirements.txt`
- [ ] Add `mod compaction_server;` to `crates/tama-core/src/lib.rs`
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix include_dir! path issues.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add embedded LLMLingua-2 compaction server"

**Acceptance criteria:**
- [ ] `compaction_server` module compiles without errors
- [ ] `include_dir!` macro correctly embeds the server files
- [ ] `get_server_dir` extracts files to config directory
- [ ] `get_python_bin` returns correct path for both venv and system python
- [ ] Python server has `/health` and `/compress` endpoints
- [ ] `/compress` handles both text and messages modes

---

### Task 3: Add Compaction Server State to ProxyState

**Context:**
The proxy needs to track the compaction server's lifecycle state (Idle → Starting → Ready/Failed) and provide methods to spawn, check health, and shutdown the server. This follows the same pattern as `ModelState` for TTS backends.

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`

**What to implement:**

Add to `proxy/types.rs`:

```rust
/// State for the compaction server subprocess.
#[derive(Debug, Clone)]
pub enum CompactionServerState {
    /// Server is not running.
    Idle,
    /// Server is starting up (process spawned, awaiting health check).
    Starting {
        pid: u32,
        port: u16,
        start_time: Instant,
    },
    /// Server is ready and accepting requests.
    Ready {
        pid: u32,
        port: u16,
    },
    /// Server failed to start.
    Failed {
        error: String,
    },
}

impl CompactionServerState {
    /// Check if the server is ready to accept requests.
    pub fn is_ready(&self) -> bool {
        matches!(self, CompactionServerState::Ready { .. })
    }

    /// Get the server port (if known).
    pub fn port(&self) -> Option<u16> {
        match self {
            CompactionServerState::Starting { port, .. } => Some(*port),
            CompactionServerState::Ready { port, .. } => Some(*port),
            _ => None,
        }
    }

    /// Get the server PID (if known).
    pub fn pid(&self) -> Option<u32> {
        match self {
            CompactionServerState::Starting { pid, .. } => Some(*pid),
            CompactionServerState::Ready { pid, .. } => Some(*pid),
            _ => None,
        }
    }
}

impl Default for CompactionServerState {
    fn default() -> Self {
        Self::Idle
    }
}
```

Add to `ProxyState` struct in `types.rs`:

```rust
/// Compaction server state — tracked separately from model backends.
pub compaction_server: Arc<tokio::sync::RwLock<CompactionServerState>>,
```

Update `ProxyState::new()` in `state.rs` to initialize the field:

```rust
compaction_server: Arc::new(tokio::sync::RwLock::new(CompactionServerState::Idle)),
```

Update `ProxyState::shutdown()` in `state.rs` to kill the compaction server:

```rust
// Kill compaction server subprocess
let compaction = self.compaction_server.read().await;
if let Some(pid) = compaction.pid() {
    drop(compaction);
    if let Err(e) = super::process::kill_process_group(pid).await {
        tracing::warn!("Failed to SIGTERM compaction server (pid {}): {}", pid, e);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    if super::process::is_process_group_alive(pid) {
        if let Err(e) = super::process::force_kill_process_group(pid).await {
            tracing::warn!("Failed to SIGKILL compaction server (pid {}): {}", pid, e);
        }
    }
    tracing::info!("Compaction server (pid: {}) stopped", pid);
}
// Reset to Idle
*self.compaction_server.write().await = CompactionServerState::Idle;
```

Add tests:
- `test_compaction_server_state_defaults` — Idle by default
- `test_compaction_server_state_is_ready` — Ready returns true, others false
- `test_compaction_server_state_port_pid` — Port and PID accessors

**Steps:**
- [ ] Write failing test `test_compaction_server_state_defaults` in `crates/tama-core/src/proxy/types.rs`
- [ ] Run `cargo test --package tama-core -- proxy::types::tests::test_compaction_server_state_defaults`
  - Did it fail with compile error? If it passed, investigate.
- [ ] Implement `CompactionServerState` enum in `crates/tama-core/src/proxy/types.rs`
- [ ] Add `compaction_server` field to `ProxyState` struct
- [ ] Initialize in `ProxyState::new()` in `state.rs`
- [ ] Add shutdown cleanup in `ProxyState::shutdown()`
- [ ] Write `test_compaction_server_state_is_ready` and `test_compaction_server_state_port_pid` tests
- [ ] Run `cargo test --package tama-core -- proxy::types::tests`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add CompactionServerState to ProxyState"

**Acceptance criteria:**
- [ ] `CompactionServerState` enum has 4 variants: Idle, Starting, Ready, Failed
- [ ] `ProxyState` has `compaction_server` field initialized to `Idle`
- [ ] `shutdown()` kills the compaction server process group
- [ ] All 3 tests pass
- [ ] `cargo build --package tama-core` succeeds

---

### Task 4: Add Compaction Server Lifecycle Methods

**Context:**
The proxy needs methods to spawn the compaction server, poll its health, and ensure it's running before forwarding requests. This follows the `load_tts_backend` pattern from `proxy/lifecycle/mod.rs`.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs`

**What to implement:**

Add the following methods to `impl ProxyState` in `lifecycle/mod.rs`:

```rust
/// Ensure the compaction server is running.
///
/// If already Ready, returns immediately.
/// If Idle, spawns the Python subprocess and polls health.
/// If Starting, waits for health check to complete.
/// If Failed, returns an error.
///
/// Returns the server URL (e.g., "http://127.0.0.1:18962").
pub async fn ensure_compaction_server(&self) -> anyhow::Result<String> {
    let config = self.config.read().await;
    let compaction = config.compaction.clone();
    drop(config);

    if !compaction.enabled {
        return Err(anyhow::anyhow!("Compaction is not enabled in config"));
    }

    // Fast path: already ready
    {
        let state = self.compaction_server.read().await;
        if let CompactionServerState::Ready { port, .. } = *state {
            return Ok(format!("http://127.0.0.1:{}", port));
        }
    }

    // Check for Failed state
    {
        let state = self.compaction_server.read().await;
        if let CompactionServerState::Failed { ref error } = *state {
            return Err(anyhow::anyhow!("Compaction server previously failed: {}", error));
        }
    }

    // If Starting, wait for it
    {
        let state = self.compaction_server.read().await;
        if matches!(*state, CompactionServerState::Starting { .. }) {
            drop(state);
            return self.wait_for_compaction_ready().await;
        }
    }

    // Idle — spawn the server
    self.spawn_compaction_server(&compaction).await
}

async fn spawn_compaction_server(&self, compaction: &crate::config::CompactionConfig) -> anyhow::Result<String> {
    use crate::config::Config;

    let base_dir = Config::base_dir().with_context(|| "Failed to get config directory")?;

    // Resolve server entrypoint and python binary
    let server_path = crate::compaction_server::get_server_entrypoint(compaction, &base_dir)
        .with_context(|| "Failed to resolve compaction server entrypoint")?;
    let python_bin = crate::compaction_server::get_python_bin(compaction);

    // Determine port
    let port = if let Some(p) = compaction.port {
        p
    } else {
        // Auto-assign via TcpListener
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .with_context(|| "Failed to bind TcpListener for port assignment")?;
        listener.local_addr()?.port()
    };

    let backend_url = format!("http://127.0.0.1:{}", port);
    let health_url = format!("{}/health", backend_url);

    // Transition to Starting state (write lock)
    {
        let mut state = self.compaction_server.write().await;
        // Double-check: another request may have started it
        if state.is_ready() {
            return Ok(format!("http://127.0.0.1:{}", state.port().unwrap()));
        }
        *state = CompactionServerState::Starting {
            pid: 0, // Updated after spawn
            port,
            start_time: Instant::now(),
        };
    }

    tracing::info!("Starting compaction server on port {}", port);

    // Derive uvicorn module name from the server filename (e.g., "main.py" → "main:app")
    let module_name = server_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("main")
        .to_string();
    let uvicorn_target = format!("{}:app", module_name);

    // Spawn the Python process
    let server_dir = server_path.parent().ok_or_else(|| anyhow::anyhow!("Server path has no parent"))?;
    let mut child = tokio::process::Command::new(&python_bin);
    super::process::configure_process_group(&mut child);
    child
        .arg("-m")
        .arg("uvicorn")
        .arg(&uvicorn_target)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .env("COMPACTION_PORT", port.to_string())
        .env("COMPACTION_DEVICE", &compaction.device)
        .current_dir(server_dir);

    let mut child = child.spawn().with_context(|| {
        format!("Failed to spawn compaction server: {}", python_bin.display())
    })?;

    let pid = child.id().ok_or_else(|| anyhow::anyhow!("Failed to get PID for compaction server"))?;

    // Update PID in Starting state
    {
        let mut state = self.compaction_server.write().await;
        if let CompactionServerState::Starting { pid: ref mut p, .. } = *state {
            *p = pid;
        }
    }

    tracing::info!("Compaction server started (pid: {})", pid);

    // Spawn reaper task
    let reaper_pid = pid;
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => {
                tracing::debug!("Compaction server process {} exited with {}", reaper_pid, status);
            }
            Err(e) => {
                tracing::warn!("Failed to wait on compaction server process {}: {}", reaper_pid, e);
            }
        }
    });

    // Health check: poll every 500ms, require 2 consecutive successes
    let timeout = std::time::Duration::from_secs(
        self.config.read().await.proxy.startup_timeout_secs
    );
    let start = Instant::now();
    let mut consecutive_successes: u32 = 0;
    let mut health_ok = false;

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if start.elapsed() >= timeout {
            tracing::warn!(
                "Compaction server startup timeout after {}s, killing process",
                timeout.as_secs()
            );
            let _ = super::process::kill_process_group(pid).await;
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if super::process::is_process_group_alive(pid) {
                let _ = super::process::force_kill_process_group(pid).await;
            }
            *self.compaction_server.write().await = CompactionServerState::Failed {
                error: format!("Startup timeout after {}s", timeout.as_secs()),
            };
            return Err(anyhow::anyhow!(
                "Compaction server failed to start (timeout after {}s)",
                timeout.as_secs()
            ));
        }

        match super::process::check_health(&health_url, Some(30)).await {
            Ok(response) if response.status().is_success() => {
                consecutive_successes += 1;
                if consecutive_successes >= 2 {
                    health_ok = true;
                    break;
                }
            }
            _ => {
                consecutive_successes = 0;
            }
        }
    }

    if !health_ok {
        *self.compaction_server.write().await = CompactionServerState::Failed {
            error: "Health check failed".to_string(),
        };
        return Err(anyhow::anyhow!("Compaction server health check failed"));
    }

    // Transition to Ready
    *self.compaction_server.write().await = CompactionServerState::Ready { pid, port };

    tracing::info!("Compaction server ready on {}", backend_url);
    Ok(backend_url)
}

async fn wait_for_compaction_ready(&self) -> anyhow::Result<String> {
    // Wait up to startup_timeout_secs for the server to become Ready
    let timeout_secs = self.config.read().await.proxy.startup_timeout_secs;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        let state = self.compaction_server.read().await;
        match *state {
            CompactionServerState::Ready { port, .. } => {
                return Ok(format!("http://127.0.0.1:{}", port));
            }
            CompactionServerState::Failed { ref error } => {
                return Err(anyhow::anyhow!("Compaction server failed: {}", error));
            }
            _ => {} // Still starting
        }
        drop(state);

        if std::time::Instant::now() >= deadline {
            return Err(anyhow::anyhow!("Timed out waiting for compaction server"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}
```

**Steps:**
- [ ] Add `ensure_compaction_server`, `spawn_compaction_server`, `wait_for_compaction_ready` methods to `proxy/lifecycle/mod.rs`
- [ ] Import `CompactionServerState` at the top of the file
- [ ] Write test `test_ensure_compaction_disabled_returns_error` in `proxy/lifecycle/tests.rs` (or add to existing test module)
- [ ] Write test `test_ensure_compaction_failed_state_returns_error`
- [ ] Run `cargo test --package tama-core -- proxy::lifecycle::tests::test_ensure_compaction`
  - Did tests fail (methods don't exist yet)? If not, investigate.
- [ ] Implement the lifecycle methods
- [ ] Run `cargo test --package tama-core -- proxy::lifecycle::tests::test_ensure_compaction`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? Fix any compilation errors.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add compaction server lifecycle methods"

**Acceptance criteria:**
- [ ] `ensure_compaction_server` returns error when compaction is disabled
- [ ] `ensure_compaction_server` returns error when in Failed state
- [ ] `ensure_compaction_server` returns server URL when ready
- [ ] `spawn_compaction_server` spawns Python subprocess with correct args
- [ ] uvicorn module name is derived from server filename (e.g., "main:app")
- [ ] Health check requires 2 consecutive successes
- [ ] Failed state is set on timeout or health check failure
- [ ] Reaper task is spawned for the child process
- [ ] `wait_for_compaction_ready` polls until Ready, Failed, or timeout
- [ ] `cargo build --package tama-core` succeeds

---

### Task 5: Add /v1/compaction Handler, Router, and Tests

**Context:**
The final piece is the axum handler for `POST /v1/compaction`. It parses the request body, forwards to the compaction server, and returns the response. On failure (server unavailable, timeout), it returns the original text/messages with `status: "skipped"`.

**Files:**
- Create: `crates/tama-core/src/proxy/handlers/compaction.rs`
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`
- Modify: `crates/tama-core/src/proxy/server/router.rs`

**What to implement:**

Create `proxy/handlers/compaction.rs`:

```rust
//! Compaction endpoint handler.
//!
//! Handles POST /v1/compaction — compresses prompts using LLMLingua-2.

use crate::config::MAX_REQUEST_BODY_SIZE;
use crate::proxy::ProxyState;
use anyhow::Context;
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Request for raw text compression.
#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum CompactionRequest {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default = "default_rate")]
        rate: f64,
        #[serde(default = "default_force_tokens")]
        force_tokens: Vec<String>,
        #[serde(default = "default_chunk_end_tokens")]
        chunk_end_tokens: Vec<String>,
    },
    #[serde(rename = "messages")]
    Messages {
        messages: Vec<serde_json::Value>,
        #[serde(default = "default_rates")]
        rates: HashMap<String, f64>,
        #[serde(default = "default_force_tokens")]
        force_tokens: Vec<String>,
        #[serde(default = "default_chunk_end_tokens")]
        chunk_end_tokens: Vec<String>,
    },
}

fn default_rate() -> f64 {
    0.3
}

fn default_force_tokens() -> Vec<String> {
    vec!["\n".to_string()]
}

fn default_chunk_end_tokens() -> Vec<String> {
    vec![".".to_string(), "\n".to_string()]
}

fn default_rates() -> HashMap<String, f64> {
    let mut map = HashMap::new();
    map.insert("system".to_string(), 0.8);
    map.insert("user".to_string(), 0.3);
    map.insert("assistant".to_string(), 0.3);
    map.insert("default".to_string(), 0.3);
    map
}

/// Response from the compaction endpoint.
#[derive(Debug, Serialize)]
pub struct CompactionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_messages: Option<Vec<serde_json::Value>>,
    pub original_tokens: u64,
    pub compressed_tokens: u64,
    pub compression_ratio: f64,
    pub latency_ms: u64,
    pub status: String,
}

impl CompactionResponse {
    /// Create a fallback response when compaction is unavailable.
    fn skipped(text: Option<String>, messages: Option<Vec<serde_json::Value>>) -> Self {
        Self {
            compressed_text: text,
            compressed_messages: messages,
            original_tokens: 0,
            compressed_tokens: 0,
            compression_ratio: 1.0,
            latency_ms: 0,
            status: "skipped".to_string(),
        }
    }
}

#[axum::debug_handler]
pub async fn handle_compaction(
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    // Read and parse request body
    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => return super::json_error_response(),
    };

    let request: CompactionRequest = match serde_json::from_slice(&body_bytes) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Failed to parse compaction request: {}", e);
            return super::json_error_response();
        }
    };

    // Check if compaction is enabled
    let config = state.config.read().await;
    if !config.compaction.enabled {
        drop(config);
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({
                "error": {
                    "message": "Compaction is not enabled. Add [compaction] section to config.toml.",
                    "type": "NotImplementedError"
                }
            })),
        )
            .into_response();
    }
    let timeout_ms = config.compaction.timeout_ms;
    drop(config);

    // Ensure server is running
    let server_url = match state.ensure_compaction_server().await {
        Ok(url) => url,
        Err(e) => {
            tracing::warn!("Compaction server unavailable: {}", e);
            // Fallback: return original text
            let response = match &request {
                CompactionRequest::Text { text, .. } => {
                    CompactionResponse::skipped(Some(text.clone()), None)
                }
                CompactionRequest::Messages { messages, .. } => {
                    CompactionResponse::skipped(None, Some(messages.clone()))
                }
            };
            return (StatusCode::SERVICE_UNAVAILABLE, Json(response)).into_response();
        }
    };

    // Build the forward request body
    let (forward_body, original_text, original_messages) = match &request {
        CompactionRequest::Text {
            text,
            rate,
            force_tokens,
            chunk_end_tokens,
        } => {
            let body = serde_json::json!({
                "mode": "text",
                "text": text,
                "rate": rate,
                "force_tokens": force_tokens,
                "chunk_end_tokens": chunk_end_tokens,
            });
            (body, Some(text.clone()), None)
        }
        CompactionRequest::Messages {
            messages,
            rates,
            force_tokens,
            chunk_end_tokens,
        } => {
            let body = serde_json::json!({
                "mode": "messages",
                "messages": messages,
                "rates": rates,
                "force_tokens": force_tokens,
                "chunk_end_tokens": chunk_end_tokens,
            });
            (body, None, Some(messages.clone()))
        }
    };

    // Forward to compaction server with timeout
    let url = format!("{}/compress", server_url);
    let timeout = Duration::from_millis(timeout_ms);

    let result = tokio::time::timeout(
        timeout,
        state
            .client
            .post(&url)
            .json(&forward_body)
            .send()
            .and_then(|resp| resp.json::<serde_json::Value>().await),
    )
    .await;

    match result {
        Ok(Ok(response)) => {
            // Parse and return the server response
            let compressed_text = response.get("compressed_text").and_then(|v| v.as_str()).map(String::from);
            let compressed_messages = response.get("compressed_messages").and_then(|v| v.as_array()).map(|arr| arr.to_vec());
            let original_tokens = response.get("original_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let compressed_tokens = response.get("compressed_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let compression_ratio = response.get("compression_ratio").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let latency_ms = response.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(0);
            let status = response.get("status").and_then(|v| v.as_str()).unwrap_or("compressed").to_string();

            (
                StatusCode::OK,
                Json(CompactionResponse {
                    compressed_text,
                    compressed_messages,
                    original_tokens,
                    compressed_tokens,
                    compression_ratio,
                    latency_ms,
                    status,
                }),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!("Compaction server returned error: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(CompactionResponse::skipped(original_text, original_messages)),
            )
                .into_response()
        }
        Err(_) => {
            tracing::warn!("Compaction request timed out after {}ms", timeout_ms);
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(CompactionResponse::skipped(original_text, original_messages)),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rate() {
        assert_eq!(default_rate(), 0.3);
    }

    #[test]
    fn test_default_force_tokens() {
        let tokens = default_force_tokens();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], "\n");
    }

    #[test]
    fn test_default_chunk_end_tokens() {
        let tokens = default_chunk_end_tokens();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], ".");
        assert_eq!(tokens[1], "\n");
    }

    #[test]
    fn test_default_rates() {
        let rates = default_rates();
        assert_eq!(rates.get("system"), Some(&0.8));
        assert_eq!(rates.get("user"), Some(&0.3));
        assert_eq!(rates.get("assistant"), Some(&0.3));
        assert_eq!(rates.get("default"), Some(&0.3));
    }

    #[test]
    fn test_compaction_response_skipped() {
        let resp = CompactionResponse::skipped(Some("original text".to_string()), None);
        assert_eq!(resp.status, "skipped");
        assert_eq!(resp.compression_ratio, 1.0);
        assert_eq!(resp.compressed_text, Some("original text".to_string()));
        assert!(resp.compressed_messages.is_none());
    }

    #[test]
    fn test_compaction_request_text_mode_deserialize() {
        let json = r#"{"mode": "text", "text": "hello world", "rate": 0.5}"#;
        let req: CompactionRequest = serde_json::from_str(json).unwrap();
        match req {
            CompactionRequest::Text { text, rate, .. } => {
                assert_eq!(text, "hello world");
                assert_eq!(rate, 0.5);
            }
            _ => panic!("Expected Text mode"),
        }
    }

    #[test]
    fn test_compaction_request_messages_mode_deserialize() {
        let json = r#"{"mode": "messages", "messages": [{"role": "user", "content": "hello"}]}"#;
        let req: CompactionRequest = serde_json::from_str(json).unwrap();
        match req {
            CompactionRequest::Messages { messages, .. } => {
                assert_eq!(messages.len(), 1);
            }
            _ => panic!("Expected Messages mode"),
        }
    }

    /// Integration test: 501 when compaction is disabled.
    #[tokio::test]
    async fn test_compaction_disabled_returns_501() {
        let config = crate::config::Config::default();
        let state = Arc::new(ProxyState::new(config, None));
        let app = crate::proxy::server::router::build_router(state.clone()).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{}/v1/compaction", addr))
            .json(&serde_json::json!({
                "mode": "text",
                "text": "test"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    /// Integration test: compaction request body size limit.
    /// `to_bytes(body, MAX_REQUEST_BODY_SIZE)` errors on oversized bodies.
    #[tokio::test]
    async fn test_compaction_body_size_limit() {
        let config = crate::config::Config::default();
        let state = Arc::new(ProxyState::new(config, None));
        let app = crate::proxy::server::router::build_router(state.clone()).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        // Send a body larger than MAX_REQUEST_BODY_SIZE (16MB)
        let large_text = "x".repeat(17 * 1024 * 1024);
        let resp = client
            .post(format!("http://{}/v1/compaction", addr))
            .body(large_text)
            .send()
            .await
            .unwrap();
        // to_bytes errors on oversized bodies → handler returns 400
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "Oversized body should return 400 Bad Request"
        );
    }
}
```

Modify `proxy/handlers/mod.rs` — add:
```rust
pub mod compaction;
```

Modify `proxy/server/router.rs` — add the route and import:

In `build_router`:
```rust
// Compaction endpoint
.route("/v1/compaction", post(handle_compaction))
```

Add import:
```rust
use crate::proxy::handlers::compaction::handle_compaction;
```

In `build_unified_router` (web-ui feature), add the same route to `proxy_routes`.

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/handlers/compaction.rs` with handler and tests
- [ ] Add `pub mod compaction;` to `crates/tama-core/src/proxy/handlers/mod.rs`
- [ ] Add `/v1/compaction` route to `build_router` in `router.rs`
- [ ] Add `/v1/compaction` route to `build_unified_router` in `router.rs` (web-ui feature)
- [ ] Add `use crate::proxy::handlers::compaction::handle_compaction;` import in `router.rs`
- [ ] Run `cargo test --package tama-core -- proxy::handlers::compaction::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo test --package tama-core -- proxy::handlers::compaction::tests::test_compaction_disabled_returns_501`
  - Did it return 501? If not, fix route ordering.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add /v1/compaction endpoint handler"

**Acceptance criteria:**
- [ ] `POST /v1/compaction` returns 501 when compaction is disabled
- [ ] `POST /v1/compaction` returns 400 for oversized body (>16MB)
- [ ] `POST /v1/compaction` returns 400 for malformed JSON
- [ ] Text mode request deserializes correctly with default values
- [ ] Messages mode request deserializes correctly with default rates
- [ ] `CompactionResponse::skipped` returns original text with status "skipped"
- [ ] Handler forwards to compaction server with timeout
- [ ] Handler returns fallback on server error or timeout
- [ ] All tests pass
- [ ] `cargo clippy --package tama-core -- -D warnings` passes
- [ ] `cargo build --workspace` succeeds

---

## Verification

After all tasks are complete:

1. `cargo build --workspace` — clean build
2. `cargo test --workspace` — all tests pass
3. `cargo clippy --workspace -- -D warnings` — no warnings
4. `cargo fmt --all -- --check` — formatted

Manual testing (requires Python + pip):
1. Set up a Python venv: `python3 -m venv /tmp/compaction-venv`
2. Install deps: `/tmp/compaction-venv/bin/pip install -r crates/tama-core/src/compaction_server/server/requirements.txt`
3. Add to config: `~/.config/tama/config.toml`:
   ```toml
   [compaction]
   enabled = true
   venv_path = "/tmp/compaction-venv"
   ```
4. Start Tama proxy
5. Test: `curl -X POST http://localhost:11434/v1/compaction -H 'Content-Type: application/json' -d '{"mode":"text","text":"This is a long test document that should be compressed by the LLMLingua-2 model. It contains multiple sentences that can be reduced while preserving the key information.","rate":0.3}'`
6. Verify response has `compressed_text`, `original_tokens`, `compressed_tokens`, `compression_ratio`, `status: "compressed"`
