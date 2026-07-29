# ProxyState Sub-Structs Plan

**Goal:** Break `ProxyState`'s 19 flat `pub(crate)` fields into three focused sub-structs (`RegistryState`, `MetricsState`, `PullState`) with real domain methods, so the public surface stops handing out `Arc<RwLock<…>>` lock guards and the two service-locator methods (`model_mgr`, `open_db`) are narrowed to crate-internal use.

**Architecture:** `ProxyState` (currently `crates/tama-core/src/proxy/types.rs:257-299`) becomes a thin composition: `registry: RegistryState` (models/model_configs/aliases caches), `metrics: MetricsState` (ProxyMetrics counters + system_metrics + metrics_tx + inference_stats channels), `pull: PullState` (pull_jobs + in_flight_pulls + pull_queue), plus the remaining standalone fields (config, client, db_dir, config_write_semaphore, backend_logs, gpu_devices_cache, model_tasks, cookie_key, langfuse_client). Migration is shim-based: Task 1 moves fields and keeps every existing accessor as a delegating shim so nothing breaks; Tasks 2–4 replace lock-guard usage with domain methods per domain; Task 5 deletes the shims; Task 6 narrows `open_db`/`model_mgr`. Audit finding F32 (`docs/reviews/2026-07-18-codebase-improvement.md` #32). Verified call-site inventory: internal tama-core access is overwhelmingly DIRECT FIELD access (~175 sites: `models` 89, `model_configs` 46, `config` 29, `inference_stats` 11, `pull_jobs` 11, …); the `tama` crate uses only `db_dir()` ×27, `config()` ×9, `pull_queue()` ×5, `client()` ×4, `models()` ×3; `open_db()` (13 sites) and `model_mgr()` (16 sites) are 100% tama-core-internal.

**Tech Stack:** Rust, Axum, tokio (RwLock/Mutex/broadcast/watch/Semaphore), rusqlite

---

### Task 1: Introduce the three sub-structs; move fields; delegating shim accessors

**Context:**
This is the big mechanical commit: `proxy/state.rs` becomes a module directory hosting the sub-structs, `ProxyState`'s struct literal in `types.rs` is recomposed, and every internal direct field access is updated to the new paths. The compiler drives the migration — there is no behavior change, no lock-type change, no test-body change. Decisions: sub-structs are `pub(crate)` types with `pub(crate)` fields (crate-internal reality stays; the PUB surface is what Tasks 2–5 fix); sub-structs derive `Clone` (all members are `Arc`/channel handles); the manual `impl Clone for ProxyState` (`types.rs:231-256`) is preserved because of its deliberate `model_tasks: RwLock::new(HashMap::new())` fresh-map quirk (:251) — keep that quirk exactly; the stale TODO comment at `types.rs:248-250` ("Consider splitting into sub-structs …") is deleted by this task. Naming: `MetricsState`'s `Arc<ProxyMetrics>` member is named `counters` (NOT `metrics`) so `state.metrics.counters` never reads as `state.metrics.metrics`; `ProxyState` fields are `registry`, `metrics`, `pull`.

**Files:**
- Create: `crates/tama-core/src/proxy/state/mod.rs` (content = current `proxy/state.rs`, constructor updated)
- Create: `crates/tama-core/src/proxy/state/registry.rs`
- Create: `crates/tama-core/src/proxy/state/metrics.rs`
- Create: `crates/tama-core/src/proxy/state/pull.rs`
- Delete: `crates/tama-core/src/proxy/state.rs`
- Modify: `crates/tama-core/src/proxy/types.rs` (struct def, Clone, `open_db`, `shutdown`, shim accessors)
- Modify: every tama-core file with direct field access (compiler-driven; known hotspots: `proxy/lifecycle/mod.rs`, `proxy/lifecycle/{idle_timeout,compaction,tts}.rs`, `proxy/forward/request.rs`, `proxy/forward/stats.rs`, `proxy/tama_handlers/**`, `proxy/server/{mod,metrics,tests}.rs`, `proxy/handlers/{status,forward}.rs`, `proxy/{rename,pull_queue,auth,scope_middleware}.rs`, `proxy/mod.rs`)

**What to implement:**

1. **`state/registry.rs`**:
   ```rust
   //! Model registry state: loaded backends, model configs, and aliases.
   use std::collections::HashMap;
   use std::sync::Arc;
   use tokio::sync::RwLock;
   use crate::proxy::types::BackendState;

   /// Caches describing which models exist and which are currently loaded.
   #[derive(Clone, Default)]
   pub(crate) struct RegistryState {
       pub(crate) models: Arc<RwLock<HashMap<String, BackendState>>>,
       pub(crate) model_configs: Arc<RwLock<HashMap<String, crate::config::ModelConfig>>>,
       /// alias_name → resolved model name (api_name or repo_id). Enabled aliases only.
       pub(crate) aliases: Arc<RwLock<HashMap<String, String>>>,
   }

   impl RegistryState {
       pub(crate) fn new() -> Self { Self::default() }
   }
   ```
   (Move the doc comments from the `types.rs` fields onto these fields.)

2. **`state/metrics.rs`**: `#[derive(Clone)] pub(crate) struct MetricsState` with `pub(crate) counters: Arc<ProxyMetrics>`, `pub(crate) system_metrics: Arc<RwLock<crate::gpu::SystemMetrics>>`, `pub(crate) metrics_tx: tokio::sync::broadcast::Sender<crate::gpu::MetricsSnapshot>`, `pub(crate) inference_stats: tokio::sync::watch::Sender<HashMap<String, LatestInferenceStats>>`, and:
   ```rust
   impl MetricsState {
       pub(crate) fn new() -> Self {
           let (metrics_tx, _) = tokio::sync::broadcast::channel(3);
           Self {
               counters: Arc::new(ProxyMetrics::default()),
               system_metrics: Arc::new(RwLock::new(crate::gpu::SystemMetrics::default())),
               metrics_tx,
               inference_stats: tokio::sync::watch::channel(HashMap::new()).0,
           }
       }
   }
   ```
   (The `broadcast::channel(3)` capacity moves here from `ProxyState::new` — preserve `3`.)

3. **`state/pull.rs`**: `#[derive(Clone, Default)] pub(crate) struct PullState` with `pub(crate) pull_jobs: Arc<RwLock<HashMap<String, PullJob>>>`, `pub(crate) in_flight_pulls: Arc<Mutex<HashSet<PathBuf>>>` (move the temp-file-corruption doc comment from `types.rs:268-270`), `pub(crate) pull_queue: Option<Arc<PullQueueService>>`, and `pub(crate) fn new(pull_queue: Option<Arc<PullQueueService>>) -> Self`.

4. **`types.rs` `ProxyState`** becomes:
   ```rust
   pub struct ProxyState {
       pub(crate) registry: crate::proxy::state::RegistryState,
       pub(crate) metrics: crate::proxy::state::MetricsState,
       pub(crate) pull: crate::proxy::state::PullState,
       pub(crate) config: Arc<tokio::sync::RwLock<crate::config::Config>>,
       pub(crate) client: reqwest::Client,
       pub(crate) db_dir: Option<std::path::PathBuf>,
       pub(crate) config_write_semaphore: Arc<tokio::sync::Semaphore>,
       pub(crate) backend_logs: crate::backends::log_stream::BackendLogManager,
       pub(crate) gpu_devices_cache: Arc<tokio::sync::RwLock<HashMap<String, GpuDeviceCacheEntry>>>,
       pub(crate) model_tasks: tokio::sync::RwLock<HashMap<String, JoinSet<()>>>,
       pub(crate) cookie_key: cookie::Key,
       pub(crate) langfuse_client: Arc<tokio::sync::RwLock<Option<Arc<crate::proxy::forward::langfuse::LangfuseClient>>>>,
   }
   ```
   Update `impl Clone for ProxyState` to clone the three sub-structs plus the surviving fields, preserving the `model_tasks` fresh-map quirk. Update `shutdown()` (`types.rs:314-344`) to the new paths (`self.metrics.metrics_tx.send(...)`, `self.registry.models.write()`, `self.pull.pull_jobs.write()`, `self.pull.in_flight_pulls.lock()`, `self.metrics.inference_stats.send_replace(...)`). Every existing accessor becomes a one-line shim, e.g. `pub fn models(&self) -> &Arc<RwLock<HashMap<String, BackendState>>> { &self.registry.models }`, `pub fn metrics(&self) -> &Arc<ProxyMetrics> { &self.metrics.counters }`, `pub fn pull_queue(&self) -> &Option<Arc<PullQueueService>> { &self.pull.pull_queue }` — ALL 18 get-accessors + `set_pull_queue` stay for now (Task 5 removes them). Delete the TODO comment above `impl Clone`.

5. **`state/mod.rs`** = the old `state.rs` body with: `mod metrics; mod pull; mod registry; pub(crate) use {metrics::MetricsState, pull::PullState, registry::RegistryState};` at the top; `ProxyState::new` constructs `registry: RegistryState::new()`, `metrics: MetricsState::new()`, `pull: PullState::new(pull_queue.clone())` (pull_queue computed exactly as today at old :18-24, including `ModelManager::open` + `PullQueueService::new` + the `queue_processor_loop` spawn at old :63-70 — unchanged). `proxy/mod.rs:12` (`mod state;`) needs no change.

6. **Mechanical path migration** (compiler-driven): `state.models` → `state.registry.models` (89 sites), `state.model_configs` → `state.registry.model_configs` (46), `state.aliases` → `state.registry.aliases` (7), `state.metrics` → `state.metrics.counters` (6 — careful: only the FIELD, not `.metrics()` calls), `state.system_metrics` → `state.metrics.system_metrics` (4), `state.metrics_tx` → `state.metrics.metrics_tx` (3), `state.inference_stats` → `state.metrics.inference_stats` (11), `state.pull_jobs` → `state.pull.pull_jobs` (11), `state.in_flight_pulls` → `state.pull.in_flight_pulls` (1), `state.pull_queue` → `state.pull.pull_queue` (field sites only). Method-call sites (`state.models()`, `state.pull_queue()`, …) compile unchanged through the shims — do NOT touch them in this task. The local variable `let metrics_state = Arc::clone(&state);` in `proxy/server/metrics.rs:155` is NOT a sub-struct — leave its name alone. Do not blind-`sed`: change the struct, then fix each `cargo check` error individually (most are one-token inserts).

**Steps:**
- [ ] Run `cargo nextest run --package tama-core` — green baseline (record the test count)
- [ ] Create `proxy/state/` with the three sub-struct files + `mod.rs`; delete `proxy/state.rs`; recompose `ProxyState` in `types.rs` with shim accessors
- [ ] Run `cargo check --package tama-core` and fix every error mechanically (path inserts only — any temptation to change logic means you made a wrong edit earlier)
- [ ] Run `cargo nextest run --package tama-core` — all pass, same count as baseline (`types.rs::test_proxy_state_accessors_exist` passes unchanged through the shims)
- [ ] Run `cargo check --package tama` — the dependent crate still compiles against the shim surface
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: compose ProxyState from RegistryState/MetricsState/PullState sub-structs"

**Acceptance criteria:**
- [ ] `ProxyState` has 12 fields (3 sub-structs + 9 standalone); the 18 get-accessors + `set_pull_queue` still exist and return identical types
- [ ] `crates/tama` compiles and `cargo nextest run --package tama` passes with zero edits to that crate
- [ ] Same tama-core test count as baseline; zero test-body edits
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 2: Config access — `with_config` / `with_config_mut` / `replace_config`; migrate the 9 `tama` sites

**Context:**
`state.config()` hands the config `RwLock` to the `tama` crate at 9 sites in 8 files: four benchmark submissions read `proxy_url()` (`api/benchmarks/{mod.rs:243, mtp.rs:84, run.rs:31, spec.rs:73}`), two log handlers read `logs_dir()` (`api/logs.rs:47`, `api.rs:41`), `backends/list.rs:263` clones `compaction`, `backends/compaction.rs:32-46` mutates several `compaction` fields then saves, and `api.rs:73-81` (`sync_proxy_config`) replaces the whole config then refreshes Langfuse. Decisions: add three domain methods on `ProxyState` (in `state/mod.rs`) — `with_config`/`with_config_mut` take closures so no guard ever crosses an `.await` or leaves the crate, and `replace_config` bundles the replace+Langfuse-refresh sequence (it must call `refresh_langfuse_client` after the swap, matching `sync_proxy_config`'s current behavior at `api.rs:79`). `config()` itself survives until Task 5 (tama-core has 29 internal direct field sites that stay as-is).

**Files:**
- Modify: `crates/tama-core/src/proxy/state/mod.rs`
- Modify: `crates/tama/src/api/benchmarks/{mod,mtp,run,spec}.rs`
- Modify: `crates/tama/src/api/logs.rs`, `crates/tama/src/api.rs`
- Modify: `crates/tama/src/api/backends/list.rs`, `crates/tama/src/api/backends/compaction.rs`

**What to implement:**

1. In `state/mod.rs`'s `impl ProxyState`:
   ```rust
   /// Read from the live config without exposing the lock. The closure runs
   /// under a read guard that is dropped before this method returns.
   pub async fn with_config<R>(&self, f: impl FnOnce(&crate::config::Config) -> R) -> R {
       let config = self.config.read().await;
       f(&config)
   }

   /// Mutate the live config without exposing the lock. Returns the closure's
   /// result (e.g. a cloned `Config` to persist) after the write guard drops.
   pub async fn with_config_mut<R>(&self, f: impl FnOnce(&mut crate::config::Config) -> R) -> R {
       let mut config = self.config.write().await;
       f(&mut config)
   }

   /// Replace the live config and refresh config-derived clients (Langfuse).
   /// Mirrors what the config PATCH endpoint did inline (api.rs::sync_proxy_config).
   pub async fn replace_config(&self, new_config: crate::config::Config) {
       {
           let mut config = self.config.write().await;
           *config = new_config;
       }
       self.refresh_langfuse_client().await;
   }
   ```
2. Migrate (exact replacements):
   - `api/benchmarks/{mod,mtp,run,spec}.rs`: `let proxy_base_url = state.config().read().await.proxy_url();` → `let proxy_base_url = state.with_config(|c| c.proxy_url()).await;`
   - `api/logs.rs:47` and `api.rs:41`: `state.config().read().await.logs_dir()` → `state.with_config(|c| c.logs_dir()).await`
   - `api/backends/list.rs:263`: `state.config().read().await.compaction.clone()` → `state.with_config(|c| c.compaction.clone()).await`
   - `api/backends/compaction.rs:32-46`: the `{ let mut config = state.config().write().await; …; let config_to_save = (*config).clone(); drop(config); … }` block →
     ```rust
     let (config_to_save, was_enabled) = state.with_config_mut(|config| {
         // the 5 statements verbatim: device from_str with unwrap_or-fallback,
         // port, request_timeout_ms, `let was_enabled = config.compaction.enabled;`,
         // `config.compaction.enabled = req.enabled;`
         ((*config).clone(), was_enabled)
     }).await;
     ```
     The subsequent `config_to_save.save()` + `if req.enabled && !was_enabled { … load_compaction_backend … }` logic is unchanged. NOTE: keep the lossy `CompactionDevice::from_str(device).unwrap_or(config.compaction.device.clone())` semantics — F16 owns the 422-validation fix, not this plan.
   - `api.rs` `sync_proxy_config` (:73-81): body → `state.replace_config(new_config).await;` (delete the manual lock dance; the Langfuse comment moves onto `replace_config` in tama-core — keep a one-line comment here pointing at it).
3. Add a test in `state/mod.rs`'s test module:
   ```rust
   #[tokio::test]
   async fn test_with_config_and_replace_config() {
       let state = ProxyState::new(crate::config::Config::default(), None);
       let port = state.with_config(|c| c.proxy.port).await;
       assert_eq!(port, crate::config::Config::default().proxy.port);
       let mut new_config = crate::config::Config::default();
       new_config.proxy.port = 19999;
       state.replace_config(new_config).await;
       assert_eq!(state.with_config(|c| c.proxy.port).await, 19999);
       state.with_config_mut(|c| c.proxy.port = 18888).await;
       assert_eq!(state.with_config(|c| c.proxy.port).await, 18888);
   }
   ```

**Steps:**
- [ ] Write `test_with_config_and_replace_config` in `state/mod.rs` tests first; run `cargo nextest run --package tama-core -- proxy::state` — it FAILS (methods don't exist)
- [ ] Implement the three methods; run `cargo nextest run --package tama-core -- proxy::state` — pass
- [ ] Migrate the 9 `tama` call sites; run `cargo check --package tama`
- [ ] Run `cargo nextest run --package tama-core` and `cargo nextest run --package tama` — all pass
- [ ] Run `rg "\.config\(\)" crates/tama/src` — zero hits
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: config access via with_config/with_config_mut/replace_config domain methods"

**Acceptance criteria:**
- [ ] `rg "\.config\(\)" crates/tama/src` — zero hits; the `tama` crate no longer holds the config lock
- [ ] `replace_config` refreshes Langfuse (observable: `sync_proxy_config` callers behave as before)
- [ ] New unit test passes; full workspace tests pass; clippy clean

---

### Task 3: `RegistryState` domain methods; migrate the 3 `tama` `models()` sites + internal verbatim sites

**Context:**
The registry methods currently on `ProxyState` (`state/mod.rs` after Task 1) only touch registry fields and belong on `RegistryState`: `get_model_state` (:106), `get_available_backend_for_model` (:143 — also reads `config` for the circuit-breaker threshold), `update_last_accessed` (:178), `reload_model_configs` (:219 — needs a DB conn), `reload_aliases` (:248 — needs a DB conn), `resolve_alias` (:261), plus the four dead-or-dying ones (`is_model_loaded`, `get_model_state_with_access`, `get_backend_pid`, `get_circuit_breaker_failures` — plan-172 may have deleted them already; move whatever still exists, do NOT resurrect and do NOT delete here). The 3 `tama` `models()` sites are simple registry queries: compaction readiness checks (`api/backends/compaction.rs:64-68`, `api/backends/list.rs:267-273`) and TTS-backend enumeration at shutdown (`main.rs:141-145`). Decisions: DB-needing methods take `&rusqlite::Connection` (the caller — `ProxyState` — owns connection acquisition via `model_mgr`); `get_available_backend_for_model` takes `&Config`; new `tts_backend_names()` covers `main.rs`; `ProxyState` keeps thin delegating methods so the 10+ internal method-call sites (`self.get_model_state(...)`, `state.resolve_alias(...)` etc.) don't change.

**Files:**
- Modify: `crates/tama-core/src/proxy/state/registry.rs`
- Modify: `crates/tama-core/src/proxy/state/mod.rs`
- Modify: `crates/tama/src/api/backends/compaction.rs`, `crates/tama/src/api/backends/list.rs`, `crates/tama/src/main.rs`

**What to implement:**

1. Move to `impl RegistryState` in `registry.rs` (bodies verbatim, adjusted for being inherent methods — `self.models` etc. now resolve directly):
   - `pub(crate) async fn get_model_state(&self, backend_name: &str) -> Option<BackendState>`
   - `pub(crate) async fn update_last_accessed(&self, backend_name: &str)`
   - `pub(crate) async fn get_available_backend_for_model(&self, config: &crate::config::Config, model_name: &str) -> Option<String>` — the `model_configs` read stays internal; `circuit_breaker_threshold` comes from the param
   - `pub(crate) async fn resolve_alias(&self, name: &str) -> String`
   - `pub(crate) async fn reload_model_configs(&self, conn: &rusqlite::Connection) -> anyhow::Result<()>` (body: `*self.model_configs.write().await = crate::db::load_model_configs(conn)?;`)
   - `pub(crate) async fn reload_aliases(&self, conn: &rusqlite::Connection) -> anyhow::Result<()>` (body: `load_aliases_for_cache` + replace map)
   - `pub(crate) async fn tts_backend_names(&self) -> Vec<String>` — new: `self.models.read().await.iter().filter(|(_, ms)| ms.is_tts_backend()).map(|(name, _)| name.clone()).collect()`
   - Move `is_model_loaded`/`get_model_state_with_access`/`get_backend_pid`/`get_circuit_breaker_failures` ONLY IF they still exist (plan-172 dependency); same verbatim-move rule.
2. `ProxyState` delegations in `state/mod.rs` (signatures unchanged): `get_model_state` → `self.registry.get_model_state(...)`; `get_available_backend_for_model` → reads config (`let config = self.config.read().await;`) then delegates; `reload_model_configs`/`reload_aliases` → open `model_mgr()` with the existing `.with_context(|| "Database directory not configured")?` then `self.registry.reload_*(mgr.conn()).await`; `resolve_alias`, `update_last_accessed` → delegate; plus `pub async fn tts_backend_names(&self) -> Vec<String>` delegate. Alias/reload tests in `state/mod.rs::tests` (`test_resolve_alias_*`, `test_reload_aliases_*`) must pass unmodified.
3. Migrate the 3 `tama` sites:
   - `api/backends/compaction.rs:64-68` → `let running = state.get_model_state("compaction").await.map(|s| s.is_ready()).unwrap_or(false);`
   - `api/backends/list.rs:267-273` → `let (compaction_running, compaction_url) = match state.get_model_state("compaction").await { Some(s) if s.is_ready() => (true, s.backend_url().map(|u| u.to_string())), _ => (false, None), };` (preserve the exact `(bool, Option<String>)` tuple shape the surrounding code destructures)
   - `main.rs:141-145` → `let tts_backends: Vec<String> = cleanup_state.state.tts_backend_names().await;`

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- proxy::state` — green baseline
- [ ] Move the methods to `RegistryState`, add delegations; run `cargo check --package tama-core`
- [ ] Run `cargo nextest run --package tama-core -- proxy` — all pass (alias/reload tests unmodified)
- [ ] Migrate the 3 `tama` sites; run `cargo check --package tama` and `cargo nextest run --package tama`
- [ ] Run `rg "\.models\(\)" crates/tama/src` — zero hits
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: RegistryState domain methods; tama models() sites use them"

**Acceptance criteria:**
- [ ] `rg "\.models\(\)" crates/tama/src` — zero hits
- [ ] `RegistryState` owns all registry logic; `ProxyState`'s same-named methods are ≤ 4-line delegations
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 4: `MetricsState` + `PullState` domain methods; migrate channel/lock call sites

**Context:**
`inference_stats` (watch channel) is poked with `send_modify` at 7 sites (`lifecycle/tts.rs:181`, `lifecycle/idle_timeout.rs:154,247`, `lifecycle/mod.rs:380,681`, `forward/request.rs:48,595`), `borrow().clone()` at 2 (`forward/stats.rs:60`, `server/metrics.rs:217`), `send_replace` in `shutdown`; `metrics_tx` is `send`ed once (`server/metrics.rs:311`) and `subscribe`d at 3 sites (`tama_handlers/system.rs:127`, `server/tests.rs:266,339`); `system_metrics` is written once (`server/metrics.rs:211`) and read at 3 (`handlers/status.rs:182`, `tama_handlers/system.rs:35`, `forward/request.rs:493`); `pull_jobs` is read/written at ~10 sites with mixed logic. Decisions: `MetricsState` gets a small complete API (`publish_metrics`, `subscribe_metrics`, `set_system_metrics`, `system_metrics_snapshot`, `inference_stats_snapshot`, `modify_inference_stats` — a generic `send_modify` wrapper covering all 7 closure sites — plus `record_inference_stats` for the plain insert pattern and `clear_inference_stats` for `shutdown`); `PullState` gets only the obviously-verbatim methods (`get_pull_job`, `upsert_pull_job`, `list_pull_jobs`, `clear`) — the complex multi-statement lock blocks in `tama_handlers/pull/{handlers,verify}.rs` stay as `pub(crate)` field access (converting them is a semantic refactor, not this plan). `forward/request.rs:321`'s `Arc::new(state.inference_stats.clone())` becomes a cloned `MetricsState` handle passed down instead (`MetricsState` derives `Clone`, and `record_inference_stats`/`modify_inference_stats` cover what the downstream code does with the sender); adjust the surrounding closure captures minimally.

**Files:**
- Modify: `crates/tama-core/src/proxy/state/metrics.rs`, `crates/tama-core/src/proxy/state/pull.rs`
- Modify: `crates/tama-core/src/proxy/server/metrics.rs`, `crates/tama-core/src/proxy/handlers/status.rs`, `crates/tama-core/src/proxy/tama_handlers/system.rs`, `crates/tama-core/src/proxy/forward/{request,stats}.rs`, `crates/tama-core/src/proxy/lifecycle/{mod,idle_timeout,tts}.rs`, `crates/tama-core/src/proxy/types.rs` (`shutdown`)
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs` (only the verbatim get/insert/list sites: :121, :245, :375, :417, :498 — inspect each; convert only true one-liners)

**What to implement:**

1. `impl MetricsState` (all `pub(crate)`):
   ```rust
   pub(crate) fn publish_metrics(&self, snapshot: crate::gpu::MetricsSnapshot);
   pub(crate) fn subscribe_metrics(&self) -> tokio::sync::broadcast::Receiver<crate::gpu::MetricsSnapshot>;
   pub(crate) async fn set_system_metrics(&self, snapshot: crate::gpu::SystemMetrics);
   pub(crate) async fn system_metrics_snapshot(&self) -> crate::gpu::SystemMetrics; // read().await.clone()
   pub(crate) fn inference_stats_snapshot(&self) -> HashMap<String, LatestInferenceStats>; // borrow().clone()
   pub(crate) fn modify_inference_stats(&self, f: impl FnOnce(&mut HashMap<String, LatestInferenceStats>)); // send_modify wrapper
   pub(crate) fn record_inference_stats(&self, backend: &str, stats: LatestInferenceStats); // modify + insert
   pub(crate) fn clear_inference_stats(&self); // send_replace(HashMap::new())
   pub(crate) fn counters(&self) -> &Arc<ProxyMetrics>; // for handlers/status.rs-style atomic bumps
   ```
2. `impl PullState` (all `pub(crate)`): `get_pull_job(&self, job_id: &str) -> Option<PullJob>` (read + `.get().cloned()`), `upsert_pull_job(&self, job_id: String, job: PullJob)` (write + insert), `list_pull_jobs(&self) -> Vec<PullJob>`, `clear(&self)` (both maps — used by `shutdown`).
3. Migrate: `server/metrics.rs:211` → `metrics_state.set_system_metrics(snapshot.clone()).await;`, `:217` → `let inference_map = metrics_state.inference_stats_snapshot();`, `:311` → `metrics_state.publish_metrics(snapshot);`; `handlers/status.rs:182` and `tama_handlers/system.rs:35` → `system_metrics_snapshot()`; `tama_handlers/system.rs:127` → `subscribe_metrics()`; the 7 `send_modify` sites → `modify_inference_stats` (closures verbatim); `forward/stats.rs:60-62` → `modify_inference_stats`; `types.rs::shutdown` → `clear_inference_stats()` + `self.pull.clear().await`; `pull_queue.rs:437,1477` read sites → `state.pull.get_pull_job(...)`-style where verbatim. Leave: `forward/request.rs:493` (reads inside a larger lock scope — convert only if it is a bare read), `server/tests.rs:266,339` (tests may keep `subscribe_metrics()`), all complex `pull/verify.rs` blocks.
4. Add unit tests in `state/metrics.rs`:
   ```rust
   #[tokio::test]
   async fn test_metrics_state_publish_subscribe_and_snapshots() { … }
   ```
   covering: publish→subscribe receives; set/snapshot round-trip; `record_inference_stats` + `inference_stats_snapshot`; `clear_inference_stats` empties the map.

**Steps:**
- [ ] Write the `MetricsState` test first; run `cargo nextest run --package tama-core -- proxy::state` — FAILS (methods missing)
- [ ] Implement both `impl` blocks + migrations; run `cargo check --package tama-core` repeatedly
- [ ] Run `cargo nextest run --package tama-core` — all pass (new test included)
- [ ] Run `rg "send_modify|\.metrics_tx\.send|metrics_tx\.subscribe" crates/tama-core/src` — only inside `state/metrics.rs` (and tests via `subscribe_metrics`)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: MetricsState/PullState domain methods for channel and job-map access"

**Acceptance criteria:**
- [ ] All 7 `send_modify` sites and both `borrow().clone()` readers go through `MetricsState` methods
- [ ] `shutdown()` uses `clear_inference_stats` + `PullState::clear`
- [ ] New `MetricsState` test passes; full `cargo nextest run --package tama-core` green; clippy clean

---

### Task 5: Delete the shim accessors; rewrite the surface test

**Context:**
After Tasks 2–4 the pub accessors have no remaining callers outside tama-core internals, and internals use fields directly. Verified-as-keepable service accessors (NOT locks, NOT removed): `client()` (returns `&reqwest::Client` — cheap-clone handle), `db_dir()` (plain `&Option<PathBuf>`; plan-160 keeps it for `api/helpers.rs` BackendManager helpers), `pull_queue()` (service handle), `backend_logs()` (service handle). Everything else is a lock-guard leak with zero `tama` callers. Decisions: delete the 14 leaking accessors AND `set_pull_queue` (dead per audit F38 — if plan-172 already deleted it, skip); convert the 2 internal `langfuse_client()` method-call sites to field access before deleting it; rewrite `test_proxy_state_accessors_exist` (`types.rs:462-489`) into a test of the SURVIVING surface. Also delete the now-unused `GpuDeviceCacheEntry`-returning `gpu_devices_cache()` accessor (field stays for `get_or_discover_gpu_devices`).

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: the 2 internal `langfuse_client()` call sites (find with `rg "\.langfuse_client\(\)" crates/tama-core/src` — currently in `proxy/forward/`)

**What to implement:**

1. Delete from `types.rs`: `config()`, `model_configs()`, `aliases()`, `models()`, `metrics()`, `pull_jobs()`, `system_metrics()`, `in_flight_pulls()`, `metrics_tx()`, `inference_stats()`, `config_write_semaphore()`, `gpu_devices_cache()`, `model_tasks()`, `langfuse_client()`, `set_pull_queue()`. KEEP: `client()`, `db_dir()`, `pull_queue()`, `backend_logs()`, `open_db()` (Task 6), `shutdown()`, and all `state/mod.rs` domain methods.
2. Convert the 2 `self.langfuse_client()` / `state.langfuse_client()` call sites to `self.langfuse_client` field access (same crate — `pub(crate)` field).
3. Replace `test_proxy_state_accessors_exist` with:
   ```rust
   /// Verify the public surface exposes service handles and sub-struct
   /// composition — not lock guards.
   #[test]
   fn test_proxy_state_public_surface() {
       let state = ProxyState::new(crate::config::Config::default(), None);
       let _: &reqwest::Client = state.client();
       let _: &Option<std::path::PathBuf> = state.db_dir();
       let _: &Option<Arc<PullQueueService>> = state.pull_queue();
       let _: &crate::backends::log_stream::BackendLogManager = state.backend_logs();
       // Sub-structs are composed and independently cloneable.
       let _registry = state.registry.clone();
       let _metrics = state.metrics.clone();
       let _pull = state.pull.clone();
   }
   ```

**Steps:**
- [ ] Run `rg "\.(config|model_configs|aliases|models|metrics|pull_jobs|system_metrics|in_flight_pulls|metrics_tx|inference_stats|config_write_semaphore|gpu_devices_cache|model_tasks|langfuse_client)\(\)" crates/tama/src crates/tama-core/src --glob '*.rs'` — expect ONLY the 2 internal `langfuse_client()` hits (fix them) and zero everything else; if any other call site survives, migrate it first (do not delete an accessor with live callers)
- [ ] Delete the 15 items; rewrite the test; run `cargo check --workspace`
- [ ] Run `cargo nextest run --workspace` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: remove ProxyState lock-guard accessors; surface is domain methods + service handles"

**Acceptance criteria:**
- [ ] `ProxyState`'s pub accessor list is exactly: `client`, `db_dir`, `pull_queue`, `backend_logs`, `open_db`, `shutdown` + domain methods
- [ ] New surface test passes; `cargo nextest run --workspace` green; clippy clean

---

### Task 6: Narrow `open_db` and `model_mgr` to `pub(crate)`

**Context:**
`open_db()` (`types.rs:304`) hands out raw `rusqlite::Connection`s and `model_mgr()` (`state/mod.rs:277`) hands out `ModelManager`s — the service-locator pattern that lets any layer bypass the Repository. Verified TODAY: all 13 `open_db()` callers (`tama_handlers/api_keys.rs` ×7, `server/metrics.rs` ×2, `server/{mod,tests}.rs`, `forward/request.rs`, `auth.rs`) and all 16 `model_mgr()` callers are inside tama-core — nothing in `crates/tama` calls either. **DEPENDENCY (explicit):** plan-160 Task 4 introduces `ApiKeyStore` (bundling the 7 `api_keys.rs` DB fns behind a connection-borrowing store) and Task 5 puts a shared `Repository` in `WebState`; it explicitly defers `open_db` itself to this plan ("Do NOT touch `ProxyState::open_db` — F32's accessor cleanup is a different plan"). Sequence this task AFTER plan-160 when possible: post-160, `ApiKeyStore` construction still needs a per-request connection and `open_db` remains its factory — that is fine and intended; this task does NOT delete `open_db`, it narrows visibility and documents its role. If plan-160 has not landed, this task is still safe (no external callers exist either way).

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs` (`open_db`)
- Modify: `crates/tama-core/src/proxy/state/mod.rs` (`model_mgr`)

**What to implement:**

1. `open_db`: change `pub fn` → `pub(crate) fn` and rewrite the doc comment:
   ```rust
   /// Open a DB connection for a quick sync operation.
   /// Returns None if db_dir is not configured (e.g., in tests).
   ///
   /// Crate-internal connection factory for proxy services (API-key validation,
   /// metrics persistence, auth). The management API in the `tama` crate uses
   /// the shared `Repository` from `WebState` (plan-160) instead — do NOT add
   /// new callers there.
   pub(crate) fn open_db(&self) -> Option<rusqlite::Connection>
   ```
2. `model_mgr`: change `pub fn` → `pub(crate) fn`, same doc-comment treatment ("Crate-internal ModelManager factory for proxy lifecycle code (`PullQueueService`, reload paths). The `tama` API layer uses the shared `Repository`.").
3. Grep-verify no visibility breakage: `cargo check --workspace`. If plan-160 HAS landed and introduced a `tama`-crate caller of either method (it should not have — verify with `rg "open_db|model_mgr" crates/tama/src`), migrate that caller to `WebState.repository` instead of widening visibility.

**Steps:**
- [ ] Run `rg "\.open_db\(\)|\.model_mgr\(\)" crates/tama/src` — MUST be zero hits before proceeding (if not, stop: migrate those sites to plan-160's `shared_repository` helper first)
- [ ] Apply the two visibility + doc changes
- [ ] Run `cargo check --workspace` and `cargo nextest run --workspace` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: narrow ProxyState::open_db/model_mgr to pub(crate) — service locator sealed"

**Acceptance criteria:**
- [ ] `open_db` and `model_mgr` are `pub(crate)`; doc comments point at the plan-160 shared Repository
- [ ] `rg "\.open_db\(\)|\.model_mgr\(\)" crates/tama/src` — zero hits
- [ ] `cargo nextest run --workspace` passes; clippy clean
