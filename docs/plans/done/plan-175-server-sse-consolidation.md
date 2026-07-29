# Server-Side SSE Consolidation Plan

**Goal:** Eliminate the duplicated SSE machinery on the server: one serde-derived event→wire mapping for `PullEvent`/`UpdateEvent`, one shared job-event stream for the two job SSE handlers, one shared broadcast→SSE loop, and uniform `KeepAlive` on every SSE endpoint.

**Architecture:** Domain event enums in tama-core get `#[serde(tag = "event")]` + a `to_sse_event()` method so the wire format is derived (not hand-rolled), which removes the payload drift between `downloads.rs` (embeds `"event"` in JSON) and `updates.rs` (does not). The `tama` crate gains a small `api/sse.rs` helper module owning the two remaining scaffolding shapes: `job_event_stream` (snapshot→replay→live) and `broadcast_to_sse` (Lagged/Closed loop). Handlers become thin: subscribe, wrap, `Sse::new(...).keep_alive(...)`.

**Tech Stack:** Rust, Axum 0.7 (SSE), tokio (broadcast), serde/serde_json

---

### Task 1: Serde-tag `PullEvent` and `UpdateEvent` + `to_sse_event()`

**Context:**
`crates/tama/src/api/downloads.rs:202-282` and `crates/tama/src/api/updates.rs:356-402` each hand-roll a ~60-line match mapping every enum variant to `Event::default().event(...).json_data(...)`, and the payloads have already drifted (downloads embeds `"event": "<Variant>"` in the JSON; updates does not). Both enums are struct-variant-only, so serde's internally-tagged representation works and produces `{"event":"<Variant>", ...fields}` — byte-identical to what downloads.rs emits today (verified). The frontend depends on this: `crates/tama/src/components/toast.rs:132` deserializes `PullEvent { event: String, ... }` (the `event` key is load-bearing), and `crates/tama/src/pages/updates.rs:180-186` reads `data.get("item_id")` etc. (an extra `event` key is harmless). Decisions: use `#[serde(tag = "event", rename_all = "PascalCase")]` — the tag gives the in-JSON `"event"` key; `rename_all = "PascalCase"` is explicit (variant idents are already PascalCase, so it is a no-op guard for future variants). Do NOT add `Deserialize` (server only serializes). Do NOT change any frontend file. Note: if plan-167 has landed, `PullEvent` lives in `crates/tama-core/src/proxy/pull_queue/events.rs` instead of the flat `pull_queue.rs` — same edit, different file.

**Files:**
- Modify: `crates/tama-core/src/proxy/pull_queue.rs` (`PullEvent` at lines 18–54; or `crates/tama-core/src/proxy/pull_queue/events.rs` if plan-167 landed)
- Modify: `crates/tama-core/src/updates/checker/mod.rs` (`UpdateEvent` at lines 21–46)

**What to implement:**

1. In `pull_queue.rs`, change the derive at line 18 from `#[derive(Debug, Clone)]` to:
   ```rust
   #[derive(Debug, Clone, serde::Serialize)]
   #[serde(tag = "event", rename_all = "PascalCase")]
   ```
   Add this impl immediately after the enum:
   ```rust
   impl PullEvent {
       /// Serialize into an SSE event: the `event:` name is the variant name and
       /// the JSON data is the internally-tagged payload (includes the `"event"` key).
       pub fn to_sse_event(&self) -> Result<axum::response::sse::Event, serde_json::Error> {
           let value = serde_json::to_value(self)?;
           let name = value
               .get("event")
               .and_then(serde_json::Value::as_str)
               .unwrap_or("unknown")
               .to_owned();
           axum::response::sse::Event::default()
               .event(name)
               .json_data(&value)
       }
   }
   ```
   (`axum` and `serde_json` are already non-optional deps of tama-core — `crates/tama-core/Cargo.toml:27,31`.)

2. In `updates/checker/mod.rs`, the enum at lines 21–46 already has `#[cfg(feature = "web-ui")]` and `#[derive(Debug, Clone, serde::Serialize)]`. Add `#[serde(tag = "event", rename_all = "PascalCase")]` under the derive, and add the same `to_sse_event()` method in a `#[cfg(feature = "web-ui")]`-gated `impl UpdateEvent` block right after the enum.

3. Add tests. For `PullEvent` (append to the existing `#[cfg(test)] mod tests` in `pull_queue.rs`, or `pull_queue/tests.rs`):
   ```rust
   #[test]
   fn test_pull_event_tagged_serialization_all_variants() {
       let cases: Vec<(PullEvent, &str)> = vec![
           (PullEvent::Started { job_id: "j".into(), repo_id: "a/b".into(), filename: "f".into(), total_bytes: Some(1) }, "Started"),
           (PullEvent::Progress { job_id: "j".into(), bytes_pulled: 1, total_bytes: None }, "Progress"),
           (PullEvent::Verifying { job_id: "j".into(), filename: "f".into() }, "Verifying"),
           (PullEvent::Completed { job_id: "j".into(), filename: "f".into(), size_bytes: 2, duration_ms: 3 }, "Completed"),
           (PullEvent::Failed { job_id: "j".into(), filename: "f".into(), error: "e".into() }, "Failed"),
           (PullEvent::Cancelled { job_id: "j".into(), filename: "f".into() }, "Cancelled"),
           (PullEvent::Queued { job_id: "j".into(), repo_id: "a/b".into(), filename: "f".into() }, "Queued"),
       ];
       for (event, expected_name) in cases {
           let v = serde_json::to_value(&event).unwrap();
           assert_eq!(v["event"], expected_name);
           assert!(event.to_sse_event().is_ok());
       }
   }
   ```
   For `UpdateEvent`, add an equivalent 4-case table test (`CheckStarted`/`CheckCompleted`/`CheckError`/`CheckSkipped`; `CheckCompleted { dto: serde_json::json!({"x": 1}) }`) in `crates/tama-core/src/updates/checker/tests.rs`, gated `#[cfg(feature = "web-ui")]` on the test fn (the enum is feature-gated; the `web-ui` feature is NOT on by default for `cargo nextest run --package tama-core` alone — it is enabled by the `tama` crate's `ssr` feature).

**Steps:**
- [ ] Write the two failing tests above (`pull_queue.rs` tests module and `updates/checker/tests.rs`)
- [ ] Run `cargo nextest run --package tama-core -- proxy::pull_queue` and `cargo nextest run --package tama-core --features web-ui -- updates::checker` — verify they FAIL (no `Serialize`/`to_sse_event` yet)
- [ ] Add the derives, serde attrs, and both `to_sse_event()` impls
- [ ] Run `cargo nextest run --package tama-core --features web-ui -- proxy::pull_queue` and `cargo nextest run --package tama-core --features web-ui -- updates::checker` — pass
- [ ] Run `cargo nextest run --package tama-core --features web-ui` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "feat: derive tagged serde + to_sse_event for PullEvent and UpdateEvent"

**Acceptance criteria:**
- [ ] `serde_json::to_value(&PullEvent::Started{..})["event"] == "Started"` (and equivalently for all 7 `PullEvent` + 4 `UpdateEvent` variants)
- [ ] Both enums expose `to_sse_event(&self) -> Result<axum::response::sse::Event, serde_json::Error>`; `UpdateEvent`'s stays behind `#[cfg(feature = "web-ui")]`
- [ ] No changes to any handler yet (downloads.rs/updates.rs matches still in place — removed in Task 3)
- [ ] `cargo nextest run --package tama-core --features web-ui` passes; clippy clean

---

### Task 2: Extract `job_event_stream(job)` shared by the two job SSE handlers

**Context:**
`crates/tama/src/api/backends/jobs.rs:64-167` (`job_events_sse`) and `crates/tama/src/api/benchmarks/history.rs:74-180` (`benchmark_events`) are near-verbatim: same subscribe→snapshot(under `tokio::join!` of `state`/`log_head`/`log_tail`/`benchmark_results` reads + `log_dropped` load)→replay(head, skipped-marker, tail, stored-result, terminal-status+error)→live-loop over `JobEvent::{Log,Status,Result}` with `Lagged`→`"[N lines dropped]"` log line and `Closed`→return. The only differences are import style (`crate::web_types::JobEvent::Log` vs bare `JobEvent::Log`) and comments. Decisions: put the helper in a NEW module `crates/tama/src/api/sse.rs` (do not stuff it into `api/helpers.rs` — that file is config-dir/DB helpers; plan-169 adds more there). The helper returns the raw stream (not `Sse`) so each handler keeps control of KeepAlive (Task 4). Keep the exact event names (`log`/`status`/`result`/`error`) and payloads (`{"line": ...}`, `{"status": ...}`, `{"results": ...}`, `{"error": ...}`) — zero wire change; `docs/api/sse.md` already documents this format.

**Files:**
- Create: `crates/tama/src/api/sse.rs`
- Modify: `crates/tama/src/api.rs` (add `pub mod sse;` after line 20 `pub mod self_update;` — keep alphabetical-ish grouping; note `updates` follows at line 21)
- Modify: `crates/tama/src/api/backends/jobs.rs`
- Modify: `crates/tama/src/api/benchmarks/history.rs`

**What to implement:**

1. `crates/tama/src/api/sse.rs`:
   ```rust
   //! Shared SSE stream builders for the management API.

   use std::sync::Arc;
   use std::sync::atomic::Ordering;

   use axum::response::sse::Event;
   use futures_util::Stream;
   use serde_json::json;

   use crate::web_types::{Job, JobEvent, JobStatus};

   /// Build the SSE event stream for a job: replay the log snapshot
   /// (head → skipped-marker → tail), replay any stored result, emit the
   /// terminal status/error if the job already finished, then stream live
   /// `JobEvent`s until a terminal status or channel close.
   ///
   /// Subscribes BEFORE snapshotting so no line emitted in between is lost.
   pub fn job_event_stream(job: Arc<Job>) -> impl Stream<Item = Result<Event, axum::Error>> {
       let mut rx = job.log_tx.subscribe();

       let (head, tail, dropped, status, _finished_at, error, stored_result) = {
           // (move the existing tokio::join! snapshot block from jobs.rs:77-93 here verbatim)
       };

       async_stream::stream! {
           // (move the replay + live-loop body from jobs.rs:95-162 here verbatim,
           //  matching on `JobEvent::Log(line)` / `JobEvent::Status(s)` / `JobEvent::Result(results_json)`)
       }
   }
   ```
   Move the body VERBATIM from `jobs.rs` lines 72–162 (it is the cleaner of the two copies — uses `tokio::sync::broadcast::error::RecvError` paths consistently; normalize the two `RecvError` references to a single `use tokio::sync::broadcast;` + `broadcast::error::RecvError::{Lagged, Closed}`).

2. In `jobs.rs`, `job_events_sse` keeps its extractor/lookup prologue (lines 65–73: `web_state.jobs` → 500, `jobs.get(&job_id)` → 404) and the body becomes:
   ```rust
   let stream = crate::api::sse::job_event_stream(job);
   Ok(Sse::new(stream))
   ```
   (Task 4 adds `.keep_alive(...)`.) Delete now-unused imports from `jobs.rs`: `async_stream::stream`, `serde_json::json`, `tokio::sync::broadcast`, `std::sync::atomic::Ordering` — keep only what the remaining `get_job` handler still uses (`get_job` uses `Ordering::Relaxed` at line 32, so KEEP `std::sync::atomic::Ordering`; verify each import against the final file — unused imports fail clippy).

3. In `history.rs`, `benchmark_events` keeps its prologue (lines 75–92) and the body becomes the same two lines. `history.rs` imports come via `use super::*;` (line 1) from `benchmarks/mod.rs` — after the edit, check `benchmarks/mod.rs` imports (`async_stream`, `json`, `Stream`, `Event`) are still used by OTHER `benchmarks/*` submodules before removing anything from `mod.rs`; when in doubt, leave `mod.rs` imports untouched.

**Steps:**
- [ ] Run `cargo nextest run --package tama` — baseline green (there are no direct tests for these two handlers; baseline = whole crate)
- [ ] Create `api/sse.rs` with `job_event_stream`, register `pub mod sse;` in `api.rs`
- [ ] Run `cargo check --package tama` — compiles (helper not yet called → would warn dead_code; if clippy complains, proceed immediately to the rewiring before running clippy)
- [ ] Rewire `job_events_sse` and `benchmark_events` to the helper; clean up unused imports
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Manually diff the old vs new handler bodies: confirm the prologue (404/500 mapping) is byte-identical and only the stream construction moved
- [ ] Commit with message: "refactor: extract shared job_event_stream SSE helper"

**Acceptance criteria:**
- [ ] `job_events_sse` and `benchmark_events` are each ≤ 25 lines; the ~85-line snapshot→replay→live body exists exactly once in `api/sse.rs`
- [ ] Event names/payloads on both endpoints unchanged (`log`, `status`, `result`, `error`; skipped-marker and lagged-marker strings byte-identical)
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 3: Extract `broadcast_to_sse(rx, to_event)` and rewire downloads/updates SSE

**Context:**
`pull_events_sse` (`crates/tama/src/api/downloads.rs:191-293`) and `update_events_sse` (`crates/tama/src/api/updates.rs:341-417`) share the same receive-loop scaffolding (recv → map event → yield; `Lagged(n)` → `Lagged` marker `{"lagged": n}`; `Closed` → break) differing only in the per-variant match — which Task 1 replaced with `to_sse_event()`. Decisions: the helper is generic over the domain event type with a `Fn(&E) -> Result<Event, serde_json::Error>` mapper so it works for both enums and any future one. This task is where the `updates/events` payload changes: every `UpdateEvent` JSON gains the `"event"` key (serde tag). Verified harmless for the frontend (`pages/updates.rs` reads individual keys; `pages/updates.rs:352` subscribes by SSE event NAME, which is unchanged). The `downloads/events` payload is byte-identical to today (already had `"event"`).

**Files:**
- Modify: `crates/tama/src/api/sse.rs` (add `broadcast_to_sse`)
- Modify: `crates/tama/src/api/downloads.rs`
- Modify: `crates/tama/src/api/updates.rs` (or `crates/tama/src/api/updates/events.rs` if plan-167 landed)

**What to implement:**

1. Add to `api/sse.rs`:
   ```rust
   use tokio::sync::broadcast;

   /// Drive a broadcast receiver into an SSE stream: map each domain event with
   /// `to_event`, emit a `Lagged` marker (`{"lagged": n}`) when the receiver falls
   /// behind, and end the stream when the channel closes.
   pub fn broadcast_to_sse<E, F>(
       mut rx: broadcast::Receiver<E>,
       to_event: F,
   ) -> impl Stream<Item = Result<Event, axum::Error>>
   where
       E: Send + 'static,
       F: Fn(&E) -> Result<Event, serde_json::Error> + Send + 'static,
   {
       async_stream::stream! {
           loop {
               match rx.recv().await {
                   Ok(event) => match to_event(&event) {
                       Ok(e) => yield Ok(e),
                       Err(e) => yield Err(axum::Error::new(e)),
                   },
                   Err(broadcast::error::RecvError::Lagged(n)) => {
                       yield Ok(Event::default()
                           .event("Lagged")
                           .json_data(json!({ "lagged": n }))?);
                   }
                   Err(broadcast::error::RecvError::Closed) => break,
               }
           }
       }
   }
   ```

2. `downloads.rs::pull_events_sse` (lines 191–293) shrinks to:
   ```rust
   pub async fn pull_events_sse(
       State(state): State<Arc<ProxyState>>,
   ) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
       let svc = state
           .pull_queue()
           .as_ref()
           .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
       let rx = svc.subscribe_events();
       let stream = crate::api::sse::broadcast_to_sse(rx, tama_core::proxy::pull_queue::PullEvent::to_sse_event);
       Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
   }
   ```
   Delete the entire hand-rolled match (lines 202–282). Remove now-unused imports: `async_stream::stream`, `tokio::sync::broadcast` (verify against the rest of the file first).

3. `updates.rs::update_events_sse` (lines 341–417) shrinks to:
   ```rust
   pub async fn update_events_sse(
       Extension(web_state): Extension<WebState>,
       State(_state): State<Arc<ProxyState>>,
   ) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
       let checker = web_state.update_checker.clone();
       let tx = checker
           .update_events_tx
           .as_ref()
           .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
       let rx = tx.subscribe();
       let event_stream = crate::api::sse::broadcast_to_sse(rx, tama_core::updates::UpdateEvent::to_sse_event);
       Ok(Sse::new(event_stream).keep_alive(KeepAlive::default()))
   }
   ```
   Keep `use tama_core::updates::UpdateEvent;` only if still referenced (the fn pointer path above is fully qualified — drop the import if unused).

**Steps:**
- [ ] Run `cargo nextest run --package tama` — baseline green
- [ ] Add `broadcast_to_sse` to `api/sse.rs`
- [ ] Rewire `pull_events_sse`; run `cargo nextest run --package tama -- downloads` — pass (the `crates/tama/tests/downloads_api.rs` integration tests exercise this module)
- [ ] Rewire `update_events_sse`; run `cargo check --package tama` — compiles
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean (watch for unused `stream`/`broadcast` imports)
- [ ] Commit with message: "refactor: route pull/update SSE through broadcast_to_sse + to_sse_event"

**Acceptance criteria:**
- [ ] The two hand-rolled ~60-line variant matches are deleted; `broadcast_to_sse` is the only recv-loop scaffolding
- [ ] `downloads/events` payloads unchanged (still contain `"event"`); `updates/events` payloads now also contain `"event"` (documented in Task 5)
- [ ] `Lagged` marker behavior identical on both endpoints
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 4: Uniform `KeepAlive` on the three outlier SSE endpoints

**Context:**
Three `Sse::new(...)` sites lack `.keep_alive(KeepAlive::default())` while the other four endpoints have it (`rg 'Sse::new' crates/`): `crates/tama/src/api/backends/jobs.rs:166`, `crates/tama/src/api/benchmarks/history.rs:184`, and `crates/tama-core/src/proxy/tama_handlers/backend_logs.rs:195`. Note: jobs.rs:164-165 and history.rs:181-183 carry an explicit "No keep-alive" comment (deliberate choice at the time: client closes EventSource on terminal status). The audit decision overrides this: keep-alive only emits periodic comment lines while the connection is open — it is harmless for terminal streams and keeps proxies/browsers from timing out idle connections, and uniformity beats per-endpoint ad-hoc policy. Decisions: add `.keep_alive(KeepAlive::default())` to all three and REPLACE the two stale comments; do not touch the four endpoints that already have it.

**Files:**
- Modify: `crates/tama/src/api/backends/jobs.rs`
- Modify: `crates/tama/src/api/benchmarks/history.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/backend_logs.rs`

**What to implement:**

1. `jobs.rs`: change the final line of `job_events_sse` to `Ok(Sse::new(stream).keep_alive(KeepAlive::default()))`; add `KeepAlive` to the existing `axum::response::sse::Event` import (line 3 → `use axum::response::sse::{Event, KeepAlive};`); replace the "No keep-alive…" comment with `// KeepAlive is uniform across all SSE endpoints; the stream still ends on terminal status.`

2. `history.rs`: same change in `benchmark_events`. Its imports come via `use super::*;` — check whether `KeepAlive` is reachable through `benchmarks/mod.rs`'s axum imports; if not, add `use axum::response::sse::KeepAlive;` explicitly to `history.rs`. Replace the stale comment the same way.

3. `backend_logs.rs:195`: change `Sse::new(stream)` to `Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())` (fully-qualified avoids touching the file's import block; or add the import — match the file's existing style, which uses fully-qualified `axum::response::sse::Event` in the body).

**Steps:**
- [ ] Apply the three edits
- [ ] Run `rg -n "Sse::new" crates/ --type rust` — verify ALL seven sites now chain `.keep_alive(`
- [ ] Run `cargo nextest run --package tama` and `cargo nextest run --package tama-core` — pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: apply SSE keep-alive uniformly across all event endpoints"

**Acceptance criteria:**
- [ ] `rg 'Sse::new' crates/` shows 7/7 sites with `.keep_alive(`
- [ ] The two stale "No keep-alive" comments are gone
- [ ] Tests pass in both crates; clippy clean

---

### Task 5: Sync `docs/api/sse.md` with the derived payloads

**Context:**
Task 3 changed the `updates/events` wire format (each payload gains `"event": "<Variant>"`), and the doc was already inaccurate for `downloads/events` (tables omit the `"event"` key that the payloads have always contained). This task is doc-only; no code changes.

**Files:**
- Modify: `docs/api/sse.md`

**What to implement:**

1. Under `## GET /tama/v1/downloads/events`, add one line above the table: `All event payloads are self-describing: the JSON object contains an `"event"` key equal to the SSE event name.` and update each table row to include `event` (e.g. `` `Queued` | `{ event: "Queued", job_id, repo_id, filename }` ``). Keep the `Lagged` row as-is (`{ lagged: N }` has no `event` key — it is not produced by the enums).

2. Under `## GET /tama/v1/updates/events`, add the same note and update the four rows likewise (`{ event: "CheckStarted", item_type, item_id, variant }`, etc.).

3. Add one sentence to the top matter (after line 3): `All endpoints send periodic keep-alive comment lines; clients that close the connection on a terminal event may ignore them.`

**Steps:**
- [ ] Make the three doc edits
- [ ] Cross-check each documented payload against the final code (`PullEvent`/`UpdateEvent` serde output, `job_event_stream` event shapes) — no drift
- [ ] Commit with message: "docs: document tagged SSE payloads and uniform keep-alive in sse.md"

**Acceptance criteria:**
- [ ] `docs/api/sse.md` payload tables for downloads and updates events include the `event` key exactly as serialized by the serde tags
- [ ] No code changed in this task
