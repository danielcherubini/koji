# Tama Structured Logging Redesign — Research Report

**Date:** 2026-07-07
**Scope:** Redesign of Tama's logging: library & filtering evaluation, log persistence (document-store store option), remote tamad ingestion, and write-path sizing.

---

## Executive Summary

1. **No library change needed.** Tama already uses `tracing` + `tracing-subscriber`, which remains the uncontested Rust standard (0.2 never shipped; 0.1.x actively maintained). The pain is plumbing, not library.
2. **Current pain points are concrete:** log-level changes from the UI require a restart (the dynamic-filter handle is dropped after startup); the web UI polls whole-file tails every 5 s; the JSON log file's output is re-parsed and mostly discarded by a text-reconstruction function; structured fields are barely used; remote model logs arrive as raw unstructured Docker tail text; several pieces of log plumbing are dead.
3. **The "document store" intuition is correct and cheap.** Best practice converges on **indexed label columns + one JSON document per row** (the Loki model: labels indexed, payload unindexed).
4. **Store location: Postgres, not a second embedded SQLite file.** Postgres is already a *hard* startup dependency of Tama (unbounded wait on connect, config stored there, pool + migrations + retry logic exist). One migration + one writer task is the marginal cost; the only substantive SQLite advantage (surviving a PG outage) is narrow and is handled by a small fallback ring.
5. **Remote logs: push, not poll.** The codebase already runs exactly this pattern for host stats (tamad pushes `StreamStats` at ~1 s; the proxy holds one long-lived reconnecting stream per host). A new `StreamLogs` server-streaming RPC clones that machinery and collapses write-time fan-out from *O(hosts × models) RPCs per 5 s* to *1 stream per online host*. Existing `Logs` poll-tail is kept as a degraded/legacy path.
6. **Write path is fully sized** (see Q6): 200-row / 250 ms batches, 1 024-entry mpsc, drop-newest with throttled markers, per-boot-instance seq dedupe, and a warn+ fallback ring for PG-outages.

---

## Q1 — What's actually wrong with the current setup? (local audit)

Current architecture (`crates/tama/src/main.rs:263-424`):

- One global `Registry` with a **dynamic `EnvFilter` behind `reload::Handle`** and **two fmt layers**: plain stdout plus a `.json()` layer into a `SwappableFileWriter` wrapping `tracing_appender::non_blocking` (`main.rs:349-371`, `294-338`).
- **UI log-level changes are persisted but not applied live.** The reload handle is touched at startup and by `tama admin`, then **dropped** — `WebState` holds no filter handle, so changing `log_level` in the config editor only takes effect on restart (`main.rs:133`, `393-396`).
- **The JSON file's only consumer is a text reconstructor**: `format_log_line` (`crates/tama-core/src/logging.rs:67-99`) JSON-parses each line of `tama.log`, rebuilds a display string, and drops everything except the message (and `gpu`).
- **The UI does the worst kind of log I/O:** `tail_lines` reads the **entire file into a `Vec`** per request (`logging.rs:103-121`); limits 200 / 10,000 lines; the logs page **polls every 5 s** (`crates/tama/src/pages/logs.rs`); each poll also fans out gRPC container-tail RPCs (3 s per-source timeout) to *every online tamad*.
- **Structured fields barely exist:** only ~71 of ~530 `info!/warn!/error!` calls carry `key=value` fields, no spans, no `#[instrument]`.
- **Two log worlds:** `tamad` uses bare `tracing_subscriber::fmt::init()` (`crates/tamad/src/main.rs:103`) on remote hosts; its model-engine logs surface only as raw Docker container tail text via gRPC.
- **Dead plumbing:** the `/logs/:backend/events` SSE endpoint's stream has **zero production callers** (`BackendLogStream::push` vestigial); `GET /logs/:backend` 404s for everything under the v3 backend model (`api/logs.rs:42`); `logging::open_log` is test-only.
- Rotation: one-shot 10 MB rotation at open-time only; the runtime non-blocking writer never rotates.
- Volume: one `info!` per proxied request on the hot path (`request.rs:92`), plus periodic metrics ticks — few hundred events/s at peak, single-digit/s typical.

---

## Q2 — Which library & filtering approach? (2025–26 ecosystem)

**Stay on tracing** — facts:

- tracing 0.1.44 / tracing-subscriber 0.3.23 / tracing-appender 0.2.5 are current (2025-11→); 0.2 has never shipped (dormant branch, [Issue #3294](https://github.com/tokio-rs/tracing/issues/3294)).
- ⚠️ **Yank incident:** tracing 0.1.42 + tracing-subscriber 0.3.21 were yanked (`valueset!` macro breaking change, [#3382](https://github.com/tokio-rs/tracing/pull/3382) / [#3424](https://github.com/tokio-rs/tracing/pull/3424)); re-released as 0.1.43 / 0.3.22. If the lockfile pins the yanked pair, update.
- `tracing-log` bridges both directions (`log::Record` → tracing and vice versa); `log`-based deps keep working.
- No serious new competitor: `log` + sinks (no spans), `slog` (stagnant), OTel SDKs wrap tracing rather than replace it.
- **Filtering capabilities** (all already usable today):
  - `EnvFilter`: per-target (`proxy::llm=debug`), module-path (`tama::proxy=trace`), wildcards, **field-value** and **current-span-context** predicates.
  - **Runtime change:** canonical pattern is `tracing_subscriber::reload` + `handle.modify` ([docs](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/reload/)) — exactly what Tama built; the part missing is plumbing the handle to the API.
  - `add_directive()` on an already-installed shared filter has under-documented interest-caching semantics — use the reload-swap path (safe). No public `EnvFilter::from_directives` ([PR #2763 open](https://github.com/tokio-rs/tracing/pull/2763)) — parse directives, rebuild from the directive string.
  - **Per-layer filtering** (`Layered` + `with_filter`): the clean "stdout stays at info, store captures trace" fix; official examples added in tracing-appender 0.2.4/0.2.5.
  - `Sampling::rate(n)` for hot periodic tick paths.
- **Best practice currently violated by Tama:** emit first-class fields — **never pre-serialized JSON strings** (they double-encode, break `EnvFilter` field predicates, and break downstream querying). Treat `target: "tama::..."` strings as a soft ABI (stable across refactors, unlike module paths).
- `tracing-appender` 0.2.4/0.2.5 additions: weekly rotation, startup pruning, **latest-symlink** for `tail -f` consumers.

Sources: [docs.rs/tracing](https://docs.rs/tracing/latest/tracing/), [tokio-rs/tracing releases](https://github.com/tokio-rs/tracing/releases), [tracing_subscriber::reload docs](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/reload/), [EnvFilter docs](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html), [Filter trait docs](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Filter.html), [APIPark dynamic-level write-up](https://apipark.com/techblog/en/mastering-tracing-subscriber-dynamic-level-2/), [rustify.rs tracing vs log 2026](https://rustify.rs/articles/rust-tracing-vs-log-crates-2026).

---

## Q3 — Is a document store the right home for logs? (storage approaches)

**Yes.** Convergence across sources: normalise the few *queried/indexed* fields into columns; keep the full document for display/flexibility; EAV is the documented loser. At log scale, a document column wins writes and a handful of query columns make reads a wash — **the Loki model in one DB**: labels indexed, payload unindexed.

- SQLite document-store precedent: `json1` built-in since 3.38 ([sqlite.org/json1.html](https://sqlite.org/json1.html)); expression indexes on JSON ([sqlite.org/expridx.html](https://www3.sqlite.org/expridx.html)); virtual generated columns ([sqlite.org/gencol.html](https://www.sqlite.org/gencol.html)); FTS5 full-text ([sqlite.org/fts5.html](https://www.sqlite.org/fts5.html)).
- Rust-ecosystem pattern (one JSON per row + `json_extract` indexes): [Anderegg — SQLite for logging](https://ricardoanderegg.com/posts/sqlite-logging-profiling-programs/), [Terser Systems — structured logs + SQLite](https://terser-systems.com/blog/2023-03-04/ad-hoc-structured-log-analysis-with-sqlite-and-duckdb/).
- Throughput: WAL + `synchronous=NORMAL` + batched commit → ~33k inserts/s measured ([travishorn.com 2026](https://travishorn.com/a-hands-on-exploration-of-sqlite-for-production/)); the multiplier is commit batching. Tama's rate is far below.
- **Retention trap:** SQLite never returns freed pages without `VACUUM`/`incremental_vacuum` ([sqlite.org/lang_vacuum.html](https://www.sqlite.org/lang_vacuum.html)). Live production instance of this exact bug shape: [openai/codex#35823](https://github.com/openai/codex/issues/35823) (rolling-window log file, `auto_vacuum=INCREMENTAL` set but never called, file grew monotonically).
- Existing crate: `tracing-subscriber-sqlite` — WIP, blocking per-event writes (the part *not* worth copying). `sled` abandoned; `redb`/`lmdb`/`rocksdb` buy nothing over a SQL store at this scale.
- Retention model (adopted): **journald-style continuous multi-bound** — max_age AND max_rows AND max_bytes, enforced inside the writer as batched `DELETE … LIMIT n` ([journald.conf(5)](https://man.archlinux.org/man/journald.conf.5)).
- UI expectations from real log viewers (Grafana Logs in Explore, LiteLLM dashboard): time-range presets (15m/1h/24h), level chips (≥ debug), source filter, full-text box, newest-first pagination, live tail, CSV export — all but the tail map to one indexed scan.

---

## Q4 — SQLite vs Postgres (deep dive): **Postgres wins**

> **Decision superseded.** Initial recommendation: Postgres. Superseded in
> design discussion in favor of embedded SQLite — see ADR-0013 (durability
> class drives storage choice). The SQLite-mode guidance in §Q3/§Q4 applies
> in full; the Postgres-specific retention details there are replaced by
> the plan's batched DELETE + `incremental_vacuum` pruner (codex#35823 note).

**Local facts** (in-repo):

- Postgres is a **hard startup dependency**: `crates/tama/src/main.rs:102-107` → `create_pool` + `connect_with_retry` (unbounded exponential backoff; the process waits for PG forever). If PG is down, Tama is effectively already down today.
- sqlx 0.9 `postgres, tls-rustls` (`Cargo.toml:60`); embedded migrations `crates/tama-core/src/db/postgres.rs:14` (`sqlx::migrate!("./migrations")`, 6 files); pool max 10 + `connect_with_retry` backoff pattern (`crates/tama-core/src/db/pool.rs`). No `postgres/copy` feature — not needed at this volume.

**Schema:**

```sql
ts     timestamptz NOT NULL,
level  smallint    NOT NULL,   -- 0..4
source text        NOT NULL,   -- "proxy", "model:306", "tamad:gpu-box", …
msg    text        NOT NULL,   -- JSON document (see open decision: jsonb vs text)
id     bigserial PRIMARY KEY
```

- **B-tree on `(level, ts)`** covers "errors since X" and newest-first.
- **`pg_trgm` GIN on `msg`** for substring/typo-tolerant search, directly usable with `ILIKE '%…%'` ([Postgres GIN](https://www.postgresql.org/docs/current/gin.html)). Built-in `to_tsvector` FTS (stemming/ranking) is deferred — the UI doesn't need it.
- **Skip GIN-inside-jsonb**: big, expensive-to-update on write-heavy tables; nothing would justify it at Tama's rate.
- **MVCC/retention:** application-side batched `DELETE` (watermark by **`id < high-water-mark`** — monotonic, autovacuum-friendlier than ts) + per-table tuning `autovacuum_vacuum_scale_factor=0.05, autovacuum_vacuum_threshold=1000` (PG defaults are too lazy for high churn; [autovacuum docs](https://www.postgresql.org/docs/current/runtime-config-vacuum.html)). `PARTITION BY RANGE (to_month(ts))` later only if the table outgrows comfortable multi-month scans — `DROP PARTITION` is a metadata op with zero vacuum cost ([partitioning docs](https://www.postgresql.org/docs/current/ddl-partitioning.html)). `pg_partman` overkill.
- **Write path:** same custom `Layer` → bounded mpsc → single writer task, emitting **batched multi-row INSERTs (50–200 rows/round-trip) through the existing pool**. TLS on 127.0.0.1 negligible at this rate. sqlx has no built-in unbounded buffer/retry (errors surface after `acquire_timeout`) — writer does backoff reusing the existing `connect_with_retry` pattern.
- **The one honest weakness (and its right-sized fix):** if PG dies, the log store dies too. The same failure already blanks the *config*, and "PG down, but I want to see why" is a single narrow scenario → a **bounded in-memory warn+ fallback ring** (see Q6), not a full second store.

**Decision table:**

| Dimension | SQLite file | Postgres (chosen) |
|---|---|---|
| Setup | 2nd embedded DB + `sqlite` sqlx feature | 1 migration + 1 writer task, zero new features |
| Search | FTS5 | `pg_trgm` GIN + `(level, ts)` B-tree |
| Retention | DELETE + `incremental_vacuum` (codex#35823 trap) | batched DELETE + 2 per-table GUCs; partition-ready |
| Backup/ops | 2nd storage target, 2nd backup story | same `pg_dump` story as config |
| Failure modes | survives PG outage (narrow win) | pool-down → bounded in-memory fallback ring |

---

## Q5 — Remote (tamad-hosted) log ingestion: **push via `StreamLogs`**

**Existing gRPC surface** (`tama-core/proto/tamad.proto`; tonic/prost): 13 RPCs; three already server-streaming — `Logs` (tail), `StreamStats` (~1 s), `StreamJob`. No bidi. `LogEntry{timestamp, level, message}` **already exists but is filled with empty strings** (`crates/tamad/src/server.rs:433-438`). The `Logs` tail shells out to `docker logs --tail 200` per RPC, no timestamps (`crates/tamad/src/host_installs/docker/runner.rs:454-480`; Docker driver everywhere = CLI, not API).

**Why push is the clone, not the invention:** stats already work this way — tamad pushes; the proxy holds one long-lived stream per tamad in `run_stream_task` with reconnect + exponential backoff (cap 30 s) and latest-snapshot cache for the UI (`crates/tama-core/src/tamad/pool.rs:334-455`).

**Design:**

1. New `StreamLogs` server-streaming RPC — one long-lived stream per online host, same bearer-token `check_auth` as every other RPC (no new auth surface). One stream per host covers all its sources (tamad itself + every running model container) with a unified per-host seq.
2. Extend `LogEntry`: `+ int64 seq` (monotonic per source, within boot), `+ string source` (`"tamad"`, `"model:<id>"`), real `timestamp` (RFC3339), `level` nullable.
3. **Container lines:** add `-t` to the docker logs arg list (`runner.rs:454`) so each line carries RFC3339 (json-file driver stores `time` per line; `docker logs -t` prefixes it, [docker logs docs](https://docs.docker.com/reference/cli/docker/container/logs/)); `level = null` (engines don't emit structured levels); long-lived `docker logs -f -t` per running container, lifecycle tied to the process table; multi-line tracebacks accepted as own rows (v1 wart).
4. **tamad's own events:** replace bare `fmt::init()` (`tamad/src/main.rs:103`) with a `Layer` fanning to stdout + a bounded buffer feeding the stream.
5. **Reconnect:** ring-buffer replay (~5–10 min) + `(instance_id, seq)` dedupe at the proxy (see Q6).
6. **Proxy side:** stream entries go straight into the log batch writer; UI live-tail = query `WHERE ts > ?` (or existing SSE).

**Reconsidered:** per-tamad local structured stores + proxy pull-on-demand — moves the fan-out to read-time (same pain), plus N files to dedupe/GC. Keep the unary `Logs` poll-tail as legacy/degraded path (survives pre-upgrade tamads).

**Security flags:**

- Transport is **plaintext HTTP/2 + static bearer token** (constant-time check, `tamad/src/server.rs:73-90`; no TLS in `tamad/src/client.rs:40-56`). Structured log payloads widen what rides on it — TLS on the tamad link is a separate task worth filing.
- **Redaction is a product decision:** engine errors may contain HF tokens / API keys / prompt content that are now indexable. Hook: per-source `no-store`/redaction flag at the Layer — decide once now, saves purging later.
- Full-text queries must be parameterised trigram lookups (no string concat).
- Docker driver is **daemon-default, not pinned per container** — hosts running `syslog`/`journald` drivers flatten `docker logs`; v1 tolerates and marks such sources (daemon-wide default change out of scope). Note the >16 KiB split-line timestamp quirk ([docker/cli#4941](https://github.com/docker/cli/issues/4941)) is low-impact for line-buffered engines.

---

## Q6 — Write-path sizing (deep dive)

**Recommended values:**

| Parameter | Value | Basis |
|---|---|---|
| Batch | **200 rows OR 250 ms**, whichever first (256 KB byte guard) | count-OR-timeout converges across promtail (~100 lines/1 s/1 MB), Prometheus remote-write (2000 samples/send), Vector (`max_events: 1000`/1 s); multi-row INSERT at 50–200 rows within ~2× of COPY → no `postgres/copy` feature needed |
| Proxy channel | **1 024-entry `tokio::mpsc`** (~0.5 MB max) | `peak 500/s × (0.25 s normal + 0.75 s worst-slow flush) ≈ 500 → 1 024` (~2× headroom); absorbs ~5 s of sustained 500 ms PG flushes |
| Drop policy (proxy) | **drop-newest** on `try_send` full (recency is the diagnostic asset); counter + **one synthetic marker per ≥5 s**: `log store: dropped N events since <ts>` | mirrors promtail's drop-whole-batch direction; throttled markers prevent spam |
| Tamad in-flight buffer | **2 048 entries or 1 MB**, drop-oldest | ~100 s slack at ~20 lines/s; fits a ~1 500-line load storm |
| Tamad replay ring | **25 000 entries / 10 MB per host**, **global** FIFO (not per-source) | ~10 min at typical multi-model rates; a flooded model must stay visible on replay; scale to `max(25k, models×3k+10k)` only if hosts run ≥12 heavy models |
| Drop marker | Structured field, not prose: `dropped: true, dropped_count: N, dropped_since_ts` on a normal-shaped entry; participates in seq numbering | UI can filter/aggregate without parsing text |
| Seq & dedupe | Key = **`(instance_id, seq)`**; `instance_id` = **per-boot UUID** (PIDs recycle); tamad sends `{instance_id, sources: {source: start_seq}}` on (re)connect; proxy keeps in-memory map `(tamad, source) → {instance_id → last_seq}` | solves tamad-restart seq collision; i64 wrap = 3×10¹³ years, non-issue |
| Dedupe state | **In-memory only**, no PG table | crash loses at most one replay-cycle of dups/gaps (benign — no uniqueness invariant on `log` rows); a table adds TX + completion callbacks to the hot path and is itself unrecoverable during PG-down |
| Out-of-order ts | keep tamad capture-time `ts` as truth (never re-stamp); table has `id BIGSERIAL`; UI orders `ts DESC`, tie-break `id DESC` | replay tails can carry slightly-ahead timestamps |
| Watermark timing | record `(instance_id, seq)` **on enqueue, not on flush completion** | a failed flush never regresses the watermark |
| Writer loop | per-batch dynamic `format!`d multi-row `INSERT`, `sqlx::query(&str)`, no manual `prepare()`/plans/macros (dynamic arity), no `SELECT MAX(…)` post-batch; occupies 1 of 10 pool connections 2–20 ms typical / 500 ms worst; graceful shutdown via `CancellationToken` + drain-to-empty capped 2 s (WorkerGuard-style) | sqlx statement-cache churn at variable arity is negligible vs the 100–1000× bigger round-trip |
| Fallback ring (PG down) | **1 024 entries or 4 MB, warn+ lines only** (all `dropped:` markers always pass), FIFO, **drain-on-reconnect** through the normal flush path (sub-second), give up on timeout | what's at risk during a PG outage is the *errors that caused it*; info/debug overlaps already-drained data; ~5 s of worst-case warn rate |
| Status/telemetry | `logstore_channel_len`, `logstore_lag_ms`, `logstore_dropped_total`, `logstore_bytes_written_total`, `logstore_flushes_total` (+ rows histogram), `logstore_pg_down_since`, `logstore_fallback_len`; SSE `logstore_status` transition event `{degraded, pg_down_since, fallback_entries, total_dropped}` | feeds existing watch/SSE dashboard pattern |

**Behaviour by mode:**

- **Burst (300–500/s, PG healthy):** 200-row batches every 250–500 ms; channel peaks at a few hundred then drains; zero drops; UI lag ≈ 0.5 s.
- **PG slow (500 ms flushes):** writer single-stuffs; channel +150/flush; first drop after ~5 s sustained at peak → marker every 5 s; API keeps 9 of 10 pool connections; SSE flags degraded.
- **PG down:** flush error → warn+ ring (no re-enqueue = no unbounded growth); SSE degraded transition; recovery drains ring, normal ingest resumes; dedupe unaffected (watermark advanced on enqueue).

---

## Evidence & credibility

| Source | Type | Use |
|---|---|---|
| `crates/tama/src/main.rs:263-424`, `crates/tama-core/src/logging.rs:67-121`, `crates/tama/src/pages/logs.rs`, `docs/api/logs.md` | Local source (highest) | Q1 audit |
| `tama-core/proto/tamad.proto`, `crates/tamad/src/server.rs:385-457`, `crates/tama-core/src/tamad/pool.rs:334-455`, `crates/tamad/src/host_installs/docker/runner.rs:454-480`, `crates/tama-core/src/tamad/client.rs:40-80`, `crates/tama/src/main.rs:102-107`, `crates/tama-core/src/db/pool.rs`, `crates/tama-core/src/db/postgres.rs:14` | Local source (highest) | Q4, Q5 |
| tokio-rs/tracing releases, docs.rs (tracing, tracing-subscriber: reload, EnvFilter, Filter) | Official docs | Q2 |
| sqlite.org (json1, expridx, gencol, fts5, wal, lang_vacuum) | Official docs | Q3 |
| postgresql.org (GIN, autovacuum, partitioning, BRIN, TOAST) | Official docs | Q4 |
| promtail/Prometheus/Vector batching docs; prometheus/prometheus#12203 | Upstream docs/PRs | Q6 |
| travishorn.com SQLite production benchmark (2026), ricardoanderegg.com, tereser/tersersystems SQLite-logging posts | Engineering blogs (secondary) | Q3 |
| openai/codex#35823 (live retention bug), docker/cli#4941, docker docs (json-file, container logs) | Issues/docs | Q3, Q5 |
| Grafana Logs Explore, LiteLLM dashboard UI | Product UI reference | Q3 |
| pgsql-hackers "jsonb pessimal for TOAST compression" thread | List archive | Q4 |

Tracked: one researcher citation ("npmonitoring.com", "goldlapel.com", "adodgecode.com") looked low-confidence during the Q4/Q5/Q6 passes; the load-bearing claims were anchorable to official docs above where it mattered.

---

## Unresolved Contradictions

Design tension (not a factual conflict), resolved in the Q4/Q5 recommendations:

- **File-based consumption** (tracing-appender + latest-symlink, grep-able on disk — Q2/Q3 angle) **vs DB as the UI source of truth** (Q4). Resolution: keep the JSON file layer as the cheap crash-surviving debug artifact; make the Postgres table the queryable store for the web UI, replacing poll-tails with one indexed scan + SSE.
- **SQLite vs Postgres** (Q3 instinct vs Q4 analysis): Postgres chosen on operational grounds; SQLite advanage narrowed to PG-outage survivability, handled by the fallback ring.

---

## Gaps / What Remains Unknown

1. `pg_trgm` GIN index size at Tama's scale (expect a few hundred MB near the 10 M-row ceiling) — measure at first rollout.
2. End-to-end write-path bench (tracing → mpsc → PG + trgm insert trigger behaviour) at Tama's real rate — engineering estimate ~10–50× headroom; verify before publishing latency claims.
3. FTS/trgm index bloat under continuous retention deletes — no published incremental-stabilisation recipe; journald-style windows keep steady-state deletes small so expect manageable; verify empirically.
4. `EnvFilter` `add_directive` interest-caching subtleties remain under-documented by the tracing team — stay on the reload-swap path.
5. Open working decisions (Q6): `msg` as JSONB vs text (**lean: text**); one stream per host vs per-source (**lean: per-host**); replay-ring scaling law; batch-cap bump to 500 for WAN deployments if `logstore_lag_ms` p99 demands.
6. Redaction policy for secret-bearing sources — product decision, hook is designed (per-source `no-store`).
7. TLS on the plaintext proxy↔tamad gRPC link — pre-existing, worth a separate task (structured logs widen exposure).
8. Engine-level correlation (multi-line tracebacks, stderr stream separation) — deferred to v2; schema tolerates it via nullable fields.
