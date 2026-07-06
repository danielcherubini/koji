# Weak Abstractions Audit — 2026-07-06

## Summary
23 findings across 6 categories. 5 high, 8 medium, 10 low.

## Context
- CONTEXT.md: loaded (tama project)
- ADRs reviewed: none found
- Plans reviewed: none found

---

## 🔴 High Severity

### 1. GPU Vendor and Model State as `String` (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/gpu/system.rs:131` (vendor), `tama-core/src/gpu/system.rs:238` (state)
- **Severity:** High
- **Confidence:** High
- **Problem:** `GpuDeviceStats.vendor` is `String` (only valid values: "nvidia", "amd") and `ModelStatus.state` is `String` (only valid values: "idle", "loading", "ready", "unloading", "failed"). Any caller can set arbitrary values, breaking pattern-matching exhaustiveness and enabling invalid states.
- **Proposal:** 
  - `enum GpuVendor { Nvidia, Amd }` with `Display` impl for serialization
  - `enum ModelState { Idle, Loading, Ready, Unloading, Failed { error: Option<String> } }` with `Display` impl
  - Replace all `String` fields and add conversion from string (for DB deserialization)

### 2. Restart Policy as `String` (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:698`
- **Severity:** High
- **Confidence:** High
- **Problem:** `Supervisor.restart_policy` is `String` with magic values "always" and "on-failure". No validation, no exhaustiveness, no semantics. The value is stored as raw text in DB and config.
- **Proposal:** `enum RestartPolicy { Always, OnFailure }` with `#[serde(rename_all = "kebab-case")]` for "on-failure". Add `Default` impl.

### 3. Compaction Device as `String` (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:653`
- **Severity:** High
- **Confidence:** High
- **Problem:** `CompactionConfig.device` is `String` with known values "cpu", "cuda", "cuda:0", "mps". No validation, no type safety.
- **Proposal:** `enum CompactionDevice { Cpu, Cuda(Option<u32>), Mps }` with serde support.

### 4. Log Level as `String` (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:375`
- **Severity:** High
- **Confidence:** High
- **Problem:** `General.log_level` is `String`. Should be a typed enum matching `tracing::Level` or `log::Level` with serde support.
- **Proposal:** `enum LogLevel { Debug, Info, Warn, Error }` with `From<tracing::Level>` and `Into<tracing::Level>` impls.

### 5. DB Query Functions Return Tuples (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/db/queries/app_config_queries.rs:211`, `tama-core/src/db/queries/app_config_queries.rs:103`
- **Severity:** High
- **Confidence:** High
- **Problem:** `get_supervisor()` returns `Option<(String, u32, u64, u64, u64, u32)>` — a 6-tuple with zero semantic meaning. `get_proxy()` returns a 12-tuple. Callers in `config/types.rs:195-216` destructure these into positional fields to build `Supervisor`/`ProxyConfig`. This is a classic "primitive obsession" anti-pattern: the DB layer exposes raw tuples instead of typed records.
- **Proposal:** 
  - Create `SupervisorConfigRecord { restart_policy: String, ... }` struct in `db/queries/types.rs`
  - Create `ProxyConfigRecord { host: String, port: u16, ... }` struct
  - Change `get_*` functions to return `Option<RecordStruct>` instead of tuples
  - This also eliminates the tuple destructuring in `config/types.rs:195-216`

---

## 🟡 Medium Severity

### 6. Duplicate Default Functions Across Crates (DRY Violation)
- **Lens:** DRY Violations
- **Files:** `tama-core/src/config/types.rs:724-799` vs `tama/src/types/config.rs:309-380`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** 20+ `default_*` functions are duplicated verbatim between the core config types and the WASM mirror types. They compute identical values (300, 120, 86400, etc.) in both files. Any change to a default must be applied in two places.
- **Proposal:** Extract all `default_*` functions into a shared module (e.g., `tama-core/src/config/defaults.rs`) and `use` them from both locations. The WASM mirror types should reference the core defaults rather than reimplementing them.

### 7. Magic Numbers in Default Functions (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:735,739,780,784,788`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Default functions return raw numeric literals: `300` (idle timeout), `120` (startup timeout), `3000` (health check timeout), `5000` (health check interval), `30000` (health check timeout ms). The values have no self-documenting meaning — you must read the function name to understand what 300 means.
- **Proposal:** Replace with named constants:
  ```rust
  pub const DEFAULT_PROXY_IDLE_TIMEOUT_SECS: u64 = 300;
  pub const DEFAULT_PROXY_STARTUP_TIMEOUT_SECS: u64 = 120;
  pub const DEFAULT_HEALTH_CHECK_TIMEOUT_MS: u64 = 30_000;
  ```

### 8. serde_json::Value Used as Generic Payload (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/proxy/tama_handlers/models.rs:82,524`, `tama-core/src/proxy/forward.rs:142`, `tama-core/src/proxy/handlers/compaction.rs:37`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `serde_json::Value` is used as the type for model lists, request/response bodies, and SSE processing. This bypasses compile-time validation and makes the code fragile. In `forward.rs`, `rewrite_json_model_name` operates on a raw `JsonValue`, meaning any field name typo is caught only at runtime.
- **Proposal:** Define proper DTO structs for OpenAI API responses (`ChatCompletionResponse`, `ChatCompletionChoice`, etc.) and use them in the proxy handlers. Keep `JsonValue` only for truly opaque passthrough cases.

### 9. HashMap<String, String> for GPU Device Metadata (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/gpu/system.rs:12,16,20,80`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `AMD_DEVICE_NAMES` and `AMD_DEVICE_UUIDS` are `OnceLock<HashMap<String, String>>` mapping PCI bus → name/UUID. The key and value types are opaque strings with no type safety. The nested `HashMap<String, HashMap<String, String>>` deserialization of rocm-smi output is similarly opaque.
- **Proposal:** 
  - `type PciBus = String;` (or better: `struct PciBus(String)`)
  - `type GpuName = String;` (or `struct GpuName(String)`)
  - `type GpuUuid = String;` (or `struct GpuUuid(String)`)
  - Create typed structs: `struct AmgDeviceMetadata { pci_bus: PciBus, name: GpuName, uuid: Option<GpuUuid> }`

### 10. ModelConfigRecord Stores JSON as Option<String> (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/db/queries/types.rs:20-24`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `ModelConfigRecord` stores `args`, `sampling`, `modalities`, `health_check`, and `spec_decoding` as `Option<String>` (raw JSON). Every consumer must manually call `serde_json::from_str` / `to_string`, creating a maintenance burden and risk of desync between the JSON schema and the consuming code.
- **Proposal:** Store these as separate normalized tables (e.g., `model_args`, `model_sampling_params`) or at minimum create typed wrapper structs with `From<String>` / `Into<String>` impls that encapsulate the JSON serialization.

### 11. ModelConfig Has 30+ Fields (God Object)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:378-469`
- **Severity:** Medium
- **Confidence:** Medium
- **Problem:** `ModelConfig` has 30+ public fields spanning multiple domains: backend selection, GPU config, sampling params, HF metadata, model card refs, health checks, and speculative decoding. This single struct is used for TOML config, DB records, API requests, and UI display. Adding a new field requires changes in 4+ serialization/deserialization paths.
- **Proposal:** Decompose into domain-specific sub-configs:
  ```rust
  struct ModelConfig {
      pub identity: ModelIdentity,        // model, quant, mmproj, mtp_model
      pub backend: ModelBackendConfig,    // backend, gpu_variant, gpu_device, port
      pub inference: ModelInferenceConfig, // args, sampling, gpu_layers, kv_unified
      pub health: ModelHealthConfig,      // health_check
      pub hf_metadata: Option<HfMetadata>, // all hf_* fields
      pub quants: BTreeMap<String, QuantEntry>,
      pub spec_decoding: SpecDecodingConfig,
  }
  ```

### 12. ModelStatus.state Comparison in Comments (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/gpu/system.rs:236,268`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The doc comment says "One of: `idle`, `loading`, `ready`, `unloading`, `failed`" but the field is `pub state: String`. The code in `system.rs:268` does string comparison `state == "failed"`. This is error-prone and not enforceable at compile time.
- **Proposal:** Convert to `enum ModelStatusState` as in Finding #1. The `#[serde(...)]` attribute can handle string serialization.

### 13. GPU Device ID as Unstructured String (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/gpu/system.rs:129`
- **Severity:** Medium
- **Confidence:** Medium
- **Problem:** `GpuDeviceStats.device_id` is `String` with values like "GPU0", "nvidia0", "amd0". The format varies by vendor but there's no type to express this. The `assign_position_ids` function constructs "GPU{n}" format, while `parse_nvidia_smi_csv_line` constructs "nvidia{n}" format — no shared abstraction.
- **Proposal:** `enum GpuDeviceId { Positional(u32) { display: "GPU0" }, VendorIndexed { vendor: GpuVendor, index: u32 } { display: "nvidia0" } }` or at minimum a `struct GpuDeviceId { vendor: GpuVendor, index: u32 }` with a `Display` impl.

---

## 🟢 Low Severity

### 14. Config::from_db Uses Tuple Destructuring (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:195-216`
- **Severity:** Low
- **Confidence:** High
- **Problem:** `Config::from_db()` destructures tuples like `proxy_row.0`, `proxy_row.1`, ..., `proxy_row.11` to build `ProxyConfig`. This is fragile — adding a field requires updating the DB query, the tuple destructuring, and the struct construction.
- **Proposal:** Resolved by Finding #5 (typed DB records). Once `get_proxy()` returns `ProxyConfigRecord`, this becomes `ProxyConfig { host: proxy_row.host, ... }`.

### 15. BackendConfig Fields as Option<String> (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:403-408`
- **Severity:** Low
- **Confidence:** High
- **Problem:** `BackendConfig` has `path: Option<String>`, `version: Option<String>`, `gpu_variant: Option<String>`. These should be typed: `path: Option<PathBuf>`, `version: Option<BackendVersion>`, `gpu_variant: Option<GpuVariant>`.
- **Proposal:** Create `struct BackendVersion(semver::Version)` and `enum GpuVariant { Cpu, Cuda, Vulkan, Mps }`.

### 16. Vec<String> for Skip Paths (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:98`
- **Severity:** Low
- **Confidence:** Medium
- **Problem:** `ProxyConfig.authenticator_skip_paths: Vec<String>` is a raw vector with no validation. Should be a typed collection with path validation.
- **Proposal:** `struct SkipPaths(Vec<PathPattern>)` where `PathPattern` validates and normalizes paths.

### 17. General.log_level as String (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:375`
- **Severity:** Low
- **Confidence:** High
- **Problem:** `General.log_level: String` with values "debug", "info", "warn", "error". Should be a typed enum matching `tracing::Level`.
- **Proposal:** `enum LogLevel { Debug, Info, Warn, Error }` with `Default = Info`.

### 18. Config::to_db() Manual Field Mapping (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:295-350`
- **Severity:** Low
- **Confidence:** Medium
- **Problem:** `Config::to_db()` manually maps each field of `Config` into individual `upsert_*` calls. This is a long manual mapping that will drift when fields are added.
- **Proposal:** Use a derive macro or builder pattern to automate the mapping. Or, create a `ConfigRecord` struct that mirrors `Config` and use `From<Config> for ConfigRecord`.

### 19. proxy/forward.rs is 1119 Lines (File Length)
- **Lens:** File Length + Structure
- **Files:** `tama-core/src/proxy/forward.rs`
- **Severity:** Low
- **Confidence:** High
- **Problem:** This single file handles HTTP forwarding, SSE streaming, header filtering, inference stats extraction, JSON rewriting, and circuit breaker logic. It mixes transport-level concerns with application-level concerns.
- **Proposal:** Split into modules:
  - `forward/request.rs` — HTTP request building and forwarding
  - `forward/response.rs` — Response handling and rewriting
  - `forward/sse.rs` — SSE streaming and model name rewriting
  - `forward/stats.rs` — Inference stats extraction

### 20. gpu/system.rs is 1121 Lines (File Length)
- **Lens:** File Length + Structure
- **Files:** `tama-core/src/gpu/system.rs`
- **Severity:** Low
- **Confidence:** High
- **Problem:** Combines GPU device detection (NVIDIA + AMD), system metrics collection, metric types, and SSE broadcast types. The AMD device detection with sysfs reads is a completely different domain from the metrics types.
- **Proposal:** Split into:
  - `gpu/system.rs` — Metric types (`SystemMetrics`, `MetricSample`, `MetricsSnapshot`)
  - `gpu/detect.rs` — Device detection (already partially done)
  - `gpu/amd.rs` — AMD-specific detection and parsing
  - `gpu/nvidia.rs` — NVIDIA-specific detection and parsing

### 21. tama/src/types/config.rs is 1013 Lines (File Length)
- **Lens:** File Length + Structure
- **Files:** `tama/src/types/config.rs`
- **Severity:** Low
- **Confidence:** Medium
- **Problem:** Mirrors all core config types for WASM with 25 `From` impls. The duplication is intentional (BTreeMap for JSON determinism) but the file is large and hard to maintain.
- **Proposal:** Use a macro or code generation to reduce the boilerplate of `From` impls. Or, use a single type with conditional serialization via feature flags.

### 22. app_config_queries.rs is 791 Lines (File Length)
- **Lens:** File Length + Structure
- **Files:** `tama-core/src/db/queries/app_config_queries.rs`
- **Severity:** Low
- **Confidence:** High
- **Problem:** Contains 12+ query functions for app config sections. Each function has its own tuple type, SQL string, and serialization logic.
- **Proposal:** Extract each section into its own module (`app_config_queries/general.rs`, `proxy.rs`, `supervisor.rs`, `compaction.rs`) with typed record structs.

### 23. ModelModalities Uses Vec<String> (Primitive Obsession)
- **Lens:** Weak Abstractions
- **Files:** `tama-core/src/config/types.rs:473-477`
- **Severity:** Low
- **Confidence:** Medium
- **Problem:** `ModelModalities` has `input: Vec<String>` and `output: Vec<String>` with values like "text", "image". No validation that these are valid modality types.
- **Proposal:** `enum Modality { Text, Image, Audio, Video }` and `struct ModelModalities { input: Vec<Modality>, output: Vec<Modality> }`.

---

## Top Recommendation

**Start with Finding #5 (Typed DB Records)** — it's the highest-impact change that unlocks improvements in multiple other findings (#14, #1, #2, #3). Once the DB layer exposes typed structs instead of tuples, the config types become cleaner, and the entire codebase gains type safety at the persistence boundary.

The second priority is **Finding #1 (GPU Vendor / Model State enums)** — this is the most impactful primitive obsession fix in the runtime-critical path, as these types are used in hot paths for metrics and proxy handling.
