# GPU Card Responsive Layout Plan

**Goal:** Make GPU cards adapt to GPU count — horizontal strip for 1-2 GPUs (stacked vertically), portrait grid for 3+ GPUs — using a single internal structure reconfigured by CSS.

**Architecture:** The `GpuDeviceCard` component renders a single 4-column internal grid (identity | bars | throughput | telemetry). CSS `:has()` on the parent `.gpu-device-grid` detects GPU count and reconfigures both the outer grid (stack vs multi-column) and the inner card layout (horizontal vs portrait). No Rust logic needed for layout switching.

**Tech Stack:** Rust, Leptos 0.7.8, CSS `:has()`, CSS Grid

---

### Task 1: Restructure GPU Device Card Component

**Context:**
The current `GpuDeviceCard` renders sections vertically stacked (header → utilization → vram → model → inference → telemetry). For the horizontal layout, these sections need to flow left-to-right in a 4-column grid. Instead of rendering two separate DOM trees (one horizontal, one portrait), we render a single 4-column internal grid that CSS reconfigures. The component structure stays the same — only the wrapping `<div>` classes change.

**Files:**
- Modify: `crates/tama-web/src/components/gpu_device_card.rs`

**What to implement:**

Replace the current card's internal structure with a single 4-column grid. The card wraps all content in a `.gpu-device-card__internal` grid with 4 named areas:

1. **Identity column** — GPU label + badge (row 1), model name (row 2)
2. **Bars column** — Utilization (label + value + progress bar), VRAM (label + value + progress bar)
3. **Throughput column** — Processing tok/s (value + label), Generation tok/s (value + label)
4. **Telemetry column** — Temp (value + label), Power (value + label), Fan (value + label)

The component renders a single `<div class="card gpu-device-card">` containing a `<div class="gpu-device-card__internal">` with the 4 column sections as direct children. Each section is a `<div>` with a BEM class:
- `.gpu-device-card__identity` — GPU label, badge, model name
- `.gpu-device-card__bars` — utilization + vram rows
- `.gpu-device-card__throughput` — inference stats (2-col internal grid)
- `.gpu-device-card__telemetry` — temp/power/fan (3-col internal grid)

**Key changes to the view! macro:**

The outer `<div class="card gpu-device-card">` stays the same. Inside, replace the current flat stack of sections with:

```rust
<div class="gpu-device-card__internal">
    // Column 1: Identity
    <div class="gpu-device-card__identity">
        <div class="gpu-device-card__header">
            <span class="gpu-device-card__title">{display_label}</span>
            <span class={badge_class}>{badge_text}</span>
        </div>
        <div class="gpu-device-card__model-section">
            <div class="gpu-device-card__model-header">
                {model_section_header} // "LOADED MODEL" or "TRANSFERRING…"
            </div>
            <div class="gpu-device-card__model-body">
                // Same model display logic as current (idle = "No model loaded", etc.)
                // IMPORTANT: Add title attribute for tooltip on long names:
                // <span title={full_model_name}>{displayed_model_name}</span>
            </div>
        </div>
    </div>

    // Column 2: Bars (utilization + vram)
    <div class="gpu-device-card__bars">
        // Utilization row (same as current)
        <div class="gpu-device-card__row">
            <div class="gpu-device-card__row-header">
                <span class="gpu-device-card__label">"Utilization"</span>
                <span class="gpu-device-card__value">{utilization_value}</span>
            </div>
            <div class="progress-bar">
                <div class="progress-bar-fill gpu-device-card__bar-fill" style={width} />
            </div>
        </div>
        // VRAM row (same as current)
        <div class="gpu-device-card__row">
            <div class="gpu-device-card__row-header">
                <span class="gpu-device-card__label">{vram_label}</span>
                <span class="gpu-device-card__value">{vram_value}</span>
            </div>
            {if let Some(vram) = &device.vram { progress bar }}
        </div>
    </div>

    // Column 3: Throughput (inference stats)
    // Only render when state is Active/Loading AND (prompt_tps.is_some() || tps.is_some())
    // Same conditional logic as current component
    <div class="gpu-device-card__throughput">
        <div class="gpu-device-card__inference-cell">
            <div class="gpu-device-card__inference-value">{prompt_tps_value}</div>
            <div class="gpu-device-card__inference-label">"Processing"</div>
        </div>
        <div class="gpu-device-card__inference-cell">
            <div class="gpu-device-card__inference-value">{tps_value}</div>
            <div class="gpu-device-card__inference-label">"Generation"</div>
        </div>
    </div>

    // Column 4: Telemetry
    <div class="gpu-device-card__telemetry">
        <div class="gpu-device-card__telemetry-cell">
            <div class="gpu-device-card__telemetry-value">
                {device.temperature_c.map(|t| format!("{t}°C")).unwrap_or_else(|| "—".to_string())}
            </div>
            <div class="gpu-device-card__telemetry-label">"Temp"</div>
        </div>
        <div class="gpu-device-card__telemetry-cell">
            <div class="gpu-device-card__telemetry-value">
                {device.power_w.map(|p| format!("{p}W")).unwrap_or_else(|| "—".to_string())}
            </div>
            <div class="gpu-device-card__telemetry-label">"Power"</div>
        </div>
        <div class="gpu-device-card__telemetry-cell">
            <div class="gpu-device-card__telemetry-value">
                {device.fan_pct.map(|f| format!("{f}%")).unwrap_or_else(|| "—".to_string())}
            </div>
            <div class="gpu-device-card__telemetry-label">"Fan"</div>
        </div>
    </div>
</div>
```

**Specific logic to preserve from current component:**
- `derive_device_state()` — unchanged, used for badge
- `loaded_model_display()` — unchanged, used for model name
- `format_vram_short()` — unchanged, used for VRAM label
- Badge class/text derivation from state — unchanged
- `vram_label` changes to "VRAM Allocation" during Loading state — unchanged
- `model_section_header` changes to "TRANSFERRING…" during Loading state — unchanged
- Inference section conditional (only show for Active/Loading with data) — unchanged
- All `—` fallbacks for missing data — unchanged

**New logic to add:**
- Model name `<span>` gets a `title` attribute. Use the resolved display name: `model.display_name.clone().or_else(|| model.api_name.clone()).unwrap_or_else(|| model.id.clone())` — this is the full name for the tooltip. For "No model loaded" / idle states, omit the title.
- The throughput column (`gpu-device-card__throughput`) is **conditionally rendered** — do NOT render the div at all when both TPS are None. This matches the current behavior (inference section is hidden when no data). The CSS `:empty` rule is a safety net only — the primary mechanism is conditional rendering in Rust.

**Class rename:** The throughput section parent class changes from `.gpu-device-card__inference` (current) to `.gpu-device-card__throughput` (new). Child classes (`.gpu-device-card__inference-cell`, `.gpu-device-card__inference-value`, `.gpu-device-card__inference-label`) stay the same.

**What NOT to change:**
- Do NOT change any of the helper functions (`derive_device_state`, `loaded_model_display`, `format_vram_short`, etc.)
- Do NOT change the component's public API (props remain the same)
- Do NOT add any new props
- Do NOT change test helpers

**Steps:**
- [ ] Read current `gpu_device_card.rs` to understand existing structure
- [ ] Restructure the `view!` macro to use the 4-column internal grid layout described above
- [ ] Preserve all existing conditional logic (state badges, model display, inference visibility, vram conditional)
- [ ] Add `title` attribute to model name span for tooltip
- [ ] Keep throughput section conditional (only render when Active/Loading AND data present)
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run
- [ ] Commit with message: "refactor(gpu-card): restructure component into 4-column internal grid"

**Acceptance criteria:**
- [ ] Component compiles without errors
- [ ] All existing tests pass (no behavior change — only structural reorganization)
- [ ] The card renders the same data as before, just wrapped in new div classes
- [ ] Model name has a `title` attribute for tooltip

---

### Task 2: CSS for Responsive Horizontal/Portrait Layout

**Context:**
The CSS controls both the outer grid (how cards are arranged) and the inner card layout (how content flows inside each card). CSS `:has()` on `.gpu-device-grid` detects whether there are 3+ children and switches between horizontal (1-2 GPUs) and portrait (3+ GPUs) modes. Missing data columns hide via CSS. Model name overflow uses ellipsis + tooltip.

**Files:**
- Modify: `crates/tama-web/css/19-gpu-device-card.css`

**What to implement:**

#### 2.1 Outer Grid — Stack vs Multi-Column

```css
.gpu-device-grid {
  display: grid;
  grid-template-columns: 1fr;       /* default: stacked (1-2 GPUs) */
  gap: 12px;
}

/* 3+ children → multi-column grid */
.gpu-device-grid:has(> :nth-child(3)) {
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
}
```

#### 2.2 Internal Card Grid — Horizontal (default, 1-2 GPUs)

```css
.gpu-device-card__internal {
  display: grid;
  grid-template-columns: 1.4fr 1fr 0.6fr 1fr;  /* identity | bars | throughput | telemetry */
  grid-template-rows: auto auto;
  gap: 0;
}

/* Vertical dividers between columns */
.gpu-device-card__identity {
  border-right: 1px solid var(--border-color);
  padding-right: 1rem;
}

.gpu-device-card__bars {
  border-right: 1px solid var(--border-color);
  padding-left: 1rem;
  padding-right: 1rem;
}

.gpu-device-card__throughput {
  border-right: 1px solid var(--border-color);
  padding-left: 1rem;
  padding-right: 1rem;
}

.gpu-device-card__telemetry {
  padding-left: 1rem;
}
```

#### 2.3 Internal Card Grid — Portrait (3+ GPUs)

When the parent grid has 3+ children, reconfigure the internal grid to single-column stacked:

```css
/* When parent has 3+ GPUs → portrait mode */
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__internal {
  grid-template-columns: 1fr;
  grid-template-rows: auto;
}

/* Remove vertical dividers in portrait mode */
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__identity,
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__bars,
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__throughput {
  border-right: none;
  padding-right: 0;
}

.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__bars,
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__throughput,
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__telemetry {
  padding-left: 0;
}

/* Add section separators in portrait mode */
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__bars,
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__throughput,
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__telemetry {
  border-top: 1px solid var(--border-color);
  padding-top: 0.75rem;
  margin-top: 0.75rem;
}
```

#### 2.4 Identity Column Styles

```css
.gpu-device-card__header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 0.25rem;
}

.gpu-device-card__title {
  font-size: 1rem;
  font-weight: 600;
  color: var(--text-primary);
}

.gpu-device-card__model-section {
  margin-top: 0.25rem;
}

.gpu-device-card__model-header {
  font-size: 0.7rem;
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  margin-bottom: 0.25rem;
}

.gpu-device-card__model-body {
  font-size: 0.8125rem;
  color: var(--text-primary);
  font-family: var(--font-mono);
  min-height: 1.2em;
  /* Overflow handling */
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
```

#### 2.5 Bars Column Styles

```css
.gpu-device-card__row {
  margin-bottom: 0.5rem;
}

.gpu-device-card__row-header {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  margin-bottom: 0.25rem;
}

.gpu-device-card__label {
  font-size: 0.75rem;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.gpu-device-card__value {
  font-size: 0.875rem;
  font-weight: 600;
  font-family: var(--font-mono);
  color: var(--text-primary);
}

.gpu-device-card__bar-fill {
  background: linear-gradient(90deg, var(--accent-purple) 0%, var(--accent-pink) 100%);
}
```

#### 2.6 Throughput Column Styles

```css
.gpu-device-card__throughput {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.gpu-device-card__throughput:empty {
  display: none;
}

.gpu-device-card__inference-cell {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
}

.gpu-device-card__inference-value {
  font-size: 14px;
  font-weight: 600;
  color: var(--accent-cyan);
  font-family: var(--font-mono);
}

.gpu-device-card__inference-label {
  font-size: 11px;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}
```

In portrait mode, throughput switches to a 2-column grid:

```css
.gpu-device-grid:has(> :nth-child(3)) .gpu-device-card__throughput {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 0.5rem;
}
```

#### 2.7 Telemetry Column Styles

```css
.gpu-device-card__telemetry {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 0.5rem;
  text-align: center;
}

.gpu-device-card__telemetry-cell {
  display: flex;
  flex-direction: column;
  align-items: center;
}

.gpu-device-card__telemetry-value {
  font-size: 16px;
  font-weight: 600;
  color: var(--text-primary);
  font-family: var(--font-mono);
}

.gpu-device-card__telemetry-label {
  font-size: 11px;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}
```

#### 2.8 Card Container Styles

```css
.dashboard-gpus {
  margin: 1.5rem 0;
}

.gpu-device-card {
  display: flex;
  flex-direction: column;
  /* Override .card padding (1.5rem) to avoid double padding with __internal */
  padding: 0;
}

/* Internal grid padding */
.gpu-device-card__internal {
  padding: 0.75rem 1rem;
}
```

#### 2.9 Hidden Throughput Handling

When the component doesn't render the throughput div (both TPS are None), the column simply disappears. CSS grid auto-handles this — the remaining columns stretch to fill. The `:empty { display: none }` rule on `.gpu-device-card__throughput` is a safety net only.

**What NOT to change:**
- Do NOT modify any other CSS files
- Do NOT add JavaScript for layout detection
- Do NOT change badge styles (handled by existing CSS)
- Do NOT change progress bar styles (handled by existing `05-buttons-forms-progress.css`)

**Steps:**
- [ ] Update `19-gpu-device-card.css` — merge all new styles into the existing file. Preserve any existing rules not listed in this plan. Remove only rules that are directly superseded by new styles (e.g., old `.gpu-device-card__inference` parent → replaced by `.gpu-device-card__throughput`). Do NOT delete the `@media (min-width: 1200px)` block — update it to also apply to the new grid if needed.
- [ ] Ensure `.gpu-device-grid` uses `:has(> :nth-child(3))` for 3+ GPU detection
- [ ] Ensure horizontal mode (default) uses 4-column grid with vertical dividers
- [ ] Ensure portrait mode (3+ GPUs) uses 1-column stacked grid with section separators
- [ ] Ensure model name has `text-overflow: ellipsis` + `white-space: nowrap` + `overflow: hidden`
- [ ] Ensure `.gpu-device-card` overrides `.card` padding with `padding: 0` to prevent double padding
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cd crates/tama-web && trunk build --release` to verify CSS bundles correctly
  - Did it succeed? If not, fix CSS syntax errors and re-run
- [ ] Commit with message: "style(gpu-card): add responsive horizontal/portrait layout with :has()"

**Acceptance criteria:**
- [ ] With 1-2 GPUs: cards stack vertically, each card is a wide horizontal strip with 4 columns
- [ ] With 3+ GPUs: cards use multi-column grid, each card is portrait (stacked sections)
- [ ] Vertical dividers appear between columns in horizontal mode
- [ ] Section separators (borders) appear between sections in portrait mode
- [ ] Model name truncates with ellipsis when too long (tooltip shows full name via `title` attr)
- [ ] Throughput column disappears when no inference data (idle GPU)
- [ ] CSS compiles without errors (no syntax errors)

---

## Verification

After both tasks are complete:

1. **Build:** `cargo build --release --workspace` — must succeed
2. **Tests:** `cargo test --workspace` — all tests must pass
3. **Clippy:** `cargo clippy --workspace -- -D warnings` — must pass
4. **Format:** `cargo fmt --all` — must be clean
5. **Visual:** Load the dashboard with 1-2 GPUs → horizontal strips. Load with 3+ GPUs → portrait grid.
