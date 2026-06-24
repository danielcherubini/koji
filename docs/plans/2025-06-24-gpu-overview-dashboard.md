# GPU Overview Dashboard Plan

**Goal:** Surface per-GPU metrics and model placement on the dashboard, so users can see at a glance which models are running on which GPUs and how each GPU is utilized.

**Architecture:** Extend the existing metrics pipeline (nvidia-smi + AMD sysfs) to expose per-device stats. The `MetricSample` SSE payload gains a `gpus: Vec<GpuDeviceStats>` field. A new `GpuDeviceCard` component renders one card per detected GPU on the dashboard, showing utilization/VRAM bars, loaded model(s), and temp/power/fan telemetry. Each `ModelStatus` carries its `gpu_device` so cards can show the cross-link.

**Tech Stack:** Rust, axum, leptos, nvidia-smi, AMD sysfs (`/sys/class/drm/card*`), SSE

---

### Task 1: Add per-GPU metrics collection in `tama-core::gpu`

**Context:**
The current `gpu::system.rs` only returns aggregate GPU stats (one `gpu_utilization_pct`, one `VramInfo`). Both nvidia-smi and AMD sysfs expose per-device data, but the existing `query_nvidia_gpu_utilization()` and `query_nvidia_vram()` helpers return only the first line of output. To power a per-GPU overview card, we need to enumerate every GPU and return util/VRAM/temp/power/fan per device.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs` — replace single-GPU query with multi-GPU query
- Modify: `crates/tama-core/src/gpu/system.rs` — add `GpuDeviceStats` struct
- Modify: `crates/tama-core/src/gpu/system.rs` — update `SystemMetrics` to include `Vec<GpuDeviceStats>`
- Modify: `crates/tama-core/src/gpu/vram.rs` — add `query_vram_per_device() -> Vec<(device_id, VramInfo)>`
- Test: in-file `#[cfg(test)] mod tests`

**What to implement:**

1. In `gpu/system.rs`, add a new public struct (after `SystemMetrics`):
   ```rust
   /// Per-GPU device statistics for a single tick. One entry per detected
   /// device (NVIDIA or AMD). Order is stable per-tick: NVIDIA devices
   /// sorted by `index`, then AMD devices by `card` number.
   #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
   pub struct GpuDeviceStats {
       /// Stable device identifier reported by the driver (e.g. "nvidia0", "amd0").
       /// Mirrors llama.cpp's `--device` flag value (e.g. "CUDA0", "ROCm0").
       pub device_id: String,
       /// Human-readable vendor: "nvidia" | "amd".
       pub vendor: String,
       /// Utilization percentage (0–100), None if unavailable.
       pub utilization_pct: Option<u8>,
       /// VRAM usage in MiB, None if unavailable.
       pub vram: Option<VramInfo>,
       /// Edge temperature in °C, None if unavailable.
       pub temperature_c: Option<u8>,
       /// Power draw in watts, None if unavailable.
       pub power_w: Option<u16>,
       /// Fan speed percentage (0–100), None if unavailable.
       pub fan_pct: Option<u8>,
   }
   ```

2. In `gpu/vram.rs`, add a new public function:
   ```rust
   /// Query VRAM for all detected GPU devices.
   /// Returns one `VramInfo` per device, paired with a stable device_id
   /// matching `GpuDeviceStats::device_id`. Devices that fail to query
   /// are silently skipped (no VRAM data is not an error).
   pub fn query_vram_per_device() -> Vec<(String, VramInfo)>
   ```

3. In `gpu/system.rs`, add private helper `query_nvidia_devices() -> Vec<GpuDeviceStats>` that calls nvidia-smi with multi-line query:
   ```
   nvidia-smi --query-gpu=index,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,fan.speed \
              --format=csv,noheader,nounits
   ```
   For each line, parse the 7 fields and build a `GpuDeviceStats` with `device_id = format!("nvidia{index}")`, `vendor = "nvidia"`.

4. Add private helper `query_amd_devices() -> Vec<GpuDeviceStats>` that walks `/sys/class/drm/card*/device/` glob. For each card, read:
   - `gpu_busy_percent` (or fall back to `gpu_metrics` binary blob / `power_state`)
   - `mem_info_vram_used`, `mem_info_vram_total`
   - `temp1_input` or `temp2_input` (hwmon path varies; take first that exists)
   - `power1_average` (µW → W)
   - Fan: scan `../hwmon/hwmon*/fan1_input` (RPM) — convert to % best-effort, or return None
   `device_id = format!("amd{card_num}")`, `vendor = "amd"`.

5. Replace `query_gpu_utilization()` with `query_gpu_utilization_aggregate() -> Option<u8>` that returns the mean of per-device utilization (or the first device's value if only one).

6. Update `collect_system_metrics_with()` to:
   - Call new `query_nvidia_devices()` and `query_amd_devices()`, concat, sort by `(vendor, device_id)`
   - Add `gpus: Vec<GpuDeviceStats>` to `SystemMetrics`
   - `gpu_utilization_pct` becomes the mean of `gpus[].utilization_pct` (or None if empty)
   - `vram` becomes the sum of `gpus[].vram` (or None)

7. In `gpu/system.rs` test module, add unit tests:
   - `test_parse_nvidia_smi_csv_line` — feeds a 7-column line, asserts parsing
   - `test_nvidia_device_id_format` — index 0 → "nvidia0", index 12 → "nvidia12"
   - `test_aggregate_utilization_mean` — 4 devices at 50/60/70/80 → 65
   - `test_aggregate_utilization_empty` — empty list → None
   - `test_aggregate_vram_sum` — 2 devices at 4GB+8GB → 12GB total
   - `test_query_nvidia_devices_handles_missing_nvidia_smi` — returns empty (or skip, requires subprocess mocking — keep simple, just test the CSV parser)

8. The CSV parser is the only unit-testable piece. Extract it as a pure function:
   ```rust
   pub(crate) fn parse_nvidia_smi_csv_line(line: &str) -> Option<GpuDeviceStats>
   ```
   This makes the nvidia-smi subprocess call testable without mocking the process.

**Steps:**
- [ ] Add `GpuDeviceStats` struct in `gpu/system.rs` with full doc comments
- [ ] Add `query_vram_per_device()` in `gpu/vram.rs`
- [ ] Extract `parse_nvidia_smi_csv_line()` as pub(crate) function in `gpu/system.rs`
- [ ] Add `query_nvidia_devices()` and `query_amd_devices()` helpers
- [ ] Add `query_gpu_utilization_aggregate()` (rename of `query_gpu_utilization`)
- [ ] Update `SystemMetrics` struct to include `gpus: Vec<GpuDeviceStats>` field
- [ ] Update `collect_system_metrics_with()` to populate the new field
- [ ] Write 5+ unit tests for the parser and aggregate functions
- [ ] Run `cargo test --package tama-core -- gpu::`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add per-GPU device stats collection"

**Acceptance criteria:**
- [ ] `GpuDeviceStats` struct exists with all 7 fields and serde derives
- [ ] `query_vram_per_device()` returns `Vec<(String, VramInfo)>`
- [ ] `parse_nvidia_smi_csv_line()` is a pure function that handles malformed input
- [ ] `SystemMetrics.gpus: Vec<GpuDeviceStats>` field added
- [ ] `collect_system_metrics_with()` populates `gpus` with at least the detected devices
- [ ] Aggregate `gpu_utilization_pct` and `vram` are derived from per-device data
- [ ] All existing tests still pass
- [ ] `cargo clippy --workspace -- -D warnings` passes

---

### Task 2: Wire per-GPU data into the SSE MetricSample

**Context:**
The dashboard consumes metrics via SSE at `/tama/v1/system/metrics/stream`. The payload is `Vec<MetricSample>`. Adding `gpus` to `MetricSample` makes the per-device data available to the web layer. The change must be additive (default-empty) to keep older cached payloads working.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs` — add `gpus: Vec<GpuDeviceStats>` to `MetricSample`
- Modify: `crates/tama-core/src/proxy/server/mod.rs` — populate `gpus` when building live samples
- Modify: `crates/tama-core/src/proxy/server/mod.rs` — populate `gpus` when reading from DB (backfill empty)

**What to implement:**

1. In `gpu/system.rs::MetricSample`, add field (after `vram`):
   ```rust
   /// Per-GPU device stats for this sample. Empty if no GPU is detected
   /// or the backend does not support per-device queries. Always present
   /// (use `#[serde(default)]`) so older cached samples still deserialize.
   #[serde(default)]
   pub gpus: Vec<GpuDeviceStats>,
   ```

2. In `proxy/server/mod.rs` around line 143 (the live sample build), add:
   ```rust
   gpus: metrics.gpus.clone(),
   ```
   (where `metrics` is the freshly collected `SystemMetrics`).

3. In `proxy/server/mod.rs::row_into_sample()` around line 318 (DB-backed sample), add:
   ```rust
   gpus: vec![], // historical rows don't store per-GPU; left empty
   ```

4. No test changes needed — existing serialization tests will verify the field round-trips.

**Steps:**
- [ ] Add `gpus: Vec<GpuDeviceStats>` field to `MetricSample` with `#[serde(default)]`
- [ ] Populate `gpus` in live sample builder
- [ ] Populate `gpus: vec![]` in DB row mapper
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: expose per-GPU stats in MetricSample SSE payload"

**Acceptance criteria:**
- [ ] `MetricSample.gpus: Vec<GpuDeviceStats>` field present
- [ ] Live SSE samples include `gpus` (non-empty when GPUs are present)
- [ ] DB-backed samples include `gpus: []` (backward compat)
- [ ] All existing tests still pass

---

### Task 3: Add `gpu_device` to `ModelStatus` and populate from config

**Context:**
With per-GPU device IDs available (`nvidia0`, `amd0`) and `ModelConfig.gpu_device` storing the user's chosen device string (e.g. `CUDA0`, `ROCm0`), the `ModelStatus` struct needs to carry the `gpu_device` so the dashboard can show "Loaded (Node 0)" on the GPU card. The web layer already has access to the `ModelConfig` when building the status; we just need to plumb the field.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs` — add `gpu_device: Option<String>` to `ModelStatus`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models.rs` — populate `gpu_device` from config in `build_model_entry()`
- Modify: `crates/tama-web/src/pages/dashboard/metrics.rs` — mirror `gpu_device: Option<String>` in web `ModelStatus`
- Modify: `crates/tama-core/src/proxy/server/mod.rs` — propagate the field to live samples (where `model_statuses` is built)

**What to implement:**

1. In `gpu/system.rs::ModelStatus`, add field (after `spec_types`):
   ```rust
   /// GPU device name this model is bound to (e.g. "CUDA0", "ROCm0"),
   /// taken from `ModelConfig.gpu_device`. None if the model is idle,
   /// unconfigured, or the backend is not llama.cpp. Display-only.
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub gpu_device: Option<String>,
   ```

2. In `proxy/tama_handlers/models.rs::build_model_entry()`, when building the JSON entry, add a `gpu_device` field from the `cfg.gpu_device`:
   ```rust
   entry["gpu_device"] = serde_json::json!(cfg.gpu_device.clone());
   ```
   (This file builds JSON `Value`s, not structs, so we don't construct a `ModelStatus` here — we add the field to the JSON the dashboard will receive.)

3. In `proxy/server/mod.rs` around line 154, when constructing `model_statuses` for the live `MetricSample`, populate `gpu_device` from the loaded `ModelConfig`. Search the code to see if there's an existing helper that builds `ModelStatus` from config — if so, plumb it there; otherwise add the field at the construction site.

4. In `tama-web/src/pages/dashboard/metrics.rs::ModelStatus`, add the matching `gpu_device: Option<String>` field with `#[serde(default)]`.

5. In `dashboard/tests.rs` (or wherever `ModelStatus` is constructed in tests), add `gpu_device: None` to the test instances.

**Steps:**
- [ ] Add `gpu_device` field to `gpu::ModelStatus` (core) with serde defaults
- [ ] Add `gpu_device: None` to test instances of `ModelStatus` if any
- [ ] Add `gpu_device` to web `ModelStatus` mirror
- [ ] Populate `gpu_device` in `build_model_entry()` JSON output
- [ ] Populate `gpu_device` in live SSE sample builder
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: surface gpu_device on ModelStatus in dashboard SSE"

**Acceptance criteria:**
- [ ] `ModelStatus.gpu_device: Option<String>` field added in core and web
- [ ] `build_model_entry()` JSON output includes `gpu_device` from `ModelConfig`
- [ ] Live SSE samples carry the field
- [ ] All existing tests still pass

---

### Task 4: Add `GpuDeviceCard` component

**Context:**
The dashboard currently shows aggregate CPU/RAM/GPU/VRAM stat cards. The mockup in `Downloads/stitch_tama_gpu_overview_redesign.zip` shows a 4-card "GPU Cluster Nodes" grid. With per-GPU data now flowing, we can render one card per detected GPU showing utilization, VRAM, the loaded model, and telemetry. This component is reusable for future per-GPU views (e.g. a dedicated GPUs page).

**Files:**
- Create: `crates/tama-web/src/components/gpu_device_card.rs` — new component
- Modify: `crates/tama-web/src/components/mod.rs` — register the new component
- Create: `crates/tama-web/css/12-gpu-device-card.css` — scoped styles
- Modify: `crates/tama-web/src/pages/dashboard/tests.rs` — unit tests for status mapping

**What to implement:**

1. In `components/gpu_device_card.rs`, define:
   ```rust
   /// Lifecycle state of a GPU device, derived from loaded model states.
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum GpuDeviceState {
       /// At least one loaded/loading model is targeting this device.
       Active,
       /// No model is loaded on this device, but the device is healthy.
       Idle,
       /// A model targeting this device is currently loading.
       Loading,
       /// A model targeting this device is in failed state.
       Failed,
   }
   ```

2. Pure helper function (testable in isolation):
   ```rust
   pub fn derive_device_state(loaded_models: &[ModelStatus], device_id: &str) -> GpuDeviceState
   ```
   Logic:
   - If any model with `gpu_device == Some(device_id)` has `state == "loading"` → `Loading`
   - Else if any has `state == "failed"` → `Failed`
   - Else if any has `state == "ready" | "loading" | "unloading"` → `Active`
   - Else → `Idle`

3. Helper to format loaded model display name on a device:
   ```rust
   pub fn first_loaded_model_display(loaded_models: &[ModelStatus], device_id: &str) -> Option<String>
   ```
   Returns the first model with `gpu_device == Some(device_id)` AND `state == "ready" | "loading" | "unloading"`, using `model_display_name()` from `dashboard::metrics`.

4. Helper to format VRAM string (e.g. `"22.4 / 24 GB"`):
   ```rust
   pub fn format_vram_short(vram: &VramInfo) -> String
   ```
   Converts MiB to GB with 1 decimal, e.g. `22937 MiB / 24576 MiB` → `"22.4 / 24 GB"`.

5. The component itself (`#[component] pub fn GpuDeviceCard(...)`):
   - Props: `device: GpuDeviceStats`, `loaded_models: Vec<ModelStatus>`
   - Computes `state` via `derive_device_state`, `loaded_model` via `first_loaded_model_display`
   - Renders: header (device_id, status badge), two progress bars (utilization, VRAM), "LOADED MODEL" section with model name or "No model loaded", three telemetry cells (Temp °C, Power W, Fan %)
   - Mirrors existing card structure (use `ListCard` or plain `div class="card gpu-device-card"`)

6. In `components/mod.rs`, add `pub mod gpu_device_card;` and `pub use gpu_device_card::*;`

7. In `css/12-gpu-device-card.css`, add styles for the card. Match the existing Tama dark theme (NOT the mockup's forest-green theme — that's a separate scope). Use:
   - Background: `var(--bg-tertiary)`
   - Border: `var(--border-color)`
   - Border-radius: `var(--radius-lg)` (12px)
   - Status badge colors: reuse `var(--accent-green|yellow|red|gray)`
   - Progress bars: use existing `.progress` or `.bar` classes from `07-gauges-charts.css`
   - Layout: CSS grid for the 3 telemetry cells, gap `var(--space-md)`
   - Import the new file from wherever the other CSS files are bundled (check `index.html` / `lib.rs` for the CSS link list)

8. In `pages/dashboard/tests.rs` (or create), add unit tests:
   - `test_derive_state_active_when_ready_model` — one ready model on device → Active
   - `test_derive_state_loading_takes_precedence` — loading + failed → Loading
   - `test_derive_state_idle_when_no_models` — empty list → Idle
   - `test_derive_state_failed_when_only_failed` — one failed model → Failed
   - `test_first_loaded_model_prefers_ready_over_loading` — verify ordering
   - `test_format_vram_short` — 22937/24576 → "22.4 / 24 GB"

**Steps:**
- [ ] Create `GpuDeviceState` enum and `derive_device_state()` in `components/gpu_device_card.rs`
- [ ] Add `first_loaded_model_display()` and `format_vram_short()` helpers
- [ ] Implement `GpuDeviceCard` component
- [ ] Register component in `components/mod.rs`
- [ ] Create `css/12-gpu-device-card.css` with dark-theme styles
- [ ] Wire CSS into the bundle (find existing CSS import site)
- [ ] Write 6+ unit tests in `pages/dashboard/tests.rs`
- [ ] Run `cargo test --package tama-web`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add GpuDeviceCard component for dashboard"

**Acceptance criteria:**
- [ ] `GpuDeviceCard` renders device_id, status badge, two progress bars, loaded model, telemetry
- [ ] `derive_device_state()` handles all 4 states correctly
- [ ] Status badge color matches the state (green=active, yellow=loading, red=failed, gray=idle)
- [ ] All 6 unit tests pass
- [ ] CSS scoped to the new component, doesn't affect other cards
- [ ] `cargo clippy --workspace -- -D warnings` passes

---

### Task 5: Render GPU device section on dashboard

**Context:**
With `GpuDeviceCard` built and `MetricSample.gpus` flowing, we need to render the section on the dashboard between the top stat cards and the Models section. The mockup shows a section header "GPU Cluster Nodes" with 4 cards in a responsive grid.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs` — render the new section
- Modify: `crates/tama-web/css/12-gpu-device-card.css` — add grid layout for the section

**What to implement:**

1. In `dashboard/mod.rs`, after the `grid-stats` block (around line 215) and before the inference-stats block, add:
   ```rust
   // GPU Devices section — only rendered if any GPU data is present
   {if let Some(latest) = buf.last() {
       if !latest.gpus.is_empty() {
           let loaded_models = latest.models.clone();
           let gpus = latest.gpus.clone();
           view! {
               <section class="dashboard-gpus">
                   <div class="page-header">
                       <h2>"GPU Devices"</h2>
                       <span class="text-muted">{format!("{} device(s)", gpus.len())}</span>
                   </div>
                   <div class="gpu-device-grid">
                       {gpus.into_iter().map(|gpu| {
                           view! {
                               <GpuDeviceCard
                                   device=gpu
                                   loaded_models=loaded_models.clone()
                               />
                           }
                       }).collect::<Vec<_>>()}
                   </div>
               </section>
           }.into_any()
       } else {
           view! { <div></div> }.into_any()
       }
   } else {
       view! { <div></div> }.into_any()
   }}
   ```

2. In `css/12-gpu-device-card.css`, add:
   ```css
   .dashboard-gpus {
     margin: var(--space-lg) 0;
   }
   .gpu-device-grid {
     display: grid;
     grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
     gap: var(--space-md);
   }
   @media (min-width: 1200px) {
     .gpu-device-grid {
       grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
     }
   }
   ```

3. Add a comment near the section explaining that the section is hidden when no GPUs are detected (laptops, CPU-only servers).

**Steps:**
- [ ] Add `use crate::components::gpu_device_card::GpuDeviceCard;` to `dashboard/mod.rs`
- [ ] Render the `<section class="dashboard-gpus">` block in the dashboard view
- [ ] Add grid layout CSS
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Manually verify: run `cargo run --bin tama` (or check existing smoke test fixtures) and confirm the section renders. If no GPU is present, the section should be hidden.
- [ ] Commit with message: "feat: render GPU devices section on dashboard"

**Acceptance criteria:**
- [ ] Section renders one `GpuDeviceCard` per detected GPU
- [ ] Section is hidden when `latest.gpus` is empty
- [ ] Grid is responsive (1-2 cards on mobile, 3-4 on desktop)
- [ ] All existing tests still pass

---

### Task 6: Show `gpu_device` on existing model card rows

**Context:**
The mockup shows each active model row with "VRAM: Loaded (Node 0/1)" — i.e. the GPU device assignment is part of the metadata line. The existing `ModelCard` component already shows a `gpu_variant` pip; we add a `gpu_device` pip (or extend the metadata line) so users see which GPU a model is bound to.

**Files:**
- Modify: `crates/tama-web/src/components/model_card.rs` — add `gpu_device` to `ModelPips` and render
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs` — pass `gpu_device` into `ModelPips`
- Modify: `crates/tama-web/src/pages/models.rs` (or wherever `ModelCard` is also used) — pass `gpu_device`
- Modify: `crates/tama-web/css/06-badges-list-card.css` — style the new pip (if needed)

**What to implement:**

1. In `components/model_card.rs::ModelPips`, add field:
   ```rust
   pub gpu_device: Option<String>,
   ```

2. In the `ModelCard` component's metadata line (or pip row, wherever `gpu_variant` is rendered), add:
   ```rust
   {pips.gpu_device.as_ref().map(|d| view! {
       <span class="model-pip" title="GPU Device">"GPU: " {d.clone()}</span>
   })}
   ```
   Place it after `gpu_variant`. If `gpu_device` is `None`, don't render anything.

3. In `pages/dashboard/mod.rs` (around line 410, where `ModelPips` is constructed), add:
   ```rust
   gpu_device: m.gpu_device.clone(),
   ```

4. In `pages/models.rs` (or the equivalent file — search for `ModelPips {` and `gpu_variant:`), add the same field. Likely 1-3 sites.

5. If no existing `.model-pip` class, add minimal CSS in `06-badges-list-card.css`:
   ```css
   .model-pip {
     display: inline-block;
     padding: 2px 8px;
     background: var(--bg-tertiary);
     border: 1px solid var(--border-color);
     border-radius: var(--radius-sm);
     font-family: var(--font-mono);
     font-size: 11px;
     color: var(--text-secondary);
   }
   ```

**Steps:**
- [ ] Add `gpu_device` to `ModelPips` struct
- [ ] Render `gpu_device` in the `ModelCard` view
- [ ] Pass `gpu_device` from `dashboard/mod.rs` to `ModelPips`
- [ ] Pass `gpu_device` from other call sites to `ModelPips`
- [ ] Add CSS for `.model-pip` if not already present
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: show gpu_device pip on model cards"

**Acceptance criteria:**
- [ ] `ModelPips.gpu_device: Option<String>` field added
- [ ] Model card shows "GPU: <device>" pip when `gpu_device` is set
- [ ] No pip when `gpu_device` is `None`
- [ ] All call sites updated (dashboard, models page, any others)
- [ ] All existing tests still pass

---

## Execution Order

Tasks 1–3 are independent of the UI (data layer). Tasks 4–6 build the UI on top. Recommended order:

1. Task 1 (per-GPU data) → Task 2 (SSE wire) → Task 3 (gpu_device on ModelStatus)
2. Task 4 (component) → Task 5 (render section) → Task 6 (model card pip)

Task 4 has a soft dependency on Task 3 (the `ModelStatus.gpu_device` field is used in `derive_device_state`). Run Task 3 before Task 4.

## Out of Scope (Deferred)

The mockup in `Downloads/stitch_tama_gpu_overview_redesign.zip` is broader than this plan covers. Items deferred to future plans:

- **Full Material 3 Expressive theme** (forest-green palette, Karla + Noto Sans fonts, pill nav, gradient metric bars). This is a project-wide design system shift — separate plan.
- **Multi-node / multi-host support** (NODE_0, NODE_1, NODE_2, NODE_3 in the mockup imply separate machines). Tama is single-process. The "Devices" framing acknowledges this.
- **Per-device VRAM Allocation vs Total** (mockup shows "VRAM Allocation 12.4 / 24 GB" on loading node). The current data is used/total; can derive "allocation" later.
- **Restart, Pull Model, notification bell buttons** — already exist in the dashboard, no changes needed.
- **Sidebar restyle** (NaN% branding, rocket Deploy Instance CTA) — design system change.
- **Spec card backgrounds** (different gradient per status) — visual polish.

## Rollback

- Task 1: Revert; existing aggregate fields still computed from per-device data.
- Task 2: Revert the `MetricSample.gpus` field addition; `#[serde(default)]` keeps older clients working.
- Task 3: Revert; `gpu_device` is optional everywhere.
- Task 4: Revert; component is isolated.
- Task 5: Revert; section is gated on `gpus.is_empty()` so removing the section reverts cleanly.
- Task 6: Revert; pip is purely additive.
