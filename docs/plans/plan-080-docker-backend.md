# Docker Backend Plan

**Goal:** Add support for running inference backends inside Docker containers (e.g., `stilldeadcode/vllm-radiance`), with auto-pull, health checks, log streaming, and full lifecycle integration.

**Architecture:** New `BackendType::Docker` variant with separate `DockerConfig` struct (ADR-0006) stored in a nullable `docker_config` JSON column on `backend_installations`. Docker-specific logic isolated in `backends/docker/` module. Lifecycle (`load_model`, `unload_model`, idle_timeout, force-unload) branches on `is_docker: bool` in `BackendState`. Containers use deterministic naming (`tama-{backend_name}`) with label-based startup reconciliation.

**Tech Stack:** Rust, tokio, SQLite (rusqlite), Docker CLI subprocess. Linux-only (macOS/Windows Docker Desktop out of scope for v1).

**Platform scope:** Linux hosts with native docker daemon. PID-liveness checks rely on host-visible container init processes.

---

### Task 1: Data Model — Types, BackendState, DB Migration, Backup/Restore

**Context:** Establish the foundation types and database schema for docker backends. DockerConfig is separate from BackendSource (ADR-0006) because they answer different questions: "how was this obtained?" vs "what runtime does it use?". BackendState gains `is_docker: bool` so all kill/detect sites can branch without DB lookup.

**Files:**
- Modify: `crates/tama-core/src/backends/types.rs` (BackendType::Docker variant + plumbing, BackendInfo gains `#[serde(default)] pub docker_config: Option<DockerConfig>`)
- Modify: `crates/tama-core/src/backends/mod.rs` (`pub mod docker;` + re-export `pub use docker::{DockerConfig, DockerVolume};`)
- Create: `crates/tama-core/src/backends/docker/mod.rs` (DockerConfig, DockerVolume structs)
- Modify: `crates/tama-core/src/proxy/types.rs` (BackendState variants add `is_docker: bool`)
- Create: `crates/tama-core/src/db/migrations/_0043_add_docker_config.rs` (ALTER TABLE backend_installations ADD COLUMN docker_config TEXT DEFAULT NULL)
- Modify: `crates/tama-core/src/db/migrations.rs` (add `mod _0043_add_docker_config;`, bump `LATEST_VERSION: i32 = 42` → `43`, add to `MIGRATIONS` array)
- Modify: `crates/tama-core/src/db/queries/backend_queries.rs` (BackendInstallationRecord gains `pub docker_config: Option<String>`, add `docker_config` to all SELECT column lists and INSERT, extend map_backend_record with `row.get(9)?`)
- Modify: `crates/tama-core/src/backends/manager.rs` (record↔info conversion for docker_config, update_version constructs BackendInfo with docker_config field)
- Modify: `crates/tama-core/src/backup/archive.rs` (add `docker_config` to backup query at line ~189)
- Modify: `crates/tama-core/src/backup/manifest.rs` (`BackendEntry.source` becomes `Option<String>`, add `#[serde(default)] pub docker_config: Option<String>`)
- Modify: `crates/tama-core/src/backup/merge.rs` (`merge_database`: detect `docker_config` via `pragma_table_info` on backup_db.backend_installations, select `bf.docker_config` or `NULL` accordingly — same pattern as model_files column detection at line ~186)
- Modify: `crates/tama/Cargo.toml` (no changes needed — tama crate already depends on tama-core)
- Modify: `crates/tama/src/api/backends/manage/remove.rs` (add `docker_config` field to literal BackendInfo construction)

**What to implement:**

1. **BackendType::Docker** in `backends/types.rs`:
   - Add `Docker` variant to enum
   - `Display` → `"docker"` (strum derive handles if already derived; otherwise add match arm)
   - `FromStr` parses `"docker"` (add match arm)
   - `is_non_inference_backend()` → `false` for Docker
   - `default_git_url()` → fallback "never reached" string (same pattern as Custom at line 67-69)
   - `EnumIs` auto-derives `is_docker()` if strum is configured; otherwise add manual method
   - Update ALL exhaustive `match` sites on BackendType in the codebase (compiler will surface these — fix with Docker arm)

2. **DockerConfig + DockerVolume** in `backends/docker/mod.rs`:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct DockerConfig {
       pub image: String,                // "stilldeadcode/vllm-radiance:0.5.8"
       #[serde(default = "default_container_port")]
       pub container_port: u16,          // default 8000
       pub model_mount: DockerVolume,
       #[serde(default)]
       pub volumes: Vec<DockerVolume>,
       #[serde(default)]
       pub devices: Vec<String>,
       #[serde(default)]
       pub gpus: Option<String>,         // "--gpus all" or CDI "nvidia.com/gpu=0"
       #[serde(default)]
       pub shm_size: Option<String>,
       #[serde(default)]
       pub cap_adds: Vec<String>,
       #[serde(default)]
       pub security_opts: Vec<String>,
       #[serde(default)]
       pub group_adds: Vec<String>,
   }

   fn default_container_port() -> u16 { 8000 }

   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct DockerVolume {
       pub host_path: String,        // "{{MODEL_DIR}}" or absolute path
       pub container_path: String,   // "/models" (must be absolute)
       #[serde(default)]
       pub read_only: bool,          // default false
   }
   ```
   - Derive Debug, Clone, Serialize, Deserialize
   - Validation function `DockerConfig::validate(&self) -> Result<()>`:
     - image non-empty; if contains `@`, validate as digest ref; if contains `:`, validate tag after last `/`; tagless images accepted (implicit :latest)
     - container_port 1-65535
     - model_mount.container_path must start with `/`
     - volumes[].container_path must start with `/`
     - Cross-field: `backend_type="docker"` requires non-null docker_config; non-docker types reject non-null docker_config (validation called from API layer, not struct itself)

3. **BackendState changes** in `proxy/types.rs`:
   - Add `is_docker: bool` field to `Starting`, `Ready`, and `Unloading` variants
   - Default `false` for all existing code paths (backward compatible)
   - Update all construction sites to include `is_docker: false` (compiler will surface)

4. **DB migration** — scan `db/migrations/` for highest number, create `_00XX_add_docker_config.rs`:
   - `ALTER TABLE backend_installations ADD COLUMN docker_config TEXT DEFAULT NULL`
   - Verify `source` column is already nullable (it is — `TEXT` without NOT NULL per _0003)
   - Register in `migrations.rs`

5. **Backup** in `backup/archive.rs`:
   - Backup query (line ~189): add `docker_config` to `SELECT name, version, backend_type, source, docker_config FROM backend_installations...`

6. **Manifest** in `backup/manifest.rs`:
   - `BackendEntry.source`: change from `String` to `Option<String>` (docker backends have source=NULL; rusqlite fails reading NULL into non-optional String)
   - Add `#[serde(default)] pub docker_config: Option<String>` to BackendEntry
   - Serde: both fields optional with default — backward compatible for old manifests

7. **Restore merge** in `backup/merge.rs`:
   - `merge_database`: add `docker_config` to the INSERT column list
   - Detect whether backup has docker_config via `backup_db.pragma_table_info('backend_installations')` — same pattern as model_files detection at line ~186
   - If present: select `bf.docker_config`; if absent: select `NULL`
   - This prevents "no such column" on old backups and preserves docker_config on new backups

**Steps:**
- [ ] Write failing test for DockerConfig serde round-trip in `backends/docker/mod.rs` #[cfg(test)]
  - Test: serialize → deserialize matches original, defaults applied (container_port=8000, empty vectors)
- [ ] Run `cargo nextest run --package tama-core -- docker::mod`
  - Did it fail? If not, investigate why.
- [ ] Implement DockerConfig + DockerVolume structs with serde derives and default_container_port()
- [ ] Write validation tests: empty image → error, non-absolute container_path → error, valid config → ok
- [ ] Run `cargo nextest run --package tama-core -- docker::mod`
  - Did all tests pass? If not, fix and re-run.
- [ ] Add BackendType::Docker variant with Display/FromStr/is_non_inference_backend/default_git_url arms
- [ ] Fix all exhaustive match sites on BackendType (compiler errors) — add Docker arm
- [ ] Write test for BackendType::Docker round-trip (Display → FromStr → Display)
- [ ] Run `cargo nextest run --package tama-core -- backends::types`
  - Did all tests pass?
- [ ] Add is_docker: bool to BackendState variants; fix all construction sites (compiler errors)
- [ ] Create DB migration file; register in migrations.rs
- [ ] Write migration test in `db/migrations/migrations_tests.rs`
- [ ] Run `cargo nextest run --package tama-core -- migrations`
- [ ] Update backup query, BackendEntry struct, manifest serde, restore CREATE TABLE + INSERT
- [ ] Write backup/restore round-trip test with docker_config: create docker backend → backup → restore → verify docker_config survives
- [ ] Also test restoring old-format backup (no docker_config field) → docker_config is None
- [ ] Run `cargo nextest run --package tama-core -- backup`
- [ ] Update BackendInfo to include docker_config; update record↔info conversion in manager.rs
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --package tama-core`
- [ ] Commit: `feat: add Docker backend data model, migration, and backup support`

**Acceptance criteria:**
- [ ] BackendType::Docker parses from "docker", displays as "docker", is_non_inference_backend() = false
- [ ] DockerConfig serializes/deserializes correctly with all defaults
- [ ] DockerConfig validation rejects invalid configs (empty image, non-absolute paths)
- [ ] BackendState variants have is_docker field, all construction sites compile
- [ ] DB migration adds docker_config column; migration test passes
- [ ] Backup includes docker_config; restore rebuilds it; old backups restore with docker_config=None
- [ ] All existing tests pass (no regressions)

---

### Task 2: Docker Module — Image Management + Fake CLI Mock

**Context:** Implement the image management functions (pull, inspect, availability check) and the fake docker CLI for testing. This is testable independently of the runner/lifecycle by using the mock.

**Files:**
- Create: `crates/tama-core/src/backends/docker/image.rs`
- Modify: `crates/tama-core/src/backends/docker/mod.rs` (export image module)
- Modify: `crates/tama-core/Cargo.toml` (add `tokio-util = { version = "0.7", features = ["sync"] }` for CancellationToken)
- Create: `crates/tama-core/tests/fixtures/fake-docker.sh` (fake docker CLI — lives in tama-core tests to avoid dependency cycle with tama-mock)

**What to implement:**

1. **`image.rs` functions:**
   ```rust
   pub async fn docker_available() -> Result<()>
   // Run `docker info`. Error if docker CLI missing or daemon unreachable.
   // Use tokio::process::Command (async).
   
   pub async fn is_image_present(image: &str) -> Result<bool>
   // Run `docker image inspect {image}`. Ok(true) if exit 0, Ok(false) if "No such image" error.
   // Use tokio::process::Command (async).
   
   pub async fn pull_image(
       image: &str,
       progress: impl Fn(String) + Send + Sync + 'static,
       timeout_secs: u64,
       cx: &tokio_util::sync::CancellationToken,
   ) -> Result<()>
   // Run `docker pull {image}`. Stream stdout/stderr lines to progress callback.
   // Respect CancellationToken (kill subprocess on cancel). Timeout after timeout_secs.
   ```

2. **Fake docker CLI** (`tests/fixtures/fake-docker.sh`):
   - Shell script that intercepts docker commands and simulates behavior
   - State directory from env var `FAKE_DOCKER_STATE_DIR` (per-test tempdir — avoids collision across parallel nextest tests)
   - Test helper pattern: each integration test copies the script into a tempdir as `docker`, chmod +x, prepends tempdir to PATH via `std::env::set_var("PATH", ...)` (process-global, safe under nextest's per-test-process isolation), and sets `FAKE_DOCKER_STATE_DIR` to a unique tempdir
   - Supported commands:
     - `docker info` → exit 0
     - `docker image inspect {image}` → exit 0 if image in state dir, exit 1 + "No such image" otherwise
     - `docker pull {image}` → writes image to state dir, outputs JSON progress lines
     - `docker run ...` → handled by Task 3 extension
     - `docker stop {name}` → handled by Task 3 extension
     - `docker rm {name}` → handled by Task 3 extension
     - `docker logs ...` → handled by Task 3 extension
     - `docker ps -a --filter ...` → handled by Task 4 extension
   - State files: `{FAKE_DOCKER_STATE_DIR}/images/` (one file per image), `{FAKE_DOCKER_STATE_DIR}/containers/` (one JSON file per container with name/id/state/pid)

**Steps:**
- [ ] Add `tokio-util = { version = "0.7", features = ["sync"] }` to tama-core/Cargo.toml
- [ ] Write failing test for docker_available() in image.rs #[cfg(test)] — expect error when docker not on PATH
- [ ] Implement docker_available() using tokio::process::Command (async)
- [ ] Run `cargo nextest run --package tama-core -- docker::image`
- [ ] Write failing test for is_image_present() — expect false for nonexistent image
- [ ] Implement is_image_present() with docker image inspect (async)
- [ ] Run tests — verify passes for both present and absent images
- [ ] Create fake-docker.sh with basic commands (info, image inspect, pull) in tests/fixtures/
- [ ] Write test helper function: copy script to tempdir as `docker`, chmod +x, prepend to PATH, set FAKE_DOCKER_STATE_DIR
- [ ] Write failing test for pull_image() using fake-docker — expect pull to succeed with progress callbacks
- [ ] Implement pull_image() with CancellationToken support, timeout, and progress streaming
- [ ] Write integration test: verify pull succeeds and progress lines received
- [ ] Write test for pull cancellation: fire cancel token mid-pull, verify subprocess killed
- [ ] Write test for pull timeout: simulate slow pull, verify timeout error + retry behavior
- [ ] Run `cargo nextest run --package tama-core -- docker::image`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `feat: add docker image management and fake CLI mock`

**Acceptance criteria:**
- [ ] docker_available() returns Ok when docker daemon running, Err when not
- [ ] is_image_present() correctly detects present/absent images
- [ ] pull_image() streams progress, respects cancellation, times out after configured duration
- [ ] Fake docker CLI handles info, image inspect, and pull commands
- [ ] Test helper copies script to per-test tempdir, sets PATH + FAKE_DOCKER_STATE_DIR
- [ ] All tests pass with fake docker on PATH

---

### Task 3: Container Runner — Spawn, Stop, Logs, Inspect

**Context:** Implement the core container lifecycle functions that build and execute docker commands. This is the workhorse that load_model/unload_model delegate to.

**Files:**
- Create: `crates/tama-core/src/backends/docker/runner.rs`
- Modify: `crates/tama-core/src/backends/docker/mod.rs` (export runner module)
- Modify: `crates/tama-mock/src/bin/fake-docker.sh` (add run, stop, rm, logs, inspect commands)

**What to implement:**

1. **Path rewrite function:**
   ```rust
   pub fn rewrite_args_for_container(
       args: &[String],
       models_dir: &std::path::Path,
       container_model_path: &str,
   ) -> Result<Vec<String>>
   // For each arg:
   // - Split form "--flag /abs/path": if value is under models_dir → rewrite to {container_model_path}/{relative}
   // - Joined form "--flag=/abs/path": split on first '=', rewrite value if under models_dir
   // - Strip surrounding quotes (shlex quoting from build_full_args) before matching paths
   // - Paths outside models_dir and not covered by any mount: Error
   // - Non-path args (flags without path values): pass through unchanged
   ```

2. **Volume resolver:**
   ```rust
   pub fn resolve_volumes(
       config: &DockerConfig,
       models_dir: &std::path::Path,
   ) -> Result<Vec<String>>
   // Substitute {{MODEL_DIR}} → models_dir in host_path. Validate host paths exist. Return ["host:container:ro" ...]
   ```

3. **Group resolver:**
   ```rust
   pub async fn resolve_group_gids(group_names: &[String]) -> Vec<String>
   // For each name: `getent group {name}` via tokio::process::Command → extract GID.
   // Skip silently if group not found (log warning). Return GID strings.
   ```

4. **Spawn container:**
   ```rust
   pub async fn spawn_container(
       backend_name: &str,
       config: &DockerConfig,
       host_port: u16,
       args: Vec<String>,
       env_vars: Vec<String>,
       models_dir: &std::path::Path,
   ) -> Result<DockerContainer>
   // Build docker run command with all flags. Return DockerContainer { name, id, pid }
   ```

5. **Stop / remove / logs / inspect:**
   ```rust
   pub async fn stop_container(name: &str) -> Result<()>
   // `docker stop -t 5 {name}`. Tolerate "No such container".
   
   pub async fn remove_container(name: &str) -> Result<()>
   // `docker rm -f {name}`. Tolerate "No such container".
   
   pub async fn logs_stream(
       container_id: &str,
       since_epoch: u64,
   ) -> Result<tokio::process::Child>
   // `docker logs -f --since {since} {id}`. Return child for stdout/stderr reading.
   
   pub async fn inspect_container(name: &str) -> Result<Option<DockerInspect>>
   // `docker inspect {name}`. Parse JSON for State.Running, State.Pid, NetworkSettings.Ports.
   ```

6. **DockerContainer struct:**
   ```rust
   pub struct DockerContainer {
       pub name: String,
       pub id: String,
       pub pid: u32,
   }
   ```

7. **Extended fake-docker.sh:** Add run (records args, creates container state, returns ID), stop, rm, logs (returns canned output), inspect (returns JSON from state).

**Steps:**
- [ ] Write failing test for rewrite_args_for_container — split form path under models dir → rewritten
- [ ] Implement rewrite_args_for_container with both split and joined form handling
- [ ] Write tests: split form rewrite, joined form rewrite, non-path arg passthrough, outside-mount error
- [ ] Run `cargo nextest run --package tama-core -- docker::runner`
- [ ] Write failing test for resolve_volumes — {{MODEL_DIR}} substitution
- [ ] Implement resolve_volumes with template substitution and host path validation
- [ ] Write tests: valid volumes, missing host path → error, MODEL_DIR substitution
- [ ] Implement resolve_group_gids with getent subprocess
- [ ] Write tests: present group → GID, missing group → skipped silently
- [ ] Extend fake-docker.sh with run, stop, rm, logs, inspect commands
- [ ] Implement spawn_container with full docker run command construction
- [ ] Write test: assert generated argv matches expected flags (with/without gpus, shm_size, etc.)
- [ ] Implement stop_container, remove_container, logs_stream, inspect_container
- [ ] Write tests for each using fake docker
- [ ] Run `cargo nextest run --package tama-core -- docker::runner`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `feat: add docker container runner with spawn, stop, logs, and path rewriting`

**Acceptance criteria:**
- [ ] rewrite_args_for_container handles split form (`--flag /path`) and joined form (`--flag=/path`)
- [ ] Paths outside models dir → error; non-path args pass through
- [ ] resolve_volumes substitutes {{MODEL_DIR}} and validates host paths exist
- [ ] resolve_group_gids uses getent, skips missing groups silently
- [ ] spawn_container builds correct docker run command with all config options
- [ ] stop/remove tolerate "No such container"; logs_stream returns child; inspect parses JSON
- [ ] All tests pass with fake docker

---

### Task 4: Startup Reconciliation

**Context:** On proxy startup, reap any managed containers left behind from crashed Tama instances. Simplified to unconditional reap (no adoption) since Tama doesn't persist loaded models across restarts.

**Files:**
- Create: `crates/tama-core/src/backends/docker/reconcile.rs`
- Modify: `crates/tama-core/src/backends/docker/mod.rs` (export reconcile module)
- Modify: Proxy startup code (find where proxy begins accepting requests — likely `proxy/mod.rs` or `server/`)

**What to implement:**

```rust
pub async fn startup_reconcile() -> Result<()>
// 1. docker ps -a --filter label=tama.managed=true --format '{{.ID}} {{.Names}}'
// 2. For each container found: docker rm -f {id}
// 3. If docker unavailable (docker_available fails): log warning + return Ok (don't block startup)
```

**Proxy startup integration:** In `crates/tama-core/src/proxy/server/mod.rs`, inside `ProxyServer::new` (line ~88), call `backends::docker::reconcile::startup_reconcile().await` **immediately before** `Self::cleanup_stale_processes(&state).await`. This ordering is critical: reconcile must run first so that `cleanup_stale_processes` doesn't adopt a live container from a crashed Tama instance as a native Ready backend. Handle the Result with log-and-continue on Err (no catch_unwind needed for async).

**Extended fake-docker.sh:** Add `docker ps` with label filter support.

**Steps:**
- [ ] Write failing test for startup_reconcile — with stale containers in fake state, verify they're removed
- [ ] Implement startup_reconcile with docker ps + rm loop
- [ ] Write test: no managed containers → no-op (no errors)
- [ ] Write test: docker unavailable → log warning, return Ok
- [ ] Add docker ps support to fake-docker.sh
- [ ] Wire into proxy startup (find the right location — likely proxy/mod.rs init or server boot)
- [ ] Run `cargo nextest run --package tama-core -- docker::reconcile`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `feat: add docker startup reconciliation — reap stale managed containers`

**Acceptance criteria:**
- [ ] startup_reconcile removes all containers with tama.managed=true label
- [ ] No managed containers → no-op, no errors
- [ ] Docker unavailable → warning logged, startup not blocked
- [ ] Wired into proxy startup before accepting requests

---

### Task 5: Lifecycle Integration — load_model, unload_model, Kill Paths

**Context:** Integrate docker backends into the existing lifecycle. load_model checks for docker_config and delegates to docker runner instead of native process spawn. Pull happens BEFORE Starting reservation to avoid startup timeout conflict. All kill paths (idle_timeout, force-unload) branch on is_docker.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` (load_model docker path, unload_model docker path)
- Modify: `crates/tama-core/src/proxy/lifecycle/idle_timeout.rs` (branch on is_docker for kill)
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/handlers.rs` (force-unload branch on is_docker)
- Modify: `crates/tama-core/src/backends/docker/mod.rs` (re-export runner/image for use in lifecycle)

**What to implement:**

1. **load_model docker path** (in `lifecycle/mod.rs`):

   Control flow (replaces the existing single reservation block):
   - **Step A — resolve backend + open manager + get_active:** Same as current code: resolve backends for model, open BackendManager, resolve gpu_variant (reuse the same variant-fallback logic as `resolve_backend_path`), call `manager.get_active(name, gpu_variant)`. If docker_config present on active installation → branch to docker path.
   - **Step B — preflight + pull (BEFORE Starting reservation):** docker_available() → create log stream → verify/pull image. Concurrent docker pulls are idempotent at the docker layer (two `docker pull` of same image race harmlessly).
   - **Step C — reserve Starting with is_docker=true:** Reuse the existing reservation block (models.write().insert(Starting { ..., is_docker: true, ... })). Startup timeout clock starts NOW (after pull).
   - **Step D — spawn container + health check:**
     1. Best-effort `docker rm -f tama-{backend_name}` (tolerate "No such container")
     2. Find free host port (retry up to 3x on port collision — match stderr for substring "port is already allocated" + non-zero exit; log full stderr on final failure)
     3. Resolve volumes ({{MODEL_DIR}} substitution, validate host paths)
     4. Rewrite model path in args (both split and joined forms, strip shlex quotes before matching)
     5. Resolve group_adds GIDs (async getent)
     6. Override args: `--host` → "0.0.0.0", `--port` → container_port
     7. Build and execute docker run command
     8. Get container PID from docker inspect
     9. Update Starting state with PID
     10. Start log streaming (`docker logs -f --since {unix_epoch_seconds}` — capture timestamp immediately before docker run)
     11. Health check: extract path component from `manager.get_health_check_url()` (e.g., "/health" from "http://localhost:8080/health"), build `http://127.0.0.1:{host_port}{path}`. Same HTTP loop + startup_timeout_secs. On timeout: `docker stop -t 5 {name}`, cleanup Starting state, return error

2. **unload_model docker path:**
   - When `state.is_docker`: `docker stop -t 5` → tolerate missing → `docker rm -f` → cancel log task → cleanup

3. **Three kill sites that need docker branches:**

   **(a) `unload_model` SIGTERM path** (lifecycle/mod.rs): When `state.is_docker`: `docker stop -t 5 tama-{backend_name}` → tolerate missing → `docker rm -f`.

   **(b) `idle_timeout.rs` stuck-Starting cleanup** (line ~200-212, `kill_process_group(pid)`): When `state.is_docker`: `docker stop -t 5 tama-{backend_name}` + `docker rm -f`. PID liveness check (`is_process_alive`) remains valid for docker (container init is host-visible) — the dead-PID detection that triggers this path works correctly.

   **(c) `handle_tama_cancel_load`** (proxy/tama_handlers/models/handlers.rs line ~222, kills Starting backends): When `state.is_docker`: `docker stop -t 5 tama-{backend_name}` + `docker rm -f` instead of `kill_process_group`/`force_kill_process_group`. Note: this is NOT a "force-unload" handler — it's the cancel-load endpoint.

**Steps:**
- [ ] Write failing integration test for full docker load_model flow (mock docker, mock health endpoint)
  - Test: preflight → pull → Starting reservation → spawn → health check → Ready state
- [ ] Implement load_model docker branch in lifecycle/mod.rs
- [ ] Run integration test — verify flow completes
- [ ] Write test for pull timeout: mock slow pull (120s+), verify NOT killed by startup timeout (pull before Starting)
- [ ] Write test for port collision retry: mock docker run port failure → retry with fresh port
- [ ] Implement unload_model docker branch
- [ ] Write test for unload flow: stop → rm → cleanup
- [ ] Add is_docker branch to idle_timeout.rs stuck-Starting kill path
- [ ] Add is_docker branch to handle_tama_cancel_load (models/handlers.rs)
- [ ] Write test for crash detection via failed health check → consecutive_failures increment
- [ ] Run `cargo nextest run --package tama-core -- lifecycle`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `feat: integrate docker backend into load_model, unload_model, and kill paths`

**Acceptance criteria:**
- [ ] load_model detects docker_config and follows docker path (pull → reserve → spawn → health)
- [ ] Pull happens BEFORE Starting reservation (not killed by 120s startup timeout)
- [ ] Port collision retries up to 3x with fresh ports
- [ ] Args rewritten for container (both split and joined forms)
- [ ] Container published on 127.0.0.1 only
- [ ] Log streaming from spawn timestamp via docker logs -f --since
- [ ] Health check timeout → docker stop + error
- [ ] unload_model calls docker stop -t 5 + rm -f, tolerates missing container
- [ ] idle_timeout stuck-Starting and handle_tama_cancel_load branch on is_docker
- [ ] All existing tests pass (no regressions)

---

### Task 6: API Surface — Backend Registration + Existing Endpoint Updates

**Context:** Create a new endpoint for registering docker backends (the existing `POST /tama/v1/backends/install` is for binary installs only), and update existing endpoints that break with docker backends. The backends API lives in the **`tama` crate** (`crates/tama/src/api/backends/`), not tama-core.

**Files:**
- Create: `crates/tama/src/api/backends/register.rs` (new handler for POST /tama/v1/backends)
- Modify: `crates/tama/src/api/backends/types.rs` (DTOs for registration request/response)
- Modify: `crates/tama/src/router.rs` (wire new route)
- Modify: `crates/tama/src/api/openapi.rs` (register new route in OpenAPI spec)
- Modify: `docs/api/backends.md` (document new endpoint per AGENTS.md conventions)
- Modify: `crates/tama/src/api/backends/manage/remove.rs` (add `docker_config` field to literal BackendInfo construction — compile fix from Task 1)
- Modify: `crates/tama/src/api/backends/manage/update.rs` (reject docker backends: "update not supported for docker backends" or map to image re-pull)
- Modify: `crates/tama/src/api/updates/check.rs` (skip docker backends in check-updates — they have source=NULL and no prebuilt/git releases)
- Modify: `crates/tama/src/api/backends/install.rs` (add "docker" to the backend_type match — currently rejects unknown types; docker should not go through binary install)

**What to implement:**

1. **New endpoint: `POST /tama/v1/backends`** (register a backend directly, bypassing binary install):
   - Request body DTO:
     ```rust
     pub struct RegisterBackendRequest {
         pub name: String,
         pub backend_type: String,    // "docker" for docker backends
         pub version: String,
         #[serde(default)]
         pub gpu_variant: String,     // defaults to "cpu"
         pub docker_config: Option<DockerConfig>,
     }
     ```
   - Validation:
     - `backend_type="docker"` + no docker_config → 400 "docker backend requires docker_config"
     - `backend_type != "docker"` + docker_config present → 400 "docker_config only valid for docker backend type"
     - Run `DockerConfig::validate()` if present
     - Run `docker_available()` preflight (error if docker not available)
   - On success: insert into backend_installations with `backend_type="docker"`, `source=NULL`, `docker_config=<json>`, `path={image}` via BackendManager
   - Response: 201 with the created BackendInfo

2. **Existing endpoint updates:**
   - `remove.rs`: add `docker_config` field to literal BackendInfo construction (compile fix)
   - `update.rs`: reject docker backends — return 400 "update not supported for docker backends" (or future: map to image re-pull)
   - `check.rs` (check-updates): skip docker backends (source=NULL, no release feed)
   - `install.rs`: add "docker" to backend_type match arm — reject with "docker backends use POST /tama/v1/backends, not /install"

3. **OpenAPI + docs:**
   - Add route to `openapi.rs` (enumerates all routes)
   - Document in `docs/api/backends.md`

**Steps:**
- [ ] Create register.rs handler with validation logic
- [ ] Create DTOs in types.rs
- [ ] Wire route in router.rs: `POST /tama/v1/backends`
- [ ] Add route to openapi.rs
- [ ] Write failing test for POST /backends with docker_config → 201 success (use existing router-based test pattern from list.rs)
- [ ] Write failing test for docker type without docker_config → 400
- [ ] Write failing test for non-docker type with docker_config → 400
- [ ] Run `cargo nextest run --package tama -- backends`
- [ ] Fix remove.rs BackendInfo construction (add docker_config field)
- [ ] Add docker rejection to update.rs, check.rs, install.rs
- [ ] Write tests for rejection cases
- [ ] Document endpoint in docs/api/backends.md
- [ ] Run `cargo nextest run --package tama`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit: `feat: add docker backend registration API and update existing endpoints`

**Acceptance criteria:**
- [ ] POST /tama/v1/backends accepts docker_config for docker type → 201 with BackendInfo
- [ ] POST /tama/v1/backends rejects docker type without docker_config → 400
- [ ] POST /tama/v1/backends rejects non-docker type with docker_config → 400
- [ ] DockerConfig validation runs at registration (image format, absolute paths, etc.)
- [ ] docker_available() preflight at registration time
- [ ] remove.rs compiles with new docker_config field on BackendInfo
- [ ] update.rs rejects docker backends
- [ ] check-updates skips docker backends
- [ ] install.rs rejects docker type (directs to POST /backends)
- [ ] OpenAPI spec updated, docs/api/backends.md updated

---

## Cross-Cutting Notes

- **Platform:** Linux only. Docker backend targets native docker daemon. macOS/Windows Docker Desktop out of scope for v1 (PID-liveness, device passthrough, and getent all assume Linux).
- **Security:** Container published on 127.0.0.1 only. Env vars visible via `docker inspect` (same as native `/proc/{pid}/environ`).
- **Testing:** All docker tests use fake-docker.sh on PATH. No real docker required for CI.
- **ADR:** ADR-0006 documents the DockerConfig separation from BackendSource.
