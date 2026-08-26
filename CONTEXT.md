# Tama

Tama is a local AI server written in Rust that provides an OpenAI-compatible API on a single port. The proxy (tama) is the central control plane — routing, model resolution, and configuration. All self-hosted concerns (backend lifecycle, installs, pulls, benchmarks, host stats) live in **tamad** daemons on the inference hosts (ADR-0010). The web control plane is Leptos/WASM (compiled via Trunk and embedded in the binary).

## Language

**Backend**:
An inference engine binary (e.g. `llama.cpp`, `ik_llama`) that serves a specific model on a local HTTP port. Managed by the tamad's lifecycle system on the host where it runs.
_Avoid_: Server, engine, runner

**tamad**:
A daemon on an inference host that owns all self-hosted concerns: installing/upgrading backend binaries, the backend process lifecycle (spawn, health poll, idle unload, restart, shutdown), model pulls to local disk, benchmarks, and host/GPU introspection. The proxy never spawns processes or reads local hardware — it talks to tamads over the shared gRPC/HTTP protocol (ADR-0010).
_Avoid_: Daemon, sidecar, worker, backend host (use "tamad" for the daemon, "host" or "inference host" for the machine)

**Provider**:
A registered inference capability in the DB. *Local* providers are backends served by exactly one tamad (`Provider.tamad_id`); *remote* providers are direct HTTP endpoints (OpenAI-compatible, Anthropic) that the proxy forwards to with no tamad involved.
_Avoid_: Backend (that's the engine binary a local provider runs), model (that's the config layered on top)

**TamadConnection**:
The proxy's registration of a reachable tamad: a UUID id, name, URL (`grpc://` or `http://`), protocol, and optional auth token. Stored in `tamad_registry`.
_Avoid_: Tamad, daemon connection, remote host

**Model**:
A user-facing configuration entry in the DB — maps a name (repo_id) to a backend, quantization variant, and optional GPU assignment. A model is not the GGUF file itself; it is the config that tells the tamad which file to load into which backend.
_Avoid_: Model card, model config (use "model" alone)

**Quant** (short for "quantization variant"):
A specific quantization of a model (e.g. `Q4_K_M`, `Q8_0`). Each quant is a separate GGUF file under the model's directory and appears as a separate row in `model_configs`.
_Avoid_: Quantization, quant level

**Proxy**:
The core Tama component — an Axum HTTP server that listens on a single port, accepts OpenAI-compatible requests, and routes them to the correct provider (local via a tamad, or remote directly). It orchestrates but never executes: it spawns no backend processes and reads no local hardware — all of that is delegated to tamads (ADR-0010).
_Avoid_: Gateway, router, control plane (use "proxy")

**gpu_variant**:
The GPU compilation target a backend binary was built for: `cuda`, `rocm`, `vulkan`, `metal`, or `cpu`. A single backend name (e.g. `llama.cpp`) can have multiple gpu_variants installed. Determines which isolation env var (`CUDA_VISIBLE_DEVICES`, `ROCR_VISIBLE_DEVICES`, etc.) the *tamad* maps to the model's configured GPU device at spawn time (the proxy never samples hardware — ADR-0010).
_Avoid_: GPU type, backend variant, accelerator

**Alias**:
A user-created name-to-model mapping stored in `model_aliases`. When a request arrives with an alias name, the proxy resolves it to the target model's name before routing. Replaced the old hardcoded wildcard model.
_Avoid_: Shortcut, nickname, virtual model

**Compaction**:
Prompt compression via Microsoft's LLMLingua-2 model. Reduces token count before prompts hit the main LLM. Runs as a Python FastAPI subprocess owned by the host **tamad's** backend lifecycle (the proxy only requests the load/unload; the tamad spawns and manages the process).
_Avoid_: Prompt compression, summarization

**Backend lifecycle**:
The tamad's system for managing backend processes: spawn, health poll, idle timeout unload, dead PID detection, auto-restart (with max restarts limit), and graceful shutdown. Applies to LLM backends, Kokoro TTS, and compaction servers. The proxy requests lifecycle operations; the tamad executes them on its host. On tamad shutdown (SIGTERM/SIGINT) the daemon kills the process groups of *all* loaded backends, so no backend outlives the daemon that owns it.
_Avoid_: Process management, backend supervisor

**ModelState**:
The state machine for a loaded model: `Starting` → `Ready` → `Unloading` → (back to idle) or `Failed`. The health monitor transitions states based on PID liveness and idle timeouts.
_Avoid_: Model status, model state machine

**Pull**:
The process of downloading a model from HuggingFace — includes API lookup, parallel chunked download, GGUF metadata parsing, and DB insertion. The download executes on the tamad's host (it's the tamad's disk); the proxy tracks it in the download queue with real-time SSE progress. Applies to GGUF files only.
_Avoid_: Download, fetch

**Pull host**:
The tamad named by `proxy.pull_backend` (a registered tamad id, FK-enforced) that executes model pulls on its own disk. The proxy never downloads — it dispatches to the pull host and relays progress; with no pull host configured, pulls fail with the explicit "no pull host configured" error (ADR-0010).
_Avoid_: pull backend (that's the config field name, not the host), download host

**Repo pull**:
A whole-repository download of a safetensors (transformers) model, executed by the *tamad* shelling out to the `hf` CLI (`hf download <repo> --local-dir …`) as a tracked subprocess; the proxy relays progress/cancel (no per-file selection, verification, or `model_files` rows — see ADR-0007). Wizard-scoped, polled for progress.
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

**Reasoning** (model capability):
A model's ability to produce reasoning/thinking content. On client-facing model info, the effective `reasoning` flag = the backend-computed capability (llama.cpp `/props`: `supports_preserve_reasoning` / `reasoning_format`) OR the derived `supportsReasoningEffort`.
_Avoid_: thinking (generic), effort (that's supportsReasoningEffort)

**supportsReasoningEffort**:
Per-model capability exposed on client-facing model info — whether reasoning effort is adjustable per request. Derived, never stored: true iff `reasoningLevels` is non-empty (ADR-0008). The `ModelConfig::supports_reasoning_effort()` helper is the single derivation point.
_Avoid_: reasoning flag, thinking support, effort flag

**Reasoning levels**:
The set of effort levels a model accepts, stored per-model as `reasoningLevels` (JSON TEXT column on `model_configs`; editor: comma-separated text input). Stored in pi's 7-level vocabulary: `off, minimal, low, medium, high, xhigh, max`. The wire off-word to backends is `none` — `off`→`none` is translated at the pi plugin, the server forwarder, and the `reasoning_options` serializer (ADR-0009).
_Avoid_: thinking levels, effort presets, reasoning options (that's the opencode-canonical derived field)
