# Structured Logging Redesign Plan

**Goal:** Replace Tama's text-file logging pipeline with an embedded SQLite log store (ephemeral, bounded, queryable) fed by a `tracing` layer, add live runtime log filtering, a filterable log UI, and — in a second stage — structured log push from tamad hosts over a new `StreamLogs` gRPC channel.

**Architecture:** Spec approved via design discussion (approach C, staged): stage 1 (Tasks 1–5) is proxy-side only — new `tama_core::logstore` module (SQLite in WAL via rusqlite; custom tracing `Layer` → bounded mpsc → single writer task → batched inserts; retention by age+rows+bytes; FTS5 full-text; live `EnvFilter` reload exposed through the existing config PATCH; new read-only `/tama/v1/logs*` API set + rebuilt log page). Stage 2 (Tasks 6–7) adds `StreamLogs` (server-streaming, one stream per online host, replay ring + `(instance_id, seq)` dedupe) so tamad daemons and engine container logs land in the same store as structured entries. Rationale: `docs/research/tama-structured-logging-redesign.md`; store decision: `docs/adr/0013-log-store-sqlite.md`. Library stays `tracing` 0.1.x / `tracing-subscriber` 0.3.x — **do not change logging crates for feature reasons**.

**Tech Stack:** Rust workspace (`tama-core`, `tama`, `tamad`); rusqlite (bundled, promoted dev→dep in tama-core); tracing + tracing-appender (existing); tonic/prost (existing); tokio; Leptos/WASM web UI (existing); no new dependencies except none — everything needed is already in the workspace manifests.

**Global rules for every task below:**
- Follow `AGENTS.md`: 4-space, `anyhow::Result`, `with_context`, tests in `#[cfg(test)]` modules, doc comments on public items.
- Targeted tests during work: `cargo nextest run --package tama-core -- logstore` (and the analogous `--package tama` / `--package tamad`); fmt after each commit: `cargo fmt --all`.
- Every task must leave the workspace green: `cargo check --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + targeted `cargo nextest run` before committing. The full 5-gate validation (fmt --check, clippy ws, clippy ssr, check csr, nextest ws) is Task 7 only.
- Commit prefix conventions: `feat(logging):` for stages 1/2 chunks.

---

### Task 1: LogStore core — SQLite schema, FTS5, query builder, retention prune

**Context:**
The whole feature rests on an embedded SQLite database that stores log entries as "one JSON document per row + indexed label columns" (the Loki model: labels indexed, payload unindexed) — see ADR-0013 and research report §Q3/§Q4. This task builds ONLY the storage layer as a new `tama-core` module: open/init (idempotent schema), batch insert, the read-query builder used by every future endpoint, per-level counts, distinct sources, and the retention pruner. It deliberately knows nothing about tracing, axum, or tamad: it is pure persistence with rusqlite. Tests run against TEMP-FILE databases (`tempfile::tempdir()`, which is already a ws dev-dep decision — note `tempfile` is a regular dep of `tama-core`, see its Cargo.toml). This task must compile and test green standalone; nothing else in the workspace references it yet.

**Decisions already made:**
- Path: `<logs_dir>/tama-logs.db` (resolved by the CALLER at boot, Task 3 — this module takes a `Path`/`PathBuf`).
- PRAGMAs at open: `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`, **`auto_vacuum = INCREMENTAL`**. The last one is load-bearing: `PRAGMA incremental_vacuum(n)` (mandated later in `prune`) is a **no-op unless the DB's auto_vacuum mode is INCREMENTAL**, and that mode can only be set while the file is empty. `tama-logs.db` is brand-new per install, so set-at-creation is the normal path. Exception: at `open()`, if the file already exists non-empty and `PRAGMA auto_vacuum` reads back `0`, run ONE `VACUUM` to migrate the mode — document this in the code as the single permitted `VACUUM` anywhere in this feature (it is a mode migration, not the routine path the "no VACUUM" rule refers to).
- Schema (single idempotent init):
  ```sql
  CREATE TABLE IF NOT EXISTS logs (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    level INTEGER NOT NULL,
    source TEXT NOT NULL,
    msg TEXT NOT NULL
  );
  CREATE INDEX IF NOT EXISTS logs_level_ts ON logs (level, ts);
  CREATE INDEX IF NOT EXISTS logs_source ON logs (source);
  CREATE VIRTUAL TABLE IF NOT EXISTS logs_fts USING fts5(msg, content='logs', content_rowid='id', tokenize='unicode61');
  CREATE TRIGGER IF NOT EXISTS logs_ai AFTER INSERT ON logs BEGIN
    INSERT INTO logs_fts(rowid, msg) VALUES (new.id, new.msg);
  END;
  CREATE TRIGGER IF NOT EXISTS logs_ad AFTER DELETE ON logs BEGIN
    INSERT INTO logs_fts(logs_fts, rowid, msg) VALUES ('delete', old.id, old.msg);
  END;
  ```
- `level` domain: `trace=0 debug=1 info=2 warn=3 error=4` (an `i32` column; never store `-1` — unknown-level entries store `-1`-free: they map to `2` and carry `level_known: false` in the JSON doc; see Task 6 — the STORE type is `i32` 0–4 and the proto layer does the `-1 → 2 + flag` conversion there).
- `msg` is `text` (NOT jsonb — this is SQLite anyway; the point is: one JSON document per row, `serde_json::Value` in/out).
- Retention: caller passes a struct of the three bounds; pruner computes an `id` watermark `W` such that keeping `id >= W` satisfies ALL of: `ts >= now - max_age`, row count `>=` implied by `max_rows` (i.e. W = (count(*) - max_rows)-th row's id, clamped), estimated bytes `>= max_bytes` index heuristic (W = id at which `SUM(LENGTH(msg)+LENGTH(source))` from oldest rows reaches `max_bytes`; efficient: iterate oldest-first in 10k chunks accumulating length until threshold crossed). DELETE in batches of 10 000 rows (`DELETE FROM logs WHERE id < W AND id >= <chunk_lo> LIMIT 10000` … `LIMIT` is not allowed on bare DELETE in SQLite **without** rowid alias — use the id-range form: `DELETE FROM logs WHERE id >= <chunk_lo> AND id < <chunk_lo + 10000>` looping while `COUNT` remains (advance chunk bounds each batch)). After ALL batches done: `PRAGMA incremental_vacuum(4096)` — MANDATORY (anti openai/codex#35823; omitting it lets the file grow monotonically). No `VACUUM`, never `VACUUM FULL`.

**Files:**
- Modify: `crates/tama-core/Cargo.toml` — promote `rusqlite` from `[dev-dependencies]` (currently `rusqlite.workspace = true`) into `[dependencies]` as `rusqlite.workspace = true`; remove from dev-deps (it stays available implicitly). Workspace already declares `rusqlite = { version = "0.34", features = ["bundled"] }`.
- Create: `crates/tama-core/src/logstore/mod.rs` — module root; re-export public types; doc header citing ADR-0013.
- Create: `crates/tama-core/src/logstore/db.rs` — `LogStore` + `LogRecord` + `LogQuery` + `LogEntry` + `PruneBounds` + `SourceInfo` + `LevelCount`.
- Create: `crates/tama-core/src/logstore/types.rs` — `LogstoreLevel` (newtype over `u8` with `TRACE..ERROR` consts, `as_str()`, `from_u8()`, `Ord`, `serde` as plain number), `Source` (newtype over `String` with constructors `proxy()`, `backend(name)`, `tamad(host)`, `tamad_model(host, model)`, `tamad_model_tail(host, model)`, and `parse(query_term)` for exact-or-prefix matching), doc types.
- Modify: `crates/tama-core/src/lib.rs` — add `pub mod logstore;` directly **after** `pub mod logging;` (which exists at ~line 19; `l` < `s`).
- Test: tests live in `#[cfg(test)] mod tests` at the bottom of each new file (project convention), using `tempfile::tempdir()`.

**What to implement (signatures to follow):**

```rust
// logstore/types.rs
pub struct LogRecord { pub ts: i64, pub level: LogstoreLevel, pub source: Source, pub msg: serde_json::Value }
pub struct LogEntry  { pub id: i64, pub ts: i64, pub level: LogstoreLevel, pub source: Source, pub msg: serde_json::Value }
// All fields optional / defaulted:
pub struct LogQuery {
    pub min_level: Option<LogstoreLevel>,      // None = any
    pub source: Option<Source>,                // None = any; matched exact OR as prefix (see source match below)
    pub q: Option<String>,                    // FTS MATCH; unicode61 tolerates most token shapes — LIKE fallback runs only when MATCH yields zero rows
    pub since: Option<i64>,                   // unix ms inclusive
    pub until: Option<i64>,                   // unix ms exclusive
    pub limit: Option<i64>,                   // default 200, hard cap 1000 (enforce clamp, not error)
    pub cursor: Option<i64>,                  // rowid; default None
    pub order: QueryOrder,                    // Desc (default) | Asc
}
pub enum QueryOrder { Desc, Asc }
pub struct PruneBounds { pub max_age_secs: i64, pub max_rows: i64, pub max_bytes: i64 }
pub struct SourceInfo { pub source: Source, pub last_ts: i64 }
pub struct LevelCount { pub level: LogstoreLevel, pub count: i64 }
```

```rust
// logstore/db.rs
pub struct LogStore { conn: rusqlite::Connection, pub path: std::path::PathBuf }
impl LogStore {
    pub fn open(path: impl Into<std::path::PathBuf>) -> Result<Self>      // create parent dirs; PRAGMAs; idempotent schema; `PRAGMA quick_check` on resume if needed
    pub fn in_memory() -> Result<Self>                                     // `:memory:` variant for tests/convenience
    pub fn insert_batch(&self, records: &[LogRecord]) -> Result<Vec<i64>>  // ONE transaction; returns generated ids in order — simplest: prepare once, exec per record, collect `last_insert_rowid`; 200-row batches keep this fast. Do NOT use COPY (that is a Postgres feature).
    pub fn query(&self, q: &LogQuery) -> Result<(Vec<LogEntry>, Option<i64>)> // (entries, next_cursor=None when window end reached)
    pub fn distinct_sources(&self) -> Result<Vec<SourceInfo>>
    pub fn level_counts_since(&self, since_ms: i64) -> Result<Vec<LevelCount>>
    pub fn prune(&self, b: &PruneBounds) -> Result<i64>                    // rows deleted; includes the incremental_vacuum step
    pub fn last_id(&self) -> Result<Option<i64>>
    pub fn delete_all(&self) -> Result<i64>                            // Task 4's DELETE endpoint: delete in the same 10k-row id-range chunks + final incremental_vacuum; returns count deleted
    pub fn fail_next_insert_for_tests(&self, on: bool)  // `#[cfg(test)]`-gated setter; the `#[cfg(test)] fail_next: bool` field makes the next insert_batch `bail!("injected")` — production build: field compiled out, zero cost.
}
```

Key behaviors (each gets a test):
- `query` builds the WHERE from: `level >= ?` (when min_level set), `ts >= ?`/`ts < ?`, `source = ?` when the term has no host ambiguity AND `source LIKE ?||'%'` prefix clause when the caller passed a prefix (implement source match as a single parameterized SQL OR of the exact + prefix variants — never string-concatenate user input).
- `q`: primary `SELECT ... WHERE <base> AND logs_fts MATCH ?`. **Malformed FTS syntax** (e.g. a `q` containing a lone `"`) makes `MATCH` raise a rusqlite error — this is NOT "zero rows": on ANY `rusqlite::Error` from the MATCH attempt, fall back to the `LIKE '%q%'` path (same result as the zero-rows fallback). Never propagate an FTS error to the handler (no 500s from search text). Also note for the docs: FTS5 indexes the whole `msg` document, so searching structural keys (`message`, `target`, `dropped`) matches most rows — expected, document it on the endpoint. `cursor` + `order`: `id < ?` for Desc-from-cursor, `id > ?` for Asc; final `ORDER BY id <DIR>`, fetch `LIMIT ?+1`; `next_cursor` = last entry's id, or `None` when rows.len() <= limit.
- `level_counts_since`: `SELECT level, COUNT(*) FROM logs WHERE ts >= ? GROUP BY level ORDER BY level` (the `(level, ts)` index does not cover a ts-only scan — fine at the 50k-row scale; add a note if it ever hurts, and do NOT add a new index now).
- `distinct_sources`: `SELECT source, MAX(ts) FROM logs GROUP BY source ORDER BY MAX(ts) DESC`.
- `prune` watermark algorithm per the decision above; must be idempotent when already within bounds (0 deleted, still runs a no-op `incremental_vacuum` — acceptable).
- `insert_batch` must set nothing up per-call beyond the prepared-statement (store nothing mutable in `LogStore` except the test fault flag).
- `source` matching by prefix: `"tamad:gpu-box"` should also match rows `tamad:gpu-box:model:x` — that is `source = 'tamad:gpu-box' OR source LIKE 'tamad:gpu-box:%'` (delimiter-aware prefix: append `:` NOT `''` so it cannot over-match `tamad:gpu-boxer`). UNIT TEST this.

**Steps:**
- [ ] `cargo check --package tama-core` before change (baseline green).
- [ ] Add the dependency promotion + empty `logstore` module skeleton (mod.rs with doc + `pub mod db; pub mod types;`) so `cargo check` passes.
- [ ] Write FAILING tests in `db.rs` tests module: open-then-insert-then-query round-trip; FTS match + LIKE fallback; source exact/prefix/no-overmatch; cursor pagination + next_cursor semantics (Desc AND Asc); level/since/until filters; level_counts + distinct_sources; prune per bound (age / rows / bytes) + "already within bounds → 0 deleted"; injected insert failure returns Err and does not lose subsequent batches (fail-next fires once).
- [ ] Run `cargo nextest run --package tama-core -- logstore` — confirm FAILURES (unimplemented).
- [ ] Implement `types.rs` then `db.rs`.
- [ ] Run `cargo nextest run --package tama-core -- logstore` — all green.
- [ ] `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings` — clean.
- [ ] Commit: `feat(logging): add logstore core (SQLite schema, FTS5, queries, prune) [tama]`

**Acceptance criteria:**
- [ ] `cargo nextest run --package tama-core -- logstore` all pass, including the prefix-no-overmatch and prune-bounds tests.
- [ ] `logstore/db.rs` + `logstore/types.rs` use only rusqlite + serde_json + anyhow (+ tempfile in tests) — no axum/tracing-appender/tokio (tokio enters the module with Task 2's `writer.rs`/`layer.rs`; that is expected and fine).
- [ ] `LogStore` is `Send` (holds a single `rusqlite::Connection`; document that the writer is the sole writer; readers open separate connections in Task 4).

---

### Task 2: LogStoreLayer + writer task + log filter module (no app wiring yet)

**Context:**
Task 1 gave us the table. This task adds the TRACING side: a `tracing_subscriber::Layer` that converts each event into a `LogRecord` and `try_send`s it over a bounded channel, and the single writer task that drains that channel into `LogStore` in batches with journaled-style degradation. The split (layer lives on the hot path = one `try_send`; writer owns the DB connection + all policy) is the one that keeps the app never blocking on log I/O. The degradation mode is: on write failure, warn+ entries (plus every `dropped:` marker regardless of level) go to an in-memory ring; the writer retries 1 s; on recovery the ring drains oldest-first through the normal path; status is broadcast via a `watch::Sender<LogStoreStatus>` (same SP/MP pattern as `ProxyState.inference_stats` per AGENTS.md). This module ALSO moves the existing `build_log_filter` (currently in `crates/tama/src/main.rs:263-282`) into `tama_core::logstore::filter` so proxy startup, the `tama admin` CLI, AND (later) tamad startup share it — the merge of RUST_LOG and the new config `log_directives` field happens in exactly one place. Nothing is wired into binaries yet in this task: it must compile + test standalone.

**Decisions already made:**
- Channel: `tokio::sync::mpsc::channel(1024)` of `LogRecord`. Drop policy on `Full`: drop the NEWEST (the in-flight event) and increment an `AtomicU64` drop counter. Never block in `on_event`.
- Writer cadence: await first record (250 ms timeout), then greedy `try_recv` up to total 200 records (byte guard: stop collecting if accumulated JSON bytes > 256 KiB), then ONE `insert_batch`.
- Drop marker: when `dropped > 0` for ≥ 5 s (wall clock), the writer itself ENQUEUES a synthetic `LogRecord` (source `log-store`, level WARN, `msg = {"message":"log store: dropped N events since <ts>","dropped":true,"dropped_count":N,"dropped_since_ts":"<rfc3339>"}`) and resets the counter. This row then flows through the normal insert path like any other.
- Ring: `VecDeque<LogRecord>`, cap 1 024 entries OR 4 MiB estimated bytes, FIFO; entries pass iff `level >= WARN || msg.get("dropped") == Some(true)`.
- Retry loop while disabled: `tokio::time::sleep(1s)` between `insert` attempts; on first success → drain ring oldest→newest through normal batches (bounded: drain completes when ring empty OR 5 s elapsed — on 5 s elapsed, drop leftover and log a WARN + bump `ring_discarded` metric).
- `LogStoreStatus` (pub in `logstore`): `{ degraded: bool, degraded_since: Option<i64> /*unix ms*/, channel_len: usize, ring_len: usize, dropped_total: u64, backoff_seen: u64 }` written by the writer on each state transition and (for channel_len/ring_len frequency) at most every 1 s; initial `{ degraded: false, ... }`.
- Shutdown: `CancellationToken`; on cancel: stop receiving, drain remaining channel records, final batch, `tokio::time::timeout(2s)` around it, close. Document the WorkerGuard-like rule in the module docs ("dropping the drain/cancel handle before app exit silently stops logging — same rule as the existing `WorkerGuard` pattern in AGENTS.md").
- `on_event` encoding (document in `layer.rs`):
  - `ts` = `SystemTime::now().duration_since(UNIX_EPOCH).as_millis() as i64` (capture time — call it that in docs; source-of-truth-for-display).
  - `level` from `event.metadata().level()`.
  - `source` = layer's configured `Source` (default `Source::proxy()`; each binary sets it when constructing the layer; tamad sets its own in Task 6).
  - `msg` JSON object: `{"message": <event message text>, "target": <meta.target()>}` + every structured field flattened as a first-class key (visit via a small `FieldValueVisitor`: primitive/Display → JSON value; `serde_json::Value`/`Field` → as-is; anything unrepresentable → its `Debug`-formatted string; attribute **named fields only** — no span fields in v1 (no spans policy)).
  - NEVER emit a field whose value is a pre-serialized JSON string of a struct — that is the Q2 best-practice rule; this task's layer makes structurally-passing things impossible (visitor).
  - Marker events (synthetic drops) are indistinguishable rows downstream.
- `filter.rs` (moved + extended):
  ```rust
  /// Builds the runtime EnvFilter from the DB/durable config: the `log_level` is the floor/default; `log_directives` (RUST_LOG-syntax string) adds target directives; RUST_LOG env vars are added ONLY while config directives are empty… — NO. EXISTING semantics at `main.rs:263-282` are: level from config is default, RUST_LOG env-target directives always merged. ADD: config `log_directives` are merged with the same 'target-only = directive contains =' rule as RUST_LOG (config directives take precedence over env directives of the SAME target — implement by adding env directives FIRST, config directives after; `EnvFilter::add_directive` replaces the last-matching-target directive, so later additions win — verify this with a unit test: `EnvFilter::new("info").add_directive(parse("a=warn")).add_directive(parse("a=error"))` → `event_enabled(error)=true, warn=false` — if the replace-last-matching behavior does NOT hold, switch implementation to: build a directive-string joined by ',' and `EnvFilter::new(joined)`).
  pub fn build_log_filter(level: &LogLevel, directives: &str) -> Result<EnvFilter, LogFilterError>  // validates ALL directives from both sources. RUST_LOG (env) keeps its TODAY behavior and is merged INTERNALLY by this fn exactly as main.rs does it now (target-only rule; env directives first, config `directives` after so config wins for the same target) — the fn reads env itself, callers never pass env. Unit-test the replace-last-matching-target behavior as specified in the doc comment above; if it does not hold in this tracing-subscriber version, build the joined directive string and `EnvFilter::new(joined)` instead (pick one, document which in the code).
  /// Swaps the loaded filter. Returns the number of directives in the replacement filter (for tests/observability).
  pub fn apply_reload<S>(handle: &tracing_subscriber::reload::Handle<EnvFilter, S>, level: &LogLevel, directives: &str) -> Result<usize, LogFilterError> where S: Subscriber + for<'a> LookupSpan<'a>
  ```
  And in `writer.rs`: `impl LogStoreStatus { pub fn ok() -> Self }` (degraded: false, rest zero) — Task 3's `watch::channel(LogStoreStatus::ok())` depends on it.
  - Keep the `crate::config::LogLevel` import path stable (today: `tama_core::config::LogLevel`; `crates/tama` aliases it as `CoreLogLevel` in some spots — do not rename anything).

**Files:**
- Create: `crates/tama-core/src/logstore/layer.rs` (`LogStoreLayer`, `FieldValueVisitor`, `build_layer(tx, source) -> LogStoreLayer`).
- Create: `crates/tama-core/src/logstore/writer.rs` (`LogStoreStatus`, `WriterConfig{batch_max_rows:200, batch_wait: Duration::from_millis(250), byte_guard: 256*1024, ring_max_entries:1024, ring_max_bytes:4*1024*1024, retry_interval: Duration::from_secs(1), drain_timeout: Duration::from_secs(5), drop_marker_window: Duration::from_secs(5)}` with `Default`, `spawn_log_writer(store: LogStore, rx: mpsc::Receiver<LogRecord>, status_tx: watch::Sender<LogStoreStatus>, token: CancellationToken) -> JoinHandle<LogStoreStatus /* final */>`). **Blocking posture:** every `insert_batch`/`prune`/`delete_all` call runs inside `tokio::task::spawn_blocking` (rusqlite is synchronous; a slow disk must not stall a runtime worker thread). `LogStore` is `Send`; the holder owns the writer's sole connection.
- Create: `crates/tama-core/src/logstore/filter.rs`.
- Modify: `crates/tama/src/admin.rs` — it calls `crate::build_log_filter(&config.general.log_level)` (~:165): update the call to the new two-arg signature passing `""` (config directives come in Task 3), import from `tama_core::logstore::filter`.
- Modify: `crates/tama/src/main.rs` — delete the local `build_log_filter` (move to filter.rs as described); update the `tracing_tests` module (`test_build_log_filter_honors_log_level`, `test_reload_handle_applies_db_log_level_after_startup`) to the new import + two-arg call (pass the env `RUST_LOG` value as `directives` per the bridge rule in Steps).
- Modify: `crates/tama-core/src/logstore/mod.rs` (re-exports).
- Test: `#[cfg(test)]` mods in each new file. Also cross-file test in `mod.rs` tests: layer→channel→writer→in-memory-store end-to-end (construct `registry` with the layer + `tracing_subscriber::reload::Layer` OR a bare `Layered<Registry>` — bare is fine here).

**Steps:**
- [ ] Baseline `cargo nextest run --package tama-core -- logstore` green.
- [ ] FAILING unit tests first: (a) `FieldValueVisitor` flattens `info!(gpu = "a", n = 1, "m")` into `{"message":"m","gpu":"a","n":1,"target":...}`; (b) drop-newest semantics: fill mpsc(1) then `on_event` → counter 1, channel holds the OLD record; (c) writer batch timing with `tokio::time::pause()` + controlled sender (3 records + tick ≤ 250 ms → one flush of 3; 400-record feed → batches of 200); (d) degradation: `LogStore::fail_next_insert_for_tests(true)` → `status.degraded` flips true within the retry interval (retries go through `tokio::time::sleep`, so `time::pause()` works; inject `retry_interval: 1 ms` via `WriterConfig` for test speed); ring admission (info dropped, warn kept, marker kept); drain order oldest→newest (insert 3, verify ids ascending); recovery clears degraded; (e) marker throttling: 2 drop periods < 5 s apart → one marker; ≥ 5 s apart → two; (f) `build_log_filter` merge chain incl. replace-last-matching-target test + invalid-directive Err; (g) shutdown: send 50 records, cancel mid-drain → all-or-most inserted within timeout, writer handle joins.
- [ ] Run `cargo nextest run --package tama-core -- logstore` → confirm failures.
- [ ] Implement `filter.rs` (moving `build_log_filter` out of `crates/tama/src/main.rs` — DELETE the local copy there; main.rs + admin.rs call sites get the new two-arg signature, main.rs passing `""` as `directives` (config `log_directives` does not exist yet — the fn's internal `RUST_LOG` merge reproduces the current behavior byte-for-byte, including the target-only rule); admin.rs passes `""`; update main.rs's `tracing_tests` module to the new import + two-arg call) — then re-verify with the existing `main.rs` tests: `cargo nextest run --package tama` (the `init_default_tracing`/`build_log_filter` suite must pass).
- [ ] Implement `layer.rs`, `writer.rs`.
- [ ] Re-run targeted tests green; `cargo nextest run --package tama` green (main.rs bridge intact); `cargo fmt --all`; `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Commit: `feat(logging): add LogStoreLayer, writer task and shared filter builder [tama]`

**Acceptance criteria:**
- [ ] Layer adds zero `unwrap/expect` on the hot path (all conversion failures → skip that FIELD, keep the record — unit-test a field that formats to an empty error).
- [ ] Degraded/drain/shutdown tests all pass with injected 1-ms timers.
- [ ] Old `build_log_filter` body is REMOVED from `crates/tama/src/main.rs` (grep must return zero matches for `fn build_log_filter` outside `tama-core/src/logstore/filter.rs`).
- [ ] `cargo nextest run --package tama-core -- logstore` + `--package tama` fully green.

---

### Task 3: Wire the store into the proxy + live log config (fields, migration, PATCH apply)

**Context:**
Tasks 1–2 exist but nothing reaches them. This task makes the PROXY process actually log to the store on boot, publishes the live filter handle via API (fixing the "level changes need a restart" gap — the handle is today consumed and dropped at `crates/tama/src/main.rs:133`), and adds the new durable config fields (in the same Postgres `app_general` table as the existing `log_level`/`logs_dir`). The apply API is the EXISTING `PATCH` structured-config route — the handler `patch_structured_config` in `crates/tama/src/api.rs` (≈:361); `merge_general` (:192) is a pure helper it calls. No new config routes are permitted (config editing goes through that one route — see the merge function and the config editor form). It also does the one emission-convention change approved in the design: the per-request forward log drops `info → debug` (the 50 k line retention window must not be burned by request noise).

**Decisions already made:**
- Store path: `<logs_dir or base_dir/logs>/tama-logs.db` — same resolution as `logs_dir` today (`crates/tama-core/src/config/loader.rs:31-37`).
- Writer lifecycle in `main.rs` STARTUP (after `Config::load_from_pool`, alongside `init_tracing` at `main.rs:384-424`): channel (capacity 1024), `status_tx: watch::channel(LogStoreStatus::ok())`, spawn writer, store in a `LogRuntime { store_reader: Arc<LogStore /*SECOND connection for reads*/>, channel_len via status, status_rx, filter_handle: Option<reload::Handle<...>> }` in `WebState` (`crates/tama/src/web_types.rs:386`) — the state that web routes receive — the filter handle MUST be `Clone`-able into handlers (it is). Open the READ connection with the same PRAGMAs except that writers may cohabit one SQLite file (WAL allows 1 W + N R; read conn: `PRAGMA query_only = 1` is OPTIONAL — skip it).
- Status → SSE: the existing SSE per-domain endpoint pattern (see `docs/api/sse.md`: `/downloads/events`, `/updates/events`, `/self-update/events` — self-describing JSON `{"event": "X", ...}`) is mirrored with the new endpoint `GET /tama/v1/logs/events` (routed in Task 4, state wired here): `LogStoreDegraded { event, since }` / `LogStoreRestored { event, had<entries> ring_flushed }`. The writer task pushes status via `status_tx` (it never touches SSE — its DB calls run on `spawn_blocking` only); a small tokio task in main.rs owns a `status_rx` clone and, on `degraded` transitions only, pushes onto this endpoint's broadcast. REUSE the exact existing mechanism: `WebState.update_tx: Arc<std::sync::Mutex<Option<tokio::sync::broadcast::Sender<String>>>>` (web_types.rs:397) consumed by `crates/tama/src/api/updates/events.rs` and created in main.rs — add an analogous `log_events_tx` WebState field for `/tama/v1/logs/events` (per-endpoint sender, WebState-stored — it is NOT app-global).
- WebState extension (CONSTRUCTION RIPPLE WARNING): `WebState` is built with struct literals in **~36 places** (main.rs + ~20 API test modules + `crates/tama/tests/router_ownership_test.rs`). Add the fields as `Option`s AND update ALL construction sites: `log_filter: Option<tracing_subscriber::reload::Handle<EnvFilter, Registry-shaped type>>`, `log_status: Option<Arc<tokio::sync::watch::Receiver<LogStoreStatus>>>` (Task 4 adds `log_read: Option<…>` — same treatment). In `main.rs` set the real values; in every test/fake site set `None` (handlers treat `None` as "not wired" — the PATCH apply no-ops with a `debug!` when `log_filter` is `None`, so existing config tests are unaffected). Discovery command: `rg -n "WebState {" crates/` — update each hit in this task's and Task 4's deltas.
- RUST_LOG precedence (explicit): `build_log_filter(level, config_directives)` keeps reading the `RUST_LOG` **environment** variable internally (unchanged from today: config level = default; RUST_LOG target-directives merged, target-only rule; **config `log_directives` win over RUST_LOG for the same target** — env added first, config after). Boot (main.rs) passes config directives; PATCH validation composes identically (same fn, same env), so validate-and-apply never disagree. Document this in `docs/api/config.md`.
- Config fields (all additive, all with serde defaults):
  - `General.log_directives: Option<String>` (`#[serde(default, skip_serializing_if = Option::is_none)]`).
  - `General.log_retention_days: u32` (default 7 — add `defaults::DEFAULT_LOG_RETENTION_DAYS = 7` to `crates/tama-core/src/config/defaults.rs`; same file has `default_update_check_interval`).
  - `General.log_retention_rows: u64` (default `50_000`, `defaults::DEFAULT_LOG_RETENTION_ROWS`).
  - `General.log_retention_max_mb: u64` (default `256`, `defaults::DEFAULT_LOG_RETENTION_MAX_MB`).
- Postgres migration: new file `crates/tama-core/migrations/00000000000006_app_general_log_fields.sql`:
  ```sql
  ALTER TABLE app_general ADD COLUMN log_directives TEXT;           -- NULL = none
  ALTER TABLE app_general ADD COLUMN log_retention_days INTEGER NOT NULL DEFAULT 7;
  ALTER TABLE app_general ADD COLUMN log_retention_rows BIGINT NOT NULL DEFAULT 50000;
  ALTER TABLE app_general ADD COLUMN log_retention_max_mb BIGINT NOT NULL DEFAULT 256;
  ```
  (sqlx `migrate!` embeds + runs at startup — the next number is correct: the highest existing is `...00000000000005_drop_active_models.sql`. Migration files run via `run_migrations` (the const is `MIGRATIONS`) at `crates/tama-core/src/db/postgres.rs:14`, and `main.rs` calls `run_migrations` (≈:111) BEFORE `Config::load_from_pool` (≈:121) — so the new columns exist by the time config loads; do nothing else.)
- `app_config_queries.rs`: extend `GeneralRecord` + the upsert + SELECT to include the four new columns. NOTE: this file has **no existing test mod** — do not look for one; add a fresh `#[cfg(test)] mod tests` modeled on `crates/tama-core/src/db/queries/tamad_queries.rs`'s tests (same test-pool/equivalent-schema guard pattern) asserting the four new columns round-trip through `upsert_general`/`get_general`. `upsert_general`'s positional parameter list grows by four — its ONLY call site is `Config::save` in `crates/tama-core/src/config/types/mod.rs` (≈:259), which must change in lockstep. The same file builds `General` from the record in `load_from_pool` (≈:78–93) — extend that too. (There are no `From<GeneralRecord>` impls; the `seed_defaults` INSERT is fine under `ALTER TABLE … DEFAULT` for pre-existing DBs.)
- **Mirror config types (WASM):** the PATCH structured handler round-trips through mirror types, so the four new fields must also land in: `crates/tama/src/types/config/general.rs` (mirror `General` struct + its struct-literal test), `crates/tama/src/types/config/patch.rs` (`GeneralPatch` — ssr-gated; new fields as `Option<T>`), `crates/tama/src/types/config/core_conv.rs` (both `From` impls between mirror `General` and `tama_core::config::General`, ≈:75 and :88, field-by-field), and the `sample_config()` test literal in `crates/tama/src/api.rs` (≈:390).
- LIVE APPLY: in the handler `patch_structured_config` (`crates/tama/src/api.rs` ≈:361 — **not** `merge_general` at :192): give it an additional extractor `Extension(web_state): Extension<WebState>` (the router applies `Extension(web_state)` as the last layer; exact precedent in the same csrf sub-router: `create_tamad` at `crates/tama/src/api/tamads/register.rs:34` takes `State` + `Extension` together). **Validation must happen at the API boundary BEFORE persist** — a bad directive persisted would brick the filter on next boot. ORDER: (1) `validate_log_config(&merged_general)` = `build_log_filter(level, directives)` — error → 400 `{ "error": "invalid log directive: '…'" }` and persist nothing; (2) persist; (3) when `log_level` or `log_directives` changed, `tama_core::logstore::filter::apply_reload(&web_state.log_filter.as_ref().ok_or_else(...)?, &new_general.log_level, &new_general.log_directives.clone().unwrap_or_default())?` — returns the directive count; treat `log_filter == None` as no-op (tests) and a post-validation `LogFilterError` as 500 (logic bug — validation ran moments earlier). Validation helper: `validate_log_config(&General) -> Result<(), LogFilterError>` in the API module; `LogFilterError`: `#[derive(Debug, thiserror::Error)]` with one variant `InvalidDirective(String)`.
- Emission change: `crates/tama-core/src/proxy/.../request.rs:92` (find it: `rg '"Forwarding request to'` ) — `info!` → `debug!` for that one call (fields unchanged; message text unchanged).
- `tama admin` CLI (`crates/tama/src/admin.rs:165-169` currently applies a console-only filter): leave its log surface as-is except that it now also gets the config `log_directives` (it already loads config — confirm; if it doesn't, pass `""` there — do NOT extend admin.rs scope beyond what compiles).

**Files:**
- Create: `crates/tama-core/migrations/00000000000006_app_general_log_fields.sql`
- Modify: `crates/tama-core/src/config/types/general.rs` (fields + Default)
- Modify: `crates/tama-core/src/config/types/mod.rs` (load_from_pool construction ≈:78–93; `Config::save` call site ≈:259 — keep the positional signature in sync with `upsert_general`)
- Modify: `crates/tama-core/src/config/defaults.rs` (three constants)
- Modify: `crates/tama-core/src/db/queries/app_config_queries.rs` (`GeneralRecord` + upsert/SELECT SQL + new `#[cfg(test)] mod tests`)
- Modify: `crates/tama/src/types/config/general.rs` (mirror `General` + its struct-literal test)
- Modify: `crates/tama/src/types/config/patch.rs` (`GeneralPatch` — new `Option<T>` fields)
- Modify: `crates/tama/src/types/config/core_conv.rs` (both `From` impls, ≈:75/:88)
- Modify: `crates/tama/src/admin.rs` (already in Task 2's Files — here update to pass `config.general.log_directives.clone().unwrap_or_default()` IF admin loads full config; confirm first — if it does not read `config.general`, leave the `""` from Task 2)
- Modify: `crates/tama/src/main.rs` (log runtime startup; WebState construction; status→SSE bridge task; `logs_dir` resolution moved into the shared `logruntime` helper)
- Modify: `crates/tama/src/web_types.rs` (WebState fields: `log_filter: Option<tracing_subscriber::reload::Handle<EnvFilter, Registry-like type>>`, `log_status: Arc<tokio::sync::watch::Receiver<LogStoreStatus>>`; plus the SSE broadcast sender if one is not already global)
- Modify: `crates/tama/src/api.rs` (merge + validate + apply; new helpers; tests)
- Modify: the request forward `info!` call (find file per `rg`)
- Test: extend existing config tests; NEW tests in `api.rs` tests mod: PATCH with valid new directives persists + filter handle modified (assert via a subscriber harness with a catching layer — build a `tracing_subscriber::reload::Handle` with a bare `fmt` subscriber in the test, swap, emit `info!`, assert different output pre/post… a simpler deterministic assertion: after apply, capture `handle.with_inner(|f| ...)` — EnvFilter does not expose directives; SETTLE: to test the apply path's integration, `apply_reload` returns `Result<Changed>` kind where `Changed` carries the parsed directive count; the test asserts Ok(count) + config persisted; the filter-itself behavior is already unit-test-tested in Task 2. Good.)

**Steps:**
- [ ] Baseline: `cargo nextest run --package tama` + `--package tama-core` green.
- [ ] FAILING test: config round-trip incl. the new four fields — `GeneralRecord` through `upsert_general`/`get_general`, `load_from_pool` construction, `Config::save` (new test mod per the decision above; column DEFAULTs cover pre-existing DBs).
- [ ] Implement migration + config plumbing (defaults, types, record, conversions, loader reads of `General` — `rg "log_level" crates/tama-core` before editing).
- [ ] FAILING tests: `validate_log_config` (good, empty, invalid directive → Err with the offending literal); PATCH flow: invalid directive → 400 + DB row unchanged (assert via a second read); valid → persisted + apply OK.
- [ ] Update ALL `WebState {...}` construction sites per the ripple note above (main.rs real values; tests `None`).
- [ ] Wire `main.rs` startup (channel, store open at resolved path, writer spawn, second read-connection, WebState extension, status→SSE bridge per the decision above). Keep the `init_tracing` doc comment accurate (mention the store writer guard rule).
- [ ] Downgrade the forward call noted above to `debug!`.
- [ ] `cargo nextest run --package tama` / `--package tama-core` green; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all`.
- [ ] Commit: `feat(logging): wire log store into proxy, live filter via PATCH, retention config [tama]`

**Acceptance criteria:**
- [ ] Booting `tama` (test/fixture pool) creates `tama-logs.db` under resolved `logs_dir`, rows appear for startup logs at the configured level.
- [ ] PATCH `log_level=debug` (or a valid directive) via the API reflects IMMEDIATELY in subsequent logs with no restart (manual/smoke via the `tama admin`/curl is fine; assert in tests the apply path per "file set" above).
- [ ] Invalid directive → HTTP 400, DB row byte-identical, filter untouched (assert in tests).
- [ ] `info!` forward call is now `debug!` (grep-verified; if any test asserts that line's presence at info level, update the test to expect debug — never weaken unrelated assertions).
- [ ] `cargo nextest run --workspace` green before commit (this task touches the boot path).

---

### Task 4: `/tama/v1/logs*` read API + legacy tail adapter + dead log code removal + `tama.log` rolling writer

**Context:**
With the store live in the proxy (Task 3), this task builds the read API that the UI (Task 5) consumes, adapts the EXISTING raw tail sources (tamad engine containers, any leftover `*.log` from local backends) as a degraded `@tail` feed (so stage-1 still shows host logs while the tamads are not yet on `StreamLogs`), deletes the dead plumbing identified in the research audit (`/logs/:backend*` routes, `BackendLogStream` manager), and fixes the known log-file rotation quirk (manual one-shot rotation at boot; the runtime non-blocking writer never rotates — swap for `tracing_appender::rolling`: `RollingFileAppender::builder().rotation(Rotation::HOURLY).max_files(24)` — 0.2.5 has it with `MakeWriter` support). **One-commit-window warning:** after this task's commit, the old `pages/logs.rs` (Task 5 rewrites it) renders against a changed payload shape — the page is degraded BY DESIGN until Task 5 lands; do not "fix" the API shape to appease the old page.

**Decisions already made:**
- New module: `crates/tama-core/src/proxy/tama_handlers/logs_api.rs` (co-located with existing handlers — `handle_all_logs` is in `tama_handlers/backend_logs.rs`; keep that file for the pieces that MOVE vs stay: see below).
- Handlers (all read through `Extension<WebState>`; **Store is `Send` but NOT `Sync`** — `rusqlite::Connection` is not `Sync` — so WebState carries `log_read: Option<Arc<std::sync::Mutex<LogStore>>>` (a second, read-only connection opened in main.rs per Task 3; this field added here in Task 4) and every handler runs its `LogStore` call inside `spawn_blocking` (matches the existing `spawn_blocking` usage in `backend_logs.rs`); readers block each other briefly — acceptable for rare UI calls; the writer holds its OWN connection so readers never block it). Endpoints:
- Endpoint contracts (response shapes — the project's JSON convention is **snake_case** (`web_types.rs` uses `rename_all = "snake_case"`):
  - `GET /tama/v1/logs?level=&source=&q=&since=&until=&limit=&cursor=&order=desc` → `200 { entries: [LogEntryDto], next_cursor: null|int }` | `400 { error }` (invalid `q` over 512 chars, invalid `level`/`order` enum). **Unrecognized `?source=` → `200` with EMPTY `entries`** (never 400 on source — old bookmarks in the `{host}:{model}` form from pre-Task-5 emitters must not 404/400; `Source::parse` exact-or-prefix decides store-vs-adapter).
  - `LogEntryDto: { id:int64, ts:int64, level:"info", source:"…", message:str, fields: obj, dropped?:bool, dropped_count?:int64, level_known?:bool(false), legacy?:bool }` where `message`/`dropped*`/`level_known` come from flattening the `msg` document; `fields` = the document minus those known keys (never include target-as-message: keep `target` inside `fields`).
  - `GET /tama/v1/logs/sources` → `200 { sources: [{source, last_ts}] }` (legacy host tails do not appear here — they are on-demand, not rows; Task 5's UI builds the host pickers from the tamad registry + the `:model:` names it knows on the page).
  - `GET /tama/v1/logs/summary?since=` → `200 { counts: {debug:int, info:int, warn:int, error:int, total:int} }`.
  - `GET /tama/v1/logs/stream?…same filters…&after=<id>` → `text/event-stream`; each event: `event: entry`, `data: {…LogEntryDto}`; new rows above `after` per poll, until the cursor advances (repeat `query(order: Asc, cursor: after, limit: 200)` every 1 s; on a batch found, remember the max id as the new `after` — emit an `event: keepalive` on empty ticks to match the project's SSE keep-alive convention per `docs/api/sse.md`). A malformed `q` that breaks FTS5 falls back to the `LIKE` path in `LogStore::query` (Task 1) — the handler never 500s on search text; document that FTS5 matches the whole JSON document.
  - `GET /tama/v1/logs/export?…filters…&format=csv` → `text/csv` with header row `id,ts,level,source,message` (`message` = flattened doc's `"message"` string; RFC 4180 quoting via existing `csv` crate if in ws — `rg "^csv" Cargo.toml`; if absent use a manual quoter function with a test) — with hard cap 50 000 rows → `413 { error: "export cap of 50000 rows exceeded — narrow the window" }` on overflow… wait, the 413 for filter too wide pending: implement as count-first (`SELECT COUNT(*)` under filters) then cap check → 413 before streaming.
  - `DELETE /tama/v1/logs` → `202 { deleted: int, compacted: bool }` via `LogStore::delete_all()` (added in Task 1: same 10k-row id-range chunks + final `incremental_vacuum`). **Route home: the `csrf_routes` sub-router in `router.rs`** (CSRF-enforced mutation routes; same home as the other DELETEs) — not the top-level GET section.
- Legacy tail adapter (the `@tail` path): thin trait `LogTailProvider { async fn tail(&self, source: &LogTailSource) -> Result<Vec<(i64 /*fetch_ts_ms*/, String /*line*/)>, anyhow::Error> }` with impl `TamadTailProvider { pool, clients }` that reuses the current per-source tail logic from `collect_tamad_log_sources` (`tama_handlers/backend_logs.rs:146-217`) for a single source (guard by `?source=` matching `tamad\:.*`); results cached per source in `Arc<RwLock<HashMap<String, (Instant, Vec<TailLine>)>>>` with a 5 s TTL (concurrent UI polls reuse the fetch). Each row becomes a `LogEntryDto { legacy: true, level_known: false, level: "info", ts: fetch_ts, id: negative … }` — id must be unique + ordered: use `-(fetch_ts * 1000 + line_ordinal)` as i64 — these never collide with REAL ids (positive) and sort stably; document this convention in the type doc. Non-tamad sources (`proxy`, `backend:<x>`, stale local files) → the same `*.log` files in the logs dir that today's `handle_all_logs` tails (`tail_lines` at `crates/tama-core/src/logging.rs:103-121` is exactly the tool — KEEP `tail_lines` ONLY for this adapter; do not delete it). `format_log_line` (the JSON reprint) IS deleted: adapter rows pass raw lines through (`message` = the raw line, `fields = {}`).
- `handle_all_logs` (i.e. the new `GET /tama/v1/logs`) becomes: if the query carries a `source=` matching the legacy shape → the endpoint returns ONLY tail-adapter rows for that source (page shows the legacy pill; `q`/`level` are ignored on `@tail` sources — `legacy: true` in the DTO lets the UI grey them out; keep it simple and document). `source=proxy` (or no source): normal row stream. Multi-source selection is future work — v1 is single-source; state that in the doc and return 400 on a repeated `source` param (query params are singular anyway).
- Removals (with the pre-delete check from Step 1): `crates/tama/src/api/logs.rs` (old `get_backend_logs`) + its route; `handle_backend_log_sse` + `StreamLogStream`/`BackendLogManager` wiring in `tama_handlers/backend_logs.rs` and `proxy/types.rs` + `proxy/state/mod.rs` (the `log_stream`-related fields) + its re-export in `crates/tama-core/src/installations/log_stream.rs` only after the `job_log_panel.rs`/`self_update.rs` check clears them (audit said no PRODUCTION callers of `BackendLogStream::push`; if the job panel uses the TYPE for a different stream (job logs), KEEP log_stream.rs and delete only the backend-log wiring — verify and report which). `crates/tama/src/pages/logs.rs` is replaced in Task 5 (out of scope of THIS task). `docs/api/logs.md` full rewrite (new endpoints, sources vocab table, legacy-tail semantics, `logstore_status` on SSE appearing under the `/logs/events` section). `docs/api/sse.md`: add the `GET /tama/v1/logs/events` section (table of `LogStoreDegraded`/`LogStoreRestored`).
- `tama.log` rolling: in `main.rs` `init_tracing` (replace `SwappableFileWriter` usage or KEEP the swapper for the "config not loaded yet" window — decision: KEEP SwappableFileWriter (the before-Postgres-loads bootstrap window still exists; the doc comment at `main.rs:294-338` is the reason) and END its lifecycle with a `RollingFileAppender`-style ROLLING writer — danger: `tracing_appender::non_blocking` takes a `MakeWriter`; simplest correct change: swap the current single-file `appender::non_blocking(File::append)` for `RollingFileAppender::builder().rotation(Rotation::HOURLY).max_files(24).filename("tama.log")...` and DELETE the manual `MAX_LOG_SIZE`/`rotate` preboot block from `init_tracing` (`main.rs:401-408`) + `rotate`/`MAX_LOG_SIZE`/`open_log` in `logging.rs` (`open_log` is test-only anyway). Tests: the `rotate` behavior tests in `logging.rs` are deleted with the function.

**Files:**
- Create: `crates/tama-core/src/proxy/tama_handlers/logs_api.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/backend_logs.rs` (strip old `handle_all_logs` + whitespace — file may disappear entirely if ALL remaining contents move out; keep the `collect_tamad_log_sources` core as the single-source tail engine reused by the adapter — move it to the adapter or a small `tail_engine.rs` if cleaner)
- Modify: `crates/tama-core/src/proxy/tama_handlers/mod.rs` (exports)
- Modify: `crates/tama/src/router.rs` (route swap: remove the two old routes; add GET `logs`, `sources`, `summary`, `status`, `stream`, `export`, `events` (the SSE route) + DELETE `logs`)
- Modify: `crates/tama/src/web_types.rs` (`log_read: Option<Arc<std::sync::Mutex<LogStore>>>` + tail provider field; update ALL `WebState {...}` sites per Task 3's ripple note)
- Modify: `crates/tama/src/main.rs` (second read-conn open after config; legacy-tail provider construction; DROP manual rotate; rolling appender)
- Modify: `crates/tama/tests/router_ownership_test.rs` (`TAMA_MANAGED_PATHS` :78–80 lists the old log routes; `EXPECTED_TAMA_PATH_COUNT == 75` at :85; `WebState` literal at :127) — update path list + count + literal
- Modify: `crates/tama/tests/server_test.rs` (:503–540 exercises `GET /tama/v1/logs` JSON-not-HTML and `GET /tama/v1/logs/:backend/events` SSE — the SSE one must be re-pointed at `/tama/v1/logs/stream` or removed; the plain-GET one keeps passing if the new handler returns JSON)
- Modify: `crates/tama/src/api/openapi.rs` (:523–535 hand-maintained `/tama/v1/logs` doc entry — replace the single old entry with the new `logs*` operations, same style as the ~66 existing entries)
- Modify: `crates/tama-core/src/logging.rs` (KEEP `tail_lines`; DELETE `format_log_line`, `open_log`, `MAX_LOG_SIZE`, `rotate*` + their tests)
- Modify: `crates/tama-core/src/proxy/types.rs`, `crates/tama-core/src/proxy/state/mod.rs` (strip `BackendLogManager` field wiring)
- Delete: `crates/tama/src/api/logs.rs` (+ mod line in `crates/tama/src/api.rs:17`)
- Modify: `docs/api/logs.md` (rewrite), `docs/api/sse.md` (add section)

**Steps:**
- [ ] `rg -n "BackendLogStream|BackendLogManager|log_stream" crates/` and READ each hit (`job_log_panel.rs`, `self_update.rs`) — settle KEEP/DELETE per decision rule; write the conclusion into the commit message body.
- [ ] Baseline tests green; then FAILING handler tests in `logs_api.rs` tests mod (build a reader `LogStore` in a tempdir, prepopulate, serve via `axum::Router` with tower `oneshot`, per the pattern used by `system_tests.rs`/`api.rs` tests in this repo):
  - query: no filters → newest-first 200; `min_level=warn` excludes info; `source` exact and prefix (and NOT over-match); `q` FTS hit + FTS-miss LIKE fallback; `since/until` window; `cursor` walk covers all rows exactly once (no dupes, no misses); `limit` clamp 1000; invalid `q` length 500 → 400.
  - `sources` and `summary` shapes as documented. `GET /tama/v1/logs/status` is NEW in this task: `200 { …LogStoreStatus as JSON… }` — add the handler, the route (`/tama/v1/logs/status`), and the docs row here.
  - `stream` SSE: prepopulate id 1..5, `after=5`; insert more → client (axum::body::to_bytes isn't SSE-friendly; test via `utoipa`? no — use `reqwest`/`tower` to hit `/stream`, assert first bytes contain `"event: entry"` + at least two data lines with ids ascending; keepalive within 1.2 s. If SSE streaming test is too heavy here, verify via `handle_stream` unit test on the FRAME-generator function (extract `async fn log_stream_frames(store, after, cancel) -> impl Stream<Item=String>` and assert sequences — RECOMMENDED).
  - `export`: cap → 413 count-first; csv quoting test (message containing comma + quote).
  - `DELETE`: deletes rows; `compacted=true`; second call `deleted:0`.
  - Legacy adapter: fake `LogTailProvider` returns 3 lines → rows `legacy:true, id<0, level "info", level_known false`; TTL: second call within 5 s returns the same fetch ts (assert the provider was called exactly once); over-5-s re-fetch (the TTL uses `Instant::now` — make it injectable via the provider config `ttl_for_tests`, or use `tokio::time::pause` with the injected duration).
- [ ] Implement handlers + adapter + wiring (WebState fields; providers).
- [ ] Do the route swap + removals (with Step-1 decision); rewrite both doc files.
- [ ] Rolling-appender swap main.rs + `logging.rs` cleanup + delete stale tests.
- [ ] Green: `cargo nextest run --package tama-core -- logs_api` + `--package tama`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all`.
- [ ] Commit: `feat(logging): queryable /tama/v1/logs API, legacy tail adapter, dead code removal [tama]`

**Acceptance criteria:**
- [ ] Gone: `grep -rn "get_backend_logs\|handle_backend_log_sse" crates/` = empty (modulo docs); routes list in `router.rs` has NO `/logs/:backend` entries (verified by reading the file).
- [ ] New endpoint set: `logs`, `sources`, `summary`, `status`, `stream`, `export`, DELETE `logs` all present + each with a doc row.
- [ ] All handler tests pass; the over-match source test (`tamad:a` must NOT match `tamad:ab`) is explicit and green.
- [ ] `tama.log` rotation is hourly via the appender (code-verified; no `MAX_LOG_SIZE` in the repo).

---

### Task 5: Log UI rework (`/tama/logs` page) + deep-link + counters/eyebrows + SSE

**Context:**
The read API is live (Task 4). This task replaces the polling file-tail page with the queryable UI the spec describes. Project facts for orientation (verify each on touch): the page lives at `crates/tama/src/pages/logs.rs`; it currently uses `gloo_timers` 5-s polling + a `GET /tama/v1/logs` old endpoint + substring-matched CSS classes for error/warn/debug — ALL of that goes. The page gets: source picker, level chips, time presets, search box, a count eyebrow (from `/summary`), CSV export, and a store-degraded banner (from the new `/logs/events` SSE `LogStoreDegraded/Restored` plus a one-shot fetch of `/logs/status` on mount). Entries render from the DTO: time from `ts`, level badge from the ENUM field (never the string parse), source tag, message, and expandable `fields`. Live tail via `EventSource` on `/logs/stream?…&after=<max_id_seen>` with pause-on-scroll-up. Deep links: the models-page "open logs" button already targets `/tama/logs?source=…` with the OLD source naming (`{tamad-host}:{model}` per `docs/api/logs.md`); find ALL emitters (`rg "logs?source=" crates/tama/src`) and update them to the new vocab (`tamad:<host>:model:<name>`; plain host tail `tamad:<host>`; `proxy`). Note wasm build constraint: `crates/tama` compiles for wasm32 (the csr gate) — NO new deps, no blocking, all timers/SSE through web-sys `EventSource` (already enabled: check the `web-sys` feature list in the `crates/tama/Cargo.toml` manifest — `EventSource`, `EventSourceInit` are present).

**Decisions already made:**
- Page structure (single file OK, under ~600 lines; split into `components/log_page/` dir with `toolbar.rs`, `entries.rs`, `banner.rs` only if it grows past 600 — decide on measurement).
- URL state: `?source=<s>` (as today); `level`, `window`, `q` also URL-synced so bookmarkable (existing query-param convention — `rg "query param" crates/tama/src/utils"` or check the models page).
- Level chips: `all`, `debug+`, `info+`, `warn+`, `error` — maps to `level=` param (all → omit).
- Time presets: `15m`, `1h` (default), `24h`, `all` (omits `since`).
- Entry row: monospace-timestamp + level-class (`badge-level-error` etc. — reuse existing badge class naming from `06-badges-list-card.css` if the classes exist; add 4 new CSS rules if not); expandable `<details>` with the `fields` JSON pretty-printed (no external JSON viewer dep — manually JSON-formatted `serde_json::to_string_pretty` — BUT that's SERVER-side; client-side pretty print: the DTO already ships `fields` object; render as simple key/value `<dl>` — no JSON parse on client).
- Live tail: on batch arrival → append (respect `order=desc` UI default: PREPEND newest if newest-first); pause on scroll-up with a "live" pill indicator; resume on "jump to latest" click. Max in-buffer rows 2 000 (drop oldest with a small "…N older trimmed" row).
- Degraded banner: sticky top bar, red, text "log store degraded since {HH:MM:SS} — storing warn+ only", dismissible (session storage). One-shot `fetch('/tama/v1/logs/status')` on mount + SSE for transitions.
- CSS: extend `crates/tama/css/` — identify where log-page styles currently live (`rg -l "logs" crates/tama/css/`); ADD rules there (NEVER edit `crates/tama/dist/` — it's Trunk's output).
- REMOVE: the old polling + `tail_lines`-era UI code in `pages/logs.rs` (whole-file replacement), `format_log_line`'s UI twin (`crates/tama/src/utils/…` if a client-side formatter exists — `rg "reconstruct" crates/tama/src`).

**Files:**
- Rewrite: `crates/tama/src/pages/logs.rs`
- Modify: the "open logs" deep link emitters (models page + gateway dashboard if any — found via `rg`)
- Modify: one or two existing css files under `crates/tama/css/` (per `rg -l`)
- Modify: `crates/tama/src/pages/mod.rs`/route table only if prop names change (likely no)
- Test: the wasm UI has no runtime tests in CI (csr compiles are type-check only, per AGENTS.md) — therefore: compile gate + a scripted browser pass (every checklist step below is mandatory; do not skip silently); also, any new PURE helper functions (window `15m`→ms, DTO→row-model mapping, URL-param codec) get unit tests in a native module (visible from the ssr build — put shared helpers in `crates/tama/src/utils/log_page.rs` so the ssr-mode tests cover them).

**Steps:**
- [ ] `rg -n "logs?source=|pages/logs" crates/tama/src` → list of all emitters + the page module imports; snapshot the deep link format.
- [ ] Write FAILING ssr-side unit tests for the helpers (URL-param codec round-trips; window `15m/1h/24h` → correct `since` offset given an injectable `now_ms`; row-model flattening from DTO including `dropped`/`legacy` flags; scroll-buffer trim calculation).
- [ ] Run in ssr: `cargo nextest run --package tama -- log_page` → fails (helpers don't exist).
- [ ] Implement helpers + new page (server-side SSR render path is preserved — the page is rendered server-side for the first paint; keep EXISTING behavior for the initial SSR: same route `/tama/logs`; the CSR `app.rs`/route table mounts this component — do not restructure routing).
- [ ] `cargo fmt --all`; `cargo clippy --workspace --all-targets -- -D warnings` (catches csr-only issues too); `cargo check --package tama --no-default-features --features csr` (wasm-typecheck gate per AGENTS.md Step 4) → MUST pass.
- [ ] BROWSER PASS (do NOT skip — the CI gate does not run UI): build the app (`make dev` or `cargo run` ssr), with configured tamad + models (or none — cover both), verify THIS checklist: (1) the page loads with `proxy` source, entries render, newest first; (2) level chips filter server-side (use the network panel to confirm the `level` param arrives); (3) the source picker lists real sources from `/logs/sources` including tamad host rows when one is online; (4) time preset changes `since`; (5) search box does FTS (provocative: a word that appears in a `fields` value only — proves it matches the whole document not just the message); (6) live tail: execute an action that logs (e.g. trigger a pull, or a lifecycle op that logs) → row appears with no manual refresh; (7) scroll-up pauses live; jump-to-latest resumes; (8) export button downloads csv; (9) change `log_level` in the config editor → reflects immediately without a refresh (SSE stream updates in place — verify a new event arrives at the new level); (10) degraded banner: optional — only if a DB fault injection is feasible in dev, else SKIP with a note; (11) old `/logs/:backend` URL → now 404 / redirected (confirm the route is gone); (12) the models page open-log deep link lands in the page with the correct source preselected. Write the checklist results into the commit message body.
- [ ] Commit: `feat(logging): rebuild log page (filters, live tail, status banner) [tama]`

**Acceptance criteria:**
- [ ] `pages/logs.rs` no longer contains `gloo_timers` polling, the old endpoint string, or substring-level CSS matching (grep-proven).
- [ ] `cargo check --package tama --no-default-features --features csr` green (wasm type-check).
- [ ] SSR helpers tests green in ssr mode.
- [ ] Browser-pass checklist results are in the commit body (item 10 marked if skipped).

---

### Task 6: Stage 2a — tamad `StreamLogs` push (proto + tamad side)

**Context:**
Stage 2 lands structured logs from inference hosts. THIS task is the tamad half + the protocol: new proto messages and RPC (additive — old proxies and new tamads coexist), the tamad's own tracing output captured by a push layer, engine container log tails running continuously with RFC3339 timestamps, the in-flight/replay buffers with drop-oldest semantics, `(instance_id, seq)` numbering, and the registration-handshake capability flag `supports_stream_logs` so new tamads do not attempt push against old proxies. At the end of this task, a proxy that does not yet understand `StreamLogs` simply returns no flag (absent field → false) and nothing changes on the wire. Proxy-side ingestion is the NEXT task — a tamad pushing there meets `UNIMPLEMENTED`, which is treated as "no push", logged once, no retry storm (see decision below).

**Decisions already made:**
- Protocol (APPEND in `crates/tama-core/proto/tamad.proto`; do NOT renumber; `LogEntry` stays for legacy `rpc Logs`; new types):
  ```proto
  message LoggedLine {
    int64  ts = 1;               // unix ms, capture time (tamad clock)
    int32  level = 2;            // 0..4; -1 = unknown (engine line)
    string source = 3;           // "tamad" | "model:<model_name>"
    string message = 4;          // JSON doc: {"message":str, "target":str?, ...fields}
    int64  seq = 5;              // monotonic per source, per boot
    bool   dropped = 6;          // true ⇒ synthetic drop marker
    int64  dropped_count = 7;
    int64  dropped_since_ts = 8; // unix ms, meaningful when dropped
  }
  message StreamInit {
    string instance_id = 1;      // per-boot UUID v4 (NOT pid)
    map<string, int64> start_seq_by_source = 2;  // source → 0 (or the surviving ring's min-seq)
  }
  message StreamLogMessage { oneof kind { StreamInit init = 1; LoggedLine line = 2; } }
  // add to TamadService:
  rpc StreamLogs(Empty) returns (stream StreamLogMessage);
  ```
- Registration handshake: the flow is tamad `POST {proxy}/tama/v1/tamads` (tamad side: `register_once` in `crates/tamad/src/register.rs` ≈:55 → proxy side: `create_tamad` in `crates/tama/src/api/tamads/register.rs` ≈:32), which today returns `Json(TamadConnection)` — the raw DB-mapped type reused by GET/list. ADD a boolean `supports_stream_logs` to the register RESPONSE as a small wrapper struct (see the Files list) — new proxies always send `true`; old proxy + new tamad → field absent → treated as `false`. The proxy also handles a tamad that reports the flag but returns `UNIMPLEMENTED` on `StreamLogs`: treat as `false`, log once, no retries.
- Tamad side (new module `crates/tamad/src/push/`):
  - `push/layer.rs` — `PushLogLayer` (tracing `Layer`, same field-visit encoding as Task 2's layer but wired into per-source buffers): tamad's own events → a single source `"tamad"`; its own `msg` document is the same shape (`message/target/fields`); `level` = the real level.
  - `push/tails.rs` — per-running-model container tail supervisor. Container identity: `container_name_for(model_name)` at `crates/tamad/src/host_installs/docker/runner.rs:282` (the same helper the existing `Logs` RPC handler at `server.rs:385` uses); the "has a container" discriminator is `ProcessEntry.spec.docker_config_json` non-empty (native-host backends have NO tail — matches today). Process-table discovery: `ProcessTable` lives in `crates/tamad/src/process_table.rs` and exposes **no** watch/notify channel — so the supervisor POLLs `process_table.snapshot()` at 1 s (make this the primary described mechanism, not a channel hunt). For each container process: start a `docker logs -f -t <container>` child (add a `logs_follow_args` alongside `logs_tail_args` at `runner.rs:454-480`; the continuous variant gets `-t`; the legacy one-shot RPC path may share the flag — acceptable: cosmetic timestamp prefix on legacy tail lines, note it in the commit); read stdout line-by-line (leading RFC3339 from `-t` → `ts`, parse with `chrono::DateTime::parse_from_rfc3339`; malformed/absent prefix → capture-time for that line, `level_known: false`); `level = -1` (proxy maps to level 2 + `level_known: false`); `source = "model:<name>"`; `message = {"message": <line minus ts prefix>}`. Process-table change → start/stop the child; on child exit → drain remaining lines, stop. Multi-line stderr tracebacks: each physical line is its own entry; drop EMPTY lines (noise), keep all non-empty ones.
  - `push/ring.rs` — the two deques: in-flight `VecDeque` of cap 2 048 entries OR 1 MiB estimated-bytes (per-line `len()+64` estimate; if drops persist above 4 MiB/window, WARN with the source), **drop-oldest** on full + emit a `LoggedLine{dropped: true, dropped_count: N, dropped_since_ts, source, seq: next}` (the marker is IN-STREAM and ORDERED — one per overflow window, same 5 s throttle as the proxy side: track last-emitted-marker ts); replay ring `25_000` items / 10 MiB, GLOBAL FIFO across ALL sources (a flood must remain visible on replay), write-through from both feeds; on overflow there too drop-oldest + marker (same throttle).
  - `push/runtime.rs` — `LogPushRuntime` task: owns the stream open, the reconnect loop (mirror the shape of `run_stream_task` in `crates/tama-core/src/tamad/pool.rs:334-455`: reconnect, exponential backoff capped at 30 s; on `UNIMPLEMENTED` or flag-false → STOP pushing for this tamad's lifetime, log once), the sequence counters (per-source `AtomicI64`, all starting 0 each boot), `instance_id` = `uuid::Uuid::new_v4()` at process start, and the send loop: on (re)connect send `StreamInit` FIRST, then the ring oldest→newest, then live entries. Flow control: gRPC server-streaming offers no client-side backpressure knob beyond socket buffers — treat our bounded buffers as the flow control (no true backpressure; worst case the tail child's stdout pipe fills → `docker logs` blocks → tail stalls but TAMAD NEVER stalls — the child's pipe IS the buffer; if a child looks blocked > 30 s per a liveness check, kill + restart it once and WARN `tail_child_reattach`).
  - `main.rs` wiring: replace `tracing_subscriber::fmt::init()` (`crates/tamad/src/main.rs:103`) with fmt-stdout + `PushLogLayer`. At boot the handshake flag is UNKNOWN (registration is async), so: ALWAYS enable the layer + buffers (cheap, bounded) and `try_send`-drop-newest into the layer's mpsc; when the runtime is INACTIVE (old proxy) the mpsc simply cycles (nothing drains it) — bounded memory guaranteed; when ACTIVE the runtime consumes into the stream. Drop markers exist only once a push stream has been live for this boot (no spurious "drops" when push is disabled). No CLI flag needed: push is attempted iff `proxy_url` is configured; the gate is the runtime's handshake result.
- Process-table discovery (final answer — do not re-open): `ProcessTable` is `Arc<ProcessTable>` from `crates/tamad/src/process_table.rs` and exposes **no** watch/notify channel — the tail supervisor polls `process_table.snapshot()` at 1 s.

**Files:**
- Modify: `crates/tama-core/proto/tamad.proto` (add 3 messages + 1 rpc)
- Modify: `crates/tamad/src/server.rs` — `impl TamadService for TamadServiceImpl` (:168) MUST implement the new `stream_logs`: auth via `check_auth` exactly like `logs`/`stream_stats`, delegate to the `LogPushRuntime`; WITHOUT this crate `cargo check --workspace` fails at codegen (proto edit and both trait impls are ONE commit unit)
- Modify: `crates/tama-core/src/tamad/pool.rs` — `impl TamadService for StubTamad` (≈:642) gets a scripted `stream_logs` (Task 7's in-process ingest test will script the dedupe scenarios through it)
- Modify: `crates/tama/src/api/tamads/register.rs` (proxy side: `create_tamad` ≈:32 currently returns `Json(TamadConnection)` — the raw DB type, reused by GET/list — do NOT bolt the flag onto it; wrap in a small response struct `{ connection: TamadConnection, supports_stream_logs: bool /* always true from new proxies */ }` for the register endpoint; decide in-task whether GET/list get it too — if yes, same wrapper)
- Modify: `crates/tamad/src/register.rs` (tamad side: `register_once` ≈:52 currently DISCARDS the response body on success — make it parse the register response and propagate `supports_stream_logs` to the `LogPushRuntime` (e.g. a `watch::Sender<bool>` created in `main.rs` and injected at runtime construction))
- Modify: `Cargo.toml` (workspace) + `crates/tamad/Cargo.toml` — **add `chrono`** (promote to `[workspace.dependencies]` first as `chrono = { version = "0.4", features = ["serde"] }`, matching `tama-core`'s existing version pin; tamad does NOT have chrono today despite the plan's earlier assumption)
- Modify: `crates/tamad/src/main.rs` (subscriber construction; runtime start; no CLI flags — push is attempted iff `proxy_url` is configured, gated at runtime by the handshake result)
- Create: `crates/tamad/src/push/mod.rs`, `push/layer.rs`, `push/tails.rs`, `push/ring.rs`, `push/runtime.rs`
- Modify: `crates/tamad/src/host_installs/docker/runner.rs` (`logs_tail_args` + a new `logs_follow_args`)
- Modify: `crates/tamad/Cargo.toml` — no new deps expected (tonic, prost, uuid v4, chrono, tokio all there) — if something is missing (e.g. `chrono::DateTime` parsing feature), verify the manifest BEFORE adding.
- Test: `push/ring.rs` (pure logic: drop-oldest + marker emission ordering; ring overflow + marker; seq numbering across sources; replay ordering); `push/layer.rs` (tracing subscriber harness → mpsc → assert document shape); `tails.rs` line-parser (fixture bytes fed to the parser as if they were the child's stdout: RFC3339-prefixed line, continuation lines, empty-line drop, one >16 KiB line); `runtime.rs` (against an in-process tonic server stub that records the `StreamLogMessage` stream; tests: connect → `StreamInit` → ring replay → live; reconnect after server restart keeps the SAME `instance_id` (it is per-BOOT; document + test); `UNIMPLEMENTED` → stop; inactive mpsc cycles without unbounded growth).

**Steps:**
- [ ] Baseline: `cargo nextest run --package tamad` + `--package tama-core` green.
- [ ] Modify proto; `cargo check --workspace` (codegen via `tama-core/build.rs` regenerates).
- [ ] Write FAILING tests for ring / parser / runtime per the Test bullets above (an in-process tonic server stub under `#[cfg(test)]` in `push/runtime.rs` using `tokio::net::TcpListener` + `tonic::transport::Server` — reuse the pattern if the repo already has one: `rg "tonic::transport::Server" crates/`).
- [ ] Implement proto changes first, then `ring`, `layer`, `tails`, `runtime`, `runner.rs` args, `server.rs` `stream_logs` impl, `pool.rs` `StubTamad` impl, `main.rs` wiring, the register response wrapper + `register.rs` body parse, and the chrono dep — same commit.
- [ ] Tests green; `cargo fmt --all`; `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Manual smoke (DO, even briefly): run a real `tamad` (dev fixture host — per the repo's dev setup, `make dev` or the test host documented in `docs/` if present) pointed at a running proxy; observe in the proxy's logs: registration now carries `supports_stream_logs: true`; then a `UNIMPLEMENTED` handshake suppression log (proxy pre-Task-7). If you cannot run a host here, RUN the in-process runtime test as the smoke and say so in the commit body.
- [ ] Commit: `feat(logging): tamad StreamLogs push (protocol, buffers, rings, tails) [tamad]`

**Acceptance criteria:**
- [ ] Proto diff is APPEND-ONLY (new message + new rpc only; no field renumbering; `rg "stream LogEntry" proto` unchanged).
- [ ] Ring/parsers/runtime tests green including the UNIMPLEMENTED stop + replay-order tests.
- [ ] Tamad runs without proxy (`--no-proxy`-ish config, if such flag exists; ELSE unset `proxy_url`) and its buffers stay bounded (add a `#[cfg(test)]` assert: after N log events without runtime, memory channel len ≤ 1024).
- [ ] `cargo nextest run --workspace` green before commit.

---

### Task 7: Stage 2b — proxy `StreamLogs` ingest + dedupe + final validation gate

**Context:**
The tamads push (Task 6); THIS task makes the proxy INGEST. One long-lived `StreamLogs` task per online tamad (mirroring `run_stream_task` in `tama-core/src/tamad/pool.rs:334-455` — the reconnect/backoff/offline-state machinery is nearly identical), in-memory dedupe keyed on `(tamad, source) → { instance_id → last_seq }` (watermark advances when the record is ENQUEUED to the log channel, never on flush), classification into store `source` (`tamad:<host>` / `tamad:<host>:model:<name>` — `LoggedLine.source` is host-relative; add the host name), and mapping `level == -1 → LogstoreLevel::INFO` plus doc `level_known:false`. Everything then flows into the Task-3 shared channel indistinguishably from the proxy's own rows — no separate path. This task also runs the FULL validation gate and records the two doc cleanups (research-report decision note; sse/logs docs if Task 4 missed the event table).

**Decisions already made:**
- Dedupe module `tama-core/src/logstore/dedupe.rs` — PURE (no IO, no tokio):
  ```rust
  pub enum Decision { Fresh, Duplicate, NewInstance, OldInstanceReplay }
  pub struct InstanceState {
      current: (String /*instance_id*/, i64 /*last seq*/),
      seen_olds: HashSet<String>,
  }
  /// DedupState internals:
  ///   current-lines: Map<tamad_id, Map<source, InstanceState>>
  ///   expected:      Map<tamad_id, Map<source, String /*instance_id from latest StreamInit*/>>
  impl DedupState {
      pub fn on_init(&mut self, tamad: &str, source: &str, instance_id: &str)
      pub fn on_message(&mut self, tamad: &str, source: &str, instance_id: &str, seq: i64) -> Decision
  }
  /// RULES (on_message):
  /// 1. First contact for (tamad, source) → Fresh; record id as current, seq as last.
  /// 2. instance_id == current: seq <= last → Duplicate; seq > last → Fresh (last = seq).
  /// 3. instance_id == expected (from the latest StreamInit) AND != current
  ///    → NewInstance: current = (instance_id, seq); move the previous current id into seen_olds.
  /// 4. instance_id is neither {current, expected} nor in seen_olds
  ///    → OldInstanceReplay: accept THIS message only (do not update last); add id to seen_olds.
  ///    This is the "tail of the previous flight arrives late" case.
  /// 5. instance_id in seen_olds → Duplicate.
  /// On connection-lost: keep ALL state (no reset) — late replays are handled by rules 4/5.
  /// Memory: seen_olds grows at the tamad's reboot rate per host (single digits for years — no cap).
  ```
  Note: `StreamInit` ALWAYS precedes the lines of a (re)connected stream, so `expected` is populated before any line is judged — this is what keeps a genuine new boot (rule 3) from being misread as a late replay (rule 4).
- Ingest task `tama-core/src/tamad/stream_logs.rs` — `spawn_stream_logs_ingest(tamad_id, conn_ref, tx: mpsc::Sender<LogRecord>, dedupe: Arc<Mutex<DedupState>>, host_name: String)`; lifecycle hook: started when a connection comes up, stopped on offline (mirror the stats path — find the exact start/stop call sites in `pool.rs` near `run_stream_task` (~:334-455) and co-locate). On each `StreamInit` received: call `dedupe.on_init(tamad, source, instance_id)` for every source in `start_seq_by_source` (plus any source seen in live lines — defensive). Enqueue shape: `LogRecord { ts, level: mapped, source: Source::tamad_model(host, model) | Source::tamad(host), msg: parsed doc — proto `message` is a JSON doc string, `serde_json::from_str` with fallback `{"message": raw}` on parse failure; add flat top-level keys `instance_id`, `host`, `seq` (flat, not nested — friendlier to FTS; document in the type doc) }`.
- Shared channel: the ingest task's `tx` is CLONED from the same channel the proxy's Task-3 layer uses (`mpsc::Sender: Clone`). NOTE: there is **no task-options struct to extend** — `run_stream_task(db_pool, handle, backoff_base)` (`pool.rs:334`) takes loose params, and `TamadPool` is constructed inside `ProxyState::new` (`crates/tama-core/src/proxy/state/mod.rs:64`) with **40+ test call sites — do NOT change `ProxyState::new`'s signature**. Mechanism: add `log_tx: Arc<std::sync::Mutex<Option<mpsc::Sender<LogRecord>>>>` (set once at boot) to `TamadPool` in `pool.rs` with a `set_log_tx(&self, tx)` setter; `main.rs` (Task 3 created the channel) calls the setter after `ProxyState` construction; the stream-task startup path passes the `tx` (cloned) into `spawn_stream_logs_ingest`; `proxy/state/mod.rs` is otherwise untouched. `None` (not wired) defensively disables ingest.
- Docs: (a) `docs/research/tama-structured-logging-redesign.md` — ADD a "Decision Superseded" note under the §Q4 header: "Initial recommendation: Postgres. Superseded in design discussion in favor of embedded SQLite — see ADR-0013 (durability class drives storage choice). The SQLite-mode guidance in §Q3/§Q4 applies in full; the Postgres-specific retention details there are replaced by the plan's batched DELETE + `incremental_vacuum` pruner (codex#35823 note)." (b) verify `docs/api/sse.md` has the `/tama/v1/logs/events` table (added by Task 4 — fix here if missing).
- Validation gate (AGENTS.md, EXACT): 1 `cargo fmt --all --check` 2 `cargo clippy --workspace --all-targets -- -D warnings` 3 `cargo clippy --package tama --features ssr --all-targets -- -D warnings` 4 `cargo check --package tama --no-default-features --features csr` 5 `cargo nextest run --workspace`. ALL green before commit. WASM RUNTIME is not covered by any of them (AGENTS.md warns) — the log UI browser pass was done in Task 5; this task's browser pass: with a real tamad (dev host) — open the log page, filter by `source` = the tamad host, verify structured `tamad:` rows AND engine `model:` rows with real timestamps; if no host is available in the environment, RUN the in-process integration test (below) and document "host not available in env" in the commit body.

**Files:**
- Create: `crates/tama-core/src/logstore/dedupe.rs`
- Create: `crates/tama-core/src/tamad/stream_logs.rs`
- Modify: `crates/tama-core/src/tamad/mod.rs` (module export + `stream_logs` in the pool struct / the call sites in `pool.rs`)
- Modify: `crates/tama-core/src/tamad/pool.rs` (`TamadPool` gains `log_tx` field + setter; run/stop hook for the ingest task alongside the stream tasks)
- Modify: `crates/tama/src/main.rs` (create the channel; after `ProxyState` is built, call `proxy_state.tamad_pool().set_log_tx(tx.clone())`)
- Modify: `docs/research/tama-structured-logging-redesign.md` (decision note)
- Verify/modify: `docs/api/sse.md`
- Test: `logstore::dedupe` table tests (all decisions: rules 1–5, `on_init` before messages, multiple reboots, connection-lost continuation); `stream_logs` integration test with an in-process tonic fake that sends: `StreamInit`, normal seq, duplicate seq, new instance (init-announced), old-instance late replay, and `level = -1`; assert exactly the expected `LogRecord`s landed in a test channel (collect via mpsc); assert `host` added and `instance_id` present in the doc.

**Steps:**
- [ ] Baseline green (`--package tama-core`).
- [ ] FAILING: dedupe table tests (stub `DedupState` that only returns `Fresh` — confirm the table tests fail on duplicates) → implement → green.
- [ ] FAILING: integration test with the fake client (currently no module) → implement `stream_logs.rs` + pool hooks + main.rs clone → green.
- [ ] Level mapping unit test (`-1 → INFO + level_known:false`; `4 → ERROR`; others passthrough).
- [ ] Docs note + sse verification.
- [ ] FULL GATE (the 5 commands above) — fix all hits; this task may surface dead-code warnings from earlier cross-task wiring (e.g. a field only the pool sets) — clean them up properly (no `#[allow]` band-aids; if a field is genuinely dead, remove it).
- [ ] Browser pass (or document the in-process fallback per decision).
- [ ] Commit: `feat(logging): proxy StreamLogs ingest + dedupe; full validation [tama]`

**Acceptance criteria:**
- [ ] Dedupe table + integration tests green; `DedupState` has zero `tokio`/IO imports.
- [ ] End-to-end: real tamad (or in-process fake) → rows visible in store AND in UI with correct `source` taxonomy; duplicate seq produces NO duplicate rows (db count assert).
- [ ] 5/5 gate commands exit 0 (paste the command outputs into the commit body).
- [ ] Research doc carries the "Decision Superseded" note pointing at ADR-0013.

---

**Execution order & staging note:** Tasks 1→7 sequential. Tasks 1–5 = STAGE 1 (deployable independently: proxy-only store + UI + live filter; legacy-tail adapters keep remote visibility meanwhile). Task 6 = deployable before 7 (harmless: old proxy → flag absent → no push). Task 7 completes the loop. After 7: legacy cleanup is possible (roll the new tamad host by host; when zero legacy hosts remain, the `@tail` adapter + `rpc Logs` can be retired — OUT OF SCOPE for this plan; noted for follow-up).

**Follow-ups explicitly NOT in this plan** (from the design; leave as backlog entries): TLS on the tamad link; `no_store` redaction policy (product decision — hook is not even in the proto above; when adopted, add `repeated string no_store_sources` to `StreamInit`); engine multiline / stream separation (v2); partitioning past ~10M rows; FTS5 + `incremental_vacuum` steady-state verification at first deploy; write-path p99 latency measurement at real rates.
