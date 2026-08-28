# Logs API

Read API over the structured log store (`tama-logs.db`, proxied from
tama-core). Every read endpoint accepts the same query filters — a
subset of the table below — and returns rows in the `LogEntryDto`
shape. All times are **unix milliseconds**.

Read endpoints return `503 ServiceUnavailableError` when the log store
is not wired (degraded runtime), and `500 ServerError` on store fail-
ures (logged server-side). Parameter errors return `400` with
`{"error": {"message": "...", "type": "ValidationError"}}`.

Sources:
- [Source vocabulary](#source-vocabulary)

## Source vocabulary

Log rows are indexed by a `source` label. These are the shape
producers, and the values the `?source=` filter accepts:

| Source | Meaning |
|--------|---------|
| `proxy` | Proxy-side lines (the file previously known as `tama.log`) |
| `backend:<name>` | One per backend runtime (proxy-side backend lifecycle) |
| `tamad:<host>` | Per-host tamad control lines |
| `tamad:<host>:model:<name>` | Per-model tamad lines (engine notifications) |
| `tamad:<host>:model:<name>:tail` | Trailing (post-load) tamad lines for a model |

`?source=` matches **exact or delimiter-aware prefix**: `tamad:gpu-box`
matches `tamad:gpu-box` and `tamad:gpu-box:model:x` but **not**
`tamad:gpu-boxer`. A single `source` param only (repeated `source` →
`400`). Unrecognized or blank sources are a filter that matches
nothing — they return `200` with empty entries, never a `400`.

## GET /tama/v1/logs

Query the structured log store (paged).

**Query params:**

| Param | Type | Description |
|-------|------|-------------|
| `level` | string | Minimum level: `trace` \| `debug` \| `info` \| `warn` \| `error` (invalid value → `400`) |
| `source` | string | Source label or delimiter-aware prefix (see vocabulary; repeat → `400`) |
| `q` | string | Full-text search, max **512** characters (longer → `400`) |
| `since` | int | Unix ms, inclusive |
| `until` | int | Unix ms, exclusive |
| `limit` | int | Page size, clamped to `1..=1000`, default `200` |
| `cursor` | int | Rowid cursor: `id < cursor` for `desc`, `id > cursor` for `asc` |
| `order` | string | `desc` (default, newest first) \| `asc` |

**Search (`q`) semantics:** a valid FTS5 phrase is matched with
`logs_fts MATCH` against the FTS index — which indexes the **whole
stored JSON document**, so a term also hits `fields` values and
`target`, not only the message text. If the FTS query matches nothing
or is syntactically malformed, the search transparently falls back to
`msg LIKE '%term%'` — also against the whole JSON document.

**Response (200):**

```json
{ "entries": [ /* LogEntryDto */ ], "next_cursor": 12345 }
```

`next_cursor` is `null` when the window is exhausted. Pass it back as
`?cursor=` to page.

**LogEntryDto:**

```json
{
  "id": 12345,
  "ts": 1766920000000,
  "level": "info",
  "source": "tamad:gpu-box:model:qwen--qwen3.8-27b-fp8",
  "message": "Model loaded in 12.3s",
  "fields": { "target": "tama_core::model", "gpu": "cuda:0" },
  "dropped": null,
  "dropped_count": null,
  "level_known": true,
  "legacy": null
}
```

| Field | Description |
|-------|-------------|
| `id` | Positive SQLite rowid for store rows. Legacy tail rows (below) use a **synthetic negative id**: `-(fetch_ts_ms * 1000 + line_ordinal)`, unique within a fetch, ordered by line ordinal, never colliding with a real id |
| `ts` | Unix ms (store rows: recorded time; tail rows: the **fetch** time — tails are unstructured text with no per-line timestamps) |
| `level` | `trace` \| `debug` \| `info` \| `warn` \| `error` (always `info` on tail rows) |
| `message` | Flattened from the stored JSON document |
| `fields` | The document minus the known keys (`message`, `dropped`, `dropped_count`, `dropped_since_ts`, `level_known`); `target` stays inside `fields` and is never used as the message |
| `dropped` / `dropped_count` | `null` on normal rows; on **drop-marker** rows `dropped: true` with the count of rows dropped since the previous flush (the writer degrades silently-to-marker when the disk write backs up) |
| `level_known` | `true` for store rows (level was known when recorded), `false` on legacy tail rows (level is a guess) |
| `legacy` | `null` for store rows; `true` on on-demand legacy tail rows |

### Legacy tail rows (`@tail`)

When `source` is `tamad:`-shaped **and the store has no rows for it
yet**, the query is answered by the on-demand **tail adapter** (last
200 lines, cached 5s per source) instead of an empty page:

- `tamad:<host>` / `tamad:<host>:model:<name>` /
  `…:model:<name>:tail` — engine-log tail over the tamad `Logs` RPC
  (offline/stalled/wedged tamads are skipped; a flaky host never fails
  the query),
- any other label (`proxy`, `backend:<name>`, or a bare name) — last
  200 lines of the matching `*.log` file in the resolved logs dir
  (`proxy` reads `tama.log`, `backend:<name>` reads `<name>.log`).

In tail-adapter mode `q` and `level` are **ignored** — the raw tail is
returned, mapped to legacy rows (`level_known: false`, `legacy: true`,
negative ids). Once the structured bridge covers the source, store
rows win and ordinary filtering applies. Legacy host tails do **not**
appear in `/sources` (they are on-demand, not rows).

**Errors:**
- `400 Bad Request` — Invalid `level` / `order`, non-integer `since`/`until`/`limit`/`cursor`, `q` longer than 512 chars, repeated `source`
- `413` — (not applicable here; see export)

## GET /tama/v1/logs/sources

Distinct sources that have rows in the store, with the newest
timestamp per source. Legacy host tails do NOT appear here.

**Response (200):**

```json
{ "sources": [ { "source": "proxy", "last_ts": 1766920000000 } ] }
```

## GET /tama/v1/logs/summary

Per-level row counts for the count eyebrow / level chips.

**Query params:**

| Param | Type | Description |
|-------|------|-------------|
| `since` | int | Unix ms, inclusive (default `0` = all time) |

**Response (200):**

```json
{ "counts": { "debug": 10, "info": 420, "warn": 3, "error": 1, "total": 434 } }
```

The four level keys are always present (zero-filled). `total` counts
every row, including `trace` rows, which get no key of their own.

## GET /tama/v1/logs/status

Writer health snapshot (`LogStoreStatus`): is the writer dropping rows,
and how much is in its backlog.

**Response (200):**

```json
{
  "degraded": false,
  "degraded_since": null,
  "channel_len": 0,
  "ring_len": 0,
  "dropped_count": 0,
  "retries_seen": 0,
  "last_prune_deleted": null
}
```

| Field | Description |
|-------|-------------|
| `degraded` | Writer is over its write budget and dropping/deferring rows |
| `degraded_since` | Unix ms the current degraded episode started (`null` when healthy) |
| `channel_len` | Rows buffered in the writer channel right now |
| `ring_len` | Rows held in the overflow ring right now |
| `dropped_count` | Rows dropped over the writer's lifetime |
| `retries_seen` | Write retries seen by the writer |
| `last_prune_deleted` | Rows deleted by the last retention prune (`null` = no prune has run; `0` = the last prune was a no-op) |

**Store events:** degraded/restored transitions are also published on
the `GET /tama/v1/logs/events` SSE stream (fields: `docs/api/sse.md`,
"Log Store Events"). The store persists drop markers in-band
(`dropped: true` rows), so search results show the gaps after
recovery.

## GET /tama/v1/logs/stream

Server-sent event stream over the store: every new row matching the
filters is pushed as it lands. Useful for the dashboard's live tail.

**Query params:** the shared filters (`level`, `source`, `q`, `since`,
`until`) plus:

| Param | Type | Description |
|-------|------|-------------|
| `after` | int | Anchor rowid: start from rows with `id > after` (default `0` = from the beginning, first page) |

The stream polls `200` rows per second; each emitted row is one
`LogEntryDto` (store rows only — no tail adapter). Keep the highest
`id` you've seen and pass it back as `after=` on reconnect.

| Event | Data |
|-------|------|
| `entry` | one `LogEntryDto` (compact single-line JSON) |
| `keepalive` | `{"keepalive": true}` (empty poll tick, project SSE keep-alive convention) |

**Errors:** `400` on invalid filter params (same rules as query).

## GET /tama/v1/logs/export

Export the current query window as CSV (blob download,
`Content-Disposition: attachment; filename="tama-logs.csv"`).

**Query params:** the shared filters plus:

| Param | Type | Description |
|-------|------|-------------|
| `format` | string | Must be `csv` (absent = default) |

**Response (200):** `text/csv; charset=utf-8`, RFC 4180 — header
`id,ts,level,source,message`, one row per entry (`msg`-only fields;
`fields` are not exported).

**Errors:**
- `400 Bad Request` — Invalid filter params, `format` other than `csv`
- `413 Payload Too Large` — The window exceeds the hard cap of
  **50,000** rows (checked by count *before* streaming). Narrow the
  window (`source` / `level` / `since` / `until` / `q`).

## DELETE /tama/v1/logs

Delete every row in the store (chunked delete + incremental vacuum,
so the disk is reclaimed in the same call).

**CSRF-enforced** — requires a valid CSRF token like every other
`/tama/v1` mutation.

**Response (202 Accepted):**

```json
{ "deleted": 434, "compacted": true }
```

`compacted` is always `true`: vacuum runs inside the deletion.
