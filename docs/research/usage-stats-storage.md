# Research: Usage Stats for Tama (LiteLLM-style) + Storage Choice

**Date:** 2026-02-12
**Scope:** What to track (modeled on LiteLLM), where to hook in the Tama codebase, and the storage decision — SQLite vs Postgres vs Redis — including whether/how to plan a SQLite→Postgres migration.

---

## Executive Summary

LiteLLM's usage stats are: **a per-request spend log + daily rollup tables + a dashboard that reads only the rollups**. Tama already has most of the extraction plumbing — tokens, timings, and key identity are all parsed in the request path today (currently shipped only to Langfuse when enabled).

**Storage headline:** at Tama's volume (tens to thousands of requests/day), SQLite WAL is ~1000× over capacity. A full SQLite→Postgres migration is a large async-rewrite project (50 migrations re-authored, whole DB layer async). The clean option if Postgres is wanted: **stats-only in Postgres** (separate async pool, non-fatal writes), keeping SQLite for app state. Redis is not the tool for durable usage history.

**Recommended architecture:**

| Decision | Recommendation |
|---|---|
| What to track | Per request: prompt/completion/total tokens, latency ms, status, model, key_id (nullable → "anonymous"), backend/provider. Spend optional (local models are free; API backends need a pricing map — beware LiteLLM's $0-for-unknown-models footgun) |
| Schema | `usage_log` (append-only, indexed on `ts`) + `usage_daily` rollup upserted on `(date, key_id, model)`. Prune raw log after 30–90 days; keep rollups long-term |
| Writer | mpsc channel from request path → background batched writer (LiteLLM's pattern). Never synchronous writes in the axum handler. Add `PRAGMA busy_timeout` (currently unset) |
| Phase 1 storage | **SQLite, same `tama.db`** (new migration v50). Zero new infrastructure, single-file `VACUUM INTO` backup story intact, ~1000× headroom |
| Postgres | Defer as **Phase 2 opt-in**: same two tables in Postgres, separate async pool (`sqlx` PgPool with `connect_lazy` or lazy deadpool), fire-and-forget writes so a local proxy keeps working if the Postgres server is down |
| Redis | Skip — right for rate-limit counters, wrong for durable history; not needed at single-instance scale |
| Streaming usage | Make `include_usage` injection unconditional (currently langfuse-gated) + extract from the final chunk in `process_sse_line` (already parses every chunk) |
| UI | New `/tama/usage` Leptos page: daily token/spend chart, top models, per-key table, raw log table — served from rollups (LiteLLM's 1M-row lesson), SSE for live updates |

---

## Findings

### 1. What LiteLLM tracks (the model to ape)

**Per-request record** (`LiteLLM_SpendLogs` row) — [schema.prisma](https://github.com/BerriAI/litellm/blob/litellm_internal_staging/schema.prisma):
- Attribution: hashed API key, user, team_id, org_id, end_user, session_id, requester IP, tags
- Model: model, model_group (public alias), model_id, provider, api_base
- Usage: prompt_tokens, completion_tokens, total_tokens, spend (USD), cache hit/key
- Timing: start/end time + `request_duration_ms`
- Status: success/failure, call_type
- Opt-in payloads: messages, response, metadata

**Spend computation:**
- Pricing from community `model_prices_and_context_window.json`, fetched at startup with bundled fallback
- Formula: `prompt×input_rate + cached×cache_read_rate + cache_creation×cache_creation_rate + completion×output_rate` ([cost_calculator.py](https://github.com/BerriAI/litellm/blob/e15b37a1/litellm/cost_calculator.py))
- **Unknown model → $0** (documented budget-bypass footgun; opt-in `block_unknown_cost_models` fails closed, [PR #30715](https://github.com/BerriAI/litellm/pull/30715))
- Per-deployment custom pricing override; setting any field detaches from the default map

**Data model / rollups** ([db_info docs](https://docs.litellm.ai/docs/proxy/db_info)):
- `LiteLLM_SpendLogs` — one row per request, batch-written (default 10s)
- `LiteLLM_DailyUserSpend`, `DailyTeamSpend`, `DailyOrgSpend`, etc. — pre-aggregated daily rollups; **UI Usage views read these, not raw logs** — created after the UI broke at 1M+ spend-log rows ([PR #9538](https://github.com/BerriAI/litellm/pull/9538))
- Live per-key spend counters on the key row itself (what `/spend/keys` reads)
- Cost computed at request time, raw log + rollup increment in the same batched upsert ([PR #33810](https://github.com/BerriAI/litellm/pull/33810))

**Dashboard (what "aping LiteLLM" looks like):**
- Daily spend chart (date range), monthly spend total, top keys, top models, spend by provider, per-team tables, customer (end-user) usage, per-endpoint activity, per-transaction spend-logs table, model breakdown per key
- Backed by `/daily_metrics`-style aggregate endpoints + top-N endpoints

**Budgets/alerts (likely out of scope for v1):**
- Hierarchical hard caps (block) + soft budgets (alert-only), `budget_duration` resets, Slack/webhook/email alerts at 85%/95%
- Real-time enforcement via Redis pre-reservation of worst-case cost — only needed at high QPS

### 2. Tama codebase — current state and hook points

**Request flow** (all from local repo audit, `repo:path:line`):
- Single choke point for local backends: `forward_request()` — `crates/tama-core/src/proxy/forward/request.rs:21`
- Non-streaming: `usage` (prompt/completion/total tokens) **already parsed** at `request.rs:471` (langfuse `extract_usage`), same block as `extract_inference_stats` (`request.rs:459`)
- `AuthSubject` (`User { username } | Key { key_id, scopes }`) **already resolved** at `request.rs:180-192` — currently feeds langfuse user_id; the exact per-key attribution hook
- **GAP — streaming:** `stream_options.include_usage=true` is injected only when `langfuse_cfg.enabled` (`request.rs:232-242`). With langfuse off, upstreams are never asked for streaming usage. `process_sse_line` (`crates/tama-core/src/proxy/forward/sse.rs:10`) already parses every chunk for inference stats (`sse.rs:25`) but not usage
- **GAP — remote providers** bypass `forward_request` entirely (`handlers/forward.rs:103-126`, `chat.rs:60-95`); their responses already carry translated usage (`crates/tama-core/src/proxy/remote/anthropic.rs:158-177, 275`) but nothing records it
- **GAP — open mode:** with no auth configured, the middleware short-circuits with **no AuthSubject** (`crates/tama-core/src/proxy/auth/middleware.rs:42-44`) → needs an "anonymous/local" fallback identity

**Database:**
- 49 migrations, `PRAGMA user_version`-based runner (`crates/tama-core/src/db/migrations.rs`)
- No pool: background code opens a fresh connection per call (`crates/tama-core/src/proxy/types.rs:312`); management API uses one `Repository` behind `Arc<Mutex<>>`
- `PRAGMA journal_mode=WAL` on; **`PRAGMA busy_timeout` is NOT set anywhere** — latent multi-writer hazard once a second writer exists
- Existing insert+prune-in-one-transaction pattern to copy: `insert_system_metric` (`crates/tama-core/src/db/queries/metrics_queries.rs:59`), retention via `proxy.metrics_retention_secs`

**Existing metrics infra:**
- `MetricsState` with atomics (total/successful/failed requests — in-memory only, reset on restart) and `inference_stats: watch::Sender<HashMap>` (per-backend rates — no token counts)
- 2s collector persists one `system_metrics_history` row per tick (`crates/tama-core/src/proxy/server/metrics.rs:330-339`) and broadcasts snapshots
- SSE endpoint `/tama/v1/system/metrics/stream` already feeds the dashboard (`EventSource` at `crates/tama/src/pages/dashboard/mod.rs:230`); `bar_chart.rs` exists
- **No per-request persistence exists anywhere** — the only request-level data leaving the process is fire-and-forget Langfuse telemetry

**Frontend:**
- Leptos SSR; a Usage page = new route in `crates/tama/src/lib.rs` (routes at :322-332) + sidebar item (`components/sidebar.rs`) + `pages/usage/` mirroring `pages/keys/` structure

**Hook points (ranked):**
1. **Non-streaming:** record where `extract_usage` runs (`request.rs:459-480`) — usage + timings + identity all in hand
2. **Streaming:** make `include_usage` injection unconditional (`request.rs:232-242`), extract usage from the final chunk in `process_sse_line` (record only on the final chunk: `choices: []` + `usage` present)
3. **Remote:** parallel hook in `crates/tama-core/src/proxy/remote/`
4. All hooks → push an event into an mpsc channel; a single background task batches inserts + rollup upserts in one transaction (atomicity prevents rollup drift)

### 3. Storage options

**SQLite** — [Sesame Disk benchmarks](https://sesamedisk.com/sqlite-performance-tuning-high-throughput/), [marending.dev](https://marending.dev/notes/sqlite-benchmarks/):
- WAL + `synchronous=NORMAL`: ~33k–113k inserts/s. Tama at thousands of rows/day ≈ 3.6M rows/year ≈ tens of MB — ~3 orders of magnitude inside
- Single-writer discipline: one writer task + `busy_timeout` is the recommended pattern ([emschwartz.me](https://emschwartz.me/psa-your-sqlite-connection-pool-might-be-ruining-your-write-performance/))
- Known hazards at high sustained write: WAL bloat without checkpoints (124 GB incident, [hencf.org](https://hencf.org/blog/sqlite-124gb-wal-file)) — not a concern at this volume with bounded batches
- Keeps the single-file `VACUUM INTO` backup story and Windows installer simplicity (bundled rusqlite)

**Postgres:**
- Concurrency/MVCC: irrelevant for a single-process local proxy
- Better aggregations/partial/BRIN indexes: only matter at 50M+ row scale
- TimescaleDB: overkill (partitioning/compression for years of high-cardinality data)
- Postgres wins when: multi-instance access, external tools (Grafana/Metabase) query it, or the user wants long-lived stats owned by their server

**Industry patterns** (how tools actually do it):
- **LiteLLM:** raw log + daily rollups, batched 10s writes, Redis only as high-QPS buffer — [prod docs](https://docs.litellm.ai/docs/proxy/prod), [db_spend_update_writer.py](https://github.com/BerriAI/litellm/blob/main/litellm/proxy/db/db_spend_update_writer.py)
- **Helicone (cautionary tale):** synchronous per-request logging overwhelmed their DB in v1 ([V2 blog](https://www.helicone.ai/blog/introducing-helicone-v2)); Postgres→ClickHouse migration only at 3M requests/day ([ClickHouse blog](https://clickhouse.com/blog/helicones-migration-from-postgres-to-clickhouse-for-advanced-llm-monitoring))
- **Langfuse / OpenRouter / Portkey:** Postgres for app state, analytics store separate; retention tiers (30/365 days) as explicit config
- **Consensus:** append-only raw log + daily rollup keyed `(date, entity, model)`, cost computed at write time, async batched writer, explicit retention

**Redis:**
- Right for: rate-limit counters, short-window aggregations ([Redis docs](https://redis.io/docs/latest/develop/use-cases/rate-limiter/))
- Wrong for: durable per-request history (in-memory, evictable, no SQL)
- Verdict: skip; in-process atomics already cover Tama's counter needs

**Rollup design (mirror LiteLLM's proven shape):**
```sql
-- append-only, pruned after 30-90 days
CREATE TABLE usage_log (
  id INTEGER PRIMARY KEY,
  ts TEXT NOT NULL,              -- ISO-8601
  key_id INTEGER,                -- NULL = anonymous/local
  model TEXT NOT NULL,
  backend TEXT NOT NULL,
  prompt_tokens INTEGER,
  completion_tokens INTEGER,
  total_tokens INTEGER,
  latency_ms INTEGER,
  status TEXT NOT NULL           -- success | failed
);
CREATE INDEX idx_usage_log_ts ON usage_log(ts);

-- one row per (date, key, model), kept long-term (~365 rows/year per pair)
CREATE TABLE usage_daily (
  date TEXT NOT NULL,
  key_id INTEGER,
  model TEXT NOT NULL,
  requests INTEGER NOT NULL DEFAULT 0,
  prompt_tokens INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  total_latency_ms INTEGER NOT NULL DEFAULT 0,
  successful_requests INTEGER NOT NULL DEFAULT 0,
  failed_requests INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (date, key_id, model)  -- + ON CONFLICT upsert
);
```

### 4. SQLite → Postgres migration reality check

- **rusqlite cannot talk to Postgres** (wrapper over libsqlite3 only; [docs](https://docs.rs/rusqlite))
- **Full migration = async rewrite.** The closest real-world case study is Torrust Tracker (sync rusqlite+mysql → async sqlx multi-backend, [torrust.com](https://torrust.com)):
  1. Split DB layer into narrow per-concern traits
  2. Rewrite all drivers to async sqlx
  3. **Separate per-backend migration sets** (SQLite migration files don't run on Postgres)
  4. Driver factory + config variant, compatibility matrix via testcontainers
- 50 Tama migrations re-author: `?`→`$1`, `AUTOINCREMENT`→`IDENTITY`, singleton `id CHECK(id=1)` tables, `INSERT OR REPLACE`→`ON CONFLICT`, `REAL`/`TEXT` timestamp type decisions — plus pgloader gotchas (sequence bugs, INTEGER width, [pgloader issues](https://github.com/darold/pgloader/issues))
- **sqlx `Any` driver (runtime switching): avoid** — `query!` macros don't work with `Any` ([#964](https://github.com/launchbadge/sqlx/issues/964)), panics on `Numeric`/`Timestamptz`/`Uuid` types
- **Recommended Postgres pattern if/when wanted (stats-only split):**
  - Keep rusqlite for app state; add a **separate async Postgres pool** for usage tables only
  - `sqlx` `PgPool` with `connect_lazy` (or lazy deadpool — pool creation never fails even if server is down)
  - Writer is fire-and-forget: Postgres unreachable → drop/buffer events, **the local proxy must never die or block on the remote DB**
  - Unchecked `sqlx::query()` (no `DATABASE_URL` needed at build time)

**Options ranked:**
1. ⭐ **Stats in SQLite (Phase 1)** — zero new infrastructure, matches all existing patterns
2. **Stats-only in Postgres (Phase 2 opt-in)** — same two tables, separate async pool, non-fatal; the right answer if the goal is "my server owns the stats"
3. **Full async-sqlx multi-backend** — only if the goal is "all app data on Postgres"; largest effort, keep SQLite as default
4. **Redis** — skip for this purpose

---

## Unresolved Contradictions

None between research angles. One design tension worth noting: LiteLLM's "spend" semantics assume a hosted-API pricing world; Tama is a local-model-first proxy where most requests cost $0 in API fees but cost energy. Decide explicitly whether `spend` is a real currency column (needs per-backend pricing map) or just tokens+latency. Tama already has `compute_energy_cost` (electricity) — energy could be the Tama-flavored "cost" dimension.

## Gaps / Open Questions

1. **Spend semantics:** real USD (needs pricing map, LiteLLM-style $0 footgun) vs tokens/latency only vs energy cost?
2. **Langfuse overlap:** local stats as replacement, complement, or fed from the same extraction? (The `LangfuseTelemetry` struct could become the internal usage event.)
3. **Anonymous attribution:** open-mode requests need a fallback identity (e.g. `key_id NULL` = "local") — decide if that's fine for the UI
4. **Endpoint scope:** chat/completions only, or also embeddings/TTS/etc. (non-chat endpoints don't produce OpenAI-format `usage`)
5. **llama.cpp `timings.prompt_n` vs `usage.prompt_tokens` divergence** (cache-aware vs full prompt) — pick one source of truth for accounting (langfuse currently uses `usage`)
6. **sqlite aggregate query latency at real shape** unmeasured locally — expected sub-ms with rollups, worth a quick test once the table exists
7. **sqlx version-sensitive behaviors** (`Pool::connect` vs `connect_lazy` startup semantics) should be verified against a pinned version before Phase 2

## Primary Sources

- LiteLLM: [cost tracking docs](https://docs.litellm.ai/docs/proxy/cost_tracking) · [schema.prisma](https://github.com/BerriAI/litellm/blob/litellm_internal_staging/schema.prisma) · [db_info](https://docs.litellm.ai/docs/proxy/db_info) · [custom pricing](https://docs.litellm.ai/docs/proxy/custom_pricing) · [prod docs](https://docs.litellm.ai/docs/proxy/prod) · [usage.tsx](https://github.com/BerriAI/litellm/blob/a05a1eef/ui/litellm-dashboard/src/components/usage.tsx) · PRs [#9538](https://github.com/BerriAI/litellm/pull/9538), [#33810](https://github.com/BerriAI/litellm/pull/33810), [#30715](https://github.com/BerriAI/litellm/pull/30715), [#22066](https://github.com/BerriAI/litellm/pull/22066)
- Helicone: [V2 blog](https://www.helicone.ai/blog/introducing-helicone-v2) · [ClickHouse migration](https://clickhouse.com/blog/helicones-migration-from-postgres-to-clickhouse-for-advanced-llm-monitoring)
- Langfuse: [architecture handbook](https://langfuse.com/handbook/product-engineering/architecture)
- SQLite perf: [Sesame Disk](https://sesamedisk.com/sqlite-performance-tuning-high-throughput/) · [marending.dev](https://marending.dev/notes/sqlite-benchmarks/) · [emschwartz.me](https://emschwartz.me/psa-your-sqlite-connection-pool-might-be-ruining-your-write-performance/) · [WAL bloat case](https://hencf.org/blog/sqlite-124gb-wal-file)
- Postgres: [pgloader](https://pgloader.io) · [sqlx Any driver](https://docs.rs/sqlx/any) + issues [#964](https://github.com/launchbadge/sqlx/issues/964), [#3521](https://github.com/launchbadge/sqlx/issues/3521) · [Torrust Tracker case study](https://torrust.com) · [deadpool-postgres docs](https://docs.rs/deadpool-postgres)
- Tama local: `crates/tama-core/src/proxy/forward/request.rs` · `forward/sse.rs` · `forward/langfuse.rs` · `proxy/server/metrics.rs` · `db/queries/metrics_queries.rs:59` · `proxy/auth/middleware.rs` · `proxy/api_keys.rs` · `crates/tama/src/lib.rs`
