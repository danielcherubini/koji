# Plan 194 — Models Page: Live State, Filter Toolbar, Pull-Host UX

**Goal.** On the web control plane's Models page: (1) Load/Unload/Cancel visibly
change state without a manual page reload; (2) restore the #136 sort/group
toolbar and add a search box + state pills; (3) make the pull host
(`proxy.pull_backend` — the tamad that executes pulls, ADR-0010) configurable
in the Config editor and fix the stale comments that point to the removed
local-download fallback.

**Architecture.** The proxy already reports *live* lifecycle state in
`GET /tama/v1/models` (state comes from `collect_model_state_snapshots()`,
which reads each tamad's per-second process rows). The gap is purely that the
Models page (`crates/tama/src/pages/models/mod.rs`) fetches once and only
refetches after an action. Fix: one `gloo_timers` 1.5s interval that
refetches adaptively (fast while any model is transitional, 8s heartbeat
otherwise). Filters are client-side over the fetched list (dozens of rows).
The Config editor already saves the whole config via
`POST /tama/v1/config/structured` and the PATCH DTO already models
`pull_backend` — the UI field is purely additive.

**Tech Stack.** Leptos 0.7 (WASM/Trunk), gloo-timers, web-sys (`Window`,
`Document`, `Storage` features already enabled), axum (no backend changes),
existing `11-models.css` (`.models-toolbar` + select styles already exist).

**Terms** (see `CONTEXT.md`): *tamad* = daemon on an inference host that owns
all self-hosted concerns (lifecycle, pulls, installs, benchmarks); *proxy* =
the routing/config component that orchestrates but never touches host disk or
processes (ADR-0010 — "the proxy spawns nothing"); *pull host* = the tamad
named by `proxy.pull_backend` that executes model pulls on its own disk;
*ModelState* = `idle | starting | ready | unloading | failed`.

## Plan-level RULES

Branch
- Branch: `feature/models-page-live-filters-pull-host`, off `main` (follow the
  `gitflow-branching` skill). One commit per task (T1–T5), in order.

Gate (before every commit — mirrors CI):
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- `cargo nextest run --workspace`

Targeted commands during development (AGENTS.md):
- `cargo nextest run --package tama -- models::` (frontend tests live in the
  `tama` crate)
- `cargo nextest run --package tama-core -- config::` (for T5's comment-only
  change the full gate suffices — comments don't affect tests)

Regrep rule.
- Line numbers below are as-of-now approximations. Re-grep with `rg -n`
  before editing. If a symbol moved, the text of the task holds; the location
  is overwritten by the grep.

Fixed decisions (already approved in discussion — do NOT re-litigate):
1. Adaptive polling (NOT a new SSE endpoint, NOT reusing the metrics stream).
2. Full toolbar: search + state pills (All/Loaded/Idle/Failed/Disabled) +
   sort (Name/Status/GPU/Family/Vendor) + group-by (None/GPU/Family/Vendor/
   Status). Sort + group persist to localStorage exactly as #136 did; search
   text and selected pill are session state.
3. Pull host = a `<select>` in the Config editor's Proxy → Tuning section fed
   by `GET /tama/v1/tamads`; plus FOUR stale-comment fixes (two of which
   say "download**s** locally", which naive greps miss — see T5); plus a
   link from the pull wizard's "no pull host configured" error to the
   Config page (in BOTH wizard Downloading steps).

---

### Task 1: Adaptive polling for live lifecycle state (Models page)

**Context:**
The Models page is fetch-once: it builds a `LocalResource` keyed on a `refresh`
counter and only increments that counter after load/unload/cancel/check-all
actions return. But the load POST is **long-running**: `handle_tama_load_model`
(routed at `crates/tama-core/src/proxy/server/router.rs:62-66`) calls
`load_model_on_tamad`, whose `LoadModel` RPC **blocks until the tamad's
spawn + health poll completes** (see the doc comment in
`crates/tama-core/src/proxy/lifecycle/spec.rs`) — minutes on a large model.
During that window the page shows zero feedback (badge stuck at "Idle",
the click appears to do nothing), and even after the POST returns there is
no follow-up refetch, so any state change landing after that single
refetch (late `ready` flip, idle-timeout unload, reconciler respawn) never
renders — the reported "feels like nothing is happening" bug. `GET
/tama/v1/models` (handler `list_models` in
`crates/tama/src/api/models/info.rs`, snapshot state keyed by `db_id`)
already reflects live state, so a refetch cadence is the whole fix. The
cadence is adaptive: ~1.5s while any model is in a transitional state
(`starting`/`unloading`), an 8s steady-state heartbeat otherwise, paused
while the tab is hidden.

**Files:**
- Modify: `crates/tama/src/pages/models/mod.rs`

**What to implement:**

1. In `crates/tama/src/pages/models/mod.rs`, add at module level (near the
   other helpers):

   ```rust
   // `tama`'s default feature is `ssr`; BOTH CI clippy gates compile the
   // native (ssr) build where the scheduler below is cfg'd out — so these
   // consts must be cfg-gated too, or `dead_code` -D-warnings fails the gate.
   #[cfg(not(feature = "ssr"))]
   /// Fixed interval of the polling scheduler (ms). One interval, dual
   /// condition — no dynamic rescheduling.
   const FAST_TICK_MS: u64 = 1_500;
   #[cfg(not(feature = "ssr"))]
   /// Steady-state heartbeat between refetches (ms) when nothing is
   /// transitional.
   const HEARTBEAT_MS: u64 = 8_000;

   /// Current wall-clock time in epoch milliseconds. Saturates to 0 on
   /// systems whose clock predates the epoch.
   fn now_ms() -> u64 {
       std::time::SystemTime::now()
           .duration_since(std::time::UNIX_EPOCH)
           .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
           .unwrap_or(0)
   }

   /// Scheduler decision: refetch iff a model is in a transitional state,
   /// the initial fetch has never completed (`last_fetch_ms == 0`), or the
   /// last successful fetch is at least `heartbeat_ms` old.
   fn should_refetch(
       transitional: bool,
       last_fetch_ms: u64,
       now_ms: u64,
       heartbeat_ms: u64,
   ) -> bool {
       transitional
           || last_fetch_ms == 0
           || now_ms.saturating_sub(last_fetch_ms) >= heartbeat_ms
   }
   ```

2. Inside the `Models()` component, before the existing `models` LocalResource,
   add the three bookkeeping signals:

   ```rust
   /// Epoch ms of the last successful fetch (0 = none yet).
   let last_fetch_ms = RwSignal::new(0u64);
   /// True while any model in the last fetch was Starting or Unloading.
   let transitional = RwSignal::new(false);
   /// True while a fetch request is in flight (initial value `true`: the
   /// first fetch is pending on mount). The scheduler skips ticks while this
   /// is set.
   let fetching = RwSignal::new(true);
   ```

   Replace the `models` LocalResource's fetch closure (keep the same
   resource, the same `refresh` keying, and the same `None`-on-failure
   behavior) so it maintains the bookkeeping in one place. `fetching` goes
   true at start and false at end — a **single post-await exit**, no
   scattered early returns outside the inner block. Bookkeeping is recorded
   only on a successful parse. Deliberately side effects in the closure
   rather than an `Effect`, to avoid `Guard`-take interactions with the
   view:

   ```rust
   let models = LocalResource::new(move || async move {
       let _ = refresh.get(); // track the signal
       fetching.set(true);
       let parsed = async {
           let resp = get_request("/tama/v1/models").send().await.ok()?;
           // POLARITY: `handle_response` returns TRUE when a 401 redirect
           // was triggered (caller must bail) and FALSE for a valid
           // response — keep this exact form from the existing closure.
           if handle_response(&resp) {
               return None;
           }
           resp.json::<ModelsResponse>().await.ok()
       }
       .await;
       fetching.set(false);
       if let Some(p) = &parsed {
           last_fetch_ms.set(now_ms());
           transitional.set(p.models.iter().any(|m| matches!(
               m.state,
               ModelState::Starting | ModelState::Unloading
           )));
       }
       parsed
   });
   ```

   (`models`, `last_fetch_ms`, `transitional`, `fetching` are all `Copy` —
   capture by value. Do NOT use `?` / early `return` outside the inner
   block: the single post-await `fetching.set(false)` must run on every
   path, including HTTP errors and parse failures.)

3. After the resource is created, add the scheduler, compile-gated for SSR
   (the whole frontend runtime is wasm-only). **API notes (verified against
   the locked versions):**
   - Leptos 0.7 (0.7.8) resource read API has **no `is_pending()`** —
     that was the pre-0.7 `ResourceState` API. Do not call it; the
     `fetching` signal below replaces the in-flight check entirely.
   - The locked **gloo-timers has no free `set_interval`** — the crate
     exposes `gloo_timers::callback::Interval` (`Interval::new(ms, cb)`;
     **dropping the handle CANCELS the interval**, so `.forget()` it).
   - In a bare timer callback there is no reactive observer, so plain
     `.get()` is correct (`.try_get()` returns `Option` in 0.7 — do not use
     it for these `bool`/`u64` args).

   ```rust
   #[cfg(not(feature = "ssr"))]
   {
       let refresh_i = refresh;
       let last_fetch_ms_i = last_fetch_ms;
       let transitional_i = transitional;
       let fetching_i = fetching;
       let interval =
           gloo_timers::callback::Interval::new(FAST_TICK_MS as u32, move || {
               // Skip while the tab is hidden. On return the NEXT tick
               // (within one fast tick ≈1.5s) does the catch-up refetch.
               let hidden = web_sys::window()
                   .and_then(|w| w.document())
                   .map(|d| d.hidden())
                   .unwrap_or(false);
               if hidden {
                   return;
               }
               // Skip while a fetch is in flight (prevents request
               // overlap; covers the initial load too).
               if fetching_i.get() {
                   return;
               }
               if should_refetch(
                   transitional_i.get(),
                   last_fetch_ms_i.get(),
                   now_ms(),
                   HEARTBEAT_MS,
               ) {
                   refresh_i.try_update(|n| *n += 1);
               }
           });
       interval.forget(); // keep alive for the app lifetime
   }
   ```

4. In the four existing actions (`load_action`, `unload_action`,
   `cancel_action`, `check_all_action`), add **one line each** right next to
   the existing `refresh.update(|n| *n += 1);` call:

   ```rust
   transitional.set(true); // optimistic: a fetch is warranted from the next tick onward
   ```

   (This guarantees a refetch within one 1.5s tick of an action, even if the
   optimistic action refetch itself lands before the tamad has flipped state.
   The bookkeeping in the resource closure corrects `transitional` when the
   next fetch arrives.)

5. Do NOT change: `ModelCard` usage, the pull modal, the tab structure, the
   load/unload/cancel POST URLs or their error handling, anything in
   `tama-core`.

6. Unit tests (in the existing `#[cfg(test)] mod tests` in this file):

   ```rust
   #[test]
   fn test_should_refetch_when_transitional() {
       assert!(should_refetch(true, 1_000, 1_001, 8_000)); // 1ms since fetch, still refetch
   }

   #[test]
   fn test_should_refetch_after_heartbeat() {
       assert!(should_refetch(false, 1_000, 9_000, 8_000)); // 8000ms elapsed
   }

   #[test]
   fn test_no_refetch_before_heartbeat_and_not_transitional() {
       assert!(!should_refetch(false, 1_000, 5_000, 8_000));
   }

   #[test]
   fn test_should_refetch_never_overshoots_on_clock_jitter() {
       assert!(!should_refetch(false, 5_000, 1_000, 8_000)); // now < last_fetch
       assert!(should_refetch(false, 0, 1, 8_000)); // last_fetch 0 = never fetched
   }
   ```

**Steps:**
- [ ] Write the four `should_refetch` tests in `crates/tama/src/pages/models/mod.rs`
- [ ] Run `cargo nextest run --package tama -- models::`
  - Did `should_refetch` tests fail to compile (function doesn't exist yet)? If they pass unexpectedly, stop and investigate why.
- [ ] Implement items 1–4 above
- [ ] Run `cargo nextest run --package tama -- models::`
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo check --package tama` and `cargo check --package tama --features ssr`
  - Did both succeed (the SSR-gated interval must not break `--features ssr`)? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Verify manually (dev server, `make dev` or the running UI): click **Load** on an idle model → within ~1.5s the badge shows **Starting…** with a working **Cancel** button; within seconds–minutes the badge flips to **Loaded** with an **Unload** button — no manual reload. Click **Unload** → **Unloading…** → **Idle**. Use DevTools Network: the Models page must issue `GET /tama/v1/models` at ~1.5s cadence only while a model is starting/unloading, otherwise at most every 8s.
- [ ] Run the full gate (see Plan-level RULES)
- [ ] Commit with message: `feat(models): live lifecycle sync via adaptive polling on Models page`

**Acceptance criteria:**
- [ ] Clicking Load/Unload/Cancel on the Models page visibly updates the badge and action button without any manual page reload.
- [ ] While any model is `starting` or `unloading`, the page refetches ≤1.5s apart; when all models are idle/ready/failed it refetches ≤8s apart.
- [ ] No refetches occur while the document is hidden; the catch-up refetch happens within one fast tick (≤1.5s) of the tab becoming visible again.
- [ ] The four `should_refetch` unit tests pass; full gate green.

---

### Task 2: Filter toolbar — search, state pills, sort, group-by

**Context:**
Commit `c7decf9a` (#136) added a sort + group-by toolbar to the Models page;
`d0990bd8` (#137) moved that clipboard to the dashboard, and the plan-192/193
dashboard refactor deleted it from there too — the helper code survives as
dead code in `crates/tama/src/pages/dashboard/mod.rs` (lines ~16–207, marked
`#[allow(dead_code)] // Used only by unit tests in dashboard/tests.rs`). This
task restores the toolbar on the Models page with full #136 parity and adds
the two controls it never had: a search box and state pills. Everything is
client-side over the already-fetched list; the pipeline order is fixed:
search → state pill → sort → group-by.

**Files:**
- Create: `crates/tama/src/pages/models/filters.rs`
- Modify: `crates/tama/src/pages/models/mod.rs` (declare `mod filters;`,
  add the toolbar view + signals, wire the pipeline into the list render)
- Modify: `crates/tama/css/11-models.css` (toolbar/pill/group styles;
  `.models-toolbar` and `.models-toolbar select` classes already exist
  around line 128 — extend, don't duplicate)
- Test: tests inside `crates/tama/src/pages/models/filters.rs`
  (`#[cfg(test)] mod tests`)

**What to implement:**

1. `crates/tama/src/pages/models/filters.rs` — all functions
   `pub(super)` so only the models page module tree sees them. `ModelEntry`
   is the private struct in `super` (`models/mod.rs`); import it as
   `use super::ModelEntry;`. Also reuse
   `crate::components::model_card::model_status_badge_label` (pub(crate)) for
   the Status group key and `super::gpu_group_label` for the GPU label.

   ```rust
   /// Sort criteria (restored from #136).
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
   pub(super) enum SortBy { #[default] Name, Status, Gpu, Family, Vendor }

   /// Optional group-by (restored from #136).
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub(super) enum GroupBy { Gpu, Family, Vendor, Status }

   /// Single-select view filter.
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
   pub(super) enum ViewFilter {
       #[default]
       All,
       Loaded,
       Idle,
       Failed,
       Disabled,
   }
   ```

   Pure functions (port the #136 logic from the dead dashboard copies in
   `dashboard/mod.rs` — re-grep there for the **suffixed** names
   `extract_gpu_sort_key_model_status`, `extract_sort_key_model_status`,
   `extract_vendor_model_status`, `extract_group_key_model_status`,
   `group_display_order`, `sort_models_status`, `parse_sort_by`,
   `parse_group_by` — and adapt to `ModelEntry`):

   - `pub(super) fn matches_search(m: &ModelEntry, query: &str) -> bool`
     — `true` when `query.trim()` is empty, else when the case-insensitive
     `contains` matches any of: `m.display_name`, `m.api_name`, `m.model`
     (the repo/model id), `m.quant` (all `Option<String>`, treat `None` as
     non-matching).
   - `pub(super) fn matches_view(m: &ModelEntry, v: ViewFilter) -> bool`
     — `All` → true; `Loaded` → `matches!(m.state, ModelState::Ready)`;
     `Idle` → `ModelState::Idle`; `Failed` → `ModelState::Failed`;
     `Disabled` → `!m.enabled`. Note: `Disabled` is an `enabled` flag, not a
     lifecycle state — a model can be Disabled AND Loaded.
   - `pub(super) fn extract_vendor(m: &ModelEntry) -> String`
     — #136 precedence: `display_name` split on `':'`, else `api_name` split
     on `':'`, else `hf_base_model` split on `'/'`; first non-empty piece,
     else `"Other"`.
   - `pub(super) fn extract_gpu_sort_key(gpu_device: &Option<String>) -> (u32, u32)`
     — `(0, index)` when `gpu_device` is `Some` with trailing digits,
     `(1, 0)` when `None` or digit-less (non-GPU sorts last, #136 behavior).
   - `pub(super) fn extract_sort_key(m: &ModelEntry, sort_by: SortBy) -> String`
     — `Name` → `super::model_display_name(m)`; `Status` →
     `m.state.as_str().to_string()` (**raw state string — #136 parity:**
     the dead-code copy sorts alphabetically over
     `"failed","idle","ready","starting","unloading"`; do NOT use the
     human labels here — "Loaded" would sort to a different slot than
     "ready"); `Family` →
     `m.hf_architecture_type.clone().unwrap_or_default()`; `Vendor` →
     `extract_vendor(m)`; `Gpu` → `String::new()` (handled separately).
   - `pub(super) fn sort_models(models: &mut [ModelEntry], sort_by: SortBy)`
     — `Gpu` → sort by `extract_gpu_sort_key(&m.gpu_device)`; everything else
     → `sort_by_key(|m| extract_sort_key(m, sort_by))`.
   - `pub(super) fn extract_group_key(m: &ModelEntry, by: GroupBy) -> String`
     — `Gpu` → `super::gpu_group_label(&m.gpu_device)`; `Family` →
     `m.hf_architecture_type.clone().unwrap_or_else(|| "Unknown".into())`;
     `Vendor` → `extract_vendor(m)`; `Status` →
     `model_status_badge_label(&m.state)`.to_string().
   - `pub(super) fn group_display_order(by: GroupBy, key: &str) -> u32`
     — `Gpu` → extract the trailing index of `key`
     ("GPU 0" → 0), `u32::MAX` for `"No GPU"` (sorts last); anything else 0.
   - `pub(super) fn parse_sort_by(s: &str) -> SortBy`
     — `"status"|"gpu"|"family"|"vendor"`, else `SortBy::Name`.
   - `pub(super) fn parse_group_by(s: &str) -> Option<GroupBy>`
     — `"gpu"|"family"|"vendor"|"status"`, else `None`.
   - `pub(super) fn parse_view_filter(s: &str) -> ViewFilter`
     — `"loaded"|"idle"|"failed"|"disabled"`, else `ViewFilter::All`.
   - `pub(super) fn apply_pipeline(models: &[ModelEntry], query: &str, view: ViewFilter, sort_by: SortBy) -> Vec<ModelEntry>`
     — `models.iter().filter(|m| matches_search(m, query) && matches_view(m, view)).cloned().collect()`, then `sort_models` in place, return.
   - `pub(super) fn group_survivors(models: &[ModelEntry], by: GroupBy) -> Vec<(String, Vec<ModelEntry>)>`
     — bucket by `extract_group_key`, sort group keys by
     `group_display_order` (ties: label ascending), return ordered buckets
     with the models in their existing (already-sorted) order.

   Unit tests (port from `crates/tama/src/pages/dashboard/tests.rs` — re-grep
   lines ~573–760 for `test_extract_vendor_*`, `test_extract_gpu_sort_key_*`,
   `test_parse_sort_by`, `test_parse_group_by`, `test_extract_group_key_*`,
   `test_group_display_order_*`, `test_sort_models_by_*` — adapting the
   constructor from `ModelStateSnapshot` to `ModelEntry`; a small
   `fn entry(name: &str, gpu: Option<&str>, state: &str) -> ModelEntry`
   helper makes this trivial — plus new tests):
   - `test_matches_search_empty_query_matches_all`
   - `test_matches_search_repo_and_quant` (match on `m.model`, match on
     `m.quant`, non-match)
   - `test_matches_search_case_insensitive_display_name`
   - `test_matches_view_loaded_uses_ready_state` /
     `test_matches_view_disabled_uses_enabled_flag` /
     `test_matches_view_all`
   - `test_parse_view_filter_unknown_defaults_to_all`
   - `test_apply_pipeline_composes_search_and_view`
   - `test_group_survivors_gpu_order_no_gpu_last`

2. `crates/tama/src/pages/models/mod.rs` — component wiring:

   ```rust
   mod filters;
   use filters::{apply_pipeline, group_survivors, parse_group_by, parse_sort_by, GroupBy, SortBy, ViewFilter};
   ```

   New signals inside `Models()` (initialize sort/group from localStorage per
   #136, using the `web_sys::window().local_storage()` pattern already in
   `crates/tama/src/utils/mod.rs` lines ~43–52):

   ```rust
   let search_query = Signal::new(String::new());          // session state
   let view_filter = RwSignal::new(ViewFilter::All);       // session state
   let sort_by = RwSignal::new(read_stored("tama-models-sort-by")
       .as_deref().map(parse_sort_by).unwrap_or_default()); // persist (as string)
   let group_by = RwSignal::new(read_stored("tama-models-group-by")
       .as_deref().and_then(parse_group_by));              // persist (as string)
   ```
   (Write a small local helper `fn read_stored(key: &str) -> Option<String>`
   and persist with `Effect` on `sort_by`/`group_by` — `window`/`storage`
   may be `None` in SSR; guard with `if let Ok(storage) = ...`.)

   Persistence contract: key `tama-models-sort-by` stores
   `"name"|"status"|"gpu"|"family"|"vendor"`; key `tama-models-group-by`
   stores `""` for None or `"gpu"|"family"|"vendor"|"status"`. Search and
   pill: NOT persisted.

   Toolbar view — inserted between the `check_all_status` alert block and
   the `Suspense`, inside the `Tab::Models` branch, wrapped in
   `<div class="models-toolbar models-filter-bar">`:
   - `<input type="search" class="form-input filter-search" placeholder="Filter models (name, repo, quant)…" value=move || search_query.get() on:input=move |ev| search_query.set(target_value(&ev))>`
     (do NOT add an `on:clear` handler — the native `clear` event on search
     inputs is non-standard and unimplemented in Firefox; an empty input is
     already the no-filter state (`matches_search` returns `true` for an
     empty query), and the Clear-filters button covers explicit resets.)
   - five pill `<button class="state-pill">` with labels **"All"**,
     **"Loaded"**, **"Idle"**, **"Failed"**, **"Disabled"**; single-select:
     `attr:class=move || format!("state-pill{}", if selected {" state-pill--active"} else {""})`; click sets `view_filter` (and no others).
   - right side: `<select class="filter-select">` for Sort (options:
     Name/Status/GPU/Family/Vendor) and Group-by (options: None/GPU/Family/
     Vendor/Status), both `prop:value` + `on:change` parsing via
     `parse_sort_by`/`parse_group_by`.

   List render — replace the direct `data.models.into_iter()` body in the
   `Some(data) =>` arm with the pipeline:

   ```rust
   let all = &data.models;
   let visible = apply_pipeline(all, &search_query.get(), view_filter.get(), sort_by.get());
   // flat:
   let rows: Vec<AnyView> = if group_by.get().is_none() {
       visible.iter().map(|m| <render ModelCard as today>).collect()
   } else {
       // grouped: for (label, bucket) in group_survivors(&visible, group_by.get().unwrap()):
       // <div class="model-group">
       //   <div class="group-header"><span>{label}</span><span class="group-count">{bucket.len()}</span></div>
       //   <div class="models-list">…rows…</div>
       // </div>
   };
   ```

   The ModelCard construction (all the `resolve_quant/resolve_context_length/
   ...` + callback wiring) must remain **identical** — extract it into a
   local `fn`/closure if it helps the grouped path, but do not change props.

   Empty-after-filter: when `!all.is_empty() && visible.is_empty()` render
   instead of the list:
   ```html
   <div class="card card--centered">
       <p class="text-muted">No models match your filters.</p>
       <button class="btn btn-secondary mt-2" /* clears search + pill, keeps sort/group */>
           Clear filters
       </button>
   </div>
   ```

3. `crates/tama/css/11-models.css` — append:
   - `.models-filter-bar` (flex row, `gap`, `align-items: center`, `flex-wrap: wrap`; reuse the existing `.models-toolbar` container for the row or compose both — do not create conflicting duplicate widths).
   - `.filter-search` (max-width ~280px, blocks the default `.form-input` full width if needed).
   - `.state-pill` (pill button: transparent bg, 1px border `var(--border)`-class variable, radius 999px, padding `0.25rem 0.75rem`, cursor pointer, `color: var(--text-secondary)`) and `.state-pill--active` (accent background, e.g. `var(--accent-blue)` or the existing highlight token, text `#fff`/`var(--text-on-accent)` if it exists — check `01-custom-properties.css` first).
   - `.group-header` (muted small-caps-ish label row, `margin`, `display: flex; justify-content: space-between`) and `.group-count` (muted, `font-size: 0.85em`).

**Steps:**
- [ ] Create `crates/tama/src/pages/models/filters.rs` with the enums + pure fns + all tests (tests first)
- [ ] Run `cargo nextest run --package tama -- models::filters`
  - Did it fail (won't compile/use-what? — expect compile failure of `filters.rs` consumers or `mod` not declared)? Declare `mod filters;` in `models/mod.rs` first so the module compiles, then confirm the tests are runnable.
- [ ] Implement `filters.rs` fns until tests pass: `cargo nextest run --package tama -- models::filters`
  - All passed? If not, fix and re-run.
- [ ] Wire the toolbar + pipeline + empty state into `models/mod.rs` (item 2)
- [ ] Add CSS (item 3)
- [ ] Run `cargo check --package tama` + `cargo check --package tama --features ssr`
- [ ] Verification in the dev server: with ≥1 loaded + ≥1 idle + 1 disabled + 1 failed (if you can make one) model: search narrows the list; each pill shows exactly its bucket; sorting by Status/GPU/Vendor/Family matches #136 behavior (GPU models first on GPU sort; "No GPU" group last when grouped by GPU); group headers show counts; reload → sort + group-by survive (localStorage), search + pill reset; "Clear filters" restores the full list without touching sort/group; empty result shows the Clear-filters card.
- [ ] Run the full gate
- [ ] Commit with message: `feat(models): filter toolbar — search, state pills, sort + group-by`

**Acceptance criteria:**
- [ ] `GET /tama/v1/models` is fetched exactly once per refresh tick — filtering never triggers a refetch (DevTools Network).
- [ ] Search matches display_name/api_name/model/quant case-insensitively; empty query = no search.
- [ ] Pills single-select; `Loaded` = state `ready`, `Idle` = `idle`, `Failed` = `failed`, `Disabled` = `enabled == false`, `All` shows everything.
- [ ] Sort + group-by behave exactly as #136 and persist across reload under `tama-models-sort-by` / `tama-models-group-by`; search + pill do not.
- [ ] Live interplay: with the Task 1 polling on, a model becoming `ready` moves under "Loaded" within a tick without a reload.
- [ ] All `filters::` tests pass; full gate green.

---

### Task 3: Delete the dead sort/group code from the dashboard

**Context:**
Since plan-192/193 the dashboard no longer renders a sort/group toolbar, but
`crates/tama/src/pages/dashboard/mod.rs` (~lines 16–207) still carries the
`SortBy`/`GroupBy` enums and their helper functions (`extract_gpu_sort_key_
model_status`, `gpu_group_label_model_status`, `extract_sort_key_model_status`,
`extract_vendor_model_status`, `extract_group_key_model_status`,
`group_display_order`, `sort_models_status`, `parse_sort_by`, `parse_group_by`),
each annotated `#[allow(dead_code)] // Used only by unit tests in
dashboard/tests.rs`. Task 2 re-established the canonical sort/group logic in
`models/filters.rs`, so per the DRY rule the dashboard copies (and their
tests in `dashboard/tests.rs`, re-grep lines ~573–760 — the
`extract_vendor`/`gpu_sort_key`/`gpu_group_label`/`parse_*`/
`extract_group_key`/`group_display_order`/`sort_models_by_*` tests) are
delete-able. Do NOT touch `model_gpu_label` — a `pub` function of the
`gpu_device_card` COMPONENT (`crates/tama/src/components/gpu_device_card.rs`)
imported by `dashboard/tests.rs`, not dashboard code.

**Files:**
- Modify: `crates/tama/src/pages/dashboard/mod.rs`
- Modify: `crates/tama/src/pages/dashboard/tests.rs`

**What to implement:**
1. Delete from `dashboard/mod.rs` the ENTIRE dead region — re-grep to bound
   it (currently: from the "Sort/Group enums" region header ~line 14 through
   the end of `parse_group_by` ~line 206, ending at `#[cfg(test)] mod
   tests;`). The deletion set is the whole region, which includes:
   the `SortBy` and `GroupBy` enums; `extract_gpu_index`; `extract_gpu_
   sort_key_model_status`; `gpu_group_label_model_status`;
   `extract_sort_key_model_status`; `extract_vendor_model_status`;
   `extract_group_key_model_status`; `group_display_order`;
   `capitalize_first`; `sort_models_status`; `parse_sort_by`;
   `parse_group_by`. This region contains ALL 14 `#[allow(dead_code)]`
   annotations in the file — after deletion, `rg -c "allow(dead_code)"
   crates/tama/src/pages/dashboard/mod.rs` must return **0**. Remove
   imports that become unused (re-grep first — `ModelStateSnapshot` may
   still be used by the hosts section; keep it if so).
2. Delete from `dashboard/tests.rs`: the helper `make_sort_model` (becomes
   unused → `dead_code` under `--all-targets` clippy after its consumers
die), and every test in the "Sort/Group helper tests" section — re-grep
   `sort_models_status|extract_group_key_model_status|group_display_order|parse_sort_by|parse_group_by|extract_vendor_model_status|extract_gpu_sort_key_model_status|sort_models_by_|group_key_status|test_capitalize_first`
   (the section spans ~line 528 through the end of
   `test_sort_models_by_gpu_numeric_order` ~line 768). KEEP everything else:
   `make_test_model`, `make_test_gpu`, `test_model_gpu_label_*` (that
   function is a `gpu_device_card` COMPONENT, imported by these tests —
   not dashboard code), and all metrics/hosts/telemetry tests.
3. Do NOT touch `dashboard/metrics.rs`, `ActiveModelRow`, host cards,
   `model_gpu_label` (in `crates/tama/src/components/gpu_device_card.rs`),
   or telemetry.

**Steps:**
- [ ] `rg -n "sort_models_status|extract_group_key_model_status|group_display_order|parse_sort_by|parse_group_by|extract_vendor_model_status|extract_gpu_sort_key_model_status|extract_sort_key_model_status|gpu_group_label_model_status|capitalize_first|extract_gpu_index" crates/tama/src/pages/dashboard/` — confirm the full deletion set
- [ ] Delete items 1–2
- [ ] Run `cargo check --package tama` — fix any unresolved references the grep missed
- [ ] Run `cargo nextest run --package tama -- dashboard::`
  - All remaining dashboard tests pass? If not, you deleted too much — restore the specific test and re-run.
- [ ] Run the full gate
- [ ] Commit with message: `chore(dashboard): remove dead sort/group helpers superseded by models page (plan-194 T2)`

**Acceptance criteria:**
- [ ] `rg -c "allow(dead_code)" crates/tama/src/pages/dashboard/mod.rs` returns 0 (all 14 pre-existing annotations in that file were in the sort/group region).
- [ ] No duplicate sort/group logic remains anywhere: `rg -n "enum SortBy|enum GroupBy" crates/tama/src` matches only `models/filters.rs`.
- [ ] Full dashboard test suite green; full gate green.

---

### Task 4: Config editor "Pull host" field

**Context:**
`proxy.pull_backend` (the *pull host*: a tamad's id, FK-enforced to
`tamad_registry`) names which tamad executes model pulls; when unset, pulls
fail with the explicit `"no pull host configured: set proxy.pull_backend
(the proxy itself never downloads — ADR-0010)"` error (set in
`crates/tama-core/src/proxy/tama_handlers/pull/start.rs` ~line 226).
Today this field is only settable via the raw config API/CLI — the web Config
editor (`crates/tama/src/pages/config_editor/`) has no control for it.
Everything the save path needs ALREADY exists: the frontend mirror
`crate::types::config::ProxyConfig.pull_backend: Option<String>`
(`crates/tama/src/types/config/proxy.rs` ~line 83), the save endpoint
`POST /tama/v1/config/structured` (full-body save, `crates/tama/src/api.rs`
~line 80), and the tamad list API `GET /tama/v1/tamads` returning
`Vec<TamadConnection>` (`id`, `name`, `url`, `protocol`, `token`, `status`).
The UI change is purely additive.

**Files:**
- Modify: `crates/tama/src/pages/config_editor/forms/proxy/advanced.rs`
- Test: tests inside the same file (`#[cfg(test)]` block) for the pure option-builder helper

**What to implement:**

1. In `advanced.rs`, add a local frontend DTO + helpers:

   ```rust
   #[derive(Debug, Clone, serde::Deserialize)]
   struct TamadRef {
       id: String,
       name: String,
   }
   ```

   (Deserialize only `id` + `name`; serde ignores extra fields by default on
   structs without `deny_unknown_fields` — verify no `deny_unknown_fields` on
   the type before relying on this.)

   ```rust
   /// Option entries for the pull-host select: ("", "None") first, then one
   /// per tamad labeled "<name> · <short id>".
   fn pull_host_options(tamads: &[TamadRef]) -> Vec<(String, String)> {
       let mut out = vec![("".to_string(), "None".to_string())];
       for t in tamads {
           out.push((t.id.clone(), format!("{} · {}", t.name, short_id(&t.id))));
       }
       out
   }

   /// "3f2a9c1b…" → "3f2a…" (first 4 chars + ellipsis); ids ≤ 8 chars pass through.
   fn short_id(id: &str) -> String {
       if id.len() <= 8 { id.to_string() } else { format!("{}…", &id[..4]) }
   }
   ```

   Note: UUIDs are 36 ASCII chars, so `&id[..4]` is always char-boundary-safe
   for the values the API emits; keep the plain slice slice but document the
   assumption in the fn doc comment (the reviewer flagged the non-ASCII edge
   case — it cannot occur for UUIDs from `POST /tama/v1/tamads`).

2. Component: fetch tamads once on mount of `ProxyAdvancedFields`
   (`#[cfg(not(feature = "ssr"))]` block or a `LocalResource` — follow the
   existing `get_request` pattern in this file's siblings; reuse
   `crate::utils::{get_request, handle_response}`):

   ```rust
   let tamads = LocalResource::new(|| async move {
       let resp = get_request("/tama/v1/tamads").send().await.ok()?;
       // POLARITY: `handle_response` returns TRUE = 401 (bail), FALSE = OK.
       if handle_response(&resp) { return None; }
       resp.json::<Vec<TamadRef>>().await.ok()
   });
   ```

   (Gate SSR-safe like the rest of this file — if the file currently compiles
   under `ssr` without gating, wrap the fetch/`window` parts consistently
   with `crates/tama/src/pages/config_editor/mod.rs`.)

3. UI — inside the existing **"Tuning"** `<h3>` section (the `<h3>` is at
   ~line 14; the section holds "Circuit Breaker Threshold" etc.), insert a
   new `.form-group` immediately AFTER the "Download Queue Poll Interval
   (seconds)" form-group (field `pull_queue_poll_interval_secs`,
   form-group at ~lines 80–95):

   ```rust
   <div class="form-group">
       <label class="form-label">"Pull host"</label>
       <select class="form-input"
           prop:value=move || get_proxy().pull_backend.clone().unwrap_or_default()
           on:change=move |ev| {
               let v = target_value(&ev); // existing crate::utils::target_value
               config.update(|c| if let Some(c) = c {
                   c.proxy.pull_backend = if v.is_empty() { None } else { Some(v) };
               });
           }
       >
           {move || tamads.get().map(|t| t.take()).map(|list| pull_host_options(&list)).filter(|_| true).filter_map(|opts| opts.into_iter().map(|(value, label)| {
               view! { <option prop:value=value>{label}</option> }
           }).collect::<Vec<_>>())}
       </select>
       // hint shown only when zero tamads are registered:
       {move || if tamads.get().map_or(false, |t| t.map_or(true, |l| l.is_empty())) {
           view! {
               <p class="form-hint text-muted">
                   "No tamads registered — register one via `POST /tama/v1/tamads` (docs/api/tamads.md). Pulls fail until a pull host is set (ADR-0010)."
               </p>
           }.into_any()
       } else { view! { <span/> }.into_any() }}
   </div>
   ```

   (Adapt the exact `option` closure to Leptos 0.7 idiom — the simplest
   working shape is `move || { let list = ...; opts.iter().map(...).collect::<Vec<...>>() }`; keep it readable.)

   Save: no new save path — the Config editor's existing save button already
   POSTs the whole mirror config to `POST /tama/v1/config/structured`, and
   `save_structured_config` persists `pull_backend` (FK-validated in the DB
   layer). Set the select to "None" → `pull_backend = None` → cleared on save.

4. Tests in the file-local test module:
   - `test_pull_host_options_empty_list_has_none_only`
   - `test_pull_host_options_labels_name_and_short_id`
     (e.g. id `"3f2a9c1b-0000-0000-0000-000000000000"` → label
     `gpu-box · 3f2a…`)
   - `test_short_id_passthrough_under_8_chars`

**Steps:**
- [ ] Add the helper fns + tests to `advanced.rs`
- [ ] Run `cargo nextest run --package tama -- config_editor`
  - Failing (tests for not-yet-implemented fns, or module not compiling)? Implement until green.
- [ ] Add the tamad fetch + select UI (items 2–3)
- [ ] Run `cargo check --package tama` + `cargo check --package tama --features ssr`
- [ ] Verification in the dev server: Config → Proxy → Tuning → **Pull host** dropdown lists registered tamads by name + short id; selecting one and saving persists it (`GET /tama/v1/config/structured` shows `proxy.pull_backend = <tamad id>` and the DB row `app_proxy.pull_backend` is set); selecting **None** + save clears it; with zero tamads registered the hint text is visible; setting a value and re-running a model pull succeeds (the "no pull host configured" error no longer appears).
- [ ] Run the full gate
- [ ] Commit with message: `feat(config): pull host selector in Proxy tuning (ADR-0010)`

**Acceptance criteria:**
- [ ] The select offers exactly: "None" + one row per registered tamad (name · short id); no free-text entry.
- [ ] Save flow unchanged (same button, same endpoint); `pull_backend` round-trips through the config editor, including clearing to None.
- [ ] The FK trap is structurally impossible from this UI (only registered ids selectable).
- [ ] Helper tests pass; full gate green.

---

### Task 5: Fix stale `pull_backend` comments + wizard error link

**Context:**
FOUR comments describe behavior that ADR-0010 deliberately removed (silent
local pull fallback). All four must be fixed — note the first two say
"download**s** locally", which is why a naive grep for "download locally"
misses them:
1. `crates/tama-core/src/config/types/proxy.rs` on `ProxyConfig.pull_backend`
   (~line 127-131): *"…`None` (default) → the proxy downloads locally."
   — the code path (`crates/tama-core/src/proxy/tama_handlers/pull/start.rs`
   ~line 226) is now fail-loud: `None` ⇒ pull fails with
   `"no pull host configured: set proxy.pull_backend (the proxy itself never
   downloads — ADR-0010)"`.
2. `crates/tama/src/types/config/patch.rs` on `ConfigPatch.pull_backend`
   (~line 54-55): *"`Some(None)` = clear (local pulls)"* — should read
   "(no pull host set)".
3. `crates/tama/src/types/config/proxy.rs` on the **frontend mirror**
   `ProxyConfig.pull_backend` (~line 79-81): *"…`None` → the proxy
   downloads locally."* — same false claim, same fix.
4. `crates/tama-core/src/db/queries/app_config_queries.rs` on the
   `app_proxy` row struct field (~line 51-54): *"…NULL → proxy downloads
   locally."* — same false claim, same fix.

Additionally, the pull wizard surfaces that error with no pointer to where
the fix lives — the Config page (`/tama/config`, route in
`crates/tama/src/lib.rs` ~line 335). **Where it surfaces differs by
branch (verified end-to-end):**
- **GGUF branch:** the pull job IS created, and `pull/start.rs` ~line 226
  sets `job.error = Some("no pull host configured…")` → the wizard stays
  on the Downloading step (`PullStep`, `components/pull_step.rs`) where
  the per-job `"Failed: {}"` badge (from `job.error`, ~line 32) and the
  error-summary <ul> (~line 87) display it.
- **Transformers (repo pull) branch:** `start_repo_pull` fails with the
  same message (~line 368-375 in `crates/tama-core/src/proxy/state/
  repo_pull.rs`), BEFORE any job exists; `crates/tama/src/api/repo_pulls.rs`
  maps it to **502**; the wizard's `start_repo_pull_job`
  (`pull_quant_wizard.rs`) sets `error_msg` and bounces back to
  `WizardStep::RepoInput`. The message is therefore displayed by the
  **`error_msg` banner rendered in `components/repo_input.rs`** (~line 18,
  the `.alert--error` div) — NOT by `RepoPullStep`'s failed alert (a
  created repo-pull job can only fail later with different messages; the
  missing-host text is unreachable there).

**Files:**
- Modify: `crates/tama-core/src/config/types/proxy.rs` (comment only, item 1)
- Modify: `crates/tama/src/types/config/patch.rs` (comment only, item 2)
- Modify: `crates/tama/src/types/config/proxy.rs` (comment only, item 1b)
- Modify: `crates/tama-core/src/db/queries/app_config_queries.rs`
  (comment only, item 1c)
- Modify: `crates/tama/src/components/pull_wizard/mod.rs` (shared helper
  + tests)
- Modify: `crates/tama/src/components/pull_wizard/components/pull_step.rs`
  (GGUF-branch hint render)
- Modify: `crates/tama/src/components/pull_wizard/components/repo_input.rs`
  (Transformers-branch hint render — below the `error_msg` banner)
- Modify: `crates/tama/css/13-downloads.css` (`.pull-host-hint` rule —
  NOTE: there are NO existing `.pull-job-card` rules in any css file; just
  add the one rule)

**What to implement:**

1. Rewrite the doc comment on `pull_backend` in
   `crates/tama-core/src/config/types/proxy.rs` to match truth (keep the
   first two sentences about what the field is; replace the `None`
   sentence):

   ```rust
   /// Registered tamad connection id that executes queued model pulls
   /// (plan-191 Task 6). The download ALWAYS runs on that tamad — the file
   /// lands on the tamad's disk — and the proxy relays job events into its
   /// pull queue/SSE tracking. When `None` (default), pulls fail with the
   /// explicit "no pull host configured" error; there is no local-download
   /// fallback (removed with ADR-0010 — "the proxy spawns nothing").
   /// The value must be a registered tamad id (FK to `tamad_registry`).
   ```

1b. Rewrite the `None` sentence in the doc comment on the frontend mirror
   `pull_backend` in `crates/tama/src/types/config/proxy.rs` (~line 79-81)
   to the same truth (e.g. "`None` → pulls fail with the explicit
   'no pull host configured' error; no local-download fallback,
   ADR-0010").
1c. Rewrite the `NULL → proxy downloads locally.` sentence in the doc
   comment on the `app_proxy` row struct field in
   `crates/tama-core/src/db/queries/app_config_queries.rs` (~line 51-54):
   "`NULL` → no pull host set; pulls fail with the explicit 'no pull host
   configured' error (ADR-0010)".

2. Rewrite the doc comment on `pull_backend` in
   `crates/tama/src/types/config/patch.rs`:
   `/// Tamad pull host (plan-191 Task 6). `None` = unchanged;
   /// `Some(None)` = clear (no pull host set); `Some(Some(id))` = set.`

3. Shared helper in `crates/tama/src/components/pull_wizard/mod.rs`
   (`pub(crate)`). Import it into the step files as
   `use crate::components::pull_wizard::is_missing_pull_host;` — from
   inside `components/*` files, `super` is `pull_wizard::components`, NOT
   `pull_wizard`, so do not use a `super::` path.

   ```rust
   /// True for the stable "no pull host configured" error prefix emitted
   /// by the pull start paths (tama-core `pull/start.rs` and `state/
   /// repo_pull.rs`, ADR-0010 fail-loud). Case-sensitive: the prefix is
   /// exact.
   pub(crate) fn is_missing_pull_host(msg: &str) -> bool {
       msg.starts_with("no pull host configured")
   }
   ```

3a. In `pull_step.rs` (GGUF branch): in the per-job card render, when
   `job.status == "failed"`, `job.error` is `Some(msg)`, and
   `is_missing_pull_host(msg)` — render under the badge row (inside the
   same `.pull-job-card`):

   ```rust
   <A href="/tama/config" class="pull-host-hint">
       "Set a pull host in Config → Proxy → Tuning"
   </A>
   ```

   (`use leptos_router::components::A;` — same import style as
   `crates/tama/src/components/model_card.rs`. Do NOT add a `btn-link`
   class — no such rule exists; `.pull-host-hint` alone carries the look:
   add `.pull-host-hint { font-size: 0.85rem; display: inline-block;
   margin-top: 0.25rem; }` to `crates/tama/css/13-downloads.css` and pick
   its color from the `01-custom-properties.css` accent tokens.)

3b. In `repo_input.rs` (the RepoInput step — where the Transformers branch
   lands when repo-pull start 502s, per the Context): that file already
   renders the error banner (~line 18: `error_msg.get().map(…)` → the
   `.alert--error` div). Immediately after that banner, render

   ```rust
   <A href="/tama/config" class="pull-host-hint">
       "Set a pull host in Config → Proxy → Tuning"
   </A>
   ```

   wrapped so it shows only when `error_msg.get()` is `Some(msg)` and
   `is_missing_pull_host(&msg)` (mirror the banner's own reactive closure
   style). Do NOT target `repo_pull_step.rs`'s failed alert — the
   missing-host text never reaches that step (see Context).

   The canonical error string is NOT changed — it is the single source of
   truth referenced by tests and `docs/api/`; the `starts_with` prefix is
   the stable part.

4. Tests (in `pull_wizard/mod.rs` — file-local `#[cfg(test)] mod tests`,
   create the module if absent):
   - `test_is_missing_pull_host_exact` (the full real message matches)
   - `test_is_missing_pull_host_other_errors_no_match`
     (e.g. `"HTTP 502"`, `""`, `"No pull host configured"` (capital N —
     does NOT match, documents case-sensitivity))

**Steps:**
- [ ] Write the two `is_missing_pull_host` tests first in `pull_wizard/mod.rs`
- [ ] Run `cargo nextest run --package tama -- pull_wizard`
  - Failing (fn not defined)? Expected — then implement.
- [ ] Apply the FOUR comment fixes (items 1, 1b, 1c, 2) + the two hint
      renders (3a, 3b) + the CSS rule
- [ ] Run `cargo nextest run --package tama -- pull_wizard`
  - All pass?
- [ ] Run `rg -n "downloads? locally|local pulls?" crates/tama*/src --no-heading` and confirm the ONLY remaining permitted hits (7 lines) are the CORRECT negations in exactly these six files: `crates/tama/src/api/repo_pulls.rs`, `crates/tama-core/src/proxy/state/repo_pull.rs`, `crates/tama-core/src/proxy/tama_handlers/pull/start.rs`, `crates/tama-core/src/proxy/tama_handlers/pull/start_tamad.rs`, `crates/tama-core/src/proxy/tama_handlers/pull/tests/tamad_pull.rs`, `crates/tama-core/src/proxy/tama_handlers/pull/tests/orchestration.rs` (`orchestration.rs` has TWO — a comment and a load-bearing assertion message; verify each line is a negation before keeping it; do NOT reword them) — fix any other hit
- [ ] Run the full gate (the comments are in `tama-core` — the workspace
  clippy/nextest gate covers it)
- [ ] Verification in dev server with a deployment that has no `pull_backend` set: Pull Model → GGUF branch → select a quant and start → the failed job badge shows the error AND the "Set a pull host in Config →" link below it. Transformers branch → confirm repo and start → the wizard returns to the Repo step with the error banner AND the same link below the banner. The link navigates to the Config page with Proxy → Tuning visible.
- [ ] Commit with message: `fix(config): correct stale pull_backend comments; wizard links missing-host error to config`

**Acceptance criteria:**
- [ ] No comment in the repository still claims the proxy downloads locally when `pull_backend` is unset: `rg -n "downloads? locally|local pulls?" crates/tama*/src --no-heading` — the ONLY allowed remaining permitted hits are the correct negations in exactly the six files listed in Steps (7 lines total, two of them in `orchestration.rs`; verify each line is a negation before keeping it; anything else = stale, fix it).
- [ ] The link renders under the GGUF `PullStep` per-job failure badge and under the Transformers `RepoInput` error banner for the missing-host error; other errors do not.
- [ ] The canonical error message string is byte-identical to before.
- [ ] Tests pass; full gate green.

---

## Rollout / ordering notes

- T1 and T2 are independent but touch the same file
  (`models/mod.rs`); keep the established order (T1 first) so the toolbar
  render in T2 is written against the polling-aware list code and the two
  commits do not conflict with each other.
- T3 depends on T2 (delete dashboard dead code only after the models-page
  copies exist). T4 and T5 are independent of each other; T5's wizard link
  is most useful after T4 (the link target actually has the field), but T5
  is still independently commitable.
- No backend API or DB changes anywhere — zero schema/migration work.
- After all five commits: run the full CI-parity gate one final time, push
  the branch, and open the PR (follow the repo's PR conventions).
