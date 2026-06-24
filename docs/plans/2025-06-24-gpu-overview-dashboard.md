# GPU Overview Dashboard Plan

**Goal:** Surface per-GPU metrics and model placement on the dashboard, so users can see at a glance which models are running on which GPUs and how each GPU is utilized.

**Architecture:** Extend the existing metrics pipeline (nvidia-smi + AMD sysfs) to expose per-device stats. The `MetricSample` SSE payload gains a `gpus: Vec<GpuDeviceStats>` field. A new `GpuDeviceCard` component renders one card per detected GPU on the dashboard, showing utilization/VRAM bars (purple→pink gradient), the loaded model (or "No model loaded" / "Transferring..."), and Temp/Power/Fan telemetry. The GPU's position in the array becomes its display label (e.g. "GPU 0", "GPU 1") used in both card headers and the model row's "VRAM: Loaded (GPU 0)" cross-link. The underlying `ModelConfig.gpu_device` (e.g. `CUDA0`, `ROCm0`) stays internal and is used only to dispatch the `--device` flag.

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

1. In `components/gpu_device_card.rs`, define the state enum:
   ```rust
   /// Lifecycle state of a GPU device, derived from loaded model states.
   /// Mirrors the mockup's status badges: ACTIVE / IDLE / LOADING.
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum GpuDeviceState {
       /// At least one ready/loading model is targeting this device.
       Active,
       /// A model is currently loading onto this device (transferring VRAM).
       Loading,
       /// A model targeting this device is in failed state.
       Failed,
       /// No model is loaded on this device, but the device is healthy.
       Idle,
   }
   ```
   Note: `Loading` takes precedence over `Failed` (a model that started loading but errored shows the most recent meaningful state — see `derive_device_state` logic below).

2. Pure helper functions (testable in isolation):
   ```rust
   /// Derive the device state from a list of models, given the target device_id
   /// (the llama.cpp device name, e.g. "CUDA0", "ROCm0").
   pub fn derive_device_state(loaded_models: &[ModelStatus], device_id: &str) -> GpuDeviceState

   /// Returns the display label for a GPU device, e.g. "GPU 0", "GPU 1".
   /// This is the position in the `gpus: Vec<GpuDeviceStats>` array, NOT
   /// the underlying `device_id` (e.g. "nvidia0"). The display label is
   /// what the model row uses for "VRAM: Loaded (GPU 0)".
   pub fn device_display_label(index: usize) -> String

   /// Returns the index of the GPU device whose `device_id` matches the
   /// given `gpu_device` value (e.g. "CUDA0"), or `None` if no match.
   /// Used to convert from `ModelStatus.gpu_device` to a position-based label.
   pub fn find_device_index(gpus: &[GpuDeviceStats], gpu_device: &str) -> Option<usize>

   /// Returns the display label of the GPU a model is loaded on, e.g.
   /// Some("GPU 0"). Returns None if the model has no `gpu_device` or no
   /// matching device is found.
   pub fn model_gpu_label(gpus: &[GpuDeviceStats], model: &ModelStatus) -> Option<String>

   /// Returns the first model targeting `device_id` that is in `ready`,
   /// `loading`, or `unloading` state, with a synthetic "TRANSFERRING…"
   /// prefix when state is `loading`. Returns None if no such model exists.
   pub fn loaded_model_display(
       loaded_models: &[ModelStatus],
       device_id: &str,
   ) -> Option<LoadedModelDisplay>
   ```
   where:
   ```rust
   pub struct LoadedModelDisplay {
       pub name: String,
       pub transferring: bool,  // true → render as "TRANSFERRING… <name>"
   }
   ```
   Logic for `derive_device_state`:
   - If any model with `gpu_device == Some(device_id)` has `state == "loading"` → `Loading`
   - Else if any has `state == "ready" | "unloading"` → `Active`
   - Else if any has `state == "failed"` → `Failed`
   - Else → `Idle`

3. Helper to format VRAM string (e.g. `"22.4 / 24 GB"`):
   ```rust
   pub fn format_vram_short(vram: &VramInfo) -> String
   ```
   Converts MiB to GB with 1 decimal, e.g. `22937 MiB / 24576 MiB` → `"22.4 / 24 GB"`.

4. The component itself (`#[component] pub fn GpuDeviceCard(...)`):
   - Props: `device: GpuDeviceStats`, `display_label: String` (e.g. "GPU 0"), `loaded_models: Vec<ModelStatus>`
   - Computes `state` via `derive_device_state(&loaded_models, &device.device_id)`
   - Computes `loaded` via `loaded_model_display(&loaded_models, &device.device_id)`
   - Renders:
     - **Header:** `display_label` (e.g. "GPU 0") on the left, status badge on the right (badge text: `Active`→`ACTIVE`, `Loading`→`LOADING`, `Idle`→`IDLE`, `Failed`→`FAILED`)
     - **Utilization row:** label "Utilization" + percentage + purple→pink gradient progress bar
     - **VRAM row:** label is **dynamic** — `"VRAM"` for `Active`/`Idle`/`Failed` states, `"VRAM Allocation"` for `Loading` state. Value formatted via `format_vram_short`.
     - **Model section:** sub-header is `"LOADED MODEL"` for non-Loading states, `"TRANSFERRING…"` for Loading. Body: model name, or "No model loaded" (Idle), or empty (Failed).
     - **Telemetry row:** 3 cells (Temp °C, Power W, Fan %), each with a small label above the value
   - Wraps in `div class="card gpu-device-card"`

5. In `components/mod.rs`, add `pub mod gpu_device_card;` and `pub use gpu_device_card::*;`

6. In `css/12-gpu-device-card.css`, add styles. Match the existing Tama dark theme (NOT the mockup's forest-green theme). Use:
   - Background: `var(--bg-tertiary)`
   - Border: `var(--border-color)`
   - Border-radius: `var(--radius-lg)` (12px)
   - Status badge colors: reuse `var(--accent-green|yellow|red|gray)`
   - **Progress bars: purple→pink gradient** (matches the mockup's GPU section):
     ```css
     .gpu-device-card .bar-fill--gpu {
       background: linear-gradient(90deg, var(--accent-purple) 0%, var(--accent-pink) 100%);
     }
     ```
   - Layout: CSS grid for the 3 telemetry cells, gap `var(--space-md)`
   - Import the new file from wherever the other CSS files are bundled

7. In `pages/dashboard/tests.rs` (or create), add unit tests:
   - `test_derive_state_active_when_ready_model` — one ready model on device → Active
   - `test_derive_state_loading_when_loading_model` — one loading model → Loading
   - `test_derive_state_failed_when_only_failed` — one failed model → Failed
   - `test_derive_state_idle_when_no_models` — empty list → Idle
   - `test_derive_state_loading_overrides_ready` — both loading and ready → Loading
   - `test_device_display_label_format` — index 0 → "GPU 0", index 3 → "GPU 3"
   - `test_find_device_index_match` — gpus=[nvidia0, nvidia1], find "nvidia1" → Some(1)
   - `test_find_device_index_no_match` — gpus=[nvidia0], find "ROCm0" → None
   - `test_model_gpu_label_resolves_to_position` — model.gpu_device="nvidia0", gpus=[nvidia0, nvidia1] → Some("GPU 0")
   - `test_loaded_model_display_transferring` — loading state → transferring=true
   - `test_loaded_model_display_active` — ready state → transferring=false
   - `test_loaded_model_display_none_when_idle` — empty list → None
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
- [ ] `GpuDeviceCard` renders display label (e.g. "GPU 0"), status badge, two progress bars, loaded model section, telemetry
- [ ] `derive_device_state()` handles all 4 states correctly
- [ ] `device_display_label(idx)` returns "GPU N" format
- [ ] `find_device_index()` matches `ModelStatus.gpu_device` to a position in the `gpus` array
- [ ] `model_gpu_label()` returns position-based label
- [ ] `loaded_model_display()` returns model name + transferring flag
- [ ] Progress bars use purple→pink gradient
- [ ] VRAM label is "VRAM Allocation" when state is `Loading`, else "VRAM"
- [ ] Model section header is "TRANSFERRING…" when state is `Loading`, else "LOADED MODEL"
- [ ] Idle device shows "No model loaded" placeholder
- [ ] All 13+ unit tests pass
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
                       <h2>"GPU Cluster Nodes"</h2>
                       <span class="text-muted">{format!("{} device(s)", gpus.len())}</span>
                   </div>
                   <div class="gpu-device-grid">
                       {gpus.into_iter().enumerate().map(|(idx, gpu)| {
                           let label = device_display_label(idx);
                           view! {
                               <GpuDeviceCard
                                   device=gpu
                                   display_label=label
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

2. Also update the existing **GPU stat card subtitle** (around line 175 in `dashboard/mod.rs`, where `"of 100%"` is hardcoded for the GPU card): when `latest.gpus` is non-empty, change the subtitle to:
   ```rust
   <div class="card-secondary">{format!("Aggregate Load · {} Nodes", latest.gpus.len())}</div>
   ```
   This mirrors the mockup's "GPU Total Utilization" card showing "Aggregate Load • 4 Nodes".

3. In `css/12-gpu-device-card.css`, add:
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

4. Add a comment near the section explaining that the section is hidden when no GPUs are detected (laptops, CPU-only servers).

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
- [ ] Section header is "GPU Cluster Nodes" (matches mockup, not "GPU Devices")
- [ ] Each card receives its position-based display label (e.g. "GPU 0")
- [ ] Existing "GPU Total Utilization" stat card subtitle reads "Aggregate Load · N Nodes" when `gpus` is non-empty
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
   /// Position-based GPU display label, e.g. "GPU 0" (the index in the
   /// `gpus` array), NOT the raw `gpu_device` value (e.g. "CUDA0").
   /// Derived in the dashboard by `model_gpu_label(gpus, model)`.
   pub gpu_label: Option<String>,
   ```

2. The `ModelCard` component already takes a `state: String` and renders a status badge. Refactor the row chrome so:
   - The card gets a **left border colored by state**:
     - `ready` → `var(--accent-green)`
     - `loading` → `var(--accent-yellow)`
     - `unloading` → `var(--accent-orange)` (or yellow — reuse existing)
     - `failed` → `var(--accent-red)`
     - `idle` (empty state) → `var(--border-color)` (no special color)
   - Add an optional `error_message: Option<String>` prop; if `Some` and state is `failed`, render the error below the metadata line in `var(--accent-red)` small text (matches the mockup's "Error: OOM — Insufficient VRAM..." line).

3. In the metadata line (where `Quant: Q4_K_M • Size: 42.4 GB • VRAM: Loaded (Node 0/1)` lives):
   - The existing metadata composition is a free function or inline string. Locate it.
   - Refactor to a helper `compose_model_metadata(model: &ModelStatus, gpu_label: Option<&str>) -> String` that returns:
     - `Quant: <quant> • Size: <size> GB • VRAM: <vram_state> (<gpu_label>)` when model is ready/loading/unloading
     - `Quant: <quant> • Size: <size> GB` when model is idle (no VRAM line, no GPU reference)
     - `Quant: <quant> • Size: <size> GB • <error>` when state is `failed` (replace VRAM with error)
   - The actual `vram_state` string is: `"Loaded"` for ready, `"Allocating"` for loading, `"Freeing"` for unloading. Use `gpu_label` like "GPU 0", omit parens if no label.

4. In `pages/dashboard/mod.rs` (around line 410, where `ModelPips` is constructed):
   ```rust
   let gpu_label = model_gpu_label(&latest.gpus, &m);
   let model_pips = ModelPips {
       gpu_variant: m.gpu_variant.clone(),
       cache_type_k: m.cache_type_k.clone(),
       cache_type_v: m.cache_type_v.clone(),
       spec_types: m.spec_types.clone(),
       gpu_label: gpu_label.clone(),
   };
   ```
   And pass `error_message: m.error_message.clone()` to `ModelCard`.

5. In `pages/models.rs` (or wherever `ModelCard` is also used — search for `ModelPips {` and `gpu_variant:`), pass `gpu_label: None` and `error_message: None` (other pages don't have live metric data to derive these from).

6. Add CSS for `.model-row--failed` and friends in `06-badges-list-card.css`:
   ```css
   .model-row {
     border-left: 4px solid var(--border-color);
     transition: border-color var(--transition-fast);
   }
   .model-row--ready { border-left-color: var(--accent-green); }
   .model-row--loading { border-left-color: var(--accent-yellow); }
   .model-row--unloading { border-left-color: var(--accent-orange); }
   .model-row--failed { border-left-color: var(--accent-red); }
   .model-row__error {
     color: var(--accent-red);
     font-size: 12px;
     margin-top: var(--space-xs);
   }
   ```
   Apply the modifier class based on state in the ModelCard view.

7. **Add `error_message: Option<String>` to `ModelStatus`** (in `gpu/system.rs`):
   ```rust
   /// Error message when `state == "failed"`, surfaced on the dashboard's
   /// model row. None otherwise.
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub error_message: Option<String>,
   ```
   Populate it from the proxy's model lifecycle when a load fails (e.g. OOM, missing model file, backend crash). The web mirror (`tama-web/src/pages/dashboard/metrics.rs::ModelStatus`) gets the same field. Search `proxy/tama_handlers/models.rs` and `proxy/lifecycle/` for the failure path and set `error_message` from the captured error.

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
- [ ] `ModelPips.gpu_label: Option<String>` field added (display label, not raw device name)
- [ ] `ModelStatus.error_message: Option<String>` field added in core and web, populated on failure
- [ ] Model card left border color matches state (green/yellow/red/gray)
- [ ] Model card shows "VRAM: Loaded (GPU 0)" (or state-equivalent) using position-based label
- [ ] Model card shows "Error: <message>" in red when state is `failed` and `error_message` is set
- [ ] Idle models show metadata WITHOUT VRAM line
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
- **Multi-node / multi-host support** (NODE_0, NODE_1, NODE_2, NODE_3 in the mockup imply separate machines). Tama is single-process. The section header is "GPU Cluster Nodes" (preserved for design fidelity) but the cards represent devices on the single host.
- **Sidebar restyle** (NaN% branding, rocket Deploy Instance CTA, Support/Help at bottom) — design system change.
- **Gradient text/shadows on stat cards** (some metric cards have soft glows) — visual polish.
- **Notification bell + cloud icon + user avatar in top-right** — partially exist (notification badge on Updates link), not all the way there.
- **"System Online" green-dot badge in the page header** — the existing badge says "ok" or "error" with the wrong text; this plan leaves it as-is.

## Rollback

- Task 1: Revert; existing aggregate fields still computed from per-device data.
- Task 2: Revert the `MetricSample.gpus` field addition; `#[serde(default)]` keeps older clients working.
- Task 3: Revert; `gpu_device` is optional everywhere.
- Task 4: Revert; component is isolated.
- Task 5: Revert; section is gated on `gpus.is_empty()` so removing the section reverts cleanly.
- Task 6: Revert; pip is purely additive.
