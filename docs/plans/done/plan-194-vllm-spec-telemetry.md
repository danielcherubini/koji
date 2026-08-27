# vLLM Spec-Decode Telemetry + Consistent tok/s Formatting Plan

**Goal:** Make the dashboard's "Speculative Acceptance" slot show a real number for vLLM backends (collected by the host's tamad from the engine's Prometheus `/metrics`), and make all live tok/s / ms/tok number displays use one consistent digit rule.

**Architecture:** The tamad daemon scrapes each managed ready backend's `/metrics` endpoint every 10s, diffs the three cumulative `vllm:spec_decode_*_total` counters between scrapes (body-driven — only vLLM engines expose those names), and ships the computed acceptance rate on the existing 1 Hz gRPC process-row stream (`ProcessInfo`). The proxy's existing 2s metrics loop merges those rows into the per-server `inference_stats` map, and the per-response vLLM extractor is fixed to stop nulling that field (it currently replaces the whole entry with `spec_accept_pct: None` on every response, which would overwrite the merge — see Task 3). The rest of the pipeline (SSE `MetricCurrent`, dashboard cards) already exists — with one small behavior addition: the `MetricCurrent` aggregation gains the same 30s freshness gate for `spec_accept_pct` that `tps` already has, so a stale entry never shows a ghost rate. The frontend gains shared pure formatters and `tabular-nums`.

**Tech Stack:** Rust (tonic/prost gRPC, reqwest, Leptos/WASM + Trunk), no new external dependencies except enabling reqwest's existing `blocking` feature.

**Approved design (do NOT deviate without asking):**
- Collection owner is the **tamad** (host-owned telemetry, ADR-0012 extends ADR-0010). The proxy never scrapes backends.
- Backend detection is **body-driven** (scrape every ready+alive endpoint, emit spec data only when the body contains the three `vllm:spec_decode_*_total` counter names) — NOT a `provider_name == "vllm"` string gate, so renamed vLLM installations still work.
- Acceptance rate = `100 × Δaccepted_tokens ÷ Δdrafted_tokens` — vLLM's own "Avg Draft acceptance rate" definition.
- Display: acceptance **rate % only** (1 decimal), + existing `● spec decoding active` footer. `spec_accept_pct` in `MetricCurrent` is shown only when its entry is fresh — the SAME 30s freshness gate that already gates `tps`/`prompt_tps` (Task 3); the `spec_decoding_active` footer flag keeps its existing sticky-OR semantics (once true, stays true — pre-existing, not changed here).
- Number rule: one shared `format_auto(v)` body for BOTH tok/s and ms/tok values: `v < 1` → 2 decimals, no trim ("0.30"); `1 ≤ v < 100` → 1 decimal, trim rendered trailing ".0" ("72.6"); `v ≥ 100` → 0 decimals ("3347"). Peaks use the same rule. Labels: `ITL … ms/tok` / `TTF … ms/tok`.
- llama.cpp path (per-response `timings` → `draft_n_accepted/draft_n`) is NOT changed.
- `last_updated_ms` on `inference_stats` entries is NOT touched by the tamad merge (it gates tps staleness; forwarder writes keep it authoritative).

---

### Task 1: Wire the spec fields end-to-end (proto + rows)

**Context:**
The per-model live rows already travel from each tamad to the proxy at 1 Hz over gRPC (`StreamStats` → `SystemStats` → `ProcessInfo`), and the proxy aggregates them into `ModelRow` (`crate::proxy::state::rows`). Today neither carries spec-decode data, so there is no way for the proxy to see vLLM's acceptance rate without a new scrape. This task adds the two additive fields to the wire type and to the proxy's row type, touching nothing else. No behavior change in this task — fields default to `None`/`false` everywhere.

**Files:**
- Modify: `crates/tama-core/proto/tamad.proto` (message `ProcessInfo`, currently ends at field `max_restarts = 9;`)
- Modify: `crates/tama-core/src/proxy/state/rows.rs` (struct `ModelRow`, fn `row_from`, tests fixture `proc()` at ~line 200)
- Modify (compile-only fallout — `ProcessInfo` struct literals; full census, found via `grep -rn 'ProcessInfo {' crates/ --include='*.rs'`):
  - `crates/tamad/src/lifecycle.rs` — `to_process_info` (~line 94, **the only production site**) and a test at ~line 2291
  - `crates/tamad/src/stats.rs` — `test_tick_host_snapshot` fixture at ~line 208
  - `crates/tama-core/src/tamad/mod.rs` — tests at ~line 48 and ~line 132
  - `crates/tama-core/src/proxy/state/rows.rs` — test helper `proc()` at ~line 200
  - `crates/tama-core/src/proxy/status.rs` — `seed_live_row` at ~line 362
  - `crates/tama-core/src/proxy/forward/tests/request.rs:45`
  - `crates/tama-core/src/proxy/handlers/get_model_tests.rs:24`
  - `crates/tama-core/src/proxy/handlers/tests.rs:20`
  - `crates/tama-core/src/proxy/lifecycle/tests.rs:11`
  - `crates/tama-core/src/proxy/mod.rs:334`
  - `crates/tama-core/src/proxy/server/tests.rs:9`
  - `crates/tama-core/src/proxy/tama_handlers/backend_logs.rs:353`
  - `crates/tama-core/src/proxy/tama_handlers/models/tests/cancel.rs:17`
  - `crates/tama-core/src/proxy/tama_handlers/models/tests/model_handlers.rs:19`
  - `crates/tama-core/src/proxy/tama_handlers/system_tests.rs:26`
  - `crates/tama/src/admin.rs:433` (bin crate `tama`'s test module)
  The plan ships a static census instead of a compile probe because plain `cargo check` does NOT compile `#[cfg(test)]` code — most sites only surface when test targets build. Re-run the grep after fixing to confirm zero missing. (`crates/tama-core/src/tamad/pool.rs` contains NO `ProcessInfo` literals — `stats_full` only takes a `Vec<ProcessInfo>` param; do not go looking there.)

**What to implement:**
1. In `tamad.proto`, `message ProcessInfo` (proto3), add after `max_restarts = 9;`:
   ```proto
   // Spec-decode observation scraped by the tamad from the backend's
   // Prometheus /metrics (vLLM-only; None = no spec traffic observed
   // recently or engine is not vLLM). See ADR-0012.
   optional double spec_accept_pct = 10;
   bool spec_decoding_active = 11;
   ```
2. Regenerated gpb is built by `crates/tama-core/build.rs` (`tonic_build` over `proto/tamad.proto`) — no committed generated code. `optional double` → prost `Option<f64>`, `bool` → `bool`.
3. Every `ProcessInfo` literal now misses two fields; add `spec_accept_pct: None, spec_decoding_active: false` at each census site. In `to_process_info` set the same defaults (Task 2 will populate real values).
4. In `crates/tama-core/src/proxy/state/rows.rs`:
   - `ModelRow` gains `pub spec_accept_pct: Option<f32>` and `pub spec_decoding_active: bool`.
   - `row_from(p: &ProcessInfo, last_ms: i64)` maps them: `spec_accept_pct: p.spec_accept_pct.map(|v| v as f32)`, `spec_decoding_active: p.spec_decoding_active`.
   - Update the tests' `proc()` helper with the two new params (or defaults + explicit assertions).

**Steps:**
- [ ] Add the two proto fields; in `rows.rs` extend `ModelRow` + `row_from` + a test asserting `ProcessInfo { spec_accept_pct: Some(44.5), spec_decoding_active: true, … }` → `ModelRow` carries both, and a default `ProcessInfo` → `ModelRow { spec_accept_pct: None, … }`.
- [ ] Fix every census site above (add the two defaults).
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
  - Does `Missing fields ...` error remain anywhere? (This gate compiles test code, unlike plain `cargo check`.) Fix leftovers; re-run until zero errors.
- [ ] Run `grep -rn 'ProcessInfo {' crates/ --include='*.rs'` and eyeball: every site has both new fields.
- [ ] Run `cargo nextest run --package tama-core`
  - All green? (This also compiles every tama-core test target, catching any missed site.)
- [ ] Run `cargo nextest run --package tamad` — green here too (its test fixtures were touched).
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: carry tamad spec-decode observations on the wire (ProcessInfo, ModelRow)"

**Acceptance criteria:**
- [ ] `ProcessInfo` (gpb) and `ModelRow` both expose `spec_accept_pct`/`spec_decoding_active` with `None`/`false` defaults at all 18 construction sites.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` and both crates' test suites are green.

---

### Task 2: Tamad `/metrics` spec-decode scraper

**Context:**
vLLM logs a spec-decoding summary internally every 10s, and its Prometheus endpoint exposes cumulative counters `vllm:spec_decode_num_drafts_total`, `vllm:spec_decode_num_draft_tokens_total`, `vllm:spec_decode_num_accepted_tokens_total` (each tagged with `model_name`/`engine` labels — sum across label sets). The user's production log (window counters "Accepted: 165, Drafted: 371" ⇒ "Avg Draft acceptance rate: 44.5%") confirms the interpretation `acceptance = accepted ÷ drafted`. The tamad is the only component allowed to touch backend ports (ADR-0012). Detection of a vLLM engine is **body-driven**: every `ready && alive` endpoint is scraped (10s throttle each) and spec data is emitted only when the body actually contains those counter names — so renamed vLLM installations work, and llama.cpp (whose `/metrics` uses different names) is simply no-op'd. The natural home is `StatsCollector::tick` — already once per second, called from `spawn_blocking` (blocking HTTP is legal there), receives the full process list, and returns the `SystemStats` that `stream_stats` yields.

**Files:**
- Create: `crates/tamad/src/vllm_metrics.rs` (pure parsing/diffing/URL logic + unit tests)
- Modify: the crate root module declaration file (add `mod vllm_metrics;` next to `mod stats;` — find where `mod stats;` is declared)
- Modify: `crates/tamad/src/stats.rs` (`StatsCollector` struct + `tick`; refresh the `ProcessInfo` fixture in `test_tick_host_snapshot` if Task 1 didn't already)
- Modify: `Cargo.toml` (root, line 32 — add `"blocking"` to the existing workspace `reqwest` features: `reqwest = { version = "0.12", features = ["stream", "native-tls", "json", "blocking"] }`)
- Test: `#[cfg(test)]` modules in `vllm_metrics.rs` and the existing tests module in `stats.rs` (uses the already-present dev-dep `wiremock = "0.6"`)

**What to implement:**

1. `crates/tamad/src/vllm_metrics.rs` — pure, dependency-free (only `std::time` + `url`):
   ```rust
   pub const SCRAPE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
   pub const STALE_MS: i64 = 60_000; // an observation older than 1 min reads as inactive
   pub const PER_SCRAPE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
   pub const TICK_SCRAPE_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

   #[derive(Debug, Clone, Copy, Default, PartialEq)]
   pub struct SpecCounters {
       pub drafts: f64,          // vllm:spec_decode_num_drafts_total
       pub draft_tokens: f64,    // vllm:spec_decode_num_draft_tokens_total
       pub accepted_tokens: f64, // vllm:spec_decode_num_accepted_tokens_total
   }

   // Prometheus text exposition. Skips blank lines, `#` comments, and any
   // line whose trailing value token does not parse as f64 (keep the rest
   // of the body). Sums the value across ALL label sets for each exact
   // counter name (names are matched exactly — a name with a longer
   // suffix must not match). Returns None only when none of the three
   // counter names is present at all.
   pub fn parse_spec_metrics(body: &str) -> Option<SpecCounters>

   // Diffs two successive cumulative counter sets:
   //   prev None          -> None (first scrape has no delta)
   //   any cur < prev     -> None (engine restart / counter reset — never emit)
   //   Δdraft_tokens > 0  -> Some((100.0 * Δaccepted / Δdraft_tokens, true))
   //   otherwise          -> None (no spec traffic in this window)
   pub fn observe(prev: Option<SpecCounters>, cur: SpecCounters) -> Option<(f64, bool)>

   // "http://127.0.0.1:8000/v1" -> Some("http://127.0.0.1:8000/metrics")
   // "http://host:9000" (no path) works as-is; https preserved;
   // non-http(s) schemes -> None.
   pub fn metrics_url_for(endpoint_url: &str) -> Option<String>
   ```
2. `StatsCollector` (`crates/tamad/src/stats.rs`):
   - New fields:
     ```rust
     pub struct StatsCollector {
         state: Arc<TamadState>,
         sys: sysinfo::System,
         disks: sysinfo::Disks,
         /// Scrape state per model_name (all ready+alive backends, any engine).
         spec: HashMap<String, SpecState>,
         /// Overridable in tests (0 = scrape every tick).
         scrape_interval: std::time::Duration,
         http: reqwest::blocking::Client,
     }
     struct SpecState {
         prev: Option<vllm_metrics::SpecCounters>,
         is_vllm: bool,              // observed the three counter names
         last_scrape: Option<std::time::Instant>,
         last_rate_pct: Option<f64>,
         last_active: bool,
         last_obs_ms: i64,
     }
     ```
   - `new()`: `reqwest::blocking::Client::builder().timeout(vllm_metrics::PER_SCRAPE_TIMEOUT).build()` (build shouldn't fail; on the off chance it does, `expect` — a misconfiguration), `scrape_interval: vllm_metrics::SCRAPE_INTERVAL`. Add `pub(crate) fn with_scrape_interval(self, d: Duration) -> Self` for tests.
   - In `tick()`, the `processes: Vec<ProcessInfo>` param is mutated in place before building the `SystemStats`. Track a cumulative `scrape_elapsed: Duration`. For each mutable `p` where `p.status == "ready" && p.alive` (engine-agnostic — body determines vLLM-ness):
     1. **Throttle**: skip fetch when `scrape_interval > 0` and last scrape was within `scrape_interval`; **budget**: also skip when `scrape_elapsed >= TICK_SCRAPE_BUDGET` (skipped models retry next tick — the tick must never linger, or the proxy's 5s `LIVE_FRAME_MAX_AGE` freshness gate blanks every model on the host).
     2. `let Some(url) = vllm_metrics::metrics_url_for(&p.endpoint_url) else { continue };`
     3. Fetch `self.http.get(&url).send().and_then(|r| { let ok = r.status().is_success(); r.text().map(move |t| (ok, t)) })` — ANY failure (send error, non-2xx, text error): `tracing::debug!("{} spec scrape failed: {e}", p.model_name)` (never warn — down engines would spam the log), advance `last_scrape`, keep the last observation, `continue`.
     4. `match parse_spec_metrics(&text)`:
        - `None` (no vllm counters — non-vLLM engine): set `is_vllm = false`, leave `p` at its `to_process_info` defaults, advance `last_scrape`.
        - `Some(cur)`: `state.is_vllm = true`; `if let Some((pct, _)) = observe(state.prev, cur) { state.last_rate_pct = Some(pct); state.last_active = true; state.last_obs_ms = now_ms; }` then `state.prev = Some(cur)`, `last_scrape = Some(now)`.
     5. After the loop: emit to each tracked process — `let fresh = now_ms - s.last_obs_ms <= STALE_MS;` `p.spec_accept_pct = if s.is_vllm && fresh { s.last_rate_pct } else { None };` `p.spec_decoding_active = s.is_vllm && fresh && s.last_active;`
     6. Evict state for models no longer in this tick's process list (`retain` on the set of current model names) — a restarted engine gets a fresh `prev` (its counters were reset).
   - All HTTP stays inside `tick` (which `stream_stats` already runs via `spawn_blocking` + `collector.blocking_lock()`). Do NOT make `tick` async, do NOT use the async reqwest client.

**Steps:**
- [ ] Create `vllm_metrics.rs` writing the failing unit tests first:
  - `parse_spec_metrics`: (a) fixture with 3 counters × 2 label lines each — values sum correctly; (b) realistic `/metrics` body with `# HELP`/`# TYPE` lines, unrelated metrics, labels containing `=` and spaces; (c) none of the three names → `None`; (d) a line with an unparseable value → that line skipped, the rest parsed; (e) exact-name guard: a body containing only a hypothetical `vllm:spec_decode_num_drafts_total_extra` → `None`.
  - `observe`: (a) real-log vector `prev {0,0,0}` → `cur {115, 371, 165}` ⇒ `Some((≈44.474, true))`; (b) `prev {100,300,133}` → `cur {140,450,200}` ⇒ `Some((100×67/150 ≈ 44.667, true))`; (c) any counter lower than prev ⇒ `None`; (d) `prev None` ⇒ `None`; (e) Δdraft_tokens == 0 ⇒ `None`.
  - `metrics_url_for`: `http://127.0.0.1:8000/v1` → `http://127.0.0.1:8000/metrics`; `http://host:9000` → `http://host:9000/metrics`; `https://inhost:8000/v1` → `https://inhost:8000/metrics`; `grpc://x` → `None`.
- [ ] Run `cargo nextest run --package tamad -- vllm_metrics`
  - Failed before implementing? Implement, then re-run until green.
- [ ] Wire the collector as specified; add `StatsCollector` integration tests in `stats.rs` tests using `wiremock` (dev-dep already present):
  - Positive: mock serves a fixed `/metrics` body (the three counters, one label set each) on a 127.0.0.1 ephemeral port; collector built with `with_scrape_interval(Duration::from_millis(1))`; tick 1 with process `{ status: "ready", alive: true, endpoint_url: mock.url() }` + `provider_name` of ANY value (detection is body-driven — use "vllm" for realism) ⇒ `spec_accept_pct: None` (first scrape seeds only); change the mock body to incremented counters `{115, 371, 165}`; tick 2 ⇒ `spec_accept_pct` in 44.4–44.55 and `spec_decoding_active: true`.
  - Negative (non-vLLM body): mock serves llama.cpp-style metrics with no vllm counters ⇒ two ticks ⇒ `spec_accept_pct: None`, `spec_decoding_active: false`, no panic.
  - Dead endpoint: bind a `TcpListener` on 127.0.0.1 to get a port, drop it, tick with that endpoint ⇒ completes, defaults left intact, no panic.
- [ ] Run `cargo nextest run --package tamad` — all green.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: tamad scrapes /metrics for spec-decode acceptance rate"

**Acceptance criteria:**
- [ ] A tamad with a ready vLLM model reports `spec_accept_pct`/`spec_decoding_active` on that model's `ProcessInfo` within ~20s of spec traffic (first scrape seeds, second yields the rate); renamed vLLM installations behave identically.
- [ ] Non-vLLM engines, non-ready statuses, down engines, and missing metrics paths (404) all leave defaults without errors, panics, or warn-level log spam; a degraded multi-engine host never blocks a tick past ~5s total scrape work.
- [ ] `cargo nextest run --package tamad` green.

---

### Task 3: Proxy merge + stop the forwarder nulling the field

**Context:**
The proxy's 2s metrics loop (`crates/tama-core/src/proxy/server/metrics.rs`) already: (a) snapshots the per-server `inference_stats` map and aggregates the "latest server" into `MetricCurrent` (including `spec_accept_pct` and the OR'd `spec_decoding_active`), (b) builds the DB row + 30s buckets, (c) broadcasts the SSE `MetricsSnapshot`. Today the map is written only by the forwarder from backend responses. **Two changes are required for the feature to work at all:** (1) the loop must fold the tamad-reported spec values into the map — *before* the step-2 snapshot that feeds this tick's `MetricCurrent`, or the value only surfaces next tick and the ≤2s guarantee is void; and (2) `extract_vllm_stats` (`crates/tama-core/src/proxy/forward/stats.rs`, called on EVERY vLLM response from `forward/request.rs:355` and `forward/sse.rs:25`) currently **replaces the whole entry with `spec_accept_pct: None`** ("vLLM doesn't expose spec decoding stats") — under real load that nulls the merge on every response, so the slot would read "—" almost all the time. It must carry the previous entry's value across (same sticky pattern it already uses for `spec_decoding_active`).

**Files:**
- Modify: `crates/tama-core/src/proxy/server/metrics.rs` (reorder: move the `let live = crate::proxy::live_rows(...).await;` line — currently in step 3, used for `models_loaded` — to BEFORE the step-2 `let inference_map = metrics_state.metrics.inference_stats_snapshot();` line; add `merge_tamad_spec_stats(&metrics_state, &live).await;` right after it; `live` still feeds `models_loaded` further down; loop variable is `metrics_state`, type `ProxyState`)
- Modify: `crates/tama-core/src/proxy/forward/stats.rs` (`extract_vllm_stats` only — DO NOT touch `extract_llama_cpp_stats`)
- Test: `#[cfg(test)] mod` in `server/metrics.rs`; the existing forwarder test file `crates/tama-core/src/proxy/forward/tests/extract_stats.rs`
- Note for test setup: `make_model_config` / `seed_live_row` live PRIVATE in `crates/tama-core/src/proxy/status.rs`'s tests — they cannot be imported from `server/metrics.rs` tests. Duplicate the minimal helpers there (Pattern: `ProxyState::new(config, None, crate::db::pool::test_dummy_pool())`, insert a model config into `registry.model_configs.write().await`, build a `Rows` via `crate::proxy::state::rows::row_from` or the tamad `test_support::{stats_full, handle_with_latest, insert_raw_handle}` helpers — all accessible in-crate under `test-stubs`). Read the `resolve_backends_for_model` call site in `crates/tama-core/src/proxy/status.rs` (~line 135, guarded by `config.read()` + `model_configs.read()`) for the exact receiver/argument order: `&Config, &HashMap<String, ModelConfig>, &model_key -> Vec<(String, &ModelConfig, &BackendConfig)>` where the **first tuple element is the `inference_stats` key**. Do NOT look for the pattern in `handlers/status.rs` — it doesn't call `resolve_backends_for_model`. Use the public `crate::proxy::Rows` path in signatures.

**What to implement:**

1. `merge_tamad_spec_stats` (new `pub(crate) async fn` at module scope in `crates/tama-core/src/proxy/server/metrics.rs`):
   ```rust
   pub(crate) async fn merge_tamad_spec_stats(
       state: &crate::proxy::ProxyState,
       live: &crate::proxy::Rows,
   ) {
       let cfg = state.config.read().await;
       let model_configs = state.registry.model_configs.read().await;
       for row in live.all() {
           if row.spec_accept_pct.is_none() && !row.spec_decoding_active {
               continue; // stale row default: never clear freshly-merged values
           }
           let servers = cfg.resolve_backends_for_model(&model_configs, &row.key);
           for (server_name, _, _) in &servers {
               let sn = server_name.clone();
               let pct = row.spec_accept_pct;
               let active = row.spec_decoding_active;
               state.metrics.modify_inference_stats(|m| {
                   let entry = m.entry(sn.clone()).or_default();
                   if let Some(p) = pct { entry.spec_accept_pct = Some(p); }
                   entry.spec_decoding_active = entry.spec_decoding_active || active;
               });
           }
       }
   }
   ```
   **Do NOT touch** `tps`, `prompt_tps`, `cache_hit_pct`, or `last_updated_ms` on any entry. Do NOT remove entries. (Or-merge semantics: a stale/tamdown row with defaults skips entirely, so a previously merged value survives a tamad blip until a fresh `Some(p)` overwrites it.)
2. `extract_vllm_stats` in `forward/stats.rs`: mirror the existing `prev_active` read:
   ```rust
   let prev = metrics_state.inference_stats.borrow().get(backend_name);
   let prev_active = prev.map(|s| s.spec_decoding_active).unwrap_or(false);
   let prev_spec_pct = prev.and_then(|s| s.spec_accept_pct);
   let stats = LatestInferenceStats {
       ...,
       spec_accept_pct: prev_spec_pct, // tamad-merged value (ADR-0012) — preserve across per-response replacement
       spec_decoding_active: prev_active || /* vLLM responses never set this true */,
       ...
   };
   ```
   (Read the current function first — it likely reads `prev_active` inline; restructure minimally to also capture `prev_spec_pct` and remove the old `spec_accept_pct: None` line + its comment.)
3. **Display freshness gate** — `spec_accept_pct` in `MetricCurrent` is currently read with NO freshness check (unlike `tps`/`prompt_tps`, which the step-2 code already gates via `stale_threshold_ms` on `last_updated_ms`), so a merged rate could linger forever beside a "—" tok/s (e.g. after the tamad goes stale, an engine dies, or a model unloads — the merge's skip-on-defaults deliberately never clears). Fix in `server/metrics.rs`: extract the step-2 aggregation into a small **pure, unit-testable helper** at module scope:
   ```rust
   /// Live-value aggregation for the broadcast snapshot. tps/prompt_tps AND
   /// spec_accept_pct are None when the newest entry is older than the 30s
   /// bucket window; cache_hit_pct and the OR'd spec_decoding_active flag
   /// keep their existing (ungated / sticky) semantics — do not change them.
   pub(crate) fn aggregate_inference(
       inference_map: &std::collections::HashMap<String, crate::proxy::LatestInferenceStats>,
       now_ms: i64,
       stale_threshold_ms: i64,
   ) -> (Option<f32>, Option<f32>, Option<f32>, Option<f32>, bool, Option<i64>)
   // (tps, prompt_tps, cache_hit_pct, spec_accept_pct, spec_decoding_active, inference_last_updated_ms)
   ```
   and replace the inline step-2 computation with a call to it, keeping the visible loop behavior identical for tps/prompt_tps/cache_hit/flag (this must be a behavior-preserving refactor for those four).

**Steps:**
- [ ] Write the failing forwarder test FIRST in `forward/tests/extract_stats.rs`: pre-seed `metrics_state` with an entry for the backend (`spec_accept_pct: Some(44.5)`, `spec_decoding_active: true`), run a vLLM response through `extract_inference_stats` (reuse the fixture style of the neighboring `test_extract_inference_stats_vllm_full_metrics`), assert the stored entry still has `spec_accept_pct: Some(44.5)` (and `tps` updated by the response). Fresh state still yields `None` (existing test stays green).
- [ ] Write the failing merge tests in `server/metrics.rs` tests (duplicate the minimal `ProxyState` fixture described above; `Rows` built from a `ProcessInfo` with spec values via `row_from`):
  1. Existing `inference_stats` entry for the model's server with `tps: Some(50.0)`, `last_updated_ms: 123` → after merge: `spec_accept_pct` set, `spec_decoding_active: true`, **`tps` still `Some(50.0)` and `last_updated_ms` still 123**.
  2. No existing entry → entry created via `or_default()`; assert `last_updated_ms == 0`, `tps: None`.
  3. Row with defaults (`None`/`false`) → map **stays empty** (skip branch, no entry created).
  4. Model resolving to two servers → both entries updated.
  5. `aggregate_inference` unit tests: a map whose newest entry is fresh (`last_updated_ms` within 30s) with `spec_accept_pct: Some(44.5)` ⇒ fourth tuple element `Some(44.5)`; the same map with `last_updated_ms` 60s ago ⇒ `None` (tps gated identically — existing behavior, pin it too); `cache_hit_pct` and `spec_decoding_active` are NOT gated by freshness (kin to the existing sticky semantics).
- [ ] Run `cargo nextest run --package tama-core -- forward tests server::metrics`
  - Failed before implementation? Implement `extract_vllm_stats` change + `merge_tamad_spec_stats` + the reordered call site, re-run until green.
- [ ] Run `cargo nextest run --package tama-core` — all green (the reorder must not break the existing bucket/persist tests).
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: proxy merges tamad spec observations; vLLM extractor preserves spec field"

**Acceptance criteria:**
- [ ] With a tamad reporting spec data, `MetricCurrent.spec_accept_pct`/`spec_decoding_active` reflect it in the **same** 2s loop iteration as the rows (merge precedes the step-2 snapshot) — no 1-tick lag.
- [ ] Continuous vLLM traffic no longer nulls the field: after a merge, a forced vLLM response leaves `spec_accept_pct` populated (new forwarder test proves it).
- [ ] `tps`/`prompt_tps` behavior byte-identical to before (all existing `server/metrics.rs`, `status.rs`, and forwarder tests still pass).

---

### Task 4: Frontend formatters, cards, and tabular-nums

**Context:**
The dashboard Telemetry cards format every number ad hoc: `format!("{t:.1} tok/s")` renders "72.6 tok/s" and "3347.2 tok/s" (dirty once the value crosses 4 digits), "ITL" (uppercase) vs "prefill" (lowercase) label styles are inconsistent, and at high prompt rates the sub-line's `{ms:.1}` masks small values — e.g. 33,472 prompt tok/s ⇒ 1000/33472 = 0.03 ms/tok renders as "0.0" instead of "0.03". The spec slot markup is already live-correct (the `spec-status--active` footer flips when the flag is set; it just has no data yet) and only needs the precision fix. This task applies the approved digit rule through pure, tested formatters everywhere a live tok/s or ms/tok number renders: the two Telemetry cards, the per-model-row tps badge, the dashboard cluster subtitle, and the sparkline bar-chart tooltips.

**Files:**
- Modify: `crates/tama/src/pages/dashboard/metrics.rs` (new formatters next to `ms_per_token` at ~line 270 + tests; `format_cluster_subtitle` at ~line 359 — currently renders `{t:.0} tok/s` in the dashboard title line)
- Modify: `crates/tama/src/pages/dashboard/mod.rs` (card values + sub-lines ~lines 412–427; spec slot `{format!("{p:.0}%")}` at ~line 666)
- Modify: `crates/tama/src/components/active_model_row.rs` (`{format!("{t:.0} tok/s")}` tps badge)
- Modify: `crates/tama/src/components/bar_chart.rs` (bar value/tooltip rendering ~lines 331–333 — currently always `{:.1}`; read the site first, it appends the chart's `unit_label`, so it wants the **unitless** `format_auto`, not `format_tok_s`)
- Modify: `crates/tama/src/dashboard/tests.rs` — if a different path, use `crates/tama/src/pages/dashboard/tests.rs`; the two cluster-subtitle assertions at ~lines 994/1004 ("… · 53 tok/s") must be updated to the formatter's output
- Modify: `crates/tama/css/04-cards-grid-tables.css` (`.card-value` / `.card-value-empty` / `.card-secondary` — they live HERE, not in 15-dashboard.css)
- Modify: `crates/tama/css/15-dashboard.css` (`.active-model-tps` at ~line 221)
- NEVER edit `crates/tama/dist/` — Trunk regenerates it from `css/` during `trunk build`.

**What to implement:**

1. `metrics.rs` — one core digit rule used by BOTH the tok/s and ms/tok displays, plus wrappers, next to `ms_per_token`:
   ```rust
   /// The single digit rule for live throughput/latency numbers:
   /// `v < 1` → 2 decimals, NO trailing trim ("0.30");
   /// `1 <= v < 100` → 1 decimal, trim a rendered trailing ".0" ("72.6"; "100.0" → "100");
   /// `v >= 100` → 0 decimals ("3347").
   /// Pure number — no unit suffix (callers append " tok/s" / " ms/tok").
   pub fn format_auto(v: f64) -> String
   /// "72.6 tok/s" / "3347 tok/s"
   pub fn format_tok_s(v: f64) -> String { format!("{} tok/s", format_auto(v)) }
   /// Same body as format_auto with " ms/tok": "13.8 ms/tok", "0.30 ms/tok", "25 ms/tok", "1 ms/tok" (trim)
   pub fn format_ms_per_token(v: f64) -> String { format!("{} ms/tok", format_auto(v)) }
   /// "44.5%" — one decimal, for the spec-decode acceptance rate.
   pub fn format_pct(v: f64) -> String { format!("{v:.1}%") }
   ```
   The trim in the 1-dec branch is what pins `99.96 → "100"` (Rust `{:.1}` would render `"100.0"`) while the 2-dec branch keeps its zero ("0.30", not "0.3"). Both units share the body, so [10, 100) ms/tok values get 1 decimal with trim ("13.9 ms/tok") — matching the approved "ITL 13.8 ms/tok" style.
2. `dashboard/mod.rs` (~412–427), replacing the four ad-hoc `format!` sites:
   - `tg_value`: `Some(t) => format_tok_s(t as f64)`
   - `tg_secondary`: `format!("ITL {} ms/tok · peak {}", format_ms_per_token(ms), format_tok_s(telemetry.tg_peak))`
   - `pp_value`: `Some(t) => format_tok_s(t as f64)`
   - `pp_secondary`: `format!("TTF {} ms/tok · peak {}", format_ms_per_token(ms), format_tok_s(telemetry.pp_peak))`
3. Spec slot (~line 660–672): `format!("{p:.0}%")` → `format_pct(p as f64)`. Leave the `None => "—"` branch and the footer (`spec-status--active` / inactive) untouched — already correct.
4. `active_model_row.rs` badge: `format!("{t:.0} tok/s")` → `format_tok_s(t as f64)` (import from `crate::pages::dashboard` — the module re-exports `metrics::*`).
5. `format_cluster_subtitle` (~metrics.rs:359): the trailing `{t:.0} tok/s` becomes `format_tok_s(t)`; update the two subtitle test assertions to the formatter outputs.
6. `bar_chart.rs` tooltip/value (~331–333): the `{:.1}` → `format_auto(value)` — do NOT duplicate the " tok/s" unit text if the chart already renders `unit_label` next to it.
7. CSS: add `font-variant-numeric: tabular-nums;` to `.card-value`, `.card-value-empty`, `.card-secondary` in `04-cards-grid-tables.css`, and to `.active-model-tps` in `15-dashboard.css` (all live-updating number displays in the telemetry/model sections).

**Steps:**
- [ ] Write failing formatter tests first in `metrics.rs`: `72.6 → "72.6 tok/s"`, `99.96 → "100 tok/s"` (trim), `100.0 → "100 tok/s"`, `3347.2 → "3347 tok/s"`, `format_auto(0.2987) → "0.30"` (no trim), `format_ms_per_token(1.0) → "1 ms/tok"` (trim), `format_ms_per_token(25.0) → "25 ms/tok"`, `format_pct(44.474) → "44.5%"`.
- [ ] Run `cargo nextest run --package tama -- pages::dashboard`
  - Failed? Implement, re-run green.
- [ ] Apply the markup replacements in `mod.rs`, the badge in `active_model_row.rs`, the subtitle in `metrics.rs`, the tooltip in `bar_chart.rs`, the CSS additions in both css files, and the subtitle test updates.
- [ ] Run `cargo nextest run --package tama -- pages::dashboard components::active_model_row`
- [ ] Host compile check: `cargo build --package tama` (this is the HOST build — the real SSR gate is Task 5's `clippy --features ssr --all-targets`; the Trunk frontend build runs separately in CI's frontend job — do NOT attempt `trunk build` or commit anything under `dist/`).
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "ui: consistent tok/s formatting, TTF label, live spec-decode acceptance"

**Acceptance criteria:**
- [ ] Under load the cards read e.g. "72.6 tok/s · ITL 13.8 ms/tok · peak 80 tok/s" and "3347 tok/s · TTF 0.30 ms/tok · peak 4682 tok/s" — one digit rule everywhere (cards, subtitle, row badges, chart tooltips), no "0.0" false-zero, no "3347.2".
- [ ] Spec slot shows the rate with 1 decimal; footer flips to "● spec decoding active" while traffic flows; "—"/"○ spec decoding inactive" when not.
- [ ] Number widths do not jitter while updating (tabular-nums on all four CSS targets).

---

### Task 5: ADR-0012 + full gate

**Context:**
The "tamad scrapes its own backends, the proxy never does" choice was a genuine trade-off (proxy polling the provider URL, or reading the per-request `metrics.speculative_decoding` JSON, were both viable) and is exactly the ownership decision a future reader will second-guess. `CONTEXT.md` already carries the resolved **Speculative acceptance rate** glossary term (it references ADR-0012). ADR-0011 is taken (tamad-lifecycle-authority) — 0012 is the next free number.

**What NOT to do in this task:** creating `docs/specs/` entries, archiving the plan, or updating `docs/plans/README.md` / Quick Stats — the repo's convention (see plan-193) ties those to MERGE time with a PR reference this branch doesn't have yet. Do all of that at merge, not here.

**Files:**
- Create: `docs/adr/0012-host-owned-telemetry-scraping.md`

**What to implement:**
Write the ADR in the house style of `docs/adr/0011-tamad-lifecycle-authority.md` (prose title, context, decision, "Considered Options"). Suggested content (adapt wording, add nothing new):

> **Title:** `# Host-owned telemetry scraping: the tamad polls its own backends, the proxy never does`
>
> Context: ADR-0010 fixed the *lifecycle* boundary (proxy spawns nothing) but left telemetry ambiguous. vLLM's spec-decode stats exist only on the engine's Prometheus `/metrics` endpoint — the per-response JSON the proxy already forwards carries nothing about spec decoding, so the dashboard showed "spec decoding inactive" while the engine was spec-decoding (the tamad log prints "Avg Draft acceptance rate: ~45%" every 10s). Three owners were possible for the scrape: the proxy (it holds provider URLs), the per-request forwarder (the payload `metrics.speculative_decoding` is experimental, n==1-only, and silent between requests), or the host's tamad (it owns the backend lifecycle and the port).
>
> Decision: **the tamad scrapes**. Every managed ready backend's `/metrics` is polled at 10s by the host daemon; cumulative `vllm:spec_decode_*_total` counters are diffed between scrapes (reset-tolerant) and the acceptance rate — vLLM's own "Avg Draft acceptance rate" definition, accepted ÷ drafted — rides the existing 1 Hz process-row stream to the proxy, which merges it into per-server inference stats. No new protocol surface: two additive fields on the row already traveling each second. Detection is body-driven (the counter names in the response), so renamed vLLM installations still work and non-vLLM engines are a cheap no-op.
>
> Trade-off accepted: scrape work runs inside the tamad's 1s stats tick; it is bounded (2s timeout per engine, 3s total budget per tick) so a cluster of stalled engines can't delay the frame past the proxy's 5s freshness gate.
>
> Considered Options:
> - *Proxy polls provider `/metrics`* — rejected: the proxy would start *reading* backend internals, the direction ADR-0010/0011 pushed the opposite way; on multi-host deployments it would poll across machines for data the host already owns.
> - *Per-request `metrics.speculative_decoding` JSON* — rejected: explicitly experimental in vLLM, only populated for n==1, and silent between requests — the card would flicker back to "—/inactive" across any idle gap, which is exactly the bug we shipped.
> - *Parse the engine's log lines* — rejected: regex on a human-oriented log, competing with the logs tailer for the same stream.

**Steps:**
- [ ] Write the ADR file.
- [ ] Run the full CI gate in this order (matches `.github/workflows/ci.yml` — do NOT skip the SSR clippy):
  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo clippy --package tama --features ssr --all-targets -- -D warnings
  cargo nextest run --workspace
  ```
  - If `tamad_boot_replay` e2e tests fail saying the tamad shim is stale, run `cargo build -p tamad` first and re-run (known behavior — those e2e refuse stale shims and don't rebuild them).
  - All four commands exit 0?
- [ ] Commit with message: "docs: ADR-0012 host-owned telemetry scraping"

**Acceptance criteria:**
- [ ] `docs/adr/0012-host-owned-telemetry-scraping.md` exists, matches house style, and names the chosen option + rejected alternatives.
- [ ] Full CI gate green in this worktree.
- [ ] (Out of scope, user-driven) After deploying to the inference host (`update-tama`), the Speculative Acceptance slot shows a live percentage while vLLM is spec-decoding.
