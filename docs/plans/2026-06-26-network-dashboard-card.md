# Network Dashboard Card Plan

**Goal:** Replace the redundant GPU + VRAM stat cards on the dashboard top row with a Network throughput card, yielding a CPU | Memory | Network triad.

**Architecture:** New `tama_core::network` module collects per-tick network throughput via `sysinfo::Networks` on the primary interface (default route). Throughput is computed as delta bytes / tick interval (2s) → MiB/s. Data flows through `SystemMetrics` → `MetricSample` → SSE broadcast → frontend `MetricSample` → dashboard Network card with dual-line sparkline. GPU/VRAM collection stays intact (GPU Cluster Nodes depends on it); only the two dashboard cards are removed.

**Tech Stack:** Rust (sysinfo, serde), Leptos (WASM), SQLite migration, CSS

---

### Task 1: Network collection module

**Context:**
Create a new top-level `network` module in `tama-core` that handles primary interface detection and throughput collection. This is placed at the crate root (not inside `gpu/`) because network metrics are system-level, not GPU-specific. The module uses `sysinfo::Networks` for cross-platform byte counters and reads the OS routing table to find the default-route interface.

**Files:**
- Create: `crates/tama-core/src/network.rs`
- Modify: `crates/tama-core/src/lib.rs` (add `pub mod network;`)

**What to implement:**

In `crates/tama-core/src/network.rs`:

1. **`NetworkStats` struct:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NetworkStats {
    /// Download throughput in MiB/s since last tick
    pub download_mibps: f64,
    /// Upload throughput in MiB/s since last tick
    pub upload_mibps: f64,
}
```

2. **`get_primary_interface() -> Option<String>`:**
   - Linux: Read `/proc/net/route`, find row where `Destination == "00000000"`, return the `Interface` field (3rd column, 0-indexed). Parse with `lines()` + `split('\t')` or `split_whitespace()`.
   - macOS: Run `route get default` and parse the `interface:` line.
   - Fallback for all platforms: `sysinfo::Networks::new_with_refreshed_list()` → first key that is not `"lo"` and not starting with `"lo"`.
   - Return `None` if all interfaces are loopback or detection fails.
   - Handle `std::io::Error` gracefully (log with `tracing::debug!`, try next strategy).

3. **`collect_network_stats(primary_interface: &str, networks: &mut Networks, previous_rx: u64, previous_tx: u64) -> (Option<NetworkStats>, u64, u64)`:**
   - Call `networks.refresh()`.
   - Look up `primary_interface` in `networks`. If missing, try fallback (first non-`lo`).
   - Read `received()` and `transmitted()` — these return bytes since the *previous* `refresh()` call (i.e., delta since last tick).
   - Guard against negative deltas (counter wraparound): if `current < previous`, treat delta as 0.
   - Convert: `delta_bytes / 1024.0 / 1024.0 / 2.0` → MiB/s (tick interval is 2s, hardcoded as constant `TICK_INTERVAL_SECS: f64 = 2.0`).
   - Return `(Some(NetworkStats { download_mibps, upload_mibps }), new_cumulative_rx, new_cumulative_tx)`.
   - Cumulative tracking: `previous_rx + delta_rx` and `previous_tx + delta_tx` (clamped to avoid overflow with `saturating_add`).

4. **`#[cfg(test)]` module** with tests:
   - `test_get_primary_interface_loopback_fallback` — verify non-`lo` interface is picked
   - `test_collect_network_stats_zero_delta` — no traffic → 0.0 MiB/s
   - `test_collect_network_stats_positive_delta` — known bytes → correct MiB/s
   - `test_collect_network_stats_wraparound` — counter resets → 0.0 (not negative)
   - `test_network_stats_serialization` — round-trip JSON

**Steps:**
- [ ] Write failing test for `NetworkStats` serialization in `network.rs`
- [ ] Run `cargo test --package tama-core -- network::`
  - Did it fail with compile error (module doesn't exist)? If not, investigate.
- [ ] Implement `NetworkStats` struct with derives in `network.rs`
- [ ] Implement `get_primary_interface()` with Linux `/proc/net/route` + sysinfo fallback
- [ ] Implement `collect_network_stats()` with delta computation and wraparound guard
- [ ] Add `pub mod network;` to `lib.rs`
- [ ] Run `cargo test --package tama-core -- network::`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add network module for throughput collection"

**Acceptance criteria:**
- [ ] `tama_core::network::NetworkStats` exists with `download_mibps: f64`, `upload_mibps: f64`
- [ ] `get_primary_interface()` returns `Some("eth0")` or similar on a machine with default route
- [ ] `collect_network_stats()` returns correct MiB/s for known byte deltas
- [ ] Wraparound produces 0.0, not negative values
- [ ] All unit tests pass, clippy clean, fmt clean

---

### Task 2: Wire network into metrics pipeline

**Context:**
Integrate the network module into the existing metrics collection pipeline. This touches the system metrics struct, the unified metric sample, the metrics collection loop, the database schema and queries, and Prometheus formatting. The GPU/VRAM pipeline stays unchanged — we only *add* network fields.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs` (add `network` field to `SystemMetrics`)
- Modify: `crates/tama-core/src/gpu/mod.rs` (add `network` field to `MetricSample`)
- Modify: `crates/tama-core/src/proxy/server/mod.rs` (metrics loop, `row_into_sample`)
- Create: `crates/tama-core/src/db/migrations/_0030_add_network_metrics.rs`
- Modify: `crates/tama-core/src/db/migrations.rs` (register migration)
- Modify: `crates/tama-core/src/db/queries/metrics_queries.rs` (`SystemMetricsRow`, SQL)
- Modify: `crates/tama-core/src/proxy/handlers/metrics.rs` (Prometheus gauges)

**What to implement:**

1. **`gpu/system.rs` — `SystemMetrics`:**
```rust
pub network: Option<crate::network::NetworkStats>,
```
Add to `Default` impl as `None`. In `collect_system_metrics_with()`, after GPU collection, call network collection and attach result.

2. **`gpu/mod.rs` — `MetricSample`:**
```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub network: Option<crate::network::NetworkStats>,
```

3. **`db/migrations/_0030_add_network_metrics.rs`:**
```rust
pub const MIGRATION: (i32, bool, &str) = (
    30,
    false,
    r#"
        ALTER TABLE system_metrics_history ADD COLUMN net_rx_bytes BIGINT DEFAULT 0;
        ALTER TABLE system_metrics_history ADD COLUMN net_tx_bytes BIGINT DEFAULT 0;
    "#,
);
```

4. **`db/migrations.rs`:**
- Add `mod _0030_add_network_metrics;`
- Add `_0030_add_network_metrics::MIGRATION,` to the `MIGRATIONS` array (after `_0029`).
- Bump `LATEST_VERSION: i32 = 30` (the `test_migrations_registry_is_ordered_and_complete` test asserts this matches the last migration's version).

5. **`db/queries/metrics_queries.rs` — `SystemMetricsRow`:**
```rust
pub net_rx_bytes: Option<i64>,
pub net_tx_bytes: Option<i64>,
```
Update all SQL statements:
- `insert_system_metric`: Add `net_rx_bytes, net_tx_bytes` to column list and `?13, ?14` to values.
- `get_system_metrics_since`: Add `net_rx_bytes, net_tx_bytes` to SELECT, map with `row.get(12)?` and `row.get(13)?`.
- `get_recent_system_metrics`: Add `net_rx_bytes, net_tx_bytes` to SELECT, map with `row.get(12)?` and `row.get(13)?`.
- Update `test_conn()` schema to include `net_rx_bytes BIGINT, net_tx_bytes BIGINT`.
- Update `make_row` helper to default `net_rx_bytes: None, net_tx_bytes: None`.
- Update the two literal `SystemMetricsRow { … }` builders in `test_system_metrics_row_with_null_gpu` and `test_inference_columns_exist_and_queryable` to include the new fields.

6. **`proxy/server/mod.rs` — Metrics loop:**
   - Before the loop: Detect primary interface once: `let primary_interface = crate::network::get_primary_interface();`
   - Before the loop: Create `let mut net = sysinfo::Networks::new_with_refreshed_list();`
   - Before the loop: `let mut prev_rx: u64 = 0; let mut prev_tx: u64 = 0;`
   - Before the loop: First `net.refresh()` to establish baseline (discard first tick).
   - Inside the loop, after `collect_system_metrics_with()`:
     ```rust
     let (network_stats, cum_rx, cum_tx) = if let Some(ref iface) = primary_interface {
         let (stats, rx, tx) = crate::network::collect_network_stats(iface, &mut net, prev_rx, prev_tx, 2.0);
         prev_rx = rx;
         prev_tx = tx;
         (stats, rx, tx)
     } else {
         (None, 0, 0)
     };
     ```
   - Attach `network: network_stats.clone()` to the `SystemMetrics` snapshot.
   - Attach `network: network_stats` to the `MetricSample`.
   - Add `net_rx_bytes: cum_rx as i64, net_tx_bytes: cum_tx as i64` to the `SystemMetricsRow`.

7. **`proxy/server/mod.rs` — `row_into_sample`:**
   - Map `net_rx_bytes` and `net_tx_bytes` from DB row. Since we store cumulative bytes and throughput is computed at tick time, this function cannot reconstruct throughput from a single row. Set `network: None` for historical rows loaded from DB (the sparkline will start from the first live tick, which is acceptable — historical network data isn't critical for the 15-min window).

8. **`proxy/handlers/metrics.rs` — Prometheus:**
```rust
if let Some(ref net) = sys.network {
    push_gauge_f64(&mut out, "tama:net_rx_mibps", "Network download throughput (MiB/s).", net.download_mibps);
    push_gauge_f64(&mut out, "tama:net_tx_mibps", "Network upload throughput (MiB/s).", net.upload_mibps);
}
```
Add `push_gauge_f64` helper if it doesn't exist (same pattern as `push_gauge_f32` but for `f64`).

**Steps:**
- [ ] Add `network` field to `SystemMetrics` in `gpu/system.rs`
- [ ] Add `network` field to `MetricSample` in `gpu/mod.rs`
- [ ] Create migration `_0030_add_network_metrics.rs` with BIGINT columns
- [ ] Register migration in `db/migrations.rs`
- [ ] Extend `SystemMetricsRow` with `net_rx_bytes`, `net_tx_bytes` in `metrics_queries.rs`
- [ ] Update all SQL INSERT/SELECT statements in `metrics_queries.rs`
- [ ] Update test schema and helpers in `metrics_queries.rs` tests
- [ ] Wire network collection into the metrics loop in `proxy/server/mod.rs`
- [ ] Update `row_into_sample` to handle new fields (set `network: None` for historical rows)
- [ ] Add Prometheus gauges in `proxy/handlers/metrics.rs`
- [ ] Run `cargo test --package tama-core -- db::queries::metrics_queries::tests`
  - Did all tests pass? If not, fix SQL column indices and re-run.
- [ ] Run `cargo test --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: wire network metrics into collection pipeline and DB"

**Acceptance criteria:**
- [ ] `SystemMetrics.network` is populated each tick with current throughput
- [ ] `MetricSample.network` is serialized in SSE stream (non-null when interface detected)
- [ ] DB migration applies cleanly (new BIGINT columns)
- [ ] `SystemMetricsRow` round-trips through insert + query with network fields
- [ ] Prometheus `/metrics` includes `tama:net_rx_mibps` and `tama:net_tx_mibps`
- [ ] All existing tests pass (no regression)

---

### Task 3: Sparkline dual-line support

**Context:**
Extend the `SparklineChart` component to optionally render a second dataset with its own color. This is used by the Network card to show download and upload throughput as two overlapping area charts. The change is backward compatible — when `data2` is empty, the component renders exactly as today.

**Files:**
- Modify: `crates/tama-web/src/components/sparkline.rs`

**What to implement:**

1. **New component props:**
```rust
#[prop(default = Vec::new())] data2: Vec<f32>,
#[prop(default = String::new())] color2: String,
```

2. **Second path computation:** Duplicate the fill/line path logic for `data2` when non-empty and same length as `data`. Use the same `max_value` and `safe_max` for Y-axis scaling.

3. **Second hover point:** When `data2` is active, compute a second `HoverPoint` at the same X index but with `data2`'s Y value.

4. **Render second fill + stroke:** Add `<path>` elements for data2 fill (opacity 0.15) and stroke (full opacity) inside the SVG, after the primary paths.

5. **Render second hover dot:** When hovering and `data2` is active, show a second `<circle>` at the data2 Y position with `color2`.

6. **Dual tooltip:** When `data2` is active, the tooltip shows two lines:
```html
<div class="sparkline-tooltip" style="left: X%">
    <span class="sparkline-tooltip-value" style="color: color1">↓ 15.2 MiB/s</span>
    <span class="sparkline-tooltip-value" style="color: color2">↑ 0.8 MiB/s</span>
    <span class="sparkline-tooltip-time">2m 15s ago</span>
</div>
```
Use `↓` (U+2193) prefix for data1 and `↑` (U+2191) prefix for data2. The unit label is shared and appended to each value line (e.g., `format!("↓ {:.1}{}", value1, unit)` and `format!("↑ {:.1}{}", value2, unit)`).

Change the hover signal to support dual points: use `hover: RwSignal<Option<(HoverPoint, HoverPoint)>>` when `data2` is active, or `hover: RwSignal<Option<HoverPoint>>` when single-mode. The simplest approach: always store `Option<(HoverPoint, Option<HoverPoint>)>` — the second element is `None` in single-mode.

7. **Update existing tests:** Verify `format_duration_label` tests still pass.

8. **Add new tests:**
   - `test_sparkline_data2_empty_renders_single` — verify component doesn't panic with empty data2
   - Test that dual tooltip renders both values

**Steps:**
- [ ] Add `data2` and `color2` props to `SparklineChart` component
- [ ] Compute second fill + line paths when `data2` is non-empty
- [ ] Render second `<path>` elements in the SVG view
- [ ] Compute second hover point when `data2` is active
- [ ] Render second hover dot in `hover_overlay`
- [ ] Update `tooltip_html` to show dual values with arrows and colors
- [ ] Run `cargo test --package tama-web -- sparkline`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-web`
- [ ] Commit with message: "feat: add dual-line support to SparklineChart component"

**Acceptance criteria:**
- [ ] `SparklineChart` with empty `data2` renders identically to before (backward compatible)
- [ ] `SparklineChart` with `data2` renders two fill paths and two stroke paths
- [ ] Hover shows two dots (one per dataset) at the same X position
- [ ] Tooltip shows both values with `↓`/`↑` prefixes and respective colors
- [ ] All existing sparkline tests pass

---

### Task 4: Dashboard Network card + remove GPU/VRAM cards

**Context:**
Replace the GPU and VRAM stat cards on the dashboard top row with a single Network card. The Network card shows download and upload rates as two equal lines, with a dual-line sparkline below. GPU/VRAM data is still collected and available in the "GPU Cluster Nodes" section — only the top-row cards are removed.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/metrics.rs` (frontend types)
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs` (dashboard view)
- Modify: `crates/tama-web/css/15-dashboard.css` (Network card styling)

**What to implement:**

1. **`pages/dashboard/metrics.rs` — Frontend `MetricSample` mirror:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    pub download_mibps: f64,
    pub upload_mibps: f64,
}
```
Add to frontend `MetricSample`:
```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub network: Option<NetworkStats>,
```

2. **`pages/dashboard/mod.rs` — Remove GPU + VRAM cards:**
   - Delete the GPU card block (the `if let Some(gpu_pct) = ...` conditional that renders the GPU stat-card).
   - Delete the VRAM card block (the `if let Some(vram_info) = ...` conditional that renders the VRAM stat-card).
   - Delete the `gpu_data` and `vram_data` extraction variables (and associated `vram_y_refs`, `vram_max`).

3. **`pages/dashboard/mod.rs` — Add Network card:**
   - Extract network data:
     ```rust
     let net_download_data: Vec<f32> = buf.iter().map(|s| s.network.as_ref().map(|n| n.download_mibps as f32).unwrap_or(0.0)).collect();
     let net_upload_data: Vec<f32> = buf.iter().map(|s| s.network.as_ref().map(|n| n.upload_mibps as f32).unwrap_or(0.0)).collect();
     let net_max = net_download_data.iter().chain(net_upload_data.iter()).cloned().fold(0.0_f32, f32::max).max(1.0);
     ```
   - Render Network card (conditional on `buf.last().and_then(|h| h.network.as_ref())`):
     ```html
     <div class="stat-card">
         <div class="card-header">"Network"</div>
         {match buf.last().and_then(|h| h.network.as_ref()) {
             Some(net) => view! {
                 <div class="network-rates">
                     <span class="network-rate network-rate-down">{format!("↓ {:.1} MiB/s", net.download_mibps)}</span>
                     <span class="network-rate network-rate-up">{format!("↑ {:.1} MiB/s", net.upload_mibps)}</span>
                 </div>
             }.into_any(),
             None => view! {
                 <div class="card-value-empty">"—"</div>
             }.into_any(),
         }}
         <div class="sparkline-container">
             <SparklineChart
                 data=net_download_data
                 data2=net_upload_data
                 max_value=net_max
                 color="var(--accent-blue)".to_string()
                 color2="var(--accent-green)".to_string()
                 height=60.0
                 timestamps=timestamps.clone()
                 unit_label="MiB/s".to_string()
             />
         </div>
     </div>
     ```

4. **`css/15-dashboard.css` — Network card styling:**
```css
.network-rates {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    margin-bottom: 0.5rem;
}

.network-rate {
    font-size: 1.1rem;
    font-weight: 600;
    font-family: var(--font-mono);
    line-height: 1.3;
}

.network-rate-down {
    color: var(--accent-blue);
}

.network-rate-up {
    color: var(--accent-green);
}
```

**Steps:**
- [ ] Verify CSS variables `--accent-blue` and `--accent-green` exist in `crates/tama-web/css/01-custom-properties.css` (grep for them)
- [ ] Add `NetworkStats` struct to frontend `metrics.rs`
- [ ] Add `network` field to frontend `MetricSample` in `metrics.rs`
- [ ] Remove GPU card block from `mod.rs`
- [ ] Remove VRAM card block from `mod.rs`
- [ ] Remove unused `gpu_data`, `vram_data` variables from `mod.rs`
- [ ] Add network data extraction and Network card rendering to `mod.rs`
  - Note: Every `match` arm in a `view!` block must return `.into_any()` (same pattern as existing CPU/Memory cards at lines ~174-180)
- [ ] Add `.network-rates` CSS classes to `15-dashboard.css`
- [ ] Run `cargo build --package tama-web`
  - Did it compile? Check for unused import warnings and fix.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Visually verify: dashboard shows 3 cards (CPU, Memory, Network) in the top row
- [ ] Commit with message: "feat: replace GPU/VRAM dashboard cards with Network card"

**Acceptance criteria:**
- [ ] Dashboard top row shows exactly 3 cards: CPU Usage, Memory, Network
- [ ] Network card shows `↓ X.X MiB/s` and `↑ X.X MiB/s` with blue/green colors
- [ ] Network sparkline shows dual-line (blue download, green upload)
- [ ] GPU and VRAM cards no longer appear in the top row
- [ ] "GPU Cluster Nodes" section still renders with per-GPU details (unchanged)
- [ ] No clippy warnings, fmt clean

---

### Task 5: Integration tests + final verification

**Context:**
Run the full test suite, verify the end-to-end flow, and ensure no regressions across the workspace. This includes the SSE stream integration test that verifies `MetricSample` serialization round-trips correctly.

**Files:**
- Modify: `crates/tama-core/src/proxy/server/mod.rs` (update integration tests if they assert on `MetricSample` fields)
- Modify: `crates/tama-web/src/pages/dashboard/tests.rs` (update dashboard tests if needed)

**What to implement:**

1. **Update struct-literal `MetricSample` constructors** — add `network: None`. Only update Rust struct literals (e.g., in `proxy/server/mod.rs` integration tests). JSON-string-based tests (like `metric_sample_deserializes_without_models_field` in `dashboard/tests.rs`) need NO change because `#[serde(default)]` on `network` makes them backward-compatible.

2. **Update struct-literal `SystemMetricsRow` constructors** — add `net_rx_bytes: None, net_tx_bytes: None`. Only update Rust struct literals in `metrics_queries.rs` tests.

3. **Add migration test** in `db/migrations/migrations_tests.rs` — follow the existing pattern (e.g., `test_migration_v24_adds_spec_decoding_column`) to verify that migration v30 adds `net_rx_bytes` and `net_tx_bytes` columns to `system_metrics_history`.

4. **Add forward-compatibility test** in `tama-web/src/pages/dashboard/tests.rs` — add `test_metric_sample_deserializes_without_network_field` that builds a JSON string without the `network` key and verifies it deserializes to `MetricSample { network: None, ... }` (parallel to the existing `metric_sample_deserializes_without_models_field` pattern).

5. **Update the SSE integration test** in `proxy/server/mod.rs` (the test that parses SSE events as `MetricSample`) — verify the `network` field deserializes correctly from the JSON payload.

3. **Run the full workspace test suite:**
```bash
cargo test --workspace
```

4. **Run clippy on all packages:**
```bash
cargo clippy --workspace -- -D warnings
```

5. **Run fmt check:**
```bash
cargo fmt --all -- --check
```

6. **Build release:**
```bash
cargo build --release --workspace
```

**Steps:**
- [ ] Update struct-literal `MetricSample { ... }` constructors in `proxy/server/mod.rs` tests to include `network: None`
- [ ] Update struct-literal `SystemMetricsRow { ... }` builders in `metrics_queries.rs` tests to include `net_rx_bytes: None, net_tx_bytes: None`
- [ ] Add `test_migration_v30_adds_network_columns` in `db/migrations/migrations_tests.rs`
- [ ] Add `test_metric_sample_deserializes_without_network_field` in `tama-web/src/pages/dashboard/tests.rs`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it pass? If not, fix warnings and re-run.
- [ ] Run `cargo fmt --all -- --check`
  - Did it pass? If not, run `cargo fmt --all` and re-check.
- [ ] Run `cargo build --release --workspace`
  - Did it build? If not, fix and re-run.
- [ ] Commit with message: "test: update tests for network metrics, verify workspace"

**Acceptance criteria:**
- [ ] `cargo test --workspace` passes with zero failures
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo build --release --workspace` succeeds
- [ ] SSE stream integration test verifies `network` field round-trips in JSON

---

## Summary of Changes

| File | Task | Change |
|------|------|--------|
| `tama-core/src/network.rs` | 1 | New: `NetworkStats`, `get_primary_interface()`, `collect_network_stats()` |
| `tama-core/src/lib.rs` | 1 | `pub mod network;` |
| `tama-core/src/gpu/system.rs` | 2 | `SystemMetrics.network: Option<NetworkStats>` |
| `tama-core/src/gpu/mod.rs` | 2 | `MetricSample.network: Option<NetworkStats>` |
| `tama-core/src/db/migrations/_0030_add_network_metrics.rs` | 2 | New: BIGINT columns |
| `tama-core/src/db/migrations.rs` | 2 | Register migration |
| `tama-core/src/db/queries/metrics_queries.rs` | 2 | `SystemMetricsRow` + SQL updates |
| `tama-core/src/proxy/server/mod.rs` | 2, 5 | Metrics loop wiring, integration tests |
| `tama-core/src/proxy/handlers/metrics.rs` | 2 | Prometheus gauges |
| `tama-web/src/components/sparkline.rs` | 3 | Dual-line support |
| `tama-web/src/pages/dashboard/metrics.rs` | 4 | Frontend `NetworkStats`, `MetricSample.network` |
| `tama-web/src/pages/dashboard/mod.rs` | 4 | Network card, remove GPU/VRAM cards |
| `tama-web/css/15-dashboard.css` | 4 | `.network-rates` styling |
