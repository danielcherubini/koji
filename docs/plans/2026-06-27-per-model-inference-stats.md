# Per-Model Inference Stats Plan

**Goal:** Make tok/s display per-model (not global) on GPU cards, and always show `0 tok/s` when no inference has occurred.

**Architecture:** Change the single global `inference_stats` watch channel in `ProxyState` to a `HashMap<String, LatestInferenceStats>` keyed by server name. Enrich each `ModelStatus` with its server's latest `tps`/`prompt_tps`. The frontend reads per-model stats from `ModelStatus` and passes them to the correct GPU card.

**Tech Stack:** Rust (tama-core, tama-web), Leptos (WASM frontend)

---

## Root Cause

1. `ProxyState.inference_stats` is a single `watch::Sender<Option<LatestInferenceStats>>` — every llama_cpp response overwrites it
2. The metrics collector embeds this single global value into `MetricSample.tps`/`MetricSample.prompt_tps`
3. The dashboard passes these global values identically to every `GpuDeviceCard`
4. The GPU card conditionally hides the throughput section when both values are `None`

---

### Task 1: Backend — Per-server inference stats storage

**Context:**
The core issue is that `ProxyState` stores inference stats as a single global value. When multiple models are loaded (each with its own llama_cpp server), the last response wins. This task changes storage to a HashMap keyed by server name, so each server's stats are tracked independently.

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/forward.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`

**What to implement:**

1. In `proxy/types.rs`, change `ProxyState.inference_stats` from:
   ```rust
   pub inference_stats: tokio::sync::watch::Sender<Option<LatestInferenceStats>>
   ```
   to:
   ```rust
   pub inference_stats: tokio::sync::watch::Sender<HashMap<String, LatestInferenceStats>>
   ```
   The key is `server_name` (the alias_name used in `state.models`).

2. In `proxy/state.rs` (line 48), update the channel initialization from:
   ```rust
   inference_stats: tokio::sync::watch::channel(None).0,
   ```
   to:
   ```rust
   inference_stats: tokio::sync::watch::channel(HashMap::new()).0,
   ```

3. In `proxy/types.rs`, update `ProxyState::shutdown()` (line 318) from:
   ```rust
   let _ = self.inference_stats.send_replace(None);
   ```
   to:
   ```rust
   let _ = self.inference_stats.send_replace(HashMap::new());
   ```

4. In `proxy/forward.rs`, modify `extract_inference_stats` to accept a `server_name: &str` parameter:
   ```rust
   pub fn extract_inference_stats(
       server_name: &str,
       json: &serde_json::Value,
       inference_stats: &tokio::sync::watch::Sender<HashMap<String, LatestInferenceStats>>,
   ) -> Option<LatestInferenceStats> {
   ```
   The function inserts into the HashMap and sends the updated map:
   ```rust
   let mut map = inference_stats.borrow().clone();
   map.insert(server_name.to_string(), stats);
   inference_stats.send_replace(map);
   Some(stats)
   ```

5. **Preserve per-server sticky `spec_decoding_active` flag**: The current code reads the previous global `spec_decoding_active` to make it sticky. After the refactor, read the previous value for THIS server only:
   ```rust
   let prev_active = inference_stats
       .borrow()
       .get(server_name)
       .map(|s| s.spec_decoding_active)
       .unwrap_or(false);
   ```

6. Update ALL call sites of `extract_inference_stats`:
   - `process_sse_line` — needs `server_name: &str` parameter (non-optional) passed through
   - Non-streaming response handler (line ~449) — already has `server_name` in scope
   - Streaming response handler (inside `unfold`) — needs `server_name` captured
   - All test functions that call `extract_inference_stats` (3 tests)
   - All test functions that call `process_sse_line` (9 tests in the `process_sse_line tests` module)
   - `test_inference_stats_watch_round_trip` in `types.rs` — update to use `HashMap<String, LatestInferenceStats>` channel type

7. In `process_sse_line`, add `server_name: &str` parameter (non-optional) and pass it through to `extract_inference_stats`.

8. In the streaming response handler, capture `server_name` from `forward_request`'s parameter and pass it through the Arc.

**Steps:**
- [ ] Modify `ProxyState.inference_stats` type to `HashMap<String, LatestInferenceStats>`
- [ ] Update initialization in `ProxyState::new()`
- [ ] Modify `extract_inference_stats` signature to accept `server_name: &str` and insert into HashMap
- [ ] Update `process_sse_line` to accept and forward `server_name`
- [ ] Update streaming handler to capture and pass `server_name`
- [ ] Update non-streaming handler to pass `server_name`
- [ ] Update all test functions
- [ ] Run `cargo build --workspace` — fix any compilation errors
- [ ] Run `cargo test --package tama-core -- proxy::forward` — verify forward tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: per-server inference stats storage (HashMap keyed by server_name)"

**Acceptance criteria:**
- [ ] `inference_stats` is a `HashMap<String, LatestInferenceStats>` in ProxyState
- [ ] Each llama_cpp response updates only its own server's entry in the HashMap
- [ ] All existing tests pass (updated for new signature)
- [ ] No clippy warnings

---

### Task 2: Backend — Enrich ModelStatus with per-model tps

**Context:**
Now that inference stats are tracked per-server, we need to surface them per-model. `ModelStatus` already flows per-model through `MetricSample.models` to the frontend. Adding `tps`/`prompt_tps` fields to `ModelStatus` gives the frontend exactly what it needs — per-model inference stats attached to the model they belong to.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs`
- Modify: `crates/tama-core/src/proxy/status.rs`
- Modify: `crates/tama-core/src/proxy/server/metrics.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs`

**What to implement:**

1. In `gpu/system.rs`, add fields to `ModelStatus`:
   ```rust
   /// Token generation speed for this model's backend (tokens per second).
   /// None if the model is not actively generating or no stats observed yet.
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub tps: Option<f32>,
   /// Prompt processing speed for this model's backend (tokens per second).
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub prompt_tps: Option<f32>,
   ```

2. In `proxy/status.rs`, modify `collect_model_statuses` to look up each model's server stats from the HashMap. At the start of the function, borrow the HashMap once:
   ```rust
   let inference_stats = self.inference_stats.borrow();
   ```
   Inside the loop, after building `ModelStatus`, look up the first matching server's stats:
   ```rust
   let server_stats = servers.iter()
       .find_map(|(sn, _, _)| inference_stats.get(sn));
   // first-server-wins: for the current usage (one server per model) this is sufficient
   status.tps = server_stats.and_then(|s| s.tps);
   status.prompt_tps = server_stats.and_then(|s| s.prompt_tps);
   ```

3. **Clear inference_stats entry on model unload**: In `proxy/lifecycle/mod.rs`, in `unload_model` (line ~453), after the model is removed from `state.models`, also remove the server's entry from the inference_stats HashMap:
   ```rust
   // After removing from state.models:
   self.inference_stats.send_replace(
       self.inference_stats.borrow().iter()
           .filter(|(k, _)| *k != server_name)
           .map(|(k, v)| (k.clone(), *v))
           .collect(),
   );
   ```
   This prevents stale stats from persisting after a model is unloaded.

4. **Global `MetricSample.tps`/`prompt_tps` rule**: In `proxy/server/metrics.rs`, the metrics collector reads `inference_stats` to populate the global `MetricSample.tps`/`prompt_tps` fields used by sparkline charts. With the HashMap, pick the most-recently-updated server's stats:
   ```rust
   let inference = metrics_state.inference_stats.borrow();
   let latest = inference.values()
       .max_by_key(|s| s.last_updated_ms)
       .copied();
   // Then use latest.and_then(|s| s.tps) etc.
   ```
   This preserves backward compat for anything reading the global fields (sparklines, Prometheus), while the frontend uses per-model data from `ModelStatus`.

5. Update the `row_into_sample` function in metrics.rs — it seeds from DB rows which don't have per-model tps. This is fine since `models` is already `vec![]` for seeded samples.

**Steps:**
- [ ] Add `tps: Option<f32>` and `prompt_tps: Option<f32>` to `gpu::ModelStatus` in `system.rs`
- [ ] Update `collect_model_statuses` in `status.rs` to populate these fields from the HashMap
- [ ] Keep global `tps`/`prompt_tps` on `MetricSample` for backward compat (unchanged)
- [ ] Run `cargo build --workspace` — fix compilation errors
- [ ] Run `cargo test --package tama-core -- proxy::status` — verify status tests pass
- [ ] Run `cargo test --package tama-core -- gpu::system` — verify system tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add per-model tps/prompt_tps to ModelStatus"

**Acceptance criteria:**
- [ ] `ModelStatus` has `tps` and `prompt_tps` fields (both `Option<f32>`)
- [ ] `collect_model_statuses` populates these from the per-server HashMap
- [ ] Global `MetricSample.tps`/`prompt_tps` still work (backward compat)
- [ ] All existing tests pass

---

### Task 3: Frontend — Wire per-model stats to GPU cards + always show throughput

**Context:**
The frontend needs to (a) accept the new per-model fields from `ModelStatus`, (b) pass the correct stats to each GPU card based on which model is loaded on that GPU, and (c) always render the throughput section (showing `0 tok/s` when idle).

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/metrics.rs`
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs`
- Modify: `crates/tama-web/src/components/gpu_device_card.rs`

**What to implement:**

1. In `pages/dashboard/metrics.rs`, add fields to the frontend `ModelStatus`:
   ```rust
   #[serde(default)]
   pub tps: Option<f32>,
   #[serde(default)]
   pub prompt_tps: Option<f32>,
   ```

2. In `components/gpu_device_card.rs`, add a public helper to find the model for a given GPU device:
   ```rust
   /// Find the first model targeting `device_id` (e.g. "GPU0").
   /// Models without `gpu_device` set fall back to the first GPU ("GPU0").
   pub fn model_for_device<'a>(
       loaded_models: &'a [ModelStatus],
       device_id: &str,
   ) -> Option<&'a ModelStatus> {
       loaded_models.iter().find(|m| match &m.gpu_device {
           Some(g) if g == device_id => true,
           None if device_id == "GPU0" => true,
           _ => false,
       })
   }
   ```
   This reuses the matching logic from `loaded_model_display` and `derive_device_state` in a single shared function.

3. In `pages/dashboard/mod.rs`, in the GPU cards rendering section:
   - Import `model_for_device` from `gpu_device_card`.
   - Instead of passing global `prompt_tps` and `tps_val` to every card, look up the loaded model for each GPU and use its per-model stats:

   ```rust
   use crate::components::gpu_device_card::{device_display_label, model_for_device, model_gpu_label, GpuDeviceCard};
   ```

   Replace the GPU card rendering loop with:
   ```rust
   {gpus.into_iter().enumerate().map(|(idx, gpu)| {
       let label = device_display_label(idx);
       let models = loaded_models.clone();
       let loaded_for_gpu = model_for_device(&models, &gpu.device_id);
       let gpu_prompt_tps = loaded_for_gpu.and_then(|m| m.prompt_tps);
       let gpu_tps = loaded_for_gpu.and_then(|m| m.tps);
       view! {
           <GpuDeviceCard
               device=gpu
               display_label=label
               loaded_models=models
               prompt_tps=gpu_prompt_tps
               tps=gpu_tps
           />
       }
   }).collect::<Vec<_>>()}
   ```

4. In `components/gpu_device_card.rs`:
   - Remove the conditional rendering gate (`if prompt_tps.is_some() || tps.is_some()`). The throughput section should always render for `Active` and `Loading` states.
   - Change the fallback from `"—"` to `"0 tok/s"`:
     ```rust
     // Before:
     {prompt_tps.map(|v| format!("{v:.0} tok/s")).unwrap_or_else(|| "—".to_string())}
     // After:
     {prompt_tps.map(|v| format!("{v:.0} tok/s")).unwrap_or_else(|| "0 tok/s".to_string())}
     ```
     Same for `tps`.

   The throughput section should look like:
   ```rust
   {match state {
       GpuDeviceState::Active | GpuDeviceState::Loading => {
           view! {
               <div class="gpu-device-card__throughput">
                   <div class="gpu-device-card__inference-cell">
                       <div class="gpu-device-card__inference-value">
                           {prompt_tps.map(|v| format!("{v:.0} tok/s")).unwrap_or_else(|| "0 tok/s".to_string())}
                       </div>
                       <div class="gpu-device-card__inference-label">"Processing"</div>
                   </div>
                   <div class="gpu-device-card__inference-cell">
                       <div class="gpu-device-card__inference-value">
                           {tps.map(|v| format!("{v:.0} tok/s")).unwrap_or_else(|| "0 tok/s".to_string())}
                       </div>
                       <div class="gpu-device-card__inference-label">"Generation"</div>
                   </div>
               </div>
           }.into_any()
       }
       _ => view! { <span/> }.into_any()
   }}
   ```

5. Also update the test in `gpu_device_card.rs` — the `make_model` helper needs `tps` and `prompt_tps` fields (both `None` by default).

**Steps:**
- [ ] Add `tps` and `prompt_tps` fields to frontend `ModelStatus` in `metrics.rs`
- [ ] Update dashboard GPU card rendering to look up per-model stats from `loaded_models`
- [ ] Remove conditional rendering gate in `gpu_device_card.rs` — always show throughput for Active/Loading
- [ ] Change fallback from `"—"` to `"0 tok/s"` for both Processing and Generation
- [ ] Update test helpers to include new fields
- [ ] Run `cargo build --workspace` — fix compilation errors
- [ ] Run `cargo test --package tama-web` — verify web tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: per-model tok/s on GPU cards + always show throughput"

**Acceptance criteria:**
- [ ] Each GPU card shows tok/s from its own loaded model (not global)
- [ ] When no inference has occurred, throughput shows `0 tok/s` (not hidden)
- [ ] GPU cards without a loaded model (Idle state) still hide throughput section
- [ ] All tests pass

---

## Verification Steps (after all tasks)

1. Load two models on two GPUs
2. Run inference on one model — verify only that GPU's card shows non-zero tok/s
3. Run inference on the other model — verify its card updates independently
4. Stop all inference — verify both cards show `0 tok/s` (not hidden)
5. Run `cargo check --workspace && cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace`
