# Tama

A local AI server written in Rust that provides an OpenAI-compatible API on a single port. It manages backend lifecycles — starting models on demand, routing requests, and unloading idle models — with a SvelteKit web control plane.

## Language

**Backend**:
An inference engine binary (e.g. `llama.cpp`, `ik_llama`) that serves a specific model on a local HTTP port. Managed by the proxy's lifecycle system.
_Avoid_: Server, engine, runner

**Model**:
A user-facing configuration entry in the DB — maps a name (repo_id) to a backend, quantization variant, and optional GPU assignment. A model is not the GGUF file itself; it is the config that tells the proxy which file to load into which backend.
_Avoid_: Model card, model config (use "model" alone)

**Quant** (short for "quantization variant"):
A specific quantization of a model (e.g. `Q4_K_M`, `Q8_0`). Each quant is a separate GGUF file under the model's directory and appears as a separate row in `model_configs`.
_Avoid_: Quantization, quant level

**Proxy**:
The core Tama component — an Axum HTTP server that listens on a single port, accepts OpenAI-compatible requests, routes them to the correct backend, and manages backend lifecycle (start, stop, health check, reload).
_Avoid_: Gateway, router, control plane (use "proxy")

**gpu_variant**:
The GPU compilation target a backend binary was built for: `cuda`, `rocm`, `vulkan`, `metal`, or `cpu`. A single backend name (e.g. `llama.cpp`) can have multiple gpu_variants installed. Determines which env var (`CUDA_VISIBLE_DEVICES`, `ROCR_VISIBLE_DEVICES`, etc.) the proxy sets at spawn time.
_Avoid_: GPU type, backend variant, accelerator

**Alias**:
A user-created name-to-model mapping stored in `model_aliases`. When a request arrives with an alias name, the proxy resolves it to the target model's name before routing. Replaced the old hardcoded wildcard model.
_Avoid_: Shortcut, nickname, virtual model

**Compaction**:
Prompt compression via Microsoft's LLMLingua-2 model. Reduces token count before prompts hit the main LLM. Runs as a Python FastAPI subprocess managed by the proxy's backend lifecycle.
_Avoid_: Prompt compression, summarization

**Backend lifecycle**:
The proxy's system for managing backend processes: spawn, health poll, idle timeout unload, dead PID detection, auto-restart (with max restarts limit), and graceful shutdown. Applies to LLM backends, Kokoro TTS, and compaction servers.
_Avoid_: Process management, backend supervisor

**ModelState**:
The state machine for a loaded model: `Starting` → `Ready` → `Unloading` → (back to idle) or `Failed`. The health monitor transitions states based on PID liveness and idle timeouts.
_Avoid_: Model status, model state machine

**Pull**:
The process of downloading a model from HuggingFace — includes API lookup, parallel chunked download, GGUF metadata parsing, and DB insertion. Tracked in the download queue with real-time SSE progress. Applies to GGUF files only.
_Avoid_: Download, fetch

**Repo pull**:
A whole-repository download of a safetensors (transformers) model, executed by shelling out to the `hf` CLI (`hf download <repo> --local-dir <models_dir>/<org>/<repo>`) as a tracked subprocess. No per-file selection, verification, or `model_files` rows (see ADR-0007). Wizard-scoped, polled for progress.
_Avoid_: hf pull, CLI download, whole-repo pull

**Transformers model**:
A model whose weights ship as safetensors (`hf_format = "transformers"`). Has no quants — the whole repo is the model (weight shards + `config.json` + tokenizers). Pulled via repo pull and served by the vLLM backend, which loads the repo directory as a positional path.
_Avoid_: safetensors model (ambiguous — could mean a single file), HF model, native model

**Spec decoding** (short for "speculative decoding"):
Technique to accelerate inference by having the main model predict multiple tokens at once using a draft model (MTP or ngram). Configured per-model via checkboxes and parameters in the model editor.
_Avoid_: Draft decoding, speculative sampling

**API Key**:
A named, scoped credential stored as a SHA-256 hash in the DB. Format: `tama_<32 chars base62>`. Scopes: `inference`, `management:read`, `management:write`. The plaintext key is returned once on creation and never retrievable.
_Avoid_: API token, secret key, bearer key

**AuthSubject**:
The authenticated identity attached to requests by the auth middleware: `User` (OAuth2 session, full access) or `Key` (API key, scoped access). Used by the scope middleware to enforce authorization.
_Avoid_: Auth context, principal, identity

**Suite** (benchmark suite):
An ordered group of benchmark runs (llama_bench, spec, MTP) auto-selected from a model's capabilities and executed sequentially as a single job. Sub-runs share a `suite_id` on their history rows.
_Avoid_: Benchmark batch, benchmark pack

**Scope middleware**:
The authorization layer that runs after authentication. Checks `AuthSubject` against route requirements — `User` bypasses, `Key` must have matching scopes.
_Avoid_: Permission middleware, ACL layer, role check
