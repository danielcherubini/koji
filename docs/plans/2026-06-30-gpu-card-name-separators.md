# GPU Card Name + VRAM Subtitle + Section Separators Plan

**Goal:** Show the GPU card name and total VRAM under the GPU label on each device card, and add horizontal separators to the left and middle columns to match the right column's existing separator style.

**Architecture:** Add a `name` field to `GpuDeviceStats` populated from the OS (nvidia-smi for NVIDIA, sysfs for AMD). The frontend renders a subtitle row (`{name} · {total_gb} GB`) under the GPU header, with a `border-bottom` separator. A matching separator is added between the Utilization and VRAM rows in the middle column.

**Tech Stack:** Rust (tama-core, tama-web/Leptos), CSS

**Dependencies:** Tasks must be executed in order. Each task builds on the previous.

---

### Task 1: Add `name` field to `GpuDeviceStats` in tama-core

**Context:**
The `GpuDeviceStats` struct currently has no GPU product name. We need to add one so the frontend can display "Radeon AI PRO R9700" or "GeForce RTX 4090" etc. The name is sourced from the OS: `nvidia-smi --query-gpu=name` for NVIDIA (trivial since we already run nvidia-smi), and `/sys/class/drm/card*/device/name` for AMD (extract text between `[` and `]`).

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs`

**What to implement:**

1. **Add `name: String` field to `GpuDeviceStats`** (line ~10, between `vendor` and `utilization_pct`):
   ```rust
   /// Human-readable GPU name (e.g. "Radeon AI PRO R9700", "GeForce RTX 4090").
   pub name: String,
   ```

2. **Update NVIDIA query** (`query_nvidia_devices`, line ~337):
   - Change the `--query-gpu` arg to include `name` as the 2nd field:
     ```
     "--query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,fan.speed"
     ```
   - Update `parse_nvidia_smi_csv_line` (line ~302):
     - Change expected field count from 7 to 8
     - Parse `parts[1]` as the GPU name (`.trim().to_string()`)
     - Shift all other index references by +1 (utilization = parts[2], mem_used = parts[3], etc.)
     - Add `name` to the constructed `GpuDeviceStats`

3. **Update AMD query** (`query_amd_devices`, line ~358):
   - After creating `stats`, read `/sys/class/drm/card*/device/name` and extract the bracketed portion:
     ```rust
     let name = std::fs::read_to_string(card_path.join("name"))
         .ok()
         .and_then(|s| {
             let trimmed = s.trim().to_string();
             // Extract text between [ and ] if present, e.g. "Navi 48 [Radeon AI PRO R9700]"
             if let (Some(start), Some(end)) = (trimmed.find('['), trimmed.find(']')) {
                 Some(trimmed[start + 1..end].to_string())
             } else {
                 Some(trimmed)
             }
         })
         .unwrap_or_else(|| "AMD GPU".to_string());
     ```
   - Add `name` to the `GpuDeviceStats` constructor

4. **Update all test fixtures** that construct `GpuDeviceStats` in this file:
   - `build_test_device` (line ~697) — add `name: "Test GPU".to_string()`
   - Any other inline `GpuDeviceStats { ... }` constructions in tests

**Steps:**
- [ ] Add `name: String` field to `GpuDeviceStats` struct
- [ ] Update `parse_nvidia_smi_csv_line` for 8 fields with name
- [ ] Update `query_nvidia_devices` nvidia-smi args to include `name`
- [ ] Add sysfs name reading in `query_amd_devices`
- [ ] Update all test fixtures with `name` field
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: add GPU name field to GpuDeviceStats"

**Acceptance criteria:**
- [ ] `GpuDeviceStats` has a `name: String` field
- [ ] NVIDIA query populates `name` from nvidia-smi
- [ ] AMD query populates `name` from sysfs (bracket extraction with fallback)
- [ ] All tama-core tests pass
- [ ] Clippy clean

---

### Task 2: Add `name` field to frontend `GpuDeviceStats` mirror

**Context:**
The frontend has its own mirror of `GpuDeviceStats` in `metrics.rs` that deserializes from the SSE stream. It must match the backend struct's fields exactly (JSON field names). Must use `#[serde(default)]` for safe deserialization if the backend hasn't been updated yet.

**Prerequisite:** Task 1 completed and committed.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/metrics.rs`

**What to implement:**

1. Add `pub name: String` to the frontend `GpuDeviceStats` struct (line ~57, between `vendor` and `utilization_pct`), with `#[serde(default)]`:
   ```rust
   /// Human-readable GPU name (e.g. "Radeon AI PRO R9700", "GeForce RTX 4090").
   #[serde(default)]
   pub name: String,
   ```

**Steps:**
- [ ] Add `#[serde(default)] pub name: String` field to frontend `GpuDeviceStats`
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(web): add name field to frontend GpuDeviceStats"

**Acceptance criteria:**
- [ ] Frontend `GpuDeviceStats` has `name: String` with `#[serde(default)]`
- [ ] tama-web compiles successfully

---

### Task 3: Update GPU card component — subtitle + separators

**Context:**
The GPU device card component renders 3 columns inside `gpu-device-card__internal`. The left column (`gpu-device-card__identity`) currently shows GPU label + badge, then the model section. We need to add a subtitle row between them showing `{name} · {total_gb} GB`, and add horizontal separators in both the left and middle columns.

The middle column (`gpu-device-card__bars`) contains two rows: Utilization and VRAM. We need a separator between them. The rows are direct children of the bars container, so `:first-child` correctly targets the Utilization row.

**Prerequisite:** Task 2 completed and committed.

**Files:**
- Modify: `crates/tama-web/src/components/gpu_device_card.rs`
- Modify: `crates/tama-web/css/19-gpu-device-card.css`

**What to implement:**

1. **Add helper function** `format_card_subtitle` (near `format_vram_short`):
   ```rust
   /// Format GPU card subtitle as "Name · 32 GB".
   /// Total VRAM is rounded to the nearest integer GB.
   pub fn format_card_subtitle(name: &str, vram: &VramInfo) -> String {
       let total_gb = (vram.total_mib as f64 / 1024.0 + 0.5) as u64;
       format!("{name} \u{00B7} {total_gb} GB")
   }
   ```
   - Uses middle dot (`\u{00B7}` = `·`)
   - Rounds total GB to nearest integer (31.9 → 32)

2. **Update the component's left column** (inside `GpuDeviceCard` view):
   - The left column is `gpu-device-card__identity`. Currently it contains:
     - `gpu-device-card__header` (GPU label + badge)
     - `gpu-device-card__model-section` (model info)
   - Insert a new `gpu-device-card__subtitle` div between the header and model section:
     ```rust
     <div class="gpu-device-card__subtitle">
         {if let Some(vram) = &device.vram {
             format_card_subtitle(&device.name, vram)
         } else {
             device.name.clone()
         }}
     </div>
     ```

3. **Update CSS** (`19-gpu-device-card.css`):
   - Add `.gpu-device-card__subtitle` styles (separator is on the subtitle itself):
     ```css
     .gpu-device-card__subtitle {
       font-size: 0.8rem;
       color: var(--text-muted);
       font-family: var(--font-mono);
       margin-bottom: 0.5rem;
       padding-bottom: 0.5rem;
       border-bottom: 1px solid var(--border-color);
       white-space: nowrap;
       overflow: hidden;
       text-overflow: ellipsis;
     }
     ```
   - Add separator between Utilization and VRAM rows in the middle column:
     The bars container (`.gpu-device-card__bars`) has two `.gpu-device-card__row` children. Target the first one:
     ```css
     .gpu-device-card__bars > .gpu-device-card__row:first-child {
       border-bottom: 1px solid var(--border-color);
       padding-bottom: 0.75rem;
       margin-bottom: 0.75rem;
     }
     ```
     Note: Using `> :first-child` scoped to `.gpu-device-card__bars` ensures we only target rows inside the bars container, not any other first-child rows elsewhere.

   - Portrait mode (3+ GPUs): The existing CSS already removes `border-right` from identity/bars and adds `border-top` to bars/combined. The new subtitle `border-bottom` and bars row `border-bottom` should remain in portrait mode as they separate content within each section. No additional overrides needed.

4. **Update all test fixtures** in `gpu_device_card.rs` that construct `GpuDeviceStats`:
   - `make_gpu` function (line ~245): add `name: "Test GPU".to_string()`

5. **Add tests** for `format_card_subtitle`:
   ```rust
   #[test]
   fn test_format_card_subtitle() {
       let vram = VramInfo { used_mib: 0, total_mib: 32768 }; // 32 GB
       assert_eq!(format_card_subtitle("Radeon AI PRO R9700", &vram), "Radeon AI PRO R9700 · 32 GB");
   }

   #[test]
   fn test_format_card_subtitle_rounds_31_9_to_32() {
       let vram = VramInfo { used_mib: 0, total_mib: 32760 }; // ~31.9 GB
       assert_eq!(format_card_subtitle("Radeon AI PRO R9700", &vram), "Radeon AI PRO R9700 · 32 GB");
   }
   ```

**Steps:**
- [ ] Add `format_card_subtitle` helper function
- [ ] Add subtitle div to the left column in `GpuDeviceCard` view (between header and model-section)
- [ ] Add `.gpu-device-card__subtitle` CSS rules with border-bottom separator
- [ ] Add `.gpu-device-card__bars > .gpu-device-card__row:first-child` CSS for middle column separator
- [ ] Update `make_gpu` test fixture with `name` field
- [ ] Add tests for `format_card_subtitle`
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(web): add GPU card name subtitle and section separators"

**Acceptance criteria:**
- [ ] Left column shows `{name} · {total_gb} GB` under the GPU header
- [ ] Horizontal separator between subtitle and model section (left column)
- [ ] Horizontal separator between Utilization and VRAM rows (middle column)
- [ ] Portrait mode (3+ GPUs) still works correctly
- [ ] All tama-web tests pass
- [ ] VRAM total rounds to nearest integer (31.9 → 32)

---

### Task 4: Integration build + full test

**Context:**
Final verification that the full workspace builds and all tests pass across both crates.

**Prerequisite:** Tasks 1-3 completed and committed.

**Files:**
- Workspace-wide

**Steps:**
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Squash/amend commits if needed into logical groupings

**Acceptance criteria:**
- [ ] Full workspace builds without errors
- [ ] All tests pass
- [ ] Clippy clean
- [ ] Formatting clean
