# Tamad Host Runtime Split Plan

**Goal:** Move all self-hosted concerns (backend lifecycle, installs, pulls, benchmarks, host/GPU stats) out of the proxy and into the tamad daemon, making the tama proxy a pure control plane.

**Architecture:** The proxy (tama) keeps routing, model resolution, config, the central Postgres registry, and the web UI. It never spawns a backend process or reads local hardware — every local-model operation is an RPC to the tamad that owns the model's provider (ADR-0010). Tamad is a dumb executor: it holds no model registry and no database; desired state lives only in the proxy's DB, and a proxy-side reconciler loop converges actual (per-tick process snapshots streamed over gRPC) to desired by issuing `LoadModel`/`UnloadModel`.

**Tech Stack:** Rust, gRPC (tonic 0.12 + prost, proto in `crates/tama-core/proto/tamad.proto`), axum, tokio, sysinfo, reqwest, sqlx/Postgres (central store, proxy-only writer).

**Out of scope:** TLS/mTLS between proxy and tamad (plaintext + bearer token on trusted LAN); cross-tamad failover; tamad's own binary self-update; HTTP-protocol parity for the new gRPC surface (HTTP mode stays health-only); multi-proxy high availability.

**Invariants (from ADR-0010 + approved spec):**
1. The proxy spawns no backend process and reads no local hardware — ever.
2. Tamad is a dumb executor — no model registry, no database; install dir is self-describing, process table is in-memory, token is a file.
3. Single-writer rule — only the proxy writes to central Postgres; tamad reports results in RPC responses / job terminal events.
4. One provider ↔ exactly one tamad; no cross-tamad failover.

**Staging note (important):** during Phases 1–3 the host machinery (lifecycle, installations, gpu, bench, pull) *physically remains in `tama-core`* so both binaries can link it; the proxy simply stops *using* it. Phase 4 (Task 10) physically moves the modules into `crates/tamad/` and deletes the proxy's linkage — the final state enforces invariant 1 by the dependency graph. Do not skip Task 10.

**Validation gate for every task:** before committing, run
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
```
(While iterating inside a task, use targeted runs: `cargo nextest run --package tamad` / `--package tama-core` / `--package tama`.)

---

## Phase 1 — Core loop: auth, registration, stats, lifecycle

### Task 1: proto v2 — extend `tamad.proto`

**Context:**
All later tasks build on the gRPC surface. We define the complete v2 surface up front so both sides compile against stable message shapes. The design (approved spec, Section 2): short RPCs for state changes, server-streams for anything long-running or continuous; long operations (`install`, `update`, `pull`, `benchmark`) return a job id and stream progress via `StreamJob`. The current proto has only the stub surface (ListProviders/InstallProvider/LoadModel/UnloadModel/UpdateProvider/RemoveProvider/Logs/HealthCheck).

**Files:**
- Modify: `crates/tama-core/proto/tamad.proto`
- Modify: `crates/tama-core/src/tamad/mod.rs` (add re-exports for new message types)

**What to implement:**

Add to `tamad.proto` (append new fields to existing messages only — never renumber or remove existing fields; nothing consumes the new RPCs yet so their shapes are free):

```proto
// New RPCs on service TamadService:
rpc StreamStats(StatsRequest) returns (stream SystemStats);
rpc StreamJob(JobRequest) returns (stream JobEvent);
rpc RestartProvider(RestartProviderRequest) returns (Empty);
rpc PullModel(PullModelRequest) returns (JobIdResponse);
rpc RunBenchmark(RunBenchmarkRequest) returns (JobIdResponse);

// New messages:
message StatsRequest {}

message GpuInfo {
  int32 index = 1;
  string name = 2;
  string driver_version = 3;
  int64 vram_total_bytes = 4;
  int64 vram_used_bytes = 5;
  double utilization_percent = 6;
  double temperature_c = 7;
  double power_w = 8;
}

message ProcessInfo {
  string model_name = 1;     // unique key for a loaded model's process
  string provider_name = 2;  // backend name, e.g. "llama.cpp"
  int32 pid = 3;
  bool alive = 4;
  string endpoint_url = 5;
  string status = 6;          // "starting" | "ready" | "failed" | "unloading"
}

message SystemStats {
  double cpu_usage_percent = 1;
  int64 memory_total_bytes = 2;
  int64 memory_used_bytes = 3;
  int64 swap_total_bytes = 4;
  int64 swap_used_bytes = 5;
  int64 disk_total_bytes = 6; // models dir filesystem
  int64 disk_free_bytes = 7;
  repeated GpuInfo gpus = 8;
  repeated ProcessInfo processes = 9;  // full snapshot of running backends
}

message JobIdResponse { string job_id = 1; }

message JobRequest { string job_id = 1; }

message JobEvent {
  string job_id = 1;
  string kind = 2;            // "install" | "update" | "pull" | "benchmark"
  int32 progress = 3;         // 0-100
  string message = 4;
  string status = 5;          // "running" | "succeeded" | "failed"
  string result_json = 6;     // terminal-event payload (results, file list, ...)
  string error = 7;
}

message RestartProviderRequest { string model_name = 1; }

message PullModelRequest {
  string repo_id = 1;
  repeated string quants = 2;
  string model_name = 3;
  string backend = 4;
  string hf_token = 5;        // user's HF token for gated repos, may be empty
  bool repo_pull = 6;         // true = whole-repo safetensors pull via hf CLI
}

message RunBenchmarkRequest {
  string model_name = 1;
  string kind = 2;            // "llama_bench" | "spec" | "mtp"
  string config_json = 3;     // serialized per-kind config
}
```

Extend existing messages (append-only):
- `LoadModelRequest`: add `string model_name = 5; string command = 6; repeated string args = 7; map<string,string> env = 8; string health_url = 9; int64 health_timeout_ms = 10;` — the proxy sends the fully resolved launch spec (command/args/env from the installation config in the central DB, model file path, health endpoint). Tamad does not read any DB to launch a model.
- `ProviderInfo`: add `repeated ProcessInfo loaded_models = 6;`
- Change the return type of `InstallProvider` and `UpdateProvider` from `InstallProviderResponse`/`UpdateProviderResponse` to `JobIdResponse` (safe: both server impls return `unimplemented` today and no client calls them yet). Remove the now-unused `InstallProviderResponse`/`UpdateProviderResponse` messages — this is the only *removal* in this task (the append-only rule applies to *fields*, not unused whole messages). Consequences: remove both names from the re-export list in `crates/tama-core/src/tamad/mod.rs` and from the `use` statements at the top of `crates/tamad/src/server.rs`.

Regeneration is automatic via `crates/tama-core/build.rs` (tonic-build over the proto). Add the new public types to the re-export list in `crates/tama-core/src/tamad/mod.rs` (follow the existing `pub use tamad_service::{...}` block).

**What NOT to change:** no server implementations in `crates/tamad/src/server.rs` — existing stubs stay; new RPCs get `unimplemented` stubs added so the `TamadService` trait impl still compiles (add the three new RPCs to `TamadServiceImpl` returning `Status::unimplemented`; `StreamJob`'s associated type follows the `Logs` pattern with `tokio_stream::Iter`).

**Steps:**
- [ ] Edit `crates/tama-core/proto/tamad.proto` per the block above.
- [ ] Update `crates/tama-core/src/tamad/mod.rs` re-exports.
- [ ] Add the new RPC stubs to `TamadServiceImpl` in `crates/tamad/src/server.rs` (all `Status::unimplemented("not implemented")`; `StreamJob` mirrors the existing `logs` associated-type pattern).
- [ ] Run `cargo build --workspace` — did it succeed? If not, fix and re-run.
- [ ] Add a compile-shape unit test in `crates/tama-core/src/tamad/mod.rs` `#[cfg(test)]` module: construct `SystemStats`, `JobEvent`, `PullModelRequest` with all fields and assert field round-trip (proves generated shapes match the plan).
- [ ] Run the full validation gate. All green? If not, fix and re-run.
- [ ] Commit with message: "feat(tamad): proto v2 — stats stream, jobs, pull/benchmark RPCs"

**Acceptance criteria:**
- [ ] `tamad.proto` contains all new messages/RPCs exactly as specified; no existing field numbers changed.
- [ ] `cargo build --workspace` green; tamad `TamadServiceImpl` compiles with the new RPC stubs.
- [ ] `InstallProvider`/`UpdateProvider` return `JobIdResponse`.
- [ ] Unit test on generated types passes.

---

### Task 2: tamad config, token, self-registration, and enforced auth (both sides)

**Context:**
Today the `token` column on `tamad_registry` is stored but enforced nowhere, and `TamadClient` never sends it. This task makes auth real in both directions and gives tamad its identity/config surface so a GPU box is deployed with three values (proxy URL, management credential, name). Design (approved spec, Section 1): tamad generates a per-tamad token on first run, persists it in `--data-dir` (stable across restarts), and self-registers at startup via an idempotent upsert keyed by name. `tamad_registry.name` is already `UNIQUE` (see `crates/tama-core/migrations/00000000000001_initial.sql`), so `ON CONFLICT (name)` works without a new migration.

**Files:**
- Modify: `crates/tamad/src/main.rs` (extend `CliArgs`)
- Create: `crates/tamad/src/state.rs` (token file handling)
- Create: `crates/tamad/src/register.rs` (self-registration client)
- Modify: `crates/tamad/src/server.rs` (token check on every RPC; pass expected token into `TamadServiceImpl`)
- Modify: `crates/tama-core/src/tamad/client.rs` (send token on every call)
- Modify: `crates/tama-core/src/db/queries/tamad_queries.rs` (add `upsert_tamad_by_name`)
- Modify: `crates/tama/src/api/tamads/register.rs` (POST becomes idempotent upsert)
- Modify: `crates/tama/src/api/tamads/manage.rs` (implement real `POST /tama/v1/tamads/:id/health`)
- Modify: `docs/api/tamads.md` (document upsert semantics + real health endpoint)

**What to implement:**

1. **`crates/tamad/src/state.rs`** — `pub struct TamadState { pub name: String, pub public_url: String, pub protocol: String, pub models_dir: PathBuf, pub data_dir: PathBuf, pub proxy_url: Option<String>, pub proxy_token: Option<String>, token: String }` with:
   - `TamadState::from_cli(args: &CliArgs) -> Result<Self>`: reads env `TAMA_URL` / `TAMA_TOKEN` (both optional; if either missing, self-registration is disabled with a `warn!` log — tamad still serves locally for manual registration). `--name` defaults to the hostname (`hostname` crate). `--public-url` defaults to `grpc://<hostname>:<port>` (or `http://` when protocol is `http`) where `<port>` is the port parsed from `--addr`. `--models-dir` defaults to `$HOME/.tama/models`, `--data-dir` to `$HOME/.tama`.
   - Token: if `<data_dir>/tamad.token` exists, read it; else generate 32 random bytes as 64-char lowercase hex (`rand`) and write it with mode 0600. Log `info!(token_path=..., "Tamad token ready (persisted)")`. Expose `pub fn token(&self) -> &str` — `Registrar` and `TamadServiceImpl` are constructed in `main.rs` with `state.token()`; do NOT make the field `pub`.

2. **`crates/tamad/src/register.rs`** — `pub struct Registrar { client: reqwest::Client, url: String, token: String, name: String, public_url: String, protocol: String, tamad_token: String }` with:
   - `async fn register_once(&self) -> Result<()>`: `POST {url}/tama/v1/tamads` with headers `Authorization: Bearer {proxy token}` and `Content-Type: application/json`, body `{"name", "url": public_url, "protocol", "token": tamad_token}`. Success = 200/201; log which (`debug!`).
   - `pub async fn run_loop(self)`: call `register_once` immediately, then every 300s in a `tokio::time::interval` loop; on error `warn!` and continue (tamad must never fail to serve because the proxy is down). Spawned from `main()` as a task only when `proxy_url`/`proxy_token` are both present.

3. **`crates/tamad/src/server.rs`** — auth enforcement: change `TamadServiceImpl` to hold `expected_token: String` (`TamadServiceImpl::new(token: String)`). Add a helper:
   ```rust
   fn check_auth<M>(request: &tonic::Request<M>, expected: &str) -> Result<(), tonic::Status>
   ```
   that reads the `authorization` metadata; accepted form is exactly `Bearer {expected}`; anything else → `Err(Status::unauthenticated("missing or invalid authorization"))`. Call it as the first line of **every** RPC impl (including `health_check`). Keep the existing HTTP `/health` endpoint unauthenticated (it is a liveness probe; it returns only status+version).

4. **`crates/tamad/src/main.rs`** — extend `CliArgs` with `--name`, `--public-url`, `--models-dir`, `--data-dir` (all optional, defaults as above); build `TamadState`; pass token to `TamadServiceImpl::new`; spawn `Registrar::run_loop` when configured; pass `state` (models_dir, data_dir) into `server::start` (extend its signature; store as `Arc<TamadState>` for later tasks' use).

5. **`crates/tama-core/src/tamad/client.rs`** — `TamadClient::new` already stores `connection.token`. Add a private helper `fn auth(&self) -> Option<http::HeaderMap>` / or more simply: a method `fn authed<T: Clone>(&self, req: T) -> tonic::Request<T>` that clones the request, sets `authorization: Bearer {token}` metadata when `connection.token.is_some()`, and returns it. Use it in every gRPC call site (`load_model`, `unload_model`, `health_check`, and any later ones). In every HTTP call site, add `.bearer_auth(token)` when present.

6. **Dependencies to add to `crates/tamad/Cargo.toml`** — none of these are currently in the tamad manifest: `reqwest` (workspace has it with the `json` feature — use `workspace = true`), `sysinfo`, `rand` (workspace deps), `hostname = "0.4"` (**explicit version — `hostname` is NOT a workspace dependency**, `workspace = true` will not compile), plus dev-deps `tempfile` and `wiremock` (match the versions `tama-core` uses). Verify each against the workspace `Cargo.toml` before choosing workspace-inherited vs explicit version.

7. **`crates/tama-core/src/db/queries/tamad_queries.rs`** — add:
   ```rust
   pub async fn upsert_tamad_by_name(
       pool: &PgPool,
       id: &str,          // candidate UUID generated in Rust: Uuid::new_v4().to_string()
       name: &str,
       url: &str,
       protocol: &str,
       token: Option<&str>,
   ) -> Result<(String, bool)>  // (id actually stored, created)
   ```
   SQL: `INSERT INTO tamad_registry (id, name, url, protocol, token) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (name) DO UPDATE SET url = EXCLUDED.url, protocol = EXCLUDED.protocol, token = EXCLUDED.token RETURNING id, (xmax = 0) AS created` — the `id` column is `TEXT`, so the UUID is generated in Rust (matching the existing `insert_tamad` flow — do NOT use `gen_random_uuid()` in SQL); on conflict the *stored* id is returned, not the candidate. "Created" is detected via `xmax = 0` (freshly inserted rows have `xmax = 0`; updated rows do not). Follow the existing query style in this file (raw `sqlx::query` + Row mapping).

8. **`crates/tama/src/api/tamads/register.rs`** — replace the insert path with `upsert_tamad_by_name` (generate the candidate UUID in Rust): return `201 Created` when `created == true`, `200 OK` otherwise; response body is the connection object (fetch via existing `get_tamad`). Keep all existing validation (empty name/url, protocol whitelist). Remove the 409 duplicate-name branch (upsert makes it unreachable).

9. **`crates/tama/src/api/tamads/manage.rs`** — `trigger_health_check`: load the tamad row (existing pattern in this file), construct `TamadClient::new(&connection)`, call `health_check().await` → respond `{"status": "online"|"offline"}` (200); on connection error respond 200 with `{"status": "offline", "error": "..."}` (unreachable ≠ server error — this mirrors how the UI should render it).

10. **`docs/api/tamads.md`** — update `POST /tama/v1/tamads` section (idempotent upsert by name; 201 vs 200; remove 409) and the `/health` section (real behavior).

**Steps:**
- [ ] Write failing tests first:
  - `crates/tamad/src/state.rs` `#[cfg(test)]`: token file created once, stable across two `TamadState::from_cli` calls with the same tempdir (use `tempfile::tempdir()` — add `tempfile` to tamad dev-deps if missing); mode is 0600 (check with `fs::metadata`).
  - `crates/tamad/src/server.rs` `#[cfg(test)]` (or `crates/tamad/tests/auth_test.rs`): spin up the tonic server on an ephemeral port with token `"secret"`; call `HealthCheck` with (a) no auth header → expect `Status::unauthenticated`; (b) `Bearer wrong` → `unauthenticated`; (c) `Bearer secret` → `ok` status + version.
  - `crates/tamad/src/register.rs` `#[cfg(test)]`: wiremock (dev-dep, pattern exists in `crates/tama-core` tests) — assert POST body fields, bearer header = proxy token; assert `run_loop` survives one 500 without panicking (test one immediate attempt + verify error is returned, not a full 300s loop — expose `register_once` for testing).
  - `crates/tama-core/src/db/queries/tamad_queries.rs`: upsert test following the existing test pattern for db queries in the workspace (check how `provider_queries`/`tamad_queries` are tested — live Postgres fixture or `sqlx` offline; mirror it exactly). Assert: first call `created == true`; second call with same name, different url/token → `created == false`, same id, url/token updated.
- [ ] Run `cargo nextest run --package tamad -- auth` and `cargo nextest run --package tama-core -- tamad_queries` — confirm failures are the expected compile/assert failures.
- [ ] Implement items 1–8 above.
- [ ] Run `cargo nextest run --package tamad && cargo nextest run --package tama-core -- tamad && cargo nextest run --package tama -- tamads` — all pass? Fix until green.
- [ ] Run the full validation gate. Green? Commit with message: "feat(tamad): config, persisted token, self-registration, and enforced bearer auth"

**Acceptance criteria:**
- [ ] tamad rejects every gRPC RPC without the exact `Bearer <token>`; `/health` HTTP stays open.
- [ ] `tamad.token` file is created once (0600) and reused across restarts (same tempdir test).
- [ ] `POST /tama/v1/tamads` is idempotent by name (201 first, 200 after, same UUID); docs updated.
- [ ] `TamadClient` sends the stored token on gRPC and HTTP calls.
- [ ] `POST /tama/v1/tamads/:id/health` returns real online/offline via the client.
- [ ] Full gate green.

---

### Task 3: tamad `StreamStats` — host stats + process table

**Context:**
The dashboard must show per-tamad host health, and the reconciler (Task 5) needs a per-tick channel carrying the process snapshot. Design: one server-streaming RPC, ~1s cadence, carrying CPU/RAM/swap/disk + per-GPU info + the full process snapshot. This task builds the stats producer and the in-memory process table that the lifecycle (Task 5) will populate. No proxy changes — tamad can be tested standalone.

**Files:**
- Create: `crates/tamad/src/process_table.rs`
- Create: `crates/tamad/src/stats.rs`
- Modify: `crates/tamad/src/server.rs` (implement `StreamStats`; thread `Arc<TamadState>` + `Arc<ProcessTable>` through the service)
- Modify: `crates/tamad/src/main.rs` (construct `ProcessTable`, pass into server)

**What to implement:**

1. **`process_table.rs`** — `#[derive(Default)] pub struct ProcessTable { inner: tokio::sync::RwLock<HashMap<String, ProcessEntry>> }` with:
   ```rust
   pub struct ProcessEntry {
       pub model_name: String,
       pub provider_name: String,
       pub pid: u32,
       pub endpoint_url: String,
       pub status: String,        // "starting" | "ready" | "failed" | "unloading"
       pub started_at: std::time::Instant,
       /// Full launch spec (the `LoadModelRequest` that started this process) —
       /// required so `RestartProvider` can re-load without proxy involvement (Task 5).
       pub spec: tama_core::tamad::LoadModelRequest,
   }
   impl ProcessTable {
       pub async fn insert(&self, entry: ProcessEntry);
       pub async fn remove(&self, model_name: &str) -> Option<ProcessEntry>;
       /// Alive-checked snapshot for the stats tick: `alive` = process::is_process_alive(pid)
       pub async fn snapshot(&self) -> Vec<tama_core::tamad::ProcessInfo>;
       pub async fn get(&self, model_name: &str) -> Option<ProcessEntry>;
       pub async fn list(&self) -> Vec<ProcessEntry>;
   }
   ```
   Use `tama_core::process::is_process_alive` for the liveness check (the module still lives in tama-core during staging; the physical move happens in Task 10 — do not move it now).

2. **`stats.rs`** — a **stateful** collector (CPU% needs two samples on the *same* `sysinfo::System` — a fresh `System::new()` per tick yields a meaningless CPU delta, which is why `crates/tama-core/src/proxy/server/metrics.rs` holds one `System` across its loop):
   ```rust
   pub struct StatsCollector {
       state: Arc<TamadState>,
       sys: sysinfo::System,          // refreshed once per tick; persists across ticks
       disks: sysinfo::Disks,         // refreshed per tick
   }
   impl StatsCollector {
       pub fn new(state: Arc<TamadState>) -> Self;   // sys: System::new_with_specifics (cpu+all memory), refresh once so the first tick has a baseline
       pub fn tick(&mut self, processes: Vec<ProcessInfo>) -> SystemStats;
   }
   ```
   - CPU/RAM/swap: refresh `self.sys` once per tick (`refresh_cpu_usage()` / `refresh_memory()` — mirror the refresh calls in `crates/tama-core/src/proxy/server/metrics.rs`) then read overall CPU + total/used memory + swap, in bytes. Swap is NOT populated by `collect_system_metrics_with` — read it explicitly from `self.sys` after `refresh_memory()`.
   - Disk: there is **no existing disk-sampling helper in tama-core** — use `sysinfo::Disks`: `self.disks.refresh()` then find the disk whose mount point is a prefix of `state.models_dir` (longest prefix wins) and read its total/available bytes. If no disk matches (shouldn't happen on a real host), fall back to `/`.
   - GPUs: call `tama_core::gpu::collect_system_metrics_with(&mut self.sys)` — it returns `SystemMetrics` (NOT `GpuDeviceStats` directly); the GPU list is `SystemMetrics.gpus: Vec<GpuDeviceStats>` (see `crates/tama-core/src/gpu/types.rs`). Using the `_with` variant reuses the persistent `System` — do NOT call the no-arg `collect_system_metrics()`, which creates a fresh `System` and sleeps, reintroducing the per-tick cost/CPU-delta problem this task fixes. Do not use `SystemMetrics`' CPU/RAM values (take those from `self.sys` directly, per the item above) — only `.gpus`. Map each `GpuDeviceStats` to proto `GpuInfo` explicitly:
     - `index` = numeric suffix parsed from `device_id` ("GPU0" → 0; if unparseable, use position in the vec);
     - `name` = `GpuDeviceStats.name`;
     - `driver_version` = `""` (GpuDeviceStats has no driver field — leave empty; the field stays for the future);
     - `vram_total_bytes`/`vram_used_bytes` = `vram.{total_mib,used_mib} * 1024 * 1024` (0 when `vram` is None);
     - `utilization_percent` = `utilization_pct as f64` (0 when None); `temperature_c` = `temperature_c as f64`; `power_w` = `power_w as f64` (0 when None).
   - `tick` = collect the above + append `processes`.

3. **`server.rs`** — implement `StreamStats` on `TamadServiceImpl` (which now also holds `Arc<TamadState>`, `Arc<ProcessTable>`, and `Arc<tokio::sync::Mutex<StatsCollector>>` — extend its constructor). The associated type must be a **nameable** stream implementing `futures_core::Stream` (that is tonic's generated bound — do NOT use `std::stream::Stream`, which does not satisfy it; `tokio_stream::Stream` is a re-export of `futures_core::Stream` and `tokio-stream` is already a tamad dep). Add `futures-util = { workspace = true }` to `crates/tamad/Cargo.toml` (workspace pins `0.3`) for `StreamExt::boxed()`:
   ```rust
   type StreamStatsStream = std::pin::Pin<Box<dyn tokio_stream::Stream<Item = std::result::Result<SystemStats, tonic::Status>> + Send>>;
   ```
   Implementation: `async_stream::stream! { loop { tokio::select! { _ = interval.tick() => yield Ok(collector.tick(table.snapshot().await)), _ = shutdown => break } } }.boxed()` (add `async-stream = "0.3"` to `crates/tamad/Cargo.toml` — already a workspace dependency). Auth check first (per Task 2 helper). The stream runs for the life of the connection — the proxy's reconnect logic (Task 4) handles re-establishment.

**Steps:**
- [ ] Unit tests in `process_table.rs`: insert/remove/get/list; `snapshot()` reflects entries (mock liveness by using a PID that exists — use `std::process::id()` for the test process so `alive == true`, and a large bogus PID e.g. `u32::MAX` for `alive == false`).
- [ ] Unit test in `stats.rs`: `StatsCollector::tick` on a machine without GPUs returns `gpus == []` and non-zero `memory_total_bytes` (guards against panics on GPU-less hosts); two consecutive ticks return plausible (non-NaN) `cpu_usage_percent` (proves the persistent-System delta works); disk fields > 0 for the models-dir filesystem.
- [ ] Integration test in `crates/tamad/tests/stats_test.rs`: start the real gRPC server (helper extracted from Task 2's auth test — factor the server bootstrap into a small `#[cfg(test)] pub mod test_support` in `crates/tamad/src/server.rs` if the two test files need it), call `StreamStats` with auth, collect 2 ticks (2s), assert: two `SystemStats` received, `memory_total_bytes > 0`, both ticks have the same `processes` length.
- [ ] Implement. Run `cargo nextest run --package tamad` — green?
- [ ] Run the full validation gate. Commit with message: "feat(tamad): StreamStats with host metrics and process snapshot"

**Acceptance criteria:**
- [ ] `StreamStats` emits ~1 snapshot/sec with CPU/RAM/swap/disk + GPUs + process list.
- [ ] `ProcessTable` snapshot marks dead PIDs `alive: false`.
- [ ] Works on a GPU-less host (empty gpus, no panic).
- [ ] Full gate green.

---

### Task 4: proxy `TamadPool` — stats streams, reconnect, status, dashboard fan-out

**Context:**
The proxy needs one persistent `StreamStats` connection per registered tamad, resilient to reconnects, with the latest snapshot available to the dashboard SSE pipeline and a live online/offline status. Design (approved spec, Section 4): `TamadPool` replaces the existing lazy `tamad_clients: Arc<RwLock<HashMap<String, TamadClient>>>` field on `ProxyState` (see `crates/tama-core/src/proxy/types.rs`). The proxy still runs its local lifecycle during this phase — the pool purely *adds* per-tamad visibility; nothing is re-routed yet.

**Files:**
- Create: `crates/tama-core/src/tamad/pool.rs`
- Modify: `crates/tama-core/src/tamad/mod.rs` (`pub mod pool;`)
- Modify: `crates/tama-core/src/proxy/types.rs` (replace `tamad_clients` field with `tamad_pool: Arc<TamadPool>`)
- Modify: `crates/tama-core/src/proxy/state/mod.rs` (construct pool from `tamad_registry` rows at startup; expose `load_tamads` refresh)
- Modify: `crates/tama-core/src/proxy/tama_handlers/system.rs` (metrics stream + gpu endpoints merge per-tamad data)
- Modify: `crates/tama/src/api/tamads/register.rs` / `manage.rs` (after create/update/delete, refresh the pool for that connection — add `TamadPool::upsert_connection`/`remove_connection` calls)
- Modify: any code referencing `state.tamad_clients` (grep `tamad_clients` — expected: only construction + unused reads; delete the old field)

**What to implement:**

```rust
// crates/tama-core/src/tamad/pool.rs
pub struct TamadPool {
    handles: tokio::sync::RwLock<HashMap<String /* tamad id */, Arc<TamadHandle>>>,
    db_pool: Arc<sqlx::PgPool>,
}

pub struct TamadHandle {
    pub connection: TamadConnection,
    client: TamadClient,
    latest: tokio::sync::RwLock<Option<LatestStats>>,   // SystemStats, ~1s fresh
    online: tokio::sync::watch::Sender<bool>,           // for status updates
    cancel: tokio::sync::watch::Sender<bool>,           // shutdown/reload signal
}

pub struct LatestStats { pub stats: SystemStats, pub at: std::time::Instant }

impl TamadPool {
    pub fn new(db_pool: Arc<PgPool>) -> Self;
    /// Load all rows from tamad_registry and start a stream task for each.
    pub async fn load_all(&self) -> Result<()>;
    pub async fn upsert_connection(&self, conn: &TamadConnection) -> Result<()>; // replace task if id exists
    pub async fn remove_connection(&self, id: &str) -> Result<()>;
    pub async fn get(&self, id: &str) -> Option<Arc<TamadHandle>>;
    pub async fn list_handles(&self) -> Vec<Arc<TamadHandle>>;
    pub async fn handle_for_provider(&self, tamad_id: Option<&str>) -> Option<Arc<TamadHandle>>;
}

impl TamadHandle {
    pub async fn client(&self) -> &TamadClient;         // borrow for one-off RPCs (Task 5)
    pub async fn latest(&self) -> Option<SystemStats>;
    pub async fn is_online(&self) -> bool;
}
```

Stream task (spawned per handle): loop { open `StreamStats` via `TamadClient` (extend `client.rs` with `pub async fn stream_stats(&self) -> Result<tonic::Streaming<SystemStats>>` for gRPC; HTTP protocol connections get no stats stream — log `debug!` and skip; the handle stays "unknown"), for each received snapshot: `latest.write().await = Some(...)`; on stream open: set online + `update_tamad_status(pool, id, "online")`; on stream error/close: set offline + `update_tamad_status(..., "offline")`, backoff sleep 1s→2s→4s… capped 30s, until `cancel` is set or the connection is reloaded }. Keep the task handle in the pool so `upsert_connection`/`remove_connection` can cancel it.

**Dashboard fan-out:** in `crates/tama-core/src/proxy/tama_handlers/system.rs`:
- `handle_system_metrics_stream`: the handler today does `serde_json::to_string(&snapshot)` on the typed `MetricsSnapshot` (defined in `crates/tama-core/src/gpu/types.rs` — it has **no** `hosts` field; do not add one, the metrics loop constructs those snapshots without pool access). Instead: in the handler, convert the snapshot to `serde_json::Value` (`serde_json::to_value(&snapshot)`), insert the new field, then yield the Value's string. Shape of the added field: `"hosts": [ { "tamad_id", "name", "online", "cpu_percent", "memory": {"total_bytes","used_bytes"}, "gpus": [...] } ]` built from `pool.list_handles()` + `latest()`. The field is additive and ignored by the old UI (frontend lands in Task 9).
- `handle_tama_system_gpu_devices`: union of local devices (existing) and per-tamad `gpus` from latest stats, each tagged with its tamad name (additive field `"tamad": Option<String>`).

**Steps:**
- [ ] Unit tests in `pool.rs` (use wiremock? no — the stats stream is gRPC; use the real tonic server pattern from `crates/tamad/tests` is cross-crate… instead: test the pool's bookkeeping with a fake: factor the stream loop behind an `async fn open_stream(client) -> Result<impl Stream<Item = SystemStats>>` indirection so a unit test can inject a `tokio_stream::repeat` of a fixed snapshot; assert latest() updates, online goes true, and after stream end → offline + backoff (use a 100ms test backoff override — make the backoff base configurable via an `Option<Duration>` field defaulting to 1s so tests don't sleep 1s).
- [ ] Modify proxy startup: `load_all()` after the DB pool is ready (find the existing startup sequence in `crates/tama-core/src/proxy/state/mod.rs` where `ProxyState` is built).
- [ ] Manual verification step: `cargo run --package tama -- serve` (dev), start a tamad with `TAMA_URL`/`TAMA_TOKEN` pointing at it, `curl -H "Authorization: Bearer $TAMA_TOKEN" $TAMA_URL/tama/v1/tamads` → expect the row `online` within a few seconds. Record the output in the commit message body.
- [ ] Run the full validation gate. Commit with message: "feat(proxy): TamadPool with per-tamad stats streams, reconnect, and dashboard fan-out"

**Acceptance criteria:**
- [ ] Pool maintains one stats stream per registered tamad; reconnects with capped backoff; `tamad_registry.status` tracks online/offline.
- [ ] `latest()` returns a snapshot < 5s old while the tamad is up.
- [ ] Dashboard SSE payload contains `hosts[]` with per-tamad cpu/memory/gpus.
- [ ] Creating/updating/deleting a tamad via the API adds/replaces/removes its stream task.
- [ ] Full gate green.

---

### Task 5: lifecycle over RPC — proxy re-routes load/unload/restart, reconciler loop

**Context:**
The core of the split (ADR-0010). Today `ensure_model_loaded` (in `crates/tama-core/src/proxy/lifecycle/mod.rs`, called from `crates/tama-core/src/proxy/handlers/chat.rs` and `forward.rs`) resolves the model, picks a backend, and spawns the process locally via `ProxyState::load_model` (which uses `proxy/lifecycle/*`, `installations/`, `process.rs`). After this task: the proxy resolves the *launch spec* from the central DB (installation config, model file path, GPU env), sends `LoadModel` to the model's provider's tamad, and tracks **desired** state in the DB; a reconciler loop comparing desired vs the per-tick process snapshot performs load/unload/crash-restart. The host code physically stays in `tama-core` during staging (see the plan's staging note) — the tamad *uses* the same lifecycle/installations/gpu/process modules that live there; Task 10 moves them.

**Files:**
- Create: `crates/tama/src/reconciler.rs` (proxy-side convergence loop)
- Modify: `crates/tama/src/main.rs` (spawn the reconciler task)
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` (`ensure_model_loaded` local path → RPC path; keep the function signature)
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/handlers.rs` (`handle_tama_load_model`, `handle_tama_unload_model`, `handle_tama_cancel_load` — these call the local `ProxyState::load_model`/`unload_model` and read local `BackendState`; they MUST be re-routed in this task or invariant 1 is violated)
- Modify: `crates/tama-core/src/proxy/types.rs` (replace process-tracking fields with desired-state tracking — see below; remove `load_model`/`unload_model`/`model_tasks`/`resolve_model_gpu_device`/`evict_lru_if_needed` from `ProxyState` once the request path no longer calls them — verify with grep first and remove what is genuinely unused)
- Modify: `crates/tama-core/src/proxy/state/mod.rs` (construct accordingly)
- Modify: `crates/tamad/src/server.rs` (implement `LoadModel`/`UnloadModel`/`RestartProvider`/`ListProviders` against the ProcessTable + lifecycle)
- Create: `crates/tamad/src/lifecycle.rs` (tamad-side spawn/health/unload wrapper over the existing tama-core lifecycle functions — see below)
- Modify: `crates/tama-core/src/proxy/remote/openai.rs` (no change expected — remote path untouched; listed so the executor verifies the `tamad_id: None` construction there still compiles)
- Modify: `docs/api/models.md`, `docs/api/sse.md` (document desired-vs-actual model state where the response shapes change)

**What to implement:**

1. **`crates/tamad/src/lifecycle.rs`** — thin orchestrator reusing tama-core's existing machinery (do NOT rewrite spawn/health logic):
   ```rust
   pub struct TamadLifecycle { pub table: Arc<ProcessTable>, pub state: Arc<TamadState> }
   impl TamadLifecycle {
       /// Spawn via the existing proxy/lifecycle spawn path generalized for tamad:
       /// build the command line from request (command + args, model_path inserted where
       /// the existing code injects it — mirror the arg handling in
       /// ProxyState::load_model in proxy/lifecycle/mod.rs), set env (request env +
       /// gpu env vars via tama_core::gpu for request.gpu_variant, e.g. CUDA_VISIBLE_DEVICES),
       /// spawn with the same process-group config as today (tama_core::process),
       /// health-poll request.health_url until success or health_timeout_ms,
       /// then insert a ProcessEntry {status: "ready"}. On timeout: entry {status: "failed"} + Err.
       pub async fn load(&self, req: &tama_core::tamad::LoadModelRequest) -> Result<tama_core::tamad::LoadModelResponse>;
       pub async fn unload(&self, model_name: &str) -> Result<()>;      // kill process group, remove entry
       pub async fn restart(&self, model_name: &str) -> Result<()>;    // unload + re-load using entry.spec (stored by `load` — see ProcessEntry in Task 3)
       pub async fn list(&self) -> Vec<tama_core::tamad::ProviderInfo>; // group table entries by provider_name; ProviderInfo.name = provider_name, loaded_models = that provider's ProcessInfo entries; engine/version/gpu_variant/status fields = empty/"unknown" (tamad has no DB — the proxy's DB is the source of truth for those; do not fabricate values)
   }
   ```
   **Key detail:** the existing `ProxyState::load_model` bakes in proxy-specific things (registry lookups, LRU eviction, JoinSet tracking). The tamad wrapper must *not* copy those — it only needs: resolve args/env for the given (command, model_path, gpu_variant), spawn, health-poll, track the PID. If `ProxyState::load_model`'s spawn internals are not reusable without the proxy registry, extract the pure spawn+health core into a free function in `crates/tama-core/src/proxy/lifecycle/` (e.g. `pub async fn spawn_and_await_health(spec: LaunchSpec, table_cb: ...) -> Result<SpawnedHandle>`), have BOTH `ProxyState::load_model` (until Task 10 deletes it) and the tamad wrapper call it. This extraction is the main refactor risk in the task — do it first, prove both callers compile, then re-route.

2. **`server.rs`** RPC impls (all auth-checked per Task 2):
   - `load_model`: `lifecycle.load(&req.into_inner())`; map errors to `tonic::Status::internal`. Duplicate `model_name` already in table with `alive == true` → return the existing `LoadModelResponse` (idempotent).
   - `unload_model`: `lifecycle.unload(&req.model_name)`; unknown model → `Status::not_found`.
   - `restart_provider`: `lifecycle.restart(&req.model_name)`; unknown → `not_found`.
   - `list_providers`: `Ok(lifecycle.list())`.

3. **Proxy desired-state + request path:**
   - `ensure_model_loaded` (`crates/tama-core/src/proxy/lifecycle/mod.rs`): replace the local spawn branch with: resolve the model's provider (existing `state.get_provider` path) → require `provider.tamad_id` to be `Some` (if a Local provider has no tamad, return a clear error: `Provider "{name}" has no tamad assigned`) → build `LoadModelRequest`:
     - `command`/`args`/env defaults from the installation config: reuse the existing resolution code that `ProxyState::load_model` uses (installation manager `get_default_args`/`get_default_env`/binary path — see `crates/tama-core/src/installations/manager.rs`; the proxy keeps its DB read access — the *central DB* stays proxy-side) plus model-specific args (spec decoding etc. — the same code that builds args today, including `reasoning_options`/spec-decode arg building);
     - `model_path` from the model config file path; `gpu_variant` from the model's GPU assignment (existing resolution, now without the "local GPU devices cache" step — gpu_variant comes from the model config / installation, not from scanning local devices);
     - `health_url` + `health_timeout_ms` from the installation config (existing `get_health_check_url`).
     Then call `pool.handle_for_provider(tamad_id)?.client()` → `load_model(req)`. Mark the model **desired-loaded** in the DB. (The `LoadModelRequest.params` field is now authoritative-ignored: `command`/`args`/`env` carry the fully resolved spec — leave `params` in the proto for wire compatibility, set it empty.)
   - **Management API handlers** (`crates/tama-core/src/proxy/tama_handlers/models/handlers.rs`): re-route all three local-spawn/kill handlers through the same machinery:
     - `handle_tama_load_model` (line ~180): resolve provider → tamad → `load_model(spec)` + `set_desired`; response reflects the RPC result (no more local `BackendState` reads for the "loaded" answer — use the RPC response / `latest()` snapshot).
     - `handle_tama_unload_model` (line ~371): `clear_desired` + `UnloadModel` RPC to the provider's tamad.
     - `handle_tama_cancel_load` (line ~223): operate on **desired state**, not local `BackendState` — `clear_desired` (if present) + `UnloadModel` RPC; "cancel" of an in-flight load = the reconciler's next tick unloads it once it appears (loads are short; document this in the response/`docs/api/models.md`).
   - **Desired-state storage:** add a table (new migration file in `crates/tama-core/migrations/`):
     ```sql
     CREATE TABLE desired_models (
         model_name  TEXT PRIMARY KEY,
         tamad_id    TEXT NOT NULL REFERENCES tamad_registry(id),
         loaded_at   BIGINT NOT NULL
     );
     ```
     + queries in a new `crates/tama-core/src/db/queries/desired_queries.rs` (`set_desired`, `clear_desired`, `list_desired`). "Mark desired-loaded" in `ensure_model_loaded` = `set_desired`; idle-timeout unload (existing idle logic — find it via `grep -rn "idle" crates/tama-core/src/proxy/lifecycle/`) = `clear_desired`.
   - **Reconciler** (`crates/tama/src/reconciler.rs`): a tokio task spawned in `main.rs` next to the other background tasks; `interval(1s)`; per online tamad handle:
     - `desired = list_desired for this tamad`; `actual = handle.latest()?.processes` (skip tick if `latest()` is `None` or > 5s old — never act on stale data).
     - For each desired model not in `actual` (or `alive == false`): re-issue `LoadModel` with the same spec builder (factor the spec builder from `ensure_model_loaded` into a shared fn), honoring the existing max-restarts limit — track restart counts in memory in the reconciler (`HashMap<model, (count, window_start)>`, reset when the model is healthy; reuse the existing max-restarts constant from the lifecycle code — `grep -rn "restart" crates/tama-core/src/proxy/lifecycle/` to find it).
     - For each actual model not desired: `UnloadModel`.
     - Log every action at `info!` with model + tamad.
   - **LRU eviction:** the existing `evict_lru_if_needed` (proxy-side, VRAM-based) — VRAM is now per-tamad; during this task, LRU eviction operates on the proxy's view of per-tamad process snapshots (each snapshot's `processes` = loaded set; VRAM from `gpus`). If the existing eviction logic reads local `gpu_devices_cache`, switch its input to the tamad's latest stats. If this proves too tangled for this task, **disable LRU auto-eviction with a `warn!` and a config flag** (leave the code) and note it in the PR — but do not ship a version where unbounded loads are possible on a GPU-box: as the minimum safe version, make `ensure_model_loaded` fail-fast when the tamad reports all GPUs ≥ 95% used and the model would need more VRAM than free.

4. **Startup reconciliation:** the reconciler's first tick does the full converge — after *proxy* restart: `desired_models` rows persist, tamad process table may be stale → actions converge. After *tamad* restart: its table is empty → desired models re-load. No extra code; verify in the test below.

**Steps:**
- [ ] Write the shared spec-builder + reconciler unit tests first:
  - Spec builder (tama-core, near `ensure_model_loaded`): given a fixture model config + installation config (in-memory, mirror existing test fixtures in `crates/tama-core/src/proxy/` tests), assert `LoadModelRequest` fields (command, model_path, env contains the gpu_variant env var, health_url).
  - Reconciler decision logic — factor the per-tick decision into a pure fn `fn decide(desired: &[String], actual: &[ProcessInfo], restarts: &HashMap<...>) -> Vec<Action>` in `crates/tama/src/reconciler.rs` and unit test: missing desired → `Load`; alive==false → `Load` (restart, bounded); actual-not-desired → `Unload`; healthy → none; stale snapshot (passed as `None`) → no actions.
  - Tamad `lifecycle.rs` test: `load` with a trivial command (`sh -c "sleep 30"`, health via a local HTTP listener or skip health with `health_timeout_ms = 0` → immediate ready) → ProcessTable entry alive; `unload` → entry gone, process dead (poll `is_process_alive`); `restart` → new pid.
- [ ] Run `cargo nextest run --package tama -- reconciler && cargo nextest run --package tamad -- lifecycle` — confirm expected failures.
- [ ] Implement in order: shared spawn/health extraction → tamad lifecycle + RPCs → spec builder → `ensure_model_loaded` re-route + desired table/migration → reconciler → idle-timeout/LRU adjustment.
- [ ] **End-to-end manual test** (document output in the commit message): start tamad (localhost) + tama (dev). Create an installation + model for a local provider assigned to the tamad (via existing APIs). `curl -X POST .../v1/chat/completions` with the model → expect: process visible in tamad's ProcessTable (`grpcurl` `ListProviders` shows it loaded; `ps` on the tamad shows the process), response streams. Kill the process → next reconciler tick respawns it (check logs). Idle it out (or `clear_desired` via the existing unload path) → process gone.
- [ ] Run the full validation gate. Commit with message: "feat: route local model lifecycle through tamad (reconciler + desired state)"

**Acceptance criteria:**
- [ ] `grep -rn "tokio::process\|Command::new" crates/tama/src crates/tama-core/src --include="*.rs" | grep -v test` shows no backend spawn left on the proxy side (host machinery still compiles in tama-core until Task 10, but nothing in the proxy request/management path calls it — the call sites are the four above: `ensure_model_loaded`, load/unload/cancel handlers).
- [ ] `ensure_model_loaded` errors clearly for a Local provider without a tamad.
- [ ] Crash recovery: killed backend process is respawned by the reconciler within ~2s, bounded by the existing max-restarts.
- [ ] Tamad restart → desired models auto-reload; proxy restart → convergence from `desired_models`.
- [ ] No VRAM regression: load fails fast when the target tamad's GPUs are full (or LRU operates on tamad stats).
- [ ] Full gate green; manual E2E transcript in the commit message.

---

## Phase 2 — Pulls on the tamad

### Task 6: `PullModel` job in tamad + proxy pull pipeline re-route

**Context:**
Model weights land on the tamad's disk, so the download must execute there. Today the pull machinery lives in `crates/tama-core/src/proxy/state/pull.rs` (PullState + in-flight downloads), `crates/tama-core/src/proxy/state/repo_pull.rs` (hf-CLI repo pull), `crates/tama-core/src/proxy/pull_queue/` (queue service + SSE events), and `crates/tama-core/src/proxy/tama_handlers/pull/` (API handlers). Design: the *queue and progress tracking stay proxy-side* (the SSE/DB pipeline is the UI contract — `docs/api/pulls.md`), but the download itself becomes a tamad job: proxy calls `PullModel`, receives a job id, opens `StreamJob`, and relays events into the existing pull progress mechanism.

**Files:**
- Create: `crates/tamad/src/jobs.rs` (generic in-memory job registry — also used by Tasks 7–8)
- Create: `crates/tamad/src/pulls.rs` (PullModel execution: GGUF download + hf CLI repo pull, reusing the existing download code in tama-core)
- Modify: `crates/tamad/src/server.rs` (implement `PullModel` + `StreamJob`)
- Modify: `crates/tama-core/src/tamad/client.rs` (`pull_model`, `stream_job` methods)
- Modify: `crates/tama-core/src/proxy/state/pull.rs` (in-flight download state → job-relay state)
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/` (start handler → dispatch to tamad; progress handler unchanged — fed by relay)
- Modify: `docs/api/pulls.md` (note execution happens on the model's tamad; error cases for offline tamad)

**What to implement:**

1. **`crates/tamad/src/jobs.rs`** —
   ```rust
   pub struct Job { pub id: String, pub kind: String, pub progress: i32, pub message: String,
                    pub status: String, pub result_json: Option<String>, pub error: Option<String> }
   pub struct JobRegistry { /* mpsc broadcast of JobEvents + latest-state map */ }
   impl JobRegistry {
       pub fn new() -> Arc<Self>;
       pub async fn start(&self, kind: &str, runner: impl Fn(JobHandle) -> BoxFuture<'static, anyhow::Result<String>> + Send + 'static) -> String;
       // runner is called in a spawned task; JobHandle { id, kind } has
       // .report(progress, message) and .succeed(result_json) / .fail(error)
       pub fn subscribe(&self, job_id: &str) -> tokio::sync::broadcast::Receiver<tama_core::tamad::JobEvent>;
       pub async fn get(&self, job_id: &str) -> Option<Job>;
       // Prune terminal jobs older than 1h on each insert (bounded memory).
   }
   ```
   `StreamJob` in `server.rs`: subscribe to the broadcast for `req.job_id`; unknown id → `Status::not_found`; stream ends when the terminal event is emitted. **Associated type:** Task 1's stub used the `tokio_stream::Iter` pattern (mirroring `Logs`) — replace it here with the same boxed-stream pattern as `StreamStats` (Task 3): `type StreamJobStream = std::pin::Pin<Box<dyn tokio_stream::Stream<Item = std::result::Result<JobEvent, tonic::Status>> + Send>>`, built by wrapping the `broadcast::Receiver` in `tokio_stream::wrappers::BroadcastStream` (use a generous broadcast capacity, e.g. 256, so `Lagged` is effectively impossible for low-rate job progress) and mapping receive errors (`Lagged`/`Closed`) to `Status::internal` and ending the stream — a stream that ends before a terminal event means the job is broken; the proxy relay (item 3) already marks such jobs failed.

2. **`crates/tamad/src/pulls.rs`** — `pub async fn run_pull(req: &PullModelRequest, models_dir: &Path, hf_token: &str, handle: JobHandle) -> Result<String>`:
   - **GGUF path:** the chunked downloader is `pull_chunked_with_progress` in `crates/tama-core/src/models/pull/mod.rs` (it is `pub` — callable from tamad). The HF helpers it needs — `hf_resolve_url`, `hf_auth_headers`, `get_hf_token` (same file, lines ~429–460) — are `pub(crate)` and therefore **not** callable from the tamad crate. Fix visibility in this task: make `hf_resolve_url` `pub`, and add a `pub fn hf_auth_headers_with_token(token: &str) -> HeaderMap` alongside the existing `hf_auth_headers()` (the old one, which reads the proxy's local config, stays for the proxy's HF metadata endpoints). The downloader takes a `HeaderMap`, not a token — pass `hf_auth_headers_with_token(req.hf_token)`. Map the downloader's progress callback to `handle.report(percent, message)`.
   - **Post-pull pipeline split (the subtle part):** today `crates/tama-core/src/proxy/tama_handlers/pull/start.rs` (+ `verify.rs`) runs, in order: download → `run_verification` (SHA-256 hash of the **local** file, compare, delete on fail) → GGUF/transformers metadata parse (**reads the downloaded file's headers**) → `setup_model_after_pull` (writes the model TOML card to the proxy's `configs_dir` + `model_configs`/`model_files` DB writes). Classify each step: **disk-side** (hashing, header parsing — operates on files that now live on the tamad) **moves into `run_pull`**; **registry-side** (TOML card, DB rows) **stays in the proxy** and is fed by the enriched result. Concretely, `run_pull` returns result JSON: `{"files": [{"path", "size", "sha256", "verified": bool, "is_primary_shard": bool}], "gguf_metadata": {...}|null, "transformers_metadata": {...}|null, "dir": "<models_dir>/<org>/<repo>"}`. Types: reuse `GgufMetadata` from `tama_core::models::pull` — it derives only `Debug, Clone, Default` today, so **derive `Serialize` on it** (add the derive; check `crates/tama-core/src/models/gguf.rs` for the struct definition) — and `TransformersMetadata` from `tama_core::models::transformers` (NOT `models::pull` — that's where `verify.rs` imports it from); if `TransformersMetadata` lacks `Serialize`, derive it there too.
   - **Repo-pull path** (`req.repo_pull == true`): reuse the existing hf-CLI repo-pull code (`crates/tama-core/src/proxy/state/repo_pull.rs`) as a tracked subprocess with progress → `handle.report`; result JSON: `{"dir": "...", "ok": true}`.
   - HF token: used only to build headers / `HF_TOKEN` env for the hf CLI; **never log it**.
   - Target dir: `{models_dir}/{org}/{repo}` (same layout as today — verify against the existing path builder in the pull code).
   - **Proxy-side relay (item 3) persists registry rows from `result_json`:** the existing DB write code for `model_files` (with hashes from `VerificationOutcome`) and the model-card/config creation keep running in the proxy, now reading values from the job's terminal `result_json` instead of scanning local disk. Do NOT delete the `setup_model_after_pull`/`run_verification` code in this task — refactor it so the disk-touching halves are extractable (they move to tamad in Task 10 with the rest); in this task the proxy simply stops *calling* the disk halves for tamad-routed pulls.

3. **Proxy re-route** — in the pull start handler (`crates/tama-core/src/proxy/tama_handlers/pull/start.rs` — the current in-flight download orchestrator) and the queue worker that dispatches it: after enqueueing (queue service unchanged), when a pull is dequeued for execution, resolve the model's provider → tamad → `client.pull_model(req)` → spawn a relay task: subscribe `stream_job(job_id)`; on each `JobEvent`: update the pull queue progress row + emit the existing `PullEvent` SSE (same shapes as today — the UI must not change); on terminal event: persist `model_files` rows (hashes from result JSON) + run the existing model-card/config creation from the enriched result (per item 2), mark queue item succeeded/failed.
   - Tamad offline/unreachable at dispatch → queue item `failed` with error `"tamad {name} unreachable"` (retryable per existing queue retry semantics if any — check `pull_queue/service.rs`).
   - Job stream drops before terminal (tamad died) → mark failed `"tamad disconnected mid-pull"`.

**Steps:**
- [ ] Unit tests: `jobs.rs` — start/report/succeed/fail lifecycle, broadcast receives ordered events, unknown id → not_found (via a small tonic test server reusing Task 3's test support); prune behavior.
- [ ] Tamad `pulls.rs` test: `run_pull` for a repo_pull against a local fake `hf` binary (tempdir on PATH — the existing repo_pull tests may already do this; reuse the pattern) → result JSON correct; progress events observed. For the GGUF path: test with a tiny local HTTP file server (wiremock) serving a fake GGUF — `pull_chunked_with_progress` against it, assert verification hash in result JSON matches a precomputed SHA-256.
- [ ] Proxy relay test (wiremock-style is not possible for gRPC — use a fake gRPC server in `crates/tama-core/src/tamad/pool.rs` tests or a `#[cfg(test)]` mock `TamadServiceServer`): assert pull queue progress rows update from JobEvents and terminal event writes the expected DB rows.
- [ ] Implement in order: jobs.rs → StreamJob → pulls.rs → client methods → proxy dispatch re-route.
- [ ] Manual E2E (transcript in commit message): pull a small GGUF model through the UI/API with a local tamad → progress visible in the existing SSE, files appear in the tamad's models dir, `model_files` rows in Postgres, model loadable afterward (Task 5 path).
- [ ] Run the full validation gate. Commit with message: "feat: execute model pulls on the tamad (PullModel job + StreamJob relay)"

**Acceptance criteria:**
- [ ] No download bytes flow through the proxy process (the only HF requests the proxy still makes are the wizard's metadata endpoints, which stay proxy-side per the approved spec).
- [ ] Progress + terminal state reach the existing UI unchanged.
- [ ] Tamad offline → clean job failure with actionable error.
- [ ] `hf` token is never logged.
- [ ] Full gate green.

---

## Phase 3 — Installs/upgrades, benchmarks, system endpoints

### Task 7: install / update / remove as tamad jobs + proxy re-route

**Context:**
Backend binaries (llama.cpp builds, kokoro, etc.) are installed/upgraded *on the tamad's host*. Today the installation manager (`crates/tama-core/src/installations/manager.rs`) tracks configs in the central DB, and the actual install/update execution happens proxy-side (trace from `crates/tama/src/api/installations/` — the job manager is `crates/tama/src/web_types.rs: JobManager`, jobs API in `crates/tama/src/api/installations/jobs.rs`, `docs/api/jobs.md`). After this task: install/update execution moves to tamad jobs (`InstallProvider`/`UpdateProvider` now return `JobIdResponse` per Task 1); the proxy keeps the DB as system of record and bridges `StreamJob` events into the existing `JobManager` so the jobs API + UI stay unchanged.

**Files:**
- Create: `crates/tamad/src/installs.rs` (install/update/remove execution reusing `crates/tama-core/src/installations/` installer/updater code, rooted at the tamad's install dir under `--data-dir`)
- Modify: `crates/tamad/src/server.rs` (`InstallProvider`, `UpdateProvider`, `RemoveProvider` real impls)
- Modify: `crates/tama-core/src/tamad/client.rs` (`install_provider`, `update_provider`, `remove_provider`, reusing `stream_job`)
- Modify: `crates/tama/src/api/installations/` (start handlers → dispatch to tamad + JobManager bridge; remove local execution)
- Modify: `docs/api/installations.md` (note execution host = the provider's tamad)

**What to implement:**
1. `crates/tamad/src/installs.rs`: `pub async fn run_install(req: &InstallProviderRequest, state: &TamadState, handle: JobHandle) -> Result<String>` — call the existing installer entry points in `tama_core::installations` (find the fn the proxy's install handler calls today — `grep -rn "install" crates/tama/src/api/installations/`), rooted at `state.data_dir.join("install")` instead of the proxy's install dir; stream installer output lines → `handle.report(0, line)` (progress unknown → message-only); result JSON `{"installed": true, "version": "...", "path": "..."}`. Same shape for `run_update`. `remove_provider`: kill any processes for that provider via the ProcessTable (Task 5) then delete the install dir entries.
   - **Install dir location:** the existing installer code has a hardcoded/configured root — parameterize it (pass the root path in) rather than duplicating. If the installer fns are entangled with proxy state, extract the pure parts (as in Task 5's spawn extraction).
2. `server.rs`: `install_provider`/`update_provider` → `jobs.start("install"/"update", ...)` returning `JobIdResponse { job_id }`; `remove_provider` → synchronous (auth-checked) remove, `Empty` response.
3. Proxy: the install start handler resolves the provider's tamad → `client.install_provider(...)` → create the proxy-side `JobManager` entry (existing) → bridge task: `stream_job` events → `JobManager` progress/log updates (same fields the current local execution writes — find them via the existing handler); terminal → job succeeded/failed + persist installation config rows (existing DB writes, fed by result JSON).

**Steps:**
- [ ] Unit test: tamad `installs.rs` with a fake installer (inject the installer fn as a trait/closure param so tests use a stub that writes a marker file) → job lifecycle + result JSON; remove kills a fake ProcessTable entry.
- [ ] Proxy bridge test with the mock `TamadServiceServer` pattern from Task 6: JobManager receives progress + terminal.
- [ ] Implement; manual E2E: install a backend through the UI against a local tamad (marker binary is fine for a smoke test if the real build is too slow — but the real installer must be the code path; the marker is only to keep the E2E fast, document which).
- [ ] Full gate. Commit: "feat: execute backend installs/upgrades as tamad jobs"

**Acceptance criteria:**
- [ ] Install/update/remove execute on the tamad host; central DB rows (installation configs/versions) still written by the proxy only.
- [ ] Jobs API (`GET /tama/v1/backends/jobs/:id`) shows the same progress/log UX as before.
- [ ] Tamad offline → job fails with actionable error.
- [ ] Full gate green.

---

### Task 8: benchmarks on the tamad (`RunBenchmark` job)

**Context:**
Benchmarks (llama-bench, spec, MTP) measure *tamad* hardware, so they must run there. Today the bench runners live in `crates/tama-core/src/bench/` and are invoked from `crates/tama/src/api/benchmarks/` (run.rs, spec.rs, mtp.rs, suite.rs — suite logic per ADR-0004: one sequential job). The proxy already persists benchmark history in Postgres. After this task: runners execute in tamad as a job; the proxy dispatches, relays `StreamJob`, and persists results exactly as it does today.

**Files:**
- Create: `crates/tamad/src/bench.rs` (wrapper: runs `tama_core::bench` runners, maps output to job events + result JSON)
- Modify: `crates/tamad/src/server.rs` (`RunBenchmark` impl)
- Modify: `crates/tama-core/src/tamad/client.rs` (`run_benchmark`)
- Modify: `crates/tama/src/api/benchmarks/run.rs`, `suite.rs`, `spec.rs`, `mtp.rs` (dispatch to tamad + relay; keep suite selection logic proxy-side)
- Modify: `docs/api/benchmarks.md` (execution host note)

**What to implement:**
0. **Proto extension (append-only, do this first):** add to `RunBenchmarkRequest` in `crates/tama-core/proto/tamad.proto`: `string model_path_rel = 4; string binary_path_rel = 5;` — both **relative to the tamad's own roots** (models dir / install dir respectively), because the proxy does not know the tamad's `--models-dir`/`--data-dir`. Rebuild and update re-exports as in Task 1.
1. **`crates/tamad/src/bench.rs`**: `run_benchmark(req: &RunBenchmarkRequest, state: &TamadState, handle: JobHandle) -> Result<String>`:
   - **`llama_bench` kind — DB-free core extraction (required, not optional):** `run_llama_bench` in `crates/tama-core/src/bench/llama_bench/mod.rs:76` takes `&Config` **and `&sqlx::PgPool`** and resolves model paths/quant/backend binary from the database — tamad has no DB (invariant 2), so it is **not callable as-is**. Extract a pure core: `pub async fn run_llama_bench_resolved(model_path: &Path, binary_path: &Path, gpu_variant: Option<GpuVariant>, bench_config: &LlamaBenchConfig, progress: &dyn ProgressSink) -> Result<BenchReport>` containing everything after the DB lookups (the actual subprocess execution + report assembly). Refactor the existing `run_llama_bench` to do its DB lookups and then delegate to the core (no behavior change — existing tests must pass unchanged). The tamad calls the core directly.
   - **`spec` / `mtp` kinds:** `run_spec_bench` (`bench/llama_cli_spec/mod.rs:528`) and `run_mtp_bench` (`bench/llama_cli_mtp/mod.rs:312`) are already DB-free (`config: &SpecBenchConfig/MtpBenchConfig` with `model_path` inside, `binary_override: Option<PathBuf>`, progress sink) — call them directly.
   - Path resolution in tamad: `model_path = state.models_dir.join(req.model_path_rel)`, `binary_path = state.data_dir.join("install").join(req.binary_path_rel)`; deserialize `config_json` into the per-kind config and **overwrite its `model_path` with the resolved tamad path** (the proxy-serialized config may contain proxy-side paths). If the model file or binary does not exist on this host → `handle.fail("model/binary not found on this host: <path>")`.
   - Progress → `handle.report` (coarse phase messages are fine); result JSON = the serialized report struct (same struct the proxy persists today — `grep -rn "report" crates/tama/src/api/benchmarks/run.rs` to find the exact type).
2. Proxy dispatch: each benchmark entry point resolves the model's provider → tamad, computes `model_path_rel`/`binary_path_rel` (relative to the tamad roots, using the same path conventions the lifecycle spec-builder uses — the proxy has the DB), → `run_benchmark` → `StreamJob` relay → on success, persist history rows exactly as the current code does (same tables/cols — the persistence code stays, only the *execution* moves). Suite (ADR-0004): the proxy creates one suite job in the JobManager and issues N sequential `RunBenchmark` calls to the tamad, bridging each; `suite_id` linking unchanged.

**Steps:**
- [ ] Tamad unit test: test the *wiring* with a fake runner fn (injectable, like Task 7) → result JSON round-trips to the persistence struct; missing model/binary path → fail event. Plus: unit test the extracted `run_llama_bench_resolved` core against a fake `llama-bench` binary (tempdir on PATH, echoing a minimal valid report) — proves the core executes without a DB.
- [ ] Proxy test: suite dispatch order + history rows with the mock server.
- [ ] Manual E2E: one real `llama_bench` run on a local tamad (small model, short config) → history row appears with numbers, UI unchanged.
- [ ] Full gate. Commit: "feat: run benchmarks on the tamad (RunBenchmark job)"

**Acceptance criteria:**
- [ ] Benchmark processes spawn on the tamad host (verify via `ps` during the E2E).
- [ ] History rows identical in shape to pre-change runs (same tables/columns).
- [ ] Suite semantics (one sequential job, shared suite_id) preserved.
- [ ] Full gate green.

---

### Task 9: system/GPU endpoints aggregated per tamad + dashboard host sections

**Context:**
With lifecycle/pulls/bench on tamads, the proxy's local system introspection is dead weight *except* as a template: the system endpoints (`handle_tama_system_health`, `handle_tama_system_gpu_devices`, `handle_system_metrics_stream` in `crates/tama-core/src/proxy/tama_handlers/system.rs`) and the dashboard must now present **per-tamad host sections**. Task 4 already added the `hosts[]` SSE field; this task completes the endpoints and the UI.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/system.rs` (health endpoint → include per-tamad health from pool; gpu devices → per-tamd union with `"tamad"` tag; capabilities/system-info endpoints — read `crates/tama/src/api/` for the system routes and `docs/api/system.md` — to enumerate exactly which endpoints report host facts and re-point each to pool data)
- Modify: SvelteKit frontend (find the dashboard system panel + system page via `ls crates/tama/src/pages/`) — render one host card per tamad (name, online, cpu, ram, gpus with VRAM/util/temp) + keep a proxy-local card for the proxy process itself (its own uptime/version only — no hardware)
- Modify: `docs/api/system.md`, `docs/api/sse.md` (document `hosts[]` + per-tamd tags)

**What to implement:**
- `GET /tama/v1/system/health` (or the actual path — verify in router): response gains `"hosts": [{tamad_id, name, online, version (from HealthCheck cached in the pool — extend Task 4's pool to cache the last HealthCheck result), cpu_percent, memory_used_pct, gpus_online: n}]`; the top-level proxy `status: "ok"` stays.
- GPU devices endpoint: merge per-tamd `GpuInfo` from latest stats (tagged `"tamad": name`); the refresh endpoint (`handle_tama_system_gpu_devices_refresh`) triggers a fresh `ListProviders`/stats fetch instead of local rescans — local rescans removed.
- Frontend: per-tamd host cards on the dashboard system section (reuse existing stat card components; data from the SSE `hosts[]`); system page lists tamads with their GPUs. No new backend surface beyond the fields above.
- Remove now-dead local GPU sampling calls from these handlers (the `tama_core::gpu::discover`-based local scans). Do NOT delete the `gpu` module itself (Task 10 does that).

**Steps:**
- [ ] API tests: with a mock tamad (wiremock for HTTP tamad or the tonic mock for gRPC) the endpoints return the per-tamd fields; with zero tamads the responses match pre-change shape exactly (back-compat — `hosts: []`).
- [ ] Frontend: build passes (`make dev` check or the trunk/cargo build the frontend uses — follow `crates/tama`'s build setup), screenshots optional.
- [ ] Manual: dashboard shows a live host card for a local tamad (cpu/gpu numbers ticking).
- [ ] Full gate. Commit: "feat: per-tamd system endpoints and dashboard host sections"

**Acceptance criteria:**
- [ ] All system endpoints that report host facts report per-tamd facts (verify by reading `docs/api/system.md` against the router — zero host-fact endpoints still reading local hardware).
- [ ] Dashboard renders one card per tamad with live stats.
- [ ] Zero-tamd deployments: all endpoints return the legacy shape (back-compat).
- [ ] Full gate green.

---

## Phase 4 — Deletion: enforce the boundary by the dependency graph

### Task 10: physically move host modules into the tamad crate; delete proxy local machinery

**Context:**
Staging note (top of plan): through Tasks 5–9 the host machinery physically lived in `tama-core` so both binaries could link it while the proxy's *usage* was re-routed. This final task makes ADR-0010 structural: the modules move into `crates/tamad/src/`, and `tama-core` (and therefore the `tama` binary) no longer contains any code that can spawn a backend, download weights, benchmark, or sample host GPUs. After this task, "proxy spawns nothing" is enforced by the compiler.

**Files:**
- Move (from `crates/tama-core/src/` to `crates/tamad/src/`): `process.rs`, `platform/`, `installations/`, `compaction_server/`, `gpu/`, `bench/`, `proxy/lifecycle/` (including `traits.rs`, `idle_timeout.rs`, `compaction.rs`, `tts.rs`), `proxy/pull_queue/` (the *download execution* parts — the queue service/DB/SSE event *types* used by the proxy's tracking stay in tama-core; split carefully: `pull_queue/events.rs` + `service.rs` + `recovery.rs` stay; the actual downloader + `state/pull.rs` in-flight download engine + `state/repo_pull.rs` move), `updates/` + `self_update.rs` (backend self-update — the *proxy binary's* self-update stays; only the backend-upgrade execution moves, which Task 7 already routes through tamad; delete what's left unused)
- Modify: `crates/tama-core/src/lib.rs` (remove `pub mod` entries for moved modules)
- Modify: `crates/tama-core/Cargo.toml` (remove deps now unused: check with `cargo udeps` or by build warnings — e.g. possibly `hf`-related, `tokio` features)
- Modify: `crates/tama/src/main.rs`, `crates/tama-core/src/proxy/state/mod.rs`, `crates/tama-core/src/proxy/types.rs` (remove leftover local-lifecycle fields: `backend_logs`, `gpu_devices_cache`, `model_tasks`, anything grep finds referencing moved modules)
- Modify: `crates/tama/tests/` (migrate any test that referenced moved modules; `router_ownership_test.rs`, `migrate.rs` reference tamad — check they still express the new invariants)
- Modify: `CONTEXT.md` (verify "Backend lifecycle" / "Pull" definitions match the final state — they were updated for this design; adjust if the split changed wording), `docs/api/*.md` (final pass: every doc mentioning local execution updated), `docs/adr/0010-proxy-spawns-nothing.md` (add "Status: accepted — enforced by dependency graph as of this commit" if the format allows, or leave as-is)

**What to implement:**
1. Move modules one at a time (commit-per-move is fine *within* this task's branch as long as the task ends green — but prefer one final commit with the whole move for bisectability; choose one, state it in the commit body). For each moved module: fix `crate::` → `tama_core::` paths where the code still references shared types (provider types, db queries, logging), and `use` imports in the tamad crate. The moved code's *public API* should be unchanged so existing tamad call sites (Tasks 5–9) keep compiling.
2. `proxy/lifecycle/mod.rs` is the trickiest: after Task 5 the proxy request path calls it only for *spec resolution* (args/env/health_url building) — that resolution stays in tama-core (it reads the central DB via the installations manager). Move the *process-spawn half* into tamad; keep a slim `lifecycle` module in tama-core for spec resolution + `ensure_model_loaded` (now an RPC caller). Rename for clarity: `crates/tama-core/src/proxy/lifecycle/mod.rs` keeps only resolution+dispatch; spawn/health/core becomes `crates/tama-core/src/proxy/lifecycle/` → deleted, its core lives in `crates/tamad/src/lifecycle.rs` (already created in Task 5).
3. **Grep sweep** (document results in the commit message):
   ```bash
   grep -rn "spawn\|tokio::process\|Command::new" crates/tama/src crates/tama-core/src --include="*.rs" | grep -v test
   grep -rn "sysinfo\|nvidia-smi\|rocm-smi" crates/tama/src crates/tama-core/src --include="*.rs" | grep -v test
   ```
   Every remaining hit must be justified (e.g. proxy binary's own self-update, langfuse) — list them in the commit body.
4. Update `crates/tama-core/src/lib.rs` module list + run the workspace to surface every dangling reference.

**Steps:**
- [x] Move modules; fix compile errors; `cargo build --workspace` green.
- [x] Run the grep sweep; record justified exceptions in the commit message.
- [x] `cargo nextest run --workspace` — the full suite, including the `tama` integration tests, must pass unchanged in *behavior* (adjust only test files that referenced moved internal paths).
- [x] Update CONTEXT.md + docs; re-read `docs/api/tamads.md` end-to-end for consistency.
- [x] Full validation gate. Commit with message: "refactor: move all host machinery into the tamad crate (ADR-0010 enforced by dependency graph)"

**Acceptance criteria:**
- [x] `crates/tama-core` contains no code that spawns backend processes, downloads model files, runs benchmarks, or samples local GPUs (grep sweep evidence in commit message).
- [x] `crates/tama` builds and the full test suite passes with zero test *behavior* changes.
- [x] `cargo udeps`-style check: no orphaned dependencies in tama-core (or documented reason).
- [x] Docs + CONTEXT.md consistent with the final architecture.
- [x] Full gate green.

---

## Sequencing & risk notes

- **Task 5 is the critical path and the largest.** Its spawn/health extraction (shared core used by both the old proxy path and the new tamad path) is the main refactor risk — if the extraction proves invasive beyond the plan's estimate, split it: first commit extracts the pure core (no behavior change), second commit re-routes.
- Tasks 6–9 are independent of each other once Task 5 lands (they all reuse `jobs.rs`/`StreamJob` from Task 6/7 — **do Task 6 before 7 and 8**, or pull `jobs.rs` out into its own first sub-step of Task 6's implementation; Tasks 7/8 depend on it).
- Each phase ends with a working system: after Phase 1, local models run via a local tamad end-to-end; after Phase 2, pulls work; after Phase 3, installs/benchmarks work; Phase 4 is pure cleanup.
- Frontend work is confined to Task 9 (plus no UI changes in Tasks 4–8 — the SSE/API shapes are additive only).
