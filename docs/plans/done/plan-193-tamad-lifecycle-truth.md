# Plan 193 — Tamad is the Source of Truth for Lifecycle

**Goal.** Each host (tamad) is the single source of truth for: what is
desired; what is running; the last launch spec of each process (persisted on
host disk); the restart budget; the exhausted state. The proxy (tama) becomes
read / steer / route: it reads the 1 Hz ProcessInfo stream from the pool
cache, derives UI state and routing from the rows, and steers by calling the
existing LoadModel / UnloadModel RPCs.

**Wire.** No new RPC. `ProcessInfo` gains 3 fields — `desired`,
`restart_count`, `max_restarts`. `status` gains 2 values — `restarting`,
`budget_exhausted`.

**Proxy retirement.** The `registry.models` staging mirror is deleted. The
reconciler loop is deleted. The in-flight registry + proxy-side restart
counter map are deleted. The `desired_models` Postgres table (shadow) is
dropped.

**Fixed terms.**
- config key = the value the proxy puts in `LoadModelRequest.model_name` (a
  stored name that maps 1:1 to a backend). TTS / compaction get literal keys.
- desired = the per-file `desired` control field in the T1 store
  (`<data_dir>/state/<config_key>.json`) — not a separate file.
- the mirror (`registry.models`) dies: readers → rows (T4); writers → off
  (T5); table → off (T7).

## Plan-level RULES

Branch / rollout.
- Branch: `feature/tamad-lifecycle-truth`, off `main`.
- 7 tasks, one commit each (T5 is three — 5a/5b/5c); total 8 commits. Order
  T1→T7.
- Deploy = `update-tama` (one operator ships the proxy and the host together
  — paired build, no A/B fixture).
- The one cross-version case: new proxy reading a pre-deploy old host (no new
  fields). One T3 unit test pins the zero-default decode.

Gate (before every commit):
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- `cargo nextest run --workspace`

Regrep rule.
- Every number below is an approximate value as-of-now.
- Re-grep with `rg -n` before using anything.
- If a symbol moved, the text of the task holds; the location is overwritten
  by the grep.

Invariants.
- No new RPC. No feature flag. No new workspace members.
- The proxy never sends/receives host disk.
- The host never touches Postgres (ADR-0010).
- TTS and compaction are rows. No dedicated tables or columns, ever, in this
  plan (rule R1).
- Repairs must not be done without a commit + gate.

The canonical six (status values, in order):
starting, ready, restarting, failed, budget_exhausted, unloading.
- T2: the consts are defined host-side in one module (single home); the two
  new words join the 4 that exist on the wire today.
- T3: e2e asserts the observed wire set is ⊆ the canonical six.
- The 7th word / unknown-word parse arm and any further vocabulary are next
  plan's scope, not this one.

Deletion order (readers first, writers last):
- Readers flip — T4 (the mirror still runs; the proof is the e2e: host off →
  0).
- The mirror fns + field + `registry.models` — T5 (5c).
- The reconciler module + the spawn (+ its in-flight dedupe, which dies with
  the file) — T5 (5a).
- The non-spec `desired_models` steering writes (4 sites) — T5 (5b).
- The spec's `set_desired`/`clear_desired` shadow writes (2 sites) + the
  table + the query module — T7.

freshness (the wire rule — every reader applies it):
- a row counts only if `alive` && status ∈ {ready, starting, restarting}.
- frame freshness: age < `LIVE_FRAME_MAX_AGE` (5s; a pinned const in
  `state/rows.rs`, re-exposing the deleted reconciler's `SNAPSHOT_MAX_AGE`
  bound — 1 Hz emit ⇒ 5 ticks of slack, not a made-up value).
- Offline host → 0 rows ("no host = no models", not "models = stale").

LRU.
- The call stays in proxy. Inputs move to the rows (reads) + a per-key map
  (writes). The LRU comment in `spec.rs` (≈L581, the `evict_lru_if_needed`
  block) is rewritten (T5).

The two persistent facts (never merge):
- the store (host disk): "what I started".
- the `desired_models` table (Postgres): the proxy's shadow write "what I asked the host to
  hold" (T7 drops).

Do-not.
- Re-wording `failed` / any existing UI string is forbidden. T2 adds 2 words;
  it never replaces one.
- No new magic constants: only `RESTART_WINDOW_SECS=300`,
  `DEFAULT_MAX_RESTARTS=10`, `RETRY_AFTER=60` exist in this plan.
- Don't touch `active_models` without the zero-reader probe in T7.
- E2e = `[[test]]` blocks within existing packages. No new members.

Verified anchors (no `≈` unless noted, as of writing time):
- `LoadModelRequest` wire numbers 1..12 (provider, path, GPU variant, params
  map, model name, command, args, env, health URL, health-timeout-ms, GPU
  device, Docker config JSON).
- `ProcessInfo` is wire numbers 1..6; `status` = string.
- `SystemStats.processes` = 9. `provider_info.loaded_models` = 6.
- The two ProcessInfo build sites (host): `snapshot()` (`process_table.rs`
  ≈L111) and `list()` (`lifecycle.rs` ≈L413) — verify at copy time.
- The proxy's wire types are prost/tonic-generated from `tamad.proto`
  (re-exported via `tama-core/src/tamad/mod.rs`). There is NO handwritten
  decode surface; `crates/tama-core/src/tamad/protocol.rs` is a ~33-line
  pre-plan-191 shim that is not on the wire — do not touch it. Prost skips
  unknown fields, so an old frame decodes to `desired=false`,
  `restart_count=0`, `max_restarts=0` for free (the T3 backward-compat test
  pins this).
- `Registry.models`: `Arc<RwLock<HashMap<String, BackendState>>>` (keyed by
  the canonical config key, which the code labels `backend_name` — one
  identity, three spellings; the wire key `ProcessInfo.model_name` is the
  same string).
- Mirror writers (T5 kills): `sync_tamad_mirror` (state, ≈L274) +
  `remove_mirror_by_model` (state, ≈L245) + `insert_starting_mirror` (spec,
  ≈L535).
- `collect_model_state_snapshots` in `proxy/status.rs` (≈L107) — the model
  status source; its large existing test suite (≈L476+) is re-rooted on
  rows, not deleted.
- `ensure_model_loaded` ≈L34 (`proxy/lifecycle/mod.rs`). `evict_lru` ≈L82.
  LRU: `update_last_accessed` ≈L124 (`state/mod.rs`) + `registry.rs` ≈L44.
- The reconciler launch: `main.rs` ≈187 (SSR-gated via `[[bin]]
  required-features=["ssr"]`).
- Postgres migrations = `tama-core/migrations/` (14-digit zero-padded
  prefixes; 3 today; wired at `sqlx::migrate!`). `desired_models` (migration
  0000000000000002…) has columns model_name (PK), tamad_id (FK →
  tamad_registry(id)), loaded_at + 1 index; the FK is column-side (nothing
  references this table), so the DROP is trivial. `active_models` (initial
  migration) has no FK.
- The tamad CLI is self-parse (main.rs, `CliArgs` ≈L29); a `--data-dir` arm
  exists. (The proxy uses clap — a separate binary.)
- E2e = `[[test]]` blocks: `reconciler_e2e` (crates/tama/Cargo.toml ≈L83-85
  + `crates/tama/tests/reconciler_e2e.rs`) is in-process (no binary spawn;
  dies in T5); the tamad *installs* e2e spawns the real binary
  (`tamad_binary()` → target/debug/tamad under `CARGO_MANIFEST_DIR` +
  tempdir data-dir provisioning). T2's new e2e follows the installs pattern,
  not the reconciler's.

## Task 1 — Persistent store (tamad-side, on host disk)

What
- One JSON file per model: `<data_dir>/state/<config_key>.json`.
- Body: all 12 fields of `LoadModelRequest` + a 3-field control block:
  `desired` (bool, on-disk semantics = "keep"), `user_flagged` (bool, default
  false), `max_restarts` (u32, default from `DEFAULT_MAX_RESTARTS=10`).
- `updated_at_ms` is i64.

Rules
- Atomic write: write to a temp file in the same directory + rename. File
  mode 0600. Directory 0700.
- Parse failure: log + skip. You never interrupt boot.
- Key validation: reject `..` and paths starting with `/`.
- New CLI flags are not yet added (they land in T2).
- Filenames not visible to the proxy. The proxy never reads or writes files
  on the host.

Where
- New module `crates/tamad/src/state/store.rs`, a child module under the
  existing `state.rs` (edition-2021 allows `state.rs` + `state/`
  co-naming — add `mod store;` inside `state.rs`; no file move, no rename).
  Confirm the layout with `ls crates/tamad/src`.
- `TamadState::from_cli` (≈L51): create `<data_dir>/state/` right after
  token-file setup (state.rs ≈L93-95; it is the token *file*, not a dir).

Public surface (5 items, all with tests):
- `Store::new(data_dir)`
- `Store::insert(key, req, desired)`
- `Store::get(key)` → Option<&StoredProcess>
- `Store::list()` → Vec<StoredProcess>
- `Store::delete(key)`

Tests (9):
1 dir is 0700
2 file is 0600
3 round-trip on 12 fields
4 round-trip on empty/None
5 kill mid-write → no partial file
6 orphaned tmp file → overwritten
7 rejects `..` as key; rejects `/x` as key
8 corrupted JSON → skipped. No panic
9 list on empty dir = []

Accept: gate + `rg -c "impl Store" crates/tamad` = 1 and the 5 public fns
present.

## Task 2 — Respawn + restart budget + boot sweep (tamad side)

Constants (one module; the single home for the canonical six + the two new
words + the budget knob). Location: `crates/tamad/src/lifecycle.rs` (verify:
`ls crates/tamad/src`).
- 6 strings: `starting, ready, restarting, failed, budget_exhausted,
  unloading` — `const &str` + 1 `fn is_accepted(s: &str) -> bool` (the
  single validator; e2e asserts an observed wire set ⊆ the six).
- `RESTART_WINDOW_SECS = 300` (5 min, sliding).
- Max restarts: read from the T1 store's per-key `max_restarts`; absent →
  `DEFAULT_MAX_RESTARTS = 10` (verified: `default_max_restarts()` in
  `config/types/lifecycle.rs` ≈L38-41 returns 10).
- Counters live on the *process table's* `ProcessEntry` (3 new fields:
  `restart_count: u32`, `window_starts: Vec<i64>` unix-ms trimmed to the last
  300s, `user_flagged: bool` mirror of the on-disk one).

Respawn rule (the steady state).
- Trigger: the row dies (the existing dead-PID reaper, in `load()`, ≈L96-107,
  a `tokio::spawn` wait-task that today captures only `(table, model_name,
  pid)` and marks it `failed`). T2 rides it: when the key has a desired,
  un-flagged store row and the budget is not exceeded, the reaper's terminal
  arm = `respawn(key)` — the stored spec launches (the same 12 wire fields;
  no new field). The stale-spec behavior (a replay runs the stored spec) is
  unchanged.
- Implementation: within T2, `TamadLifecycle` gains `store: Arc<Store>` and
  the reaper closure captures `Arc<TamadLifecycle>` (T1 ships the store; the
  field + closure change belong to T2's commit so the dead-PID arm can see
  the store). The arm reads the store row, and the budget gate below.
- Window over-limit: `restart_count` within the 300s window reaches `max` →
  status `budget_exhausted` + store `user_flagged=true` + no auto-respawn
  after.
- Success resets: a successful load zeroes `restart_count` and clears the
  window.

Boot sweep (the entry).
- The flag: `--no-replay-desired` (the hand-parser, main.rs, ~L29, the arm
  after `data_dir`). Default OFF = the sweep is ON. This is the only
  operational switch in the plan (a deploy-safety valve; not a feature flag —
  it has no proxy-side toggle, so the "no feature flag" invariant is
  unchanged).
- Order: after `TamadState::from_cli` holds the store; before the gRPC listen
  starts. Bounded-parallel: at most 2 loads in flight at once (a serial loop
  would chain 30–300 s health-poll latencies; a 2-cap is safe because the
  sweep and the reaper each fire a given key at most once).
- Per file: skip `user_flagged`; skip a model that is already `alive` under
  the same key (row wins over file); a failing entry is logged and left
  desired (boot never fails because one model fails).

Store-write sites (from the server handlers).
- `load_model` success → `store.insert(key, req, desired = true)` — the host
  records its own desired on the load path.
- `unload_model` success (≈L244) → `store.delete(key)` + row gone.
- No `desired` bit is added to `LoadModelRequest` (desire is host-side).

Tests (6, in-file `#[cfg(test)]`):
1 window-trim: inserts, +301s → count is 1 not 4
2 budget-trip: max=2, 2 fails → `budget_exhausted` + flag on disk
3 success-reset: fail, fail (max=3 count=2), success → count 0
4 sweep (cap-2): 2 desired unflagged → 2 parallel spawns; 1 flagged → skip;
   1 already-alive under its key → skip ⇒ 2 new spawns total
5 `--no-replay-desired` → sweep is a no-op (1 log line)
6 double-issue guard: a key already alive under the same row during the sweep
   → exactly 1 process (row wins; no double spawn)

- e2e file (the plan's first e2e; this task opens it):
  `crates/tama/tests/tamad_boot_replay.rs` + a `[[test]]` block (mirroring the
  reconciler's ≈L83-85, ssr required). It must spawn the real tamad binary
  (template `tamad_installs_e2e.rs`: `CARGO_MANIFEST_DIR`-relative
  target/debug/tamad + tempdir data-dir provisioning — that provisioning is
  test setup, not "hosting host files"). T3/T4/T6 add asserts; T7 does not.
  Accept: gate + `cargo nextest run --package tamad`.

- Cited audit (Round 2, Commit-B): pre-policy durability of the reaper — trip persists
  `user_flagged` + `persisted_restart_count` in ONE write; a verified-healthy reset zeroes
  the tally; the boot sweep refuses any row whose persisted tally has reached its cap.

## Task 3 — Wire extension (3 fields on ProcessInfo, 2 status values)

Proto (append-only; the old is untouched):
- `bool desired = 7;`
- `uint32 restart_count = 8;`
- `uint32 max_restarts = 9;`
  (plain scalars per the ADR wording; 7/8/9 are the numbers).
- `status` stays a string — the two words that join the existing 4:
  `restarting`, `budget_exhausted`. The canonical six are pinned once (T2);
  T3 asserts them; e2e only observes the emitted set ⊆ the six.
- No new RPC. No new field on any other message.

Write-side (tamad). The single builder.
- Site A: `process_table::snapshot()` (≈L111) — row → wire.
- Site B: `lifecycle::list()` (≈L413) — list → wire.
- T3 folds both sites into one free fn in `lifecycle.rs` (the T2 constants'
  home):
  `fn to_process_info(entry: &ProcessEntry, store_row: Option<&StoredProcess>)
  -> ProcessInfo`
  with `restart_count`, `max`, `desired` sourced from the ProcessEntry fields
  (T2) + the store. `ProcessTable::snapshot` stays table-pure; the caller
  (server.rs `stream_stats`) applies `to_process_info` with the one
  `Arc<Store>` carried by `TamadState`. Verify both call-sites with
  `rg "ProcessInfo" crates/tamad`; a 3rd builder is killed here (we don't
  create a third write). The `stats.rs` test literal gains the 3 fields.

Read-side (proxy). Mechanical.
- prost codegen picks up the 3 fields on next build; the generated
  `ProcessInfo` gains `desired: false, restart_count: 0, max_restarts: 0`
  defaults. An old frame decodes to those zeros for free (prost skips unknown
  fields).
- `protocol.rs` (the pre-191 handrolled shim) is NOT the wire — untouched.
- The reconciler's row type stays alive until T5 (5a); T3 gives the mirror no
  new fields — it only reads row.status today, which is unchanged.

Backward-compat test (the plan's one cross-version case):
- a `ProcessInfo` proto with the *old* shape (fields 1..6, no 7-9) → decodes
  to `desired=false`, `restart_count=0`, `max_restarts=0`. One unit test:
  `old_frame_decodes`.
- the inverse (a new shape read by an *old* binary) — prost skips the unknown
  fields, so it is inert (no test).

Test: one bridge-test in the shared e2e (T2's file): a loaded model, on the
`StreamStats` path, reports `desired = true` and a `restart_count = 0`.

Do-not:
- no renumber.
- no new message.
- no TTS/compaction special case as a table/column (they flow as rows and get
  a `desired` too — the forbidden special-case is a dedicated table, column,
  or merge).
- no dropping a value for a legacy keyword: the canonical six are validated
  by `is_accepted` (T2); the 7th word's parse arm is next plan's scope.

## Task 4 — Wire + freshness + rows (read-side flip)

The mirror is host facts with freshness; T4 makes the proxy read the rows.
This is "flip the readers".

Where
- One new file: `crates/tama-core/src/proxy/state/rows.rs`.
- One constant: `LIVE_FRAME_MAX_AGE: Duration = Duration::from_secs(5)`
  (private in `rows.rs`), with a comment: 1 Hz emit → 5 ticks of slack; this
  re-exposes the reconciler's `SNAPSHOT_MAX_AGE` bound (the reconciler dies
  in T5; the constant is re-exported here so `tama-core` keeps it). Not a
  made-up 500 ms threshold.

The surface (5 fns — in-file tests):
```
async fn live(pool: &TamadPool) -> Rows    // aggregates per-handle
impl Rows {
    fn row(&self, key) -> Option<ModelRow>
    fn online(&self, key) -> bool
    fn ready_count(&self) -> u64
    fn all(&self) -> &[ModelRow]           // the admin `status` verb walks this
}
```
- `live()` aggregates each handle's `latest_fresh(LIVE_FRAME_MAX_AGE)` (a
  `TamadHandle` already exposes `latest_fresh(max_age: Duration)`, pool.rs
  ≈L89).
- A row (eligibility) = `alive` && status ∈ {ready, starting, restarting} &&
  age < `LIVE_FRAME_MAX_AGE`. Offline host → 0 rows.

`ModelRow` fields = what the readers need: key (canonical config key),
status, alive, `endpoint` (wire field 5 — routing and the `/v1/models` fetch
require it), `last_seen_ms`, + the 3 wire fields (`desired`, `restart_count`,
`max_restarts`).

Flip — an owned, enumerated set of switches. Gate identity (run at flip-time;
type the actual count into the commit message):
```
rg -n 'models\.read\(\)|\.models\.get\(|get_model_state|backend_url|models_loaded' \
    crates/tama-core/src crates/tama/src -g '!*test*'
```
(each hit: either flip to a row, or an enumerated survivor below).
Main sites:
- routing / forward: `proxy/forward/request.rs` + `handlers/forward.rs` +
  `handlers/helpers.rs` — `backend_url` from the row.
- dashboard / status: `proxy/status.rs` `collect_model_state_snapshots` +
  `handlers/status.rs` + `server/metrics.rs` (≈302-305, which already
  computes `filter(Ready).count()`).
- system JSON: `tama_handlers/system.rs` + the `models_loaded` sample
  (server/metrics.rs). The `models_loaded` body → `rows.ready_count()` (this
  is the "*current* ready count" semantics switch, not a rename — the wire
  name stays).
- management list/detail: `tama_handlers/models/handlers.rs` (state from
  `collect_model_state_snapshots`, already a row via this task; the detail's
  `desired` from `get_desired` ≈L226 flips to the row's `desired` field in
  T7).
- OpenAI `/v1/models` fetch: `proxy/handlers/models.rs`.
- fast-path: `ensure_model_loaded` + `unload` + spec.rs's TTS/compaction
  (≈716 / ≈730 — `load_tts_on_tamad` / `load_compaction_on_tamad`).
- LRU: `update_last_accessed` → flips its callers to rows; the LRU map itself
  is untouched (dies with the T5 mirror). (`update_last_accessed` lives at
  `state/mod.rs` ≈L124 and `registry.rs` ≈L34.)
- rename.rs — audit: if it reads the mirror, flip it; if it writes, it dies
  in T5.

NOT flipped in T4 (alive until T5): the bodies of `insert_starting_mirror` /
`sync_tamad_mirror` / `remove_mirror_by_model`, the internal selection loop of
`evict_lru_if_needed`, and the 2 `remove_mirror_by_model` handler call-sites
(≈271 cancel, ≈331 unload — T5(5b) re-wires these to rows/no-ops).

e2e (in the T2 file, 3 asserts):
```
offline host       → rows = 0 (wire; mirror = 0 too)
frame old ≥5s      → rows = 0 (unit: fake LatestStats.at; e2e: stop the
                     stub host, wait ~6s)
load a model       → desired=true, restart_count=0 (the T3 bridge)
```
Accept:
- Gate 4 commands.
- the flip-rg identity: 0 outside the enumerated survivors (survivors are
  listed in the commit message).
- e2e shows both sources agree (row count == mirror online count, an
  arbitrary 0/1 assert).
- No DB change. No new flag. No 503 (that is T5).
- Commit: the actual count from the `rg` (the gate).

## Task 5 — Delete the mirror + the reconciler + the queue

Three consecutive commits (each gated; the whole is one task):
- (5a) reconciler death: `crates/tama/src/reconciler.rs` + the spawn in main
  (≈187) + the `[[test]]` block + `tests/reconciler_e2e.rs` + the reconciler's
  own `list_desired` call (reconciler.rs ≈251, dies with the file). After
  5a, the reconciler's mirror/desired readers are zero.
- (5b) the 4 non-spec `desired_models` steering writes: `lifecycle/mod.rs`
  ≈185 (evict-clear), `idle_timeout.rs` ≈97, `tama_handlers/models/
  handlers.rs` ≈260 (cancel) + ≈320 (unload); plus re-wire the 2
  `remove_mirror_by_model` handler call-sites (≈271, ≈331) to rows/no-ops.
- (5c) mirror death: the 3 mirror fns + all call sites (per the T4 rg
  identity, which is now complete), the `registry.models` field + accessors
  (callers already zero), the `BackendState` type, the two one-way
  `fetch_add`s (`models_loaded` at spec.rs ≈685, `models_unloaded` at
  `lifecycle/mod.rs` ≈254, both `AtomicU64` at `proxy/types.rs` ≈210-211;
  there is no `fetch_sub` anywhere), and the `remove_active_model` statements
  (state ≈263/371, `lifecycle/mod.rs` ≈248 — they live inside
  `unload_model`/`remove_mirror_by_model`, not a "mirror fn"). Rewrite the
  LRU comment (spec.rs ≈L581).

The ONE new surface (503):
- In `ensure_model_loaded` (proxy, ≈L34), when the row is
  `budget_exhausted` → the 503 + `retry-after: 60`.
- The string: "the model exhausted its restarts; retry in 60 seconds."
- One unit test within T5 (ensure_model_loaded is unit-callable: set the row
  to budget_exhausted, call, assert the 503 + the header). T6's e2e keeps
  only the success path (row → ready).

Do-not (T5):
- No Postgres (that is T7).
- No `active_models` (the T7 zero-reader probe decides).
- No `desired_models` (the table + spec writes + query module drop in T7;
  the 4 non-spec writers → 5b).

Gate:
- `rg -n "sync_tamad_mirror|remove_mirror_by_model|insert_starting_mirror|BackendState|InFlightMap|LoadTask|drain_in_flight|reconciler" crates/ -g '!*test*'` = 0 outside this plan file.
- `rg -n "(models_loaded|models_unloaded)" crates/ -g '!*test*'` = only the probe/then-.ready_count() gauge + JSON wire surfaces (no `fetch_add` call sites; `fetch_sub` never existed). The pull-queue `in_flight_pulls` (proxy/state/pull.rs, an unrelated subsystem) is excluded from the former.

## Task 6 — `tama admin` + the `models_loaded` semantics switch

The admin (the proxy's side = clap, verified):
- `cli.rs` command: the existing variants (the `#[derive(Subcommand)]
  pub enum Command` at ≈L27-28, currently `Migrate`). Add `admin`.
- Verbs (exactly these 4):
  - `status`            — all rows, one line of JSON (T4's
    `live()`/`Rows::all`).
  - `load <config_key>` — via the existing `ensure_model_loaded` path.
    Idempotent (already-alive row ⇒ no second LoadModel).
  - `unload <config_key>` — via the existing unload path.
  - `logs <config_key>`  — a tail of `TamadHandle::logs` (the pool gRPC,
    reuse). Help text notes: container-engine only (the wire `Logs` tails the
    `tama-<key>` container; native engines are not captured in this plan).
- Do-not add: `flag` / re-arm (a wire flag path is next plan's scope; e.g. a
  `FlagModel` RPC or a `LoadModelRequest` flag bit); `--hard` kill (out of
  scope); a read `desired <key>` verb (the row already shows host truth — no
  new verb).
- Exit codes: 0 ok / 2 not-found / 13 budget-exhausted (a CLI literal
  matching the wire word `budget_exhausted`, T3; there is no "code" on the
  wire).
- No new RPC. The 3 verbs call the existing handlers (T2/T4/T5).
- No UI change (SSR unchanged).

The semantics switch (this is the whole wire-facing part; T4/T5 enforce, T6
tests):
- Keep the name `models_loaded` (gauge `tama:models_loaded` + the JSON field
  are wire contracts). Source is now `Rows::ready_count()` — the current
  ready count. The legacy two one-way `fetch_add`s (spec ≈685, `lifecycle/
  mod.rs` ≈254, fields `types.rs` ≈210-211; no `fetch_sub`) are dead at (5c).
- One T6 unit: load a model ⇒ 1; unload ⇒ 0; assert nothing cumulative was
  incremented.

Verify-only (the TTS):
- `rg` the TTS / compaction key → confirm they load through the 2 load sites
  (no third builder; the row is the same lane).

e2e asserts (in the T2 shared file, 3 more):
```
admin load  → row = ready (via the wire)
the TTS key → loads (provider health; this plan's first TTS e2e)
wire status ⊆ canonical six (observed once at T3; this just carries)
```
Accept: gate + the 4 admin verbs work (e2e) + the e2e asserts green +
no-new-verb check.

## Task 7 — Drop the shadow (Postgres) + finish the wiring

Step 1 — prove the readers are off (gate, function-name based):
- `rg -n "set_desired|clear_desired|list_desired|get_desired|clear_desired_for_tamad|desired_queries|desired_models" crates/ -g '!*test*'` —
  after T5's (5b), only the spec shadow writes + the query module + the
  `clear_desired_for_tamad` call-site (`tama/src/api/tamads/manage.rs` ≈92,
  the tamad-delete path) remain.

Step 2 — kill the 2 spec shadow writers:
- spec.rs ≈L640: `set_desired` call → delete.
- spec.rs ≈L783: `clear_desired` call → delete.
- `clear_desired_for_tamad` (manage.rs ≈92) → delete in the same commit (its
  module goes; the host store now owns desire — no DB copy survives).
- (The 4 non-spec writers already died in T5's 5b — confirm zero by the gate
  below.)

Step 2.5 — `active_models` writers (the whole surviving set after 5c):
- `insert_active_model` (spec.rs ≈670) → delete.
- `remove_active_model` at `proxy/forward/request.rs` ≈576 (connect-failure
  cleanup; not one of the 5c mirror-fn sites) → delete.
- `rename_installation` / `rename_active` (installation_queries.rs ≈668,
  proxy/rename.rs ≈89) → delete the `active_models` UPDATE.
- the migrate command's `active_models` entries (tama/src/migrate.rs
  ≈93/98/108) → drop.
- the models/manager.rs dead wrappers (insert/remove/get/rename ≈220-256) →
  delete.
- The zero-reader probe MUST be literal: a Step 1b that enumerates readers via
  `rg -n "active_models" crates/ -g '!*test*'` and confirms the only hits are
  these writer sites + the migration file.

Step 3 — delete the module:
- `desired_queries.rs` (5 fns) → delete.
- `active_model_queries.rs` (spec.rs ≈670 + the Step 2.5 sites) → delete after
  Step 2.5 empties it.

Step 4 — migration: the two drops (next two free 14-digit numbers, verify at
copy time; both `DROP`s only, no data):
- FIRST PROBE: `SELECT count(*)` on both tables — DIAGNOSTIC ONLY: the
  zero-rows invariant stays *the assertion*, but a non-zero count now merely
  RAISEs NOTICE with the count (log the survivors) — it never blocks and
  never skips.
- drop `desired_models` — UNCONDITIONAL.
- drop `active_models` — UNCONDITIONAL. (A sqlx migration row that
  notice + RETURNs is marked `success` anyway: a "log + skip, next cycle"
  promise can never be retried — a one-shot skip leaves the table alive
  forever. The no-steering premise holds by construction (T5b/T7 removed
  the steering), and the drop must land after the pre-plan-193 proxy was
  retired (rollout step ordering).)
- Both drops are trivial — no *other* table references either (desired_models
  has a column-side FK to tamad_registry, which is fine to drop with it; no
  FK points back into either table). `drop table` on both is safe.

Step 5 — the detail handler's `desired` flip:
- `tama_handlers/models/handlers.rs` ≈L226 `get_desired` → replace with the
  wire row's `desired` (via `Rows::row(key)`). The JSON field is unchanged;
  only the source swaps. This removes the last DB read of model desire.

Step 6 — the ADR path: the ADR's consequence line is an illustration
(`<state_dir>/models/…`); T1's concrete `<data_dir>/state/…` supersedes it.
No ADR edit in this plan (the ADR asserts the whole conclusion; a plan does
not rewrite it).

Gate:
- Gate 4 commands.
- `rg` from Step 1 = 0 (migration filenames + this plan file only).
- Both migrations apply on a fresh DB.
- e2e: the list + detail read state/desire from the row (no new JSON fields;
  T7 adds no new e2e — behavior is pinned by T4/T6).

## Rollout (the operator's ladder — per deploy step)

- S1: T1 store in. Zero behavior change.
- S2: T2 + T3 — same deploy (wire fields are additive; an old host's frame
  decodes to zeros via prost, so the cross-version case is inert; the T3
  backward-compat unit pins it). Gates: the T3 unit + the T2 e2e.
- S3: T4 flip.
- S4: T5 death.
- S5: T6 admin + semantics switch.
- S6: T7 gates.

## Five artifacts (built)
- `crates/tamad/src/state/store.rs` (T1, host disk).
- `crates/tama-core/src/proxy/state/rows.rs` (T4).
- The three wire fields (T3).
- Migrations 4 and 5 (T7; 5 is probe-gated).
- `tama admin` (T6).

## Exceptions (intentional — don't 'fix' them mid-branch)
- `active_models` probe may go non-zero → the T7 gate emits the
  `desired_models` drop only; the `active_models` drop moves to the next
  cycle (not drift — the probe decides).
- Admin `flag`/re-arm needs a wire flag path → next plan's scope (excluded
  here).
- The store is proxy-invisible (ADR-0010): the proxy never touches host disk.
- A 7th status word / unknown-word parse arm → next plan's scope (excluded).
- T7 step-4 deviation: the step previously promised "log + skip; note in
  the commit" / next-cycle deferral for a non-zero pre-drop probe. A sqlx
  migration is one-shot (a notice + RETURN row is marked `success`, so a
  skipped drop never re-runs) — fix applied: probe = zero-rows invariant
  assertion (diagnostic, NOTICE-only), drop = unconditional. Supersedes the
  "moves to the next cycle" exception above.

## Gate — Deploy acceptance

1. The 4-command gate (rules), green.
2. `rg -n "registry\.models|mirror"` = 0 outside this plan.
3. `rg -n "desired_models" crates/` = the migration filenames + this plan
   only (0 code sites).
4. `rg -n "InFlightMap|LoadTask|drain_in_flight" crates/` = 0 (T5 kills the
   reconciler dedupe).
5. The 6 wire statuses = exactly 6; the canonical pin (T2) is the assertion;
   appended observation set ⊆ the six.
6. The e2e trio: (a) the rows asserts (T4); (b) the boot replay (T2);
   (c) the admin success path (T6) + the 503 unit (T5).
7. `cargo metadata --no-deps` workspaces at T7: same member set (reconciler
   e2e was never a package, only a `[[test]]` block); `[[test]]` count =
   base −1 (reconciler) +1 (boot replay).
8. `models_loaded` keeps its wire name; its value is the current ready count;
   `rg "(models_loaded|models_unloaded)" crates/ -g '!*test*'` = only the
   `Rows::ready_count()` gauge + JSON wire surfaces (no `fetch_add` sites).
9. The `models_loaded` JSON + gauge source = `Rows::ready_count()` (semantics
   switch, no rename).
