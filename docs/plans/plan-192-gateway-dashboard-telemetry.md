# Gateway Dashboard Refactor & Telemetry Plan

**Goal:** Refactor Tama's dashboard from a generic OS monitor into an uncluttered AI Gateway & Compute Fabric Dashboard with active-models monitoring, redesigned inference host cards, and pure inference telemetry.

**Architecture:** The dashboard header integrates Gateway/Proxy status and quick cluster actions. The main body presents **Active Models** (currently loaded/starting inference endpoints with live tok/s and test/unload actions), followed by the **Inference Fleet** (redesigned `tamad` node cards with GPU VRAM bars, core utilization, thermals, and loaded model badges), followed by **Inference Telemetry** (Token Generation speed, Prompt Processing speed, and Cache & Speculative Decoding metrics replacing generic CPU/RAM charts). Full model catalog management is preserved exclusively on `/tama/models`.

**Tech Stack:** Rust, Leptos 0.7 (CSR/WASM + SSR), Trunk, CSS.

---

### Task 1: Redesign HostCard Component & Fix Telemetry Formatting Bugs

**Context:**
The current `HostCard` component in `crates/tama/src/components/host_card.rs` has several visual and formatting bugs visible on live GPU nodes:
1. Double unit label (`"28.6 GiB / 32 GiB GiB"`) caused by `format_bytes_gib` and `format_bytes_gib_rounded` both returning strings with `"GiB"`, which was then concatenated with another `"GiB"`.
2. Over-aggressive GPU name truncation (`Radeon A...`) caused by restrictive grid template columns in `21-dashboard-hosts.css`.
3. Progress bar visual clipping and overlapping text.
4. Missing indication of which model(s) are actively running on that specific node.

This task fixes the formatting bugs and redesigns the host card into a structured compute and GPU monitor.

**Files:**
- Modify: `crates/tama/src/components/host_card.rs`
- Modify: `crates/tama/src/pages/dashboard/mod.rs` (format helper cleanups if needed)
- Modify: `crates/tama/css/21-dashboard-hosts.css`
- Test: `crates/tama/src/components/host_card.rs` (or `crates/tama/src/pages/dashboard/tests.rs`)

**What to implement:**
1. **Fix VRAM formatting helper in `HostGpuRow`**:
   - `format_bytes_gib(bytes)` returns `"28.6 GiB"`; `format_bytes_gib_rounded(bytes)` returns `"32 GiB"`.
   - Update `vram_label` to format as `format!("{} / {}", format_bytes_gib(used), format_bytes_gib_rounded(total))` (producing `"28.6 GiB / 32 GiB"` without extra `"GiB"`).
2. **GPU name cleaning & flexible layout in `HostGpuRow`**:
   - Clean common vendor prefixes (e.g. `Advanced Micro Devices, Inc. [AMD/ATI]` -> `AMD`, or clean device name).
   - Change `host-gpu-row` layout in CSS to a structured, flexible row:
     - Top line: GPU index & name (e.g. `GPU 0 · Radeon Pro W7900`) + core utilization % + temperature.
     - Bottom line / dedicated bar: VRAM text (`28.6 / 32 GiB (89%)`) with a full-width progress bar.
3. **Host Card structure**:
   - Header: Host name + status badge (`● online` green pill or `● offline` red pill) + tamad version / transport.
   - Compute section: Compact dual progress bars for CPU % and RAM (`used / total GiB`).
   - GPU section: List of `HostGpuRow` items.
   - Active processes section: Optional prop with default on `HostCard`:
     ```rust
     #[prop(default = Vec::new())]
     running_models: Vec<String>,
     ```
     Displays a badge for models running on this node (e.g. `🟢 Running: Qwen3.8-27B-FP8 (Port 37115)`).
4. **CSS update in `crates/tama/css/21-dashboard-hosts.css`**:
   - Clean up grid template columns for `host-card-grid` (`repeat(auto-fit, minmax(360px, 1fr))`).
   - Style `host-gpu-row` with distinct sub-rows or flexible columns avoiding text clipping.

**Steps:**
- [ ] Write failing unit test in `crates/tama/src/components/host_card.rs` testing `HostGpuRow` VRAM label formatting (assert it does NOT contain duplicate `"GiB GiB"`).
- [ ] Run `cargo nextest run --package tama -- host_card`
  - Verify it fails as expected.
- [ ] Implement the VRAM label fix, GPU row layout, and `running_models` prop in `crates/tama/src/components/host_card.rs`.
- [ ] Update `crates/tama/css/21-dashboard-hosts.css` with the updated card layout and GPU row styles.
- [ ] Run `cargo nextest run --package tama -- host_card`
  - Verify all tests pass.
- [ ] Run `cargo clippy --package tama --all-targets -- -D warnings`
  - Verify clippy is clean.
- [ ] Commit with message: `fix(dashboard): redesign host card and fix telemetry formatting bugs`

**Acceptance criteria:**
- [ ] VRAM formatting outputs clean `X.X / YY GiB` without double unit strings.
- [ ] GPU rows display full/readable model names, VRAM usage bar, %, and thermals without clipping.
- [ ] Host cards optionally display currently running models for that node with default fallback.

---

### Task 2: Refactor Dashboard Active Models Section & Header Control Plane

**Context:**
Currently, the Dashboard duplicates the full model catalog with sort/group dropdowns, which pushes active workloads down and creates clutter. The Dashboard should focus solely on **currently active / loaded / starting models**, with quick actions (`[Unload]`, `[▷ Test]`) and a direct link to `/tama/models` for full catalog management.
Additionally, the standalone "Proxy" host card takes up an empty grid slot in Hosts; its version and uptime should move to the top-right header status pill.

**Files:**
- Modify: `crates/tama/src/pages/dashboard/mod.rs`
- Modify: `crates/tama/css/15-dashboard.css`
- Modify: `crates/tama/src/pages/dashboard/tests.rs`

**What to implement:**
1. **Header Control Plane (Top Bar)**:
   - Move proxy version + uptime into the header status pill: `● Gateway Online (v2.1.0) · Up 2h 15m`.
   - Add subtitle under `Dashboard`: `<N> Nodes (<M> GPUs) · <K> Models Active · <Current> tok/s`.
   - Keep actions aligned on the top right: `[ Pull Model ]` and `[ Restart Proxy ]`.
2. **Remove Standalone Proxy Card from Hosts**:
   - In `dashboard/mod.rs`, remove the artificial `HostCard` for `"Proxy"`.
   - The Hosts grid renders *only* real registered `tamad` nodes from `hosts.get()`.
   - If 0 tamads are registered, show the clean empty state: `"No tamads registered — start a tamad on your inference host to connect compute."`.
3. **Active Models Section on Dashboard**:
   - Filter models to only those currently in `ModelState::Ready` / `ModelState::Starting` (or loaded backends).
   - Display each active model with:
     - Status indicator (green dot for ready, spinner for starting).
     - Display name + API name (`Qwen: Qwen3.8-27B-FP8 · Qwen/Qwen3.8-27B-FP8`).
     - Node destination and GPU allocation (`Node: tama (GPU 0, 1)`).
     - Precision and context info (`radiance (rocm) · fp8 · 256k ctx · Safetensors`).
     - Live throughput badge when generating (e.g. `[ 53 tok/s ]`).
     - Quick actions: `[ Unload ]` button and `[ ▷ ]` button linking to the benchmark suite (`/tama/benchmarks?tab=suite&model={id}`).
   - If 0 models are active: show clean single-line empty state: `⚪ No models currently active · [ Browse & Load a Model → ]` linking to `/tama/models`.
   - Top right of section: `[ Manage Models → ]` link navigating to `/tama/models`.
4. **Preserve Unit Test Compatibility**:
   - Keep the sort/group helper functions in `dashboard/mod.rs` with `#[allow(dead_code)]` so existing unit tests in `dashboard/tests.rs` continue to compile and pass.

**Steps:**
- [ ] Write failing test in `crates/tama/src/pages/dashboard/tests.rs` verifying active models filtering and header metadata formatting.
- [ ] Run `cargo nextest run --package tama -- dashboard::tests`
  - Verify it fails as expected.
- [ ] Implement the header status pill and active models section in `crates/tama/src/pages/dashboard/mod.rs`.
- [ ] Remove the proxy card from the hosts list in `dashboard/mod.rs`.
- [ ] Update `crates/tama/css/15-dashboard.css` for the active models card layout, preserving `.dashboard-models` and `.dashboard-models .page-header` selectors.
- [ ] Run `cargo nextest run --package tama -- dashboard::tests`
  - Verify tests pass.
- [ ] Run `cargo clippy --package tama --all-targets -- -D warnings`
  - Verify clippy is clean.
- [ ] Commit with message: `feat(dashboard): refactor header control plane and active models section`

**Acceptance criteria:**
- [ ] Header shows integrated Gateway status, version, uptime, and cluster summary.
- [ ] Hosts section only contains actual `tamad` nodes.
- [ ] Dashboard displays only loaded/starting models with throughput badges and unload/test buttons.
- [ ] Empty state links cleanly to `/tama/models`.
- [ ] All 24 existing unit tests in `dashboard/tests.rs` pass.

---

### Task 3: Replace Generic CPU/RAM with Pure Inference Telemetry Grid

**Context:**
Generic host OS metrics (CPU % and RAM) are low-signal for an AI gateway. The bottom telemetry section should feature **pure inference telemetry** using existing live and bucketed metrics:
1. **Token Generation Speed (`TG tok/s`)** — live generation throughput and 15m green sparkline (`tg_data`).
2. **Prompt Processing Speed (`PP tok/s`)** — prefill throughput and 15m blue sparkline (`pp_data`).
3. **Cache & Speculative Efficiency** — Prompt cache hit rate (`cur.cache_hit_pct`) and speculative decoding acceptance rate (`cur.spec_accept_pct`).

**Files:**
- Modify: `crates/tama/src/pages/dashboard/mod.rs`
- Modify: `crates/tama/src/pages/dashboard/metrics.rs`
- Modify: `crates/tama/css/15-dashboard.css`
- Test: `crates/tama/src/pages/dashboard/tests.rs`

**What to implement:**
1. **Inference Telemetry Cards**:
   - Replace the CPU Usage and Memory stat cards in `dashboard/mod.rs` with:
     - **Generation Speed (`TG`)**: Displays `cur.tps` in `tok/s` (with derived ITL `1000.0 / tps` ms/tok when `tps > 0`), peak tok/s in window, and 15m sparkline (`tg_data` in green `var(--accent-green)`).
     - **Prompt Processing (`PP`)**: Displays `cur.prompt_tps` in `tok/s` (with derived prefill latency `1000.0 / prompt_tps` ms/tok when `prompt_tps > 0`), and 15m sparkline (`pp_data` in blue `var(--accent-blue)`).
     - **Cache & Speculative Efficiency**: Displays `cur.cache_hit_pct` (Prefix/KV Cache Hit %) and `cur.spec_accept_pct` (Speculative draft acceptance rate) with active decoding indicator.
2. **Positioning**:
   - Place the `grid-stats` section below the `dashboard-hosts` section.
   - Add section header: `<h3>Inference Telemetry</h3>` with `(Past 15 minutes)` subtitle.

**Steps:**
- [ ] Write failing test in `crates/tama/src/pages/dashboard/tests.rs` verifying inference telemetry series generation.
- [ ] Run `cargo nextest run --package tama -- dashboard::tests`
  - Verify it fails as expected.
- [ ] Update `crates/tama/src/pages/dashboard/mod.rs` and `metrics.rs` to render the 3 inference telemetry cards below Hosts.
- [ ] Update `crates/tama/css/15-dashboard.css` with layout styling for the telemetry grid.
- [ ] Run `cargo nextest run --package tama -- dashboard::tests`
  - Verify all tests pass.
- [ ] Run `cargo clippy --package tama --all-targets -- -D warnings`
  - Verify clippy is clean.
- [ ] Commit with message: `feat(dashboard): replace CPU/RAM charts with dedicated inference telemetry grid`

**Acceptance criteria:**
- [ ] CPU % and Memory charts are removed from the dashboard.
- [ ] 3 dedicated inference cards (Generation Speed, Prompt Processing, Cache & Speculative Efficiency) render below Hosts.
- [ ] Sparklines accurately render historical 15m trends from pre-aggregated SSE buckets.

---

### Task 4: CSS Polish, Responsive Layout & Full End-to-End Validation

**Context:**
Ensure all new dashboard components adhere strictly to the project's CSS conventions (editing `crates/tama/css/`, never `dist/`), dark-mode palette, and responsive breakpoints, and run the full workspace CI validation gate.

**Files:**
- Modify: `crates/tama/css/15-dashboard.css`
- Modify: `crates/tama/css/21-dashboard-hosts.css`
- Modify: `crates/tama/css/07-gauges-charts.css` (if chart styling adjustments are needed)
- Test: Full workspace test suites

**What to implement:**
1. **Responsive Breakpoints & Polished Styling**:
   - Verify Active Models cards look great on desktop (horizontal single-line layout) and mobile (stacked).
   - Verify Host cards and GPU rows wrap cleanly on narrow viewports without horizontal scrolling or text clipping.
   - Verify Telemetry 3-card grid stacks to 1 column on mobile (`@media (max-width: 768px)`).
2. **Preserve CSS Contract Selectors**:
   - Ensure `.dashboard-models` and `.dashboard-models .page-header` rules remain present in `15-dashboard.css` for `crates/tama/tests/css_test.rs`.
3. **Trunk WASM Build**:
   - Build frontend WASM (`trunk build --release --public-url /tama --no-default-features --features csr` from `crates/tama`).
4. **Full CI Validation Gate**:
   - `cargo fmt --all --check`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
   - `cargo nextest run --workspace`
   - `cargo nextest run --package tama -- css_test`

**Steps:**
- [ ] Review and polish CSS in `crates/tama/css/15-dashboard.css` and `crates/tama/css/21-dashboard-hosts.css`.
- [ ] Build WASM frontend: `cd crates/tama && trunk build --release --public-url /tama --no-default-features --features csr`.
- [ ] Run `cargo fmt --all --check`.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`.
- [ ] Run `cargo nextest run --workspace`.
- [ ] Run `cargo nextest run --package tama -- css_test`.
  - Verify all workspace tests pass.
- [ ] Commit with message: `chore(dashboard): polish responsive styles and validate full CI gate`

**Acceptance criteria:**
- [ ] Frontend WASM builds without warnings or errors.
- [ ] Full CI validation gate is 100% green.
- [ ] CSS contract test `css_test` passes.
- [ ] Responsive layouts adjust cleanly across screen widths.
