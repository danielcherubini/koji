# Detached Health Gate Plan (Plan 194)

**Goal:** Make tamad's model-start health gate survive caller cancellation (HTTP client disconnect / gRPC drop), so a loading model reliably transitions `starting → ready|failed` even when whoever triggered the load goes away mid-boot.

**Architecture:** Three layers of defense. (1) tamad detaches the spawn→health-gate→terminal-row-update tail into `tokio::spawn`'d tasks, making the `LoadModel` RPC non-blocking; (2) the proxy's `load_spec_on_tamad` compensates by polling the tamad's live wire rows until a terminal status, preserving today's "callers only proceed once actually ready" semantics; (3) a tamad-side reconciliation sweep adopts (marks `ready`) or tears down (marks `failed`) any orphaned `starting` row whose backend answers health — healing rows stranded by older binaries, crashes, or future bugs.

**Tech Stack:** Rust, tokio (detached tasks, intervals), tonic gRPC, axum, reqwest (`process::check_health`), existing `ProcessTable` + `live_rows` wire-row infrastructure.

---

## Background (verified on production host `tama`, 2026-08-26)

The bug this plan fixes was diagnosed end-to-end on the live system:

1. Proxy POST `/tama/v1/models/{id}/load` → axum handler awaits `load_model_on_tamad` → blocking tonic `LoadModel` RPC → tamad handler awaits `lifecycle.load()` → spawn + `wait_for_health` poll loop.
2. vLLM-class backends take minutes to boot. The initiating HTTP client times out long before that. Hyper cancels the axum handler future → drops the tonic client call → **tonic cancels tamad's handler future**, silently killing everything after the `STARTING` row insert.
3. Observed symptoms: zero packets to the health port (tcpdump), no teardown/FAILED log lines, model row stuck `starting` forever while the container itself served `200` on `/health`.
4. The comment in `handle_tama_cancel_load` ("loads are short, so a cancel is best-effort") documents the wrong assumption this plan removes.

## Behavioral contract changes (read carefully)

After Task 2:

| Scenario | Before | After |
|---|---|---|
| Gate configured (`health_url` non-empty AND `health_timeout_ms > 0`) | RPC blocks minutes; returns `Ok(resp{status:"ready"})` or `Err` on unhealthy | RPC returns `Ok(resp{status:"starting"})` within seconds of spawn; row flips to `ready`/`failed` asynchronously |
| No gate configured (`health_url` empty OR `health_timeout_ms == 0`) | Instant `Ok(status:"ready")`, synchronous | **Unchanged** (stays synchronous instant-ready) |
| Spawn failure | `Err` synchronously | **Unchanged** (`Err` synchronously) |
| Unhealthy at gate timeout | `Err` returned to caller; teardown done inline | Teardown + `failed` row happen in detached task; caller sees early `Ok(starting)`; proxy waiter converts the row outcome into `Err` |

Caller-visible semantics through the proxy are preserved because Task 3 makes the proxy wait on the wire row. Wire-compatibility:

- **new proxy + old tamad**: old tamad still blocks the RPC until healthy; the proxy's post-RPC wait loop finds the row already `ready` within one 1 Hz stats frame. Compatible.
- **old proxy + new tamad**: old proxy receives early `Ok("starting")` and already treats `Ok` as loaded (it just logs and returns the config key); readiness flows via wire rows exactly as designed in plan-193 T4/T5c. Compatible.

Known acceptable side effects (do NOT try to "fix" these):
- The respawn supervisor's sequential loop no longer serializes on boot time; overlapping respawns each carry their own detached gate. Update its log wording only if trivially affected.
- The management-API load endpoint still holds the HTTP request open until ready (proxy-side wait). Cancellation of that HTTP request is now harmless by construction — do not additionally shield it with `tokio::spawn`; keep the change surface minimal.

---

### Task 1: Extract health-gate settle tails into reusable helpers (behavior-preserving refactor)

**Context:**
Both load paths in `crates/tamad/src/lifecycle.rs` — the native path in `load()` (~lines 259–470) and the Docker path in `load_container()` (~lines 486–665) — duplicate the same tail logic: after spawning and inserting the `STARTING` row, they (a) run `wait_for_health` unless the gate is disabled, (b) on failure tear down (kill process group / stop+remove container) and record a `FAILED` row, or (c) on success record a `READY` row and, if the success was *verified* (real gate ran), call `store.zero_persisted_restart_count(model_name)`. This refactor extracts those tails verbatim into named async methods so Task 2 can detach them without touching their logic. Behavior after this task must be byte-for-byte identical; every existing test must still pass unmodified (except none should need modification).

**Files:**
- Modify: `crates/tamad/src/lifecycle.rs`
- Test: existing `#[cfg(test)] mod tests` in `crates/tamad/src/lifecycle.rs` (no new tests required; existing ones prove no regression)

**What to implement:**

Add two private async methods on `TamadLifecycle`. Their return-type contract, stated once and applying to both: each returns `anyhow::Result<()>` where `Err` means "gate settled unhealthy" (the existing `anyhow!` bail strings move verbatim into the methods); rows are recorded internally before returning:

```rust
/// Settles the native-path health gate: polls `wait_for_health` (unless
/// gate disabled), then records the terminal row (READY or FAILED) with
/// the exact bookkeeping semantics documented inline below, including
/// the verified-ready persisted-tally reset.
async fn settle_native_gate(
    &self,
    req: LoadModelRequest,
    pid: u32,
    inheriting_attempt: bool,
    previous: Option<ProcessEntry>,
    timeout: Duration,
) -> Result<()>

/// Same shape for the Docker path: on unhealthy, stops and removes the
/// container instead of killing a process group.
async fn settle_container_gate(

    &self,
    req: LoadModelRequest,
    pid: u32,
    inheriting_attempt: bool,
    previous: Option<ProcessEntry>,
    timeout: Duration,
    container_name: String,
)
```

Move the existing tail code out of `load()` and `load_container()` into these methods **verbatim** — same log messages, same `owns_row` guards, same `entry_for(...)` calls with the same arguments, same error strings. The unhealthy branch's error propagates via the `Result<()>` return so `load()`/`load_container()` can propagate it exactly as today.

While extracting `settle_native_gate`, also add an `info!` log at the verified-READY insert: `info!(model = %req.model_name, pid, "backend became healthy (detached gate)")` — this becomes the observable success signal once the gate runs detached (Task 2) and is what the rollout verification greps for.

In `load()` and `load_container()`, replace the moved code with calls like:

```rust
let timeout = Duration::from_millis(req.health_timeout_ms.max(0) as u64);
if req.health_url.is_empty() || req.health_timeout_ms == 0 {
    // No gate configured → instant ready (unchanged synchronous path).
    // Keep the existing READY-insert + response construction here, or
    // factor it minimally — preserve current behavior exactly.
} else {
    self.settle_native_gate(req.clone(), pid, inheriting_attempt, previous.clone(), timeout).await?;
}
```

Be careful with borrows: `previous` and `req` are used later for `entry_for` in the instant-ready branch and the final `LoadModelResponse`; clone what the settle method needs. Also introduce a small public helper in `crates/tamad/src/host_installs/docker/runner.rs`:

```rust
/// Deterministic container name for a model (used by spawn, stop, remove).
pub fn container_name_for(model_name: &str) -> String {
    format!("tama-{}", model_name)
}
```

and use it everywhere the pattern appears **within the tamad crate** — all three sites: `runner.rs::spawn_container` (~line 291), `lifecycle.rs::unload` (~line 671), and `server.rs`'s docker log lookup (~line 414). (A proxy-side `service_name` helper in tama-core shares the format string but is out of scope.)

**Steps:**
- [ ] Read `crates/tamad/src/lifecycle.rs` fully (both paths + `status` module + `entry_for`, `owns_row`, `wait_for_health`, `shared_copy`)
- [ ] Add `container_name_for` to `crates/tamad/src/host_installs/docker/runner.rs`; replace the inline format string in `spawn_container`
- [ ] Extract `settle_native_gate` and `settle_container_gate` as described; wire them into `load()` / `load_container()` behind the gate-configured condition
- [ ] Run `cargo nextest run --package tamad -- lifecycle`
  - Did ALL existing lifecycle tests pass unmodified? If any fail, you moved something incorrectly — diff against git HEAD and fix. Do not modify tests in this task.
- [ ] Run `cargo fmt --all` then `cargo clippy --package tamad --all-targets -- -D warnings`
  - Did both succeed? Fix and re-run if not.
- [ ] Commit with message: `refactor(tamad): extract health-gate settle tails into settle_native_gate/settle_container_gate`

**Acceptance criteria:**
- [ ] `cargo nextest run --package tamad` passes with zero test modifications vs. parent commit
- [ ] `grep -n "zero_persisted_restart_count" crates/tamad/src/lifecycle.rs` shows the call inside `settle_native_gate` (or the shared verified-success helper), not duplicated in `load()`
- [ ] `container_name_for` is the single source of the `tama-{model}` naming rule

---

### Task 2: Detach the health gate — `LoadModel` returns `starting` immediately

**Context:**
This is the core fix. Today, when a real health gate is configured, tamad's gRPC handler future lives for the entire boot (minutes). If the caller disappears, tonic cancels that future and everything after the `STARTING` row insert silently vanishes — the row stays `starting` forever while the backend boots happily unobserved (verified on prod: stuck `starting` row, zero health traffic, healthy container). After this task, the gate runs in a detached tokio task built from `TamadLifecycle::shared_copy()` (a `Send` trio of `Arc`s that already exists precisely for spawned work — see its doc comment), and the RPC returns `Ok(LoadModelResponse{ status: "starting", .. })` seconds after spawn. Terminal outcomes land on the process table asynchronously.

**Files:**
- Modify: `crates/tamad/src/lifecycle.rs`
- Modify: `crates/tama/tests/tamad_boot_replay.rs` (only if it asserts ready-after-`load()`)
- Test: `crates/tamad/src/lifecycle.rs` `#[cfg(test)]` module (rewrite gate-dependent tests)

**What to implement:**

1. In `load()` (native path), replace the awaited call to `settle_native_gate` with a detached spawn:

```rust
if req.health_url.is_empty() || req.health_timeout_ms == 0 {
    // ... unchanged instant-ready branch, still synchronous ...
} else {
    let lc = self.shared_copy();
    let req2 = req.clone();
    let prev2 = previous.clone();
    tokio::spawn(async move {
        if let Err(e) = lc.settle_native_gate(req2, pid, inheriting_attempt, prev2, timeout).await {
            warn!(model = %req.model_name, error = %e, "detached health gate settled unhealthy");
        }
    });
}
```

then construct and return `LoadModelResponse { endpoint_url: Self::endpoint_from_health_url(&req.health_url), pid: pid as i32, status: status::STARTING.to_string() }`.

2. Apply the identical transformation in `load_container()` with `settle_container_gate` (pass `container_name_for(&req.model_name)`).

3. Update doc comments that promise blocking behavior:
   - `load()`'s rustdoc ("health-poll until success or timeout, and record the process in the table") → describe the new split: spawn synchronous, gate detached when configured.
   - `load_container()`'s rustdoc ("Spawn a Docker-backed backend (container) and health-poll it to ready …") → same treatment.
   - The boot sweep's "replayed desired model" log in `replay_desired` now fires at `starting` time when gates are configured — adjust wording if trivially affected.
   - The module-level comment block in `crates/tama-core/src/proxy/lifecycle/spec.rs` around line ~577 ("The `LoadModel` RPC blocks until the tamad's spawn + health poll completes") — leave the *code* alone (Task 3 rewrites it) but this comment lives in tama-core; update it in Task 3 instead. Only touch tamad-side comments here.

4. Rewrite the gate-dependent tests in `lifecycle.rs`'s test module. The complete list of tests needing rewrite (verified by review — nothing else in this file, server.rs, installs.rs, or tamad_boot_replay.rs relies on blocking gate behavior):
   - `test_load_with_health_check` (~1336): currently expects `load()` to have completed the gate. Change to: call `load()`, assert `resp.status == status::STARTING`, then poll `table.get(model_name)` in a loop (e.g. 100 ms sleep, up to ~5 s) until `status == READY`, assert `endpoint_url` matches the sniffed port. The existing local TCP health sniffer setup (~line 1981 area, reused by several tests) stays as-is.
   - `test_load_health_timeout` (~1376, the `http://127.0.0.1:1/health` test): `load()` now returns `Ok(starting)`; poll the table until `FAILED` (allow up to the configured short timeout + margin); assert the process group was killed (existing helpers `is_process_group_alive`).
   - `test_load_ready_url_positive_timeout_is_verified_and_resets_window` (~1527): same pattern — trigger `load()`, await the terminal row transition via polling, then assert verified-ready tally bookkeeping exactly as before.
   - Do NOT touch: the two gate-less tests (~1486 and ~1583), `test_load_marks_failed_when_backend_crashes`, `test_reap_budget_trip_flags_and_refuses`, `test_reap_success_resets_counter` (~1980 — already polls the table), `crates/tama/tests/tamad_boot_replay.rs` (all its fixtures use `health_timeout_ms: 0`), `server.rs` tests (single gate-less load at ~1993), installs.rs (`fake_req` is gate-less).

5. Double-check the respawn supervisor (`start_respawn_supervisor`, ~line 207): its `match lifecycle.load(&req).await` arms still compile; its success log now fires at `starting` time. Adjust the message text to `"respawn supervisor relaunched the backend (gate detached)"` — nothing structural.

6. Confirm `server.rs`'s `load_model` handler needs **no changes**: the idempotent early-return branch reads `entry.status` which may now legitimately be `"starting"` — that is correct and desired (a re-issued load during boot returns `starting` instead of double-spawning). Verify no test in `crates/tamad/src/server.rs` asserts `status == "ready"` straight off `load_model` (the ~1992 fixture passes `health_timeout_ms: 0`, i.e. instant-ready, so it is unaffected).

**Steps:**
- [ ] Write/adjust failing tests first: pick ONE rewritten test (e.g. `test_load_with_health_check`) to encode the new contract (`Ok(starting)` + async `READY`), run `cargo nextest run --package tamad -- lifecycle::tests::test_load_with_health_check`, confirm it fails against current code
- [ ] Implement the detachment in `load()` and `load_container()` per above
- [ ] Rewrite remaining gate-dependent tests to the poll-for-terminal pattern
- [ ] Run `cargo nextest run --package tamad`
  - Did all pass? If flaky timing, raise poll budgets (never shrink production timeouts to accommodate tests; tests own their own generous margins)
- [ ] Run `cargo nextest run --workspace`
- [ ] Run `cargo fmt --all` && `cargo clippy --workspace --all-targets -- -D warnings` && `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --workspace`
- [ ] Commit with message: `feat(tamad): detach health gate from LoadModel RPC; return starting immediately`

**Acceptance criteria:**
- [ ] With a configured gate, `LoadModel` responds in < ~5 s regardless of backend boot duration (test proves it: gate waits on a sniffer server held closed for ≥2 s while `load()` already returned)
- [ ] Dropping the caller cannot strand the row: the detached task owns the settle; tests simulate completion without any live RPC caller
- [ ] All workspace tests green; both clippy targets clean

---

### Task 3: Proxy waits on wire rows for the terminal outcome

**Context:**
Task 2 makes the RPC return `starting`. Callers through the proxy (`ensure_model_loaded` for chat/forward auto-load, the management API load endpoint, TTS/compaction loads) must still only observe success once the backend is genuinely healthy — otherwise chat requests would race a dead endpoint. The tamad's live wire row (plan-193 T4) is the source of truth, so the proxy polls row status until a terminal outcome, converting failure/timeout into `Err` (so `on_load_error` handlers produce the usual 503 `LoadModelError`). This also fixes the original UX bug end-to-end: a UI disconnect during load now merely abandons the *waiter*, never the load itself.

**CRITICAL wire-row semantics (review finding — read before implementing):**

`live_rows` (see `crates/tama-core/src/proxy/state/rows.rs`) **filters out `failed` rows entirely**: `live()` only emits rows where `(alive && status ∈ {ready, starting, restarting}) || status == "budget_exhausted"`, and `alive` folds as `status != FAILED && pid alive` (lifecycle.rs `to_process_info`). So the production status provider can only ever produce `Some("starting") | Some("ready") | Some("budget_exhausted") | None` — a gate failure manifests as **the row disappearing** (`None`), never as `Some("failed")`. The waiter must therefore treat "seen-then-gone" as terminal failure, while tolerating brief stats-stream staleness (frames older than `LIVE_FRAME_MAX_AGE` = 5 s also yield zero rows).

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle/spec.rs` (function `load_spec_on_tamad`, ~lines 550–613, plus the blocking-behavior comment block above it)
- Test: `crates/tama-core/src/proxy/lifecycle/spec/tests.rs` (add tests for the extracted waiter)

**What to implement:**

1. Add a generic, unit-testable polling helper in `spec.rs`:

```rust
/// Polls a row-status provider until it reports `ready`, an explicit
/// failure status, or the deadline elapses.
///
/// Provider contract: `Some(status)` = current row status ("starting",
/// "ready", "restarting", "budget_exhausted", or any other word);
/// `None` = row not visible. `None` is ambiguous between "not yet seen"
/// and "row died and was filtered out", so the helper tracks it:
/// - Before the first `Some(_)`: `None` just means keep waiting (the row
///   may not be published yet).
/// - After at least one `Some(_)` was observed: sustained `None` means
///   the row died (filtered out as failed/dead) → return Err. To avoid
///   false-failing on a stats-stream hiccup (>LIVE_FRAME_MAX_AGE frames
///   yield zero rows), require `gone_threshold` consecutive `None`
///   observations before declaring death. Callers size this generously
///   (≥ 15 s worth of ticks).
pub(crate) async fn wait_for_terminal_row<F, Fut>(
    mut status_of: F,
    poll_every: Duration,
    deadline: Duration,
    gone_threshold: u32,
) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
```

Semantics per tick:
- `Some(s)` with `s == "ready"` → return `Ok(())`; reset the gone counter.
- `Some(s)` with `s == "budget_exhausted"` → return `Err(anyhow::Error::new(crate::proxy::lifecycle::BudgetExhausted))` — the TYPED mark is mandatory here (it is available in this crate/module; chat/forward callers translate via `budget_exhausted_response_for(err)` which requires `err.is::<BudgetExhausted>()` to emit the 503 + retry-after shape). Do NOT use a plain anyhow message.
- Any other `Some(s)` (e.g. "starting", "restarting") → reset the gone counter, keep waiting.
- `None` after having seen ≥1 `Some` → increment gone counter; if counter ≥ `gone_threshold` → return `Err(anyhow!("backend '{}' died during startup", ...))` — include whatever context the caller passes via a format-able label parameter if convenient (or let `load_spec_on_tamad` wrap the error with `.context(...)`).
- `None` before ever seeing a row → keep waiting.
- Deadline elapsed → `Err(anyhow!("backend did not become ready within {:?}", deadline))`.

2. In `load_spec_on_tamad`, after the successful `handle.load_model(&spec.request)` call:
   - Replace the "blocks until …" comment block with the new contract (RPC returns fast; readiness comes from wire rows).
   - Compute `deadline = Duration::from_secs(state.config.read().await.proxy.startup_timeout_secs.max(1))` and call the waiter with a status closure. **Closure capture discipline (compile trap):** `state` is a `&ProxyState`; a naive `|| async move { live_rows(state.tamad_pool()...) }` borrows `state` into the returned future, making `Fut` lifetime-parameterized, which will NOT unify with the single-`Fut` generic bound. Use owned clones instead — `state.tamad_pool()` returns an owned `Arc<TamadPool>` and `spec.backend_name` is cloneable:

```rust
let pool = state.tamad_pool();
let key = spec.backend_name.clone();
let status_of = || {
    let p = pool.clone();
    let k = key.clone();
    async move {
        crate::proxy::live_rows(p.as_ref()).await.row(&k).map(|r| r.status)
    }
};
```

   (The closure itself borrows `pool`/`key` only to clone them; each returned future owns its clones and is `'static`, so `Fut` unifies.) Size `gone_threshold` as `(15s / poll_every)` with `poll_every = 500ms` → 30 ticks; note in a comment that this exceeds 3× `LIVE_FRAME_MAX_AGE` so a stale frame never trips it.
   - On `Ok(())`: keep the existing `tracing::info!("model loaded on tamad", ...)`.
   - On `Err`: `return Err(e)` (the existing `on_load_error` plumbing in `ensure_model_loaded` and the management handler convert it).
   - Also update the stale rustdoc on `handle_tama_cancel_load` in `crates/tama-core/src/proxy/tama_handlers/models/handlers.rs` ("loads are short, so a cancel is best-effort") → e.g. "the load survives cancellation by construction: the tamad owns the health gate (plan-194); cancelling here abandons only this waiter".
   - Compatibility note to include in a code comment: an OLD tamad blocks the RPC until healthy, so by the time `handle.load_model` returns, the first row poll observes `ready` immediately — the waiter degenerates to a single poll.

3. Tests in `crates/tama-core/src/proxy/lifecycle/spec/tests.rs` targeting `wait_for_terminal_row` (closure-generic, no gRPC stub gymnastics needed for these):
   - ready on first poll → Ok
   - None,None,ready → Ok (row visibility lag tolerated before first sighting)
   - Some("starting") × N then None × gone_threshold → Err (died-after-sighting path; use small numbers: threshold 3, poll 10 ms)
   - budget_exhausted → Err where `.downcast_ref::<BudgetExhausted>().is_some()`
   - perpetual None (never seen) → deadline Err with `poll_every` ~10 ms and `deadline` ~100 ms
   - Integration fix REQUIRED for the existing stub harness test `test_load_spec_on_tamad_load_succeeds`: it drives `setup_stub_load(false, Some(1000ms))` whose `StubTamad` emits NO process rows (`stats_processes` defaults empty). After Task 3 the waiter would burn the full 120 s default deadline and fail. Extend `setup_stub_load` to seed `stats_processes` with one `ProcessInfo { model_name: "test-model", alive: true, status: "ready", .. }` (the stub's stats stream already clones `stats_processes` into every frame, so `live_rows` sees it once fresh). NOTE: shortening `startup_timeout_secs` in the harness alone does NOT fix this test (the waiter still errs on perpetual-None) — seeding the row is mandatory. Failed-RPC tests are unaffected (they error before the waiter).

**Steps:**
- [ ] Write the five waiter tests first in `spec/tests.rs`; run `cargo nextest run --package tama-core -- proxy::lifecycle::spec` and confirm compile-fail/fail (helper doesn't exist yet)
- [ ] Implement `wait_for_terminal_row` + wire it into `load_spec_on_tamad`; update the stale comment block
- [ ] Run `cargo nextest run --package tama-core`
  - Did all pass? The stub-harness fix from item 3 (seeding `stats_processes` with a ready `test-model` row) is mandatory — shortening `startup_timeout_secs` alone does NOT work (the waiter still errs on perpetual-None).
- [ ] Run `cargo fmt --all` && `cargo clippy --workspace --all-targets -- -D warnings` && `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --workspace`
- [ ] Commit with message: `feat(proxy): wait on wire rows for load outcome; tolerate early-starting RPC responses`

**Acceptance criteria:**
- [ ] `ensure_model_loaded` still resolves only when the backend row is `ready` (chat requests cannot hit a booting endpoint)
- [ ] A backend that dies mid-boot surfaces as an error well before the deadline: the seen-then-gone path fires after ~15 s of sustained row absence, and pure gate timeout still surfaces as the deadline error — both flow through the existing `on_load_error` path
- [ ] New helper fully covered by unit tests; whole workspace green

---

### Task 4: tamad reconciliation sweep for orphaned `starting` rows

**Context:**
Defense in depth. Rows can still end up stranded in `starting` **within a live tamad process**: bugs in older deployed binaries (exactly the incident that motivated this plan), a spawn-frame race, or any future code path that forgets the detached-settle discipline. (Note: `ProcessTable` is in-memory and NOT restored across restarts — a crash simply loses rows, so cross-restart scenarios are out of scope.) The sweep periodically inspects `starting` rows whose launch is older than a grace period: if the backend answers its health URL, adopt it as a *verified* ready (same bookkeeping as a real gate pass, including the persisted-tally reset); if it is far past its health deadline, tear it down (kill process group for native, stop+remove container for Docker) and record `failed`. This turns "stranded starting row" from a permanent wedged state into a self-healing blip.

**Files:**
- Modify: `crates/tamad/src/lifecycle.rs` (new `start_starting_reconciler` + `reconcile_once`)
- Modify: `crates/tamad/src/main.rs` (start the reconciler next to the existing `TamadLifecycle::start_respawn_supervisor(&lifecycle)` call)
- Test: `crates/tamad/src/lifecycle.rs` test module

**What to implement:**

1. Public API on `TamadLifecycle`:

```rust
/// Starts the detached reconciliation sweep for orphaned `starting`
/// rows (plan-194). Runs every 5 seconds until the lifecycle drops.
pub fn start_starting_reconciler(lifecycle: &Arc<TamadLifecycle>) -> tokio::task::JoinHandle<()>
```

built exactly like `start_respawn_supervisor` (clone the `Arc`, `tokio::spawn`, loop on a `tokio::time::interval(Duration::from_secs(5))`), delegating each tick to:

```rust
/// One reconciliation pass over all `starting` rows. Exposed (pub(crate))
/// for deterministic testing.
pub(crate) async fn reconcile_once(&self)
```

with the injectable-knobs variant that `reconcile_once` delegates to (TWO parameters — grace AND deadline floor — so tests never sleep real production timeouts):

```rust
/// Test-injectable core. Production: grace = 10s, min_deadline = 120_000ms.
pub(crate) async fn reconcile_once_with(&self, grace: Duration, min_deadline: Duration)
```

2. `reconcile_once` logic, for each entry from `self.table.list()` with `entry.status == status::STARTING`:

   - Let `spec = entry.spec.clone()` (full `LoadModelRequest` is stored on every entry — see `ProcessEntry.spec`), `age = entry.started_at.elapsed()`.
   - **Gate-less specs** (`health_url` empty or `health_timeout_ms == 0`): skip entirely — those were meant to be instant-ready; a lingering one indicates an in-flight operation too young to judge. Do not touch.
   - **Grace period**: skip while `age < Duration::from_secs(10)` (an active detached gate from Task 2 owns the row until then; the sweep must not race it. Both writers guard with `owns_row`, but skipping avoids redundant pings).
   - **Health probe**: `crate::process::check_health(&spec.health_url, Some(5)).await` — if it succeeds (`status().is_success()`), and `self.owns_row(&entry.model_name, entry.pid).await`, insert `Self::entry_for(&spec, entry.pid, status::READY, false, &Some(entry.clone()))` and run the verified-success bookkeeping: `self.store.zero_persisted_restart_count(&entry.model_name)` with the same warn-on-error text style as the settle path. Log `info!(model, pid, "reconciler adopted orphaned starting row as ready")`.
   - **Deadline breach**: if `age > max(2 × health_timeout_ms, min_deadline)` (production `min_deadline` = 120_000 ms):
     - Teardown: if `spec.docker_config_json` is non-empty → `runner::stop_container(&runner::container_name_for(&entry.model_name))` then `remove_container(...)` (both best-effort `let _ =`); else → `kill_process_group(entry.pid)`, brief 250 ms sleep, `is_process_group_alive` check, `force_kill_process_group` (mirror the native settle tail).
     - Then if `owns_row(...)`: insert `FAILED` via `Self::entry_for(&spec, entry.pid, status::FAILED, false, &Some(entry.clone()))`. Log `warn!(model, "reconciler tore down orphaned starting row past health deadline")`.
   - **Otherwise** (within grace/deadline, unhealthy): do nothing this tick — the detached gate may still land it.

   Ordering note: evaluate deadline-breach BEFORE health-probe per entry, so a corpse is torn down promptly even though its port would refuse anyway.

3. Wire-up in `crates/tamad/src/main.rs`: find the `TamadLifecycle::start_respawn_supervisor(&lifecycle)` call and add `let _reconciler = TamadLifecycle::start_starting_reconciler(&lifecycle);` beside it, keeping the existing binding style (the supervisor call site shows the pattern for keeping the join handle alive/dropped).

4. Tests (in `lifecycle.rs` test module, reusing the local TCP health-sniffer pattern from the existing gate tests; both call `reconcile_once_with(Duration::ZERO, small_deadline)` directly so no real sleeps are needed):
   - `test_reconciler_adopts_healthy_orphan`: build lifecycle + table; manually `table.insert` a `STARTING` entry whose `spec.health_url` points at the sniffer (accepting connections); backdate age by constructing the entry then using a helper that rewrites `started_at` (`started_at` is a pub `Instant` field — `checked_sub(Duration::from_secs(11))`), or simply call with `grace = Duration::ZERO`. Assert row becomes `READY` and persisted-tally reset ran (assert via observable store state or absence of error).
   - `test_reconciler_fails_deadline_breach`: spec pointing at `http://127.0.0.1:1/health`, `health_timeout_ms: 500`; pass `min_deadline = Duration::from_millis(100)` and an aged/backdated entry. Native path (no docker json): expect `FAILED` row and dead pid. Skip the docker-teardown assertion in tests (docker unavailable in CI); cover the branch by code inspection only.
   - `test_reconciler_skips_young_and_gainless`: gate-less spec row and a fresh (<grace) gated row remain untouched after a pass with `grace = Duration::from_secs(10)`.

**Steps:**
- [ ] Write the three tests first (they won't compile — `start_starting_reconciler` missing), confirm failure
- [ ] Implement `reconcile_once_with`, `reconcile_once`, `start_starting_reconciler` + main.rs wiring
- [ ] Run `cargo nextest run --package tamad -- lifecycle`
  - Did the new tests pass and all prior tests stay green? Fix and re-run if not.
- [ ] Run `cargo fmt --all` && `cargo clippy --package tamad --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --workspace`
- [ ] Commit with message: `feat(tamad): reconcile orphaned starting rows (adopt healthy, tear down expired)`

**Acceptance criteria:**
- [ ] A `starting` row whose backend answers health becomes `ready` within one sweep, with verified-ready tally semantics
- [ ] A `starting` row past its health deadline is torn down and marked `failed` — no leaked GPU memory containers/process groups
- [ ] Active gates, gate-less rows, and young rows are never touched
- [ ] Whole workspace green; both clippy targets clean

---

## Rollout / verification notes

- Deploy order is irrelevant (wire formats unchanged; compatibility matrix in the contract section covers mixed versions). Deploying the proxy first is marginally safer.
- Post-deploy verification on `tama` (root): start a big vLLM model, kill the initiating HTTP connection seconds in, then `journalctl -u tamad.service -f` — expect the detached gate to keep polling and log the transition via the `"backend became healthy (detached gate)"` info! added in Task 1, and `GET /tama/v1/models` to show `state: "ready"` without any surviving requester.
- The incident reproduction (stuck `starting` + silent gate death) is impossible after Task 2 by construction; Task 4 additionally heals anything that predates the fix.

## Explicit non-goals

- No changes to `handle_tama_load_model`'s synchronous shape (shielding is unnecessary once tamad owns the gate).
- No SSE/job-stream progress reporting for loads (separate concern).
- No changes to restart-budget accounting rules — the sweep reuses the established verified-ready/instant-ready semantics verbatim.
- No early-abort for a detached gate whose row disappears (e.g. unload during boot): it keeps polling a dead port until its own deadline, which is bounded and `owns_row`-guarded, hence harmless. Revisit only if observed in practice.
