# Dashboard Bar Charts Plan

**Goal:** Replace the dense sparkline area charts on the dashboard stat cards (CPU, Memory, Network) with vertical bar charts that are easier to read and interact with.

**Architecture:** Create a new `BarChart` component alongside the existing `SparklineChart`. The `BarChart` aggregates raw timestamped data into 30-second buckets (avg per bucket), renders each bucket as a vertical `<rect>` with opacity proportional to height, and supports an optional second data series for side-by-side paired bars (network download/upload). The dashboard swaps `SparklineChart` for `BarChart` in the three stat cards.

**Tech Stack:** Leptos (WASM), SVG, Rust

---

## Design Decisions (from brainstorming)

- **30-second buckets** → ~30 bars for a 15-minute window
- **Average aggregation** per bucket for all metrics
- **Opacity scaling**: 0.25 (value=0) to 1.0 (value=max_value), linear interpolation
- **Bar shape**: Flat bottom, 2px rounded top corners
- **Hover**: Highlight bar(s) + tooltip with value and relative time (same tooltip style as current sparkline)
- **Network**: Side-by-side paired bars (blue download, green upload)
- **Keep SparklineChart** — create `BarChart` as a new component alongside it

---

### Task 1: Create BarChart component

**Context:**
The dashboard needs a new bar chart component to replace the sparkline area charts. The component takes raw timestamped data, buckets it into 30-second windows, and renders SVG `<rect>` elements with varying opacity. It supports an optional second data series for paired bars (network download/upload).

**Files:**
- Create: `crates/tama/src/components/bar_chart.rs`
- Modify: `crates/tama/src/components/mod.rs` (export new module)

**What to implement:**

A new Leptos component `BarChart` in `crates/tama/src/components/bar_chart.rs` with the following structure:

```rust
#[component]
pub fn BarChart(
    data: Vec<f32>,
    max_value: f32,
    color: String,
    height: f32,
    #[prop(default = Vec::new())] timestamps: Vec<i64>,
    #[prop(default = String::new())] unit_label: String,
    #[prop(default = Vec::new())] data2: Vec<f32>,
    #[prop(default = String::new())] color2: String,
) -> impl IntoView
```

**Bucketing logic:**
- If timestamps are provided and valid (non-empty, same length as data), sort data points into 30-second buckets starting from the oldest timestamp
- Each bucket: collect all points whose timestamp falls within `[bucket_start, bucket_start + 30000ms)`, compute the average
- If no timestamps, divide data into equal-sized chunks (fallback)
- Skip empty buckets (no data points) — don't render bars for them

**Bar rendering:**
- SVG with `viewBox="0 0 100 {height}"`, `preserveAspectRatio="none"`
- Each bar is an `<svg:rect>` element
- Bar width: computed from `100.0 / num_buckets`, with a small gap (~15% of bar width) between bars
- For paired bars (data2 active): each bar gets half the slot width, side-by-side with a tiny gap
- Bar x-position: evenly spaced across the 0-100 viewBox width
- Bar y-position: `height - (value / safe_max * height)`, clamped to [0, height]
- Bar height: `(value / safe_max * height)`, minimum 1px so zero-value bars are still visible
- Bar fill: the provided color string (e.g. `"var(--accent-green)"`)
- Bar fill-opacity: `0.25 + 0.75 * (value / safe_max)`, clamped to [0.25, 1.0]
- Bar rx (top corner radius): 2px (use `rx="2"` on the rect — since rects render with the radius on all corners but the bottom is at the baseline this effectively rounds only the top)

**Hover interaction:**
- `on:mousemove` on the SVG: find the nearest bucket index from mouse X position
- Set a `RwSignal<Option<usize>>` hover signal to the bucket index
- On hover: the hovered bar(s) get increased opacity (add ~0.15 on top, capped at 1.0) or a subtle brightening
- `on:mouseleave`: clear hover signal
- Tooltip: same HTML tooltip pattern as `SparklineChart` — absolutely positioned div showing value + unit + relative time
- For paired bars: show both values on hover (primary value with primary color, secondary with secondary color)

**Time axis labels:**
- Same as SparklineChart: `-15m` / `now` labels below the chart when timestamps are valid

**Export:**
Add to `crates/tama/src/components/mod.rs`:
```rust
pub mod bar_chart;
```

**Steps:**
- [ ] Create `crates/tama/src/components/bar_chart.rs` with the `BarChart` component
- [ ] Implement the bucketing helper function (30s windows, avg aggregation)
- [ ] Implement bar rendering with opacity scaling and corner radius
- [ ] Implement paired bar support (data2/color2)
- [ ] Implement hover interaction (snap to bucket, highlight, tooltip)
- [ ] Add time axis labels
- [ ] Add `pub mod bar_chart;` to `crates/tama/src/components/mod.rs`
- [ ] Run `cargo build --package tama`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add BarChart component for dashboard stat cards"

**Acceptance criteria:**
- [ ] `BarChart` compiles without errors or warnings
- [ ] Renders vertical bars with opacity proportional to value
- [ ] Hover highlights the nearest bar and shows a tooltip
- [ ] Paired bars (data2) render side-by-side when provided
- [ ] Time axis labels show `-15m` / `now` when timestamps are provided

---

### Task 2: Replace SparklineChart with BarChart in dashboard stat cards

**Context:**
The dashboard's CPU, Memory, and Network stat cards currently use `SparklineChart` which renders dense area charts. Swap them for `BarChart` to get the cleaner bar chart visualization.

**Files:**
- Modify: `crates/tama/src/pages/dashboard/mod.rs`

**What to implement:**

In `crates/tama/src/pages/dashboard/mod.rs`:

1. **Update imports**: Add `use crate::components::bar_chart::BarChart;` (keep the sparkline import in case it's used elsewhere in the file — if not, remove it)

2. **Replace the three `SparklineChart` usages** with `BarChart`:

   **CPU card** — replace:
   ```rust
   <SparklineChart
       data=cpu_data
       max_value=100.0
       color="var(--accent-green)".to_string()
       height=60.0
       timestamps=timestamps.clone()
       unit_label="%".to_string()
       y_refs=cpu_y_refs
   />
   ```
   With:
   ```rust
   <BarChart
       data=cpu_data
       max_value=100.0
       color="var(--accent-green)".to_string()
       height=60.0
       timestamps=timestamps.clone()
       unit_label="%".to_string()
   />
   ```
   Note: `y_refs` is not needed for bar charts (no reference lines).

   **Memory card** — replace:
   ```rust
   <SparklineChart
       data=mem_data
       max_value=mem_max
       color="var(--accent-blue)".to_string()
       height=60.0
       timestamps=timestamps.clone()
       unit_label="MiB".to_string()
       y_refs=mem_y_refs
   />
   ```
   With:
   ```rust
   <BarChart
       data=mem_data
       max_value=mem_max
       color="var(--accent-blue)".to_string()
       height=60.0
       timestamps=timestamps.clone()
       unit_label="MiB".to_string()
   />
   ```

   **Network card** — replace:
   ```rust
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
   ```
   With:
   ```rust
   <BarChart
       data=net_download_data
       data2=net_upload_data
       max_value=net_max
       color="var(--accent-blue)".to_string()
       color2="var(--accent-green)".to_string()
       height=60.0
       timestamps=timestamps.clone()
       unit_label="MiB/s".to_string()
   />
   ```

3. **Remove unused variables**: `cpu_y_refs` and `mem_y_refs` are no longer needed (bar charts don't use reference lines). Remove their declarations.

4. **Remove unused import**: If `SparklineChart` is no longer imported/used anywhere in the file, remove `use crate::components::sparkline::SparklineChart;`. (Check that it's not used elsewhere in the file first.)

**Steps:**
- [ ] Add `use crate::components::bar_chart::BarChart;` import
- [ ] Replace CPU card's `SparklineChart` with `BarChart`
- [ ] Replace Memory card's `SparklineChart` with `BarChart`
- [ ] Replace Network card's `SparklineChart` with `BarChart`
- [ ] Remove `cpu_y_refs` and `mem_y_refs` declarations
- [ ] Remove `SparklineChart` import if no longer used
- [ ] Run `cargo build --package tama`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "feat: replace sparkline with bar charts on dashboard stat cards"

**Acceptance criteria:**
- [ ] Dashboard builds without errors
- [ ] CPU card shows vertical green bars instead of area chart
- [ ] Memory card shows vertical blue bars instead of area chart
- [ ] Network card shows paired blue/green bars instead of dual area chart
- [ ] Hover tooltips work on all three bar charts
- [ ] No clippy warnings

---

### Task 3: Add CSS for bar chart hover effects

**Context:**
The bar chart needs CSS styles for hover states, tooltip positioning, and any visual polish. The existing sparkline CSS provides a template for the tooltip styles which can be reused.

**Files:**
- Modify: `crates/tama/dist/css/07-gauges-charts.css`

**What to implement:**

The BarChart component reuses the same HTML structure as SparklineChart (`.sparkline-container`, `.sparkline-tooltip`, `.sparkline-time-axis` classes). No new CSS classes are needed — the existing sparkline styles cover all layout, tooltip, and time-axis requirements.

Add only one new rule for bar hover transition:

```css
/* Bar chart — bars use SVG rects, add smooth opacity transition */
.bar-rect {
  transition: fill-opacity 0.15s ease;
}
```

Apply `class="bar-rect"` to each `<rect>` in the SVG. This gives smooth opacity transitions on hover without needing new container/tooltip styles.

**Steps:**
- [ ] Add `.bar-rect` rule to `crates/tama/dist/css/07-gauges-charts.css`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "style: add CSS for bar chart hover transition"

---

## Verification

After all tasks are complete:

1. `cargo build --release --workspace` — full release build succeeds
2. `cargo clippy --workspace -- -D warnings` — no warnings
3. `cargo test --workspace` — all existing tests pass
4. Manual test: Open dashboard, verify:
   - CPU card shows ~30 green bars with varying opacity
   - Memory card shows ~30 blue bars
   - Network card shows paired blue/green bars
   - Hovering any bar shows tooltip with value + time
   - Bars update in real-time as new SSE data arrives
