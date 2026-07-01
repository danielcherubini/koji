# Model Editor: GPU Isolation UI Polish Plan

**Goal:** Polish the "GPU Device" selector in the model editor — rename label, fix default text, style refresh button.
**Architecture:** Purely cosmetic. Two files: Rust (Leptos view) and CSS.
**Tech Stack:** Leptos (Rust), CSS

---

### Task 1: Polish GPU Isolation label, default option, and refresh button

**Context:**
The "GPU Device" selector in the model editor's general form has three cosmetic issues:
1. The label reads "GPU Device" but the feature is GPU isolation (pinning a model to a specific GPU via env var)
2. The default option reads "Default (backend default)" which is verbose and misleading — "None" is clearer
3. The refresh button has no CSS styling (`form-icon-button` class is undefined), so it renders as a raw browser button

This task fixes all three in a single commit since they're tightly coupled to the same UI region.

**Files:**
- Modify: `crates/tama-web/src/pages/model_editor/general_form.rs`
- Modify: `crates/tama-web/css/14-model-editor.css`

**What to implement:**

In `general_form.rs` (around line 325), find the GPU device label and select block:

1. Change the label text from `"GPU Device"` to `"GPU Isolation"`
2. Remove the `<span class="form-actions-inline">` wrapper around the refresh button (unused CSS class)
3. Simplify the refresh button to just a `<button class="form-icon-button">` directly inside the `<label>`
4. Change the default `<option value="">` text:
   - `"Default (backend default)"` → `"None"`
   - `"Default (could not list devices)"` → `"None (could not list devices)"`

The refreshed Rust code for the label + button should look like:
```rust
<label class="form-label" for="field-gpu-device">
    "GPU Isolation"
    <button
        class="form-icon-button"
        title="Refresh GPU devices"
        disabled=move || gpu_fetching.get()
        on:click=move |_| {
            refresh_devices.run(());
        }
    >
        {move || {
            if gpu_fetching.get() {
                "⟳".to_string()
            } else {
                "↻".to_string()
            }
        }}
    </button>
</label>
```

And the default option should look like:
```rust
<option value="">
    {move || {
        if gpu_devices.get().is_empty() && !gpu_fetching.get() {
            "None (could not list devices)"
        } else {
            "None"
        }
    }}
</option>
```

In `14-model-editor.css`, add the `.form-icon-button` styles at the end of the file:
```css
.form-icon-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 1.4rem;
  height: 1.4rem;
  padding: 0;
  margin-left: 0.35rem;
  background: none;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  font-size: 0.95rem;
  line-height: 1;
  cursor: pointer;
  transition: color var(--transition-fast), border-color var(--transition-fast);
  vertical-align: middle;
}
.form-icon-button:hover {
  color: var(--text-primary);
  border-color: var(--border-color);
}
.form-icon-button:disabled {
  opacity: 0.3;
  cursor: not-allowed;
}
```

**Steps:**
- [ ] Edit `crates/tama-web/src/pages/model_editor/general_form.rs`:
  - Change label text to `"GPU Isolation"`
  - Remove `<span class="form-actions-inline">` wrapper
  - Change default option text to `"None"` / `"None (could not list devices)"`
- [ ] Edit `crates/tama-web/css/14-model-editor.css`:
  - Append `.form-icon-button` styles at the end of the file
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --package tama-web --lib --features ssr`
  - Did all tests pass? If not, fix and re-run.
  - Note: This change is purely cosmetic (label text + CSS). The tama-web test suite covers SSR/API backend logic, not Leptos UI components. No new tests are needed for this change.
- [ ] Commit with message: "style: polish model editor GPU isolation label and refresh button"

**Acceptance criteria:**
- [ ] Label reads "GPU Isolation"
- [ ] Default option reads "None" (or "None (could not list devices)" when devices can't be listed)
- [ ] Refresh button is styled as a small square icon button (not a raw browser button)
- [ ] Refresh button has hover state (lighter color + border) and disabled state (faded)
- [ ] Build succeeds with no clippy warnings
