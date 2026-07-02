# Kokoro-FastAPI subprocess for TTS

## Context and Problem Statement

Tama needed text-to-speech synthesis to support OpenAI-compatible `/v1/audio/speech` and `/v1/audio/stream` endpoints. The initial approach used `kokoro-micro` (a Rust ONNX binding) for synthesis, but this required managing ONNX runtime dependencies, ROCm GPU initialization, and model loading in Rust — all of which proved fragile and hard to maintain.

## Decision Drivers

* Reliable synthesis — Kokoro-FastAPI is the reference implementation
* GPU acceleration — Kokoro-FastAPI supports CUDA and ROCm natively
* Maintenance — upstream Kokoro improvements are automatically available
* Simplicity — no ONNX runtime binding to maintain in Rust

## Considered Options

* Kokoro-FastAPI subprocess (Python + pip)
* kokoro-micro (Rust ONNX binding) — status quo at the time
* Piper TTS (C++ ONNX, fully Rust-bindable)

## Decision Outcome

Chosen option: "Kokoro-FastAPI subprocess", because it is the reference implementation with active maintenance, GPU support, and a well-defined HTTP API. Tama manages the Kokoro-FastAPI instance as a subprocess — installing it via pip into a virtual environment, starting the HTTP server, and proxying `/v1/audio/*` requests to it. The subprocess lifecycle is managed by the same backend lifecycle system used for LLM backends.

Piper TTS was evaluated and initially implemented but removed due to limited model quality compared to Kokoro and the complexity of maintaining two TTS engines.

### Consequences

* Good, because Kokoro-FastAPI is the reference — quality and features are always current
* Good, because GPU acceleration (CUDA/ROCm) works out of the box
* Good, because the HTTP API is stable and well-documented
* Bad, because requires Python and pip for installation
* Bad, because subprocess adds startup latency and memory overhead
* Bad, because OpenBLAS installation requires system package management (apt/dnf)

### Confirmation

The TTS backend is registered in the backend lifecycle with its own installer (`install_tts_kokoro`). The proxy routes `/v1/audio/*` to the Kokoro-FastAPI subprocess. The web UI has a TTS backends page for installation and management. Piper was removed in favor of Kokoro-only support.

## Pros and Cons of the Options

### Kokoro-FastAPI subprocess

Run Kokoro-FastAPI as a managed Python subprocess.

* Good, because reference implementation — best quality
* Good, because GPU support (CUDA + ROCm) is built-in
* Good, because HTTP API is clean and stable
* Good, because upstream updates are pip install away
* Bad, because requires Python, pip, and system dependencies (OpenBLAS)
* Bad, because subprocess adds latency and memory overhead

### kokoro-micro (Rust ONNX binding)

Call Kokoro directly from Rust via ONNX runtime.

* Good, because no subprocess — lower latency
* Good, because single binary — no Python dependency
* Bad, because ONNX runtime binding is fragile and hard to maintain
* Bad, because GPU support requires manual ROCm/CUDA initialization
* Bad, because upstream Kokoro changes may break the binding

### Piper TTS

C++ ONNX-based TTS with Rust bindings.

* Good, because fully bindable to Rust — no subprocess
* Good, because lightweight and fast
* Bad, because lower quality than Kokoro
* Bad, because limited voice/model selection
* Bad, because maintaining two TTS engines doubles complexity

## More Information

* PR #70: [add Kokoro TTS backend via subprocess](https://github.com/danielcherubini/tama/pull/70)
