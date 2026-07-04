# Python subprocess for auxiliary services

Auxiliary services (Kokoro-FastAPI for TTS, LLMLingua-2 for compaction) run as Python FastAPI subprocesses managed by the proxy's backend lifecycle system. The proxy spawns the server lazily on first request, polls `/health` for readiness, forwards HTTP requests, and reaps the process on shutdown. Server files are embedded via `include_dir!` so no external installation is needed beyond Python and pip.

This pattern was established with Kokoro-FastAPI (PR #70) as the replacement for `kokoro-micro` (a Rust ONNX binding that proved fragile and hard to maintain). LLMLingua-2 compaction (PR #116) followed the same pattern — routing through the existing backend lifecycle instead of custom subprocess management.

The subprocess is managed like any other backend: spawn with configurable timeout, health poll until ready, idle timeout for unload, and process group cleanup on shutdown. Config lives in `ProxyConfig` with fields for venv path, device, port, and timeout.

**Status:** accepted

**Considered Options:**

- **Python subprocess** (chosen) — reference implementations, active maintenance, GPU support out of the box. Managed by the same lifecycle as LLM backends.
- **Rust native binding** (kokoro-micro for TTS) — lower latency but fragile ONNX runtime binding, manual GPU init, upstream changes break the binding
- **External service** (user-managed Docker/container) — adds deployment complexity; Tama is designed as a single-binary local server
- **HTTP-only** (user runs their own server) — shifts operational burden to the user; defeats the purpose of automatic management

**Consequences:**

- Good, because reference implementations are always current — upstream improvements are `pip install` away
- Good, because GPU acceleration (CUDA/ROCm) works out of the box through Python's torch/onnxruntime
- Good, because the same lifecycle pattern works for TTS, compaction, and any future auxiliary service
- Bad, because requires Python and pip on the system
- Bad, because subprocess adds startup latency and memory overhead
- Bad, because system dependencies (OpenBLAS, torch) require package management
