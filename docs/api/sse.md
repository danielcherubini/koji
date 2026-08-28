# SSE Streams

All SSE endpoints return `text/event-stream`. Use the browser `EventSource` API or an equivalent library.

All endpoints send periodic keep-alive comment lines; clients that close the connection on a terminal event may ignore them.

## GET /tama/v1/downloads/events

Stream download lifecycle events.

| Event | Data |
|-------|------|

All event payloads are self-describing: the JSON object contains an `"event"` key equal to the SSE event name.

| Event | Data |
|-------|------|
| `Queued` | `{ event: "Queued", job_id, repo_id, filename }` |
| `Started` | `{ event: "Started", job_id, repo_id, filename, total_bytes }` |
| `Progress` | `{ event: "Progress", job_id, bytes_pulled, total_bytes }` |
| `Verifying` | `{ event: "Verifying", job_id, filename }` |
| `Completed` | `{ event: "Completed", job_id, filename, size_bytes, duration_ms }` |
| `Failed` | `{ event: "Failed", job_id, filename, error }` |
| `Cancelled` | `{ event: "Cancelled", job_id, filename }` |
| `Lagged` | `{ lagged: N }` (client fell behind) |

## GET /tama/v1/updates/events

Stream update check lifecycle events.

| Event | Data |
|-------|------|

All event payloads are self-describing: the JSON object contains an `"event"` key equal to the SSE event name.

| Event | Data |
|-------|------|
| `CheckStarted` | `{ event: "CheckStarted", item_type, item_id, variant }` |
| `CheckCompleted` | `{ event: "CheckCompleted", item_type, item_id, variant, dto }` |
| `CheckError` | `{ event: "CheckError", item_type, item_id, variant, error }` |
| `CheckSkipped` | `{ event: "CheckSkipped", item_type, reason }` |

## GET /tama/v1/self-update/events

Stream self-update progress events.

| Event | Data |
|-------|------|
| `log` | `{ line: "..." }` (progress message) |
| `status` | `{ type: "status", status: "succeeded" or "failed", ... }` |
| `restarting` | `{}` (process about to restart) |

## GET /tama/v1/backends/jobs/:id/events

Stream backend job events (install, update, restore).

| Event | Data |
|-------|------|
| `log` | `{ line: "..." }` |
| `status` | `{ status: "Running" or "Succeeded" or "Failed" }` |
| `result` | `{ results: "..." }` (benchmark results JSON) |
| `error` | `{ error: "..." }` |

On connect, the stream replays the log head/tail snapshot, then switches to live streaming. The stream closes on terminal status (`Succeeded`/`Failed`).

## GET /tama/v1/benchmarks/jobs/:id/events

Stream benchmark job events. Same format as backend job events above.

## GET /tama/v1/logs/stream

Live tail of the structured log store (plan-195). Polls 200 rows per second
and pushes every new row matching the filters; `after` anchors the stream to
an already-seen rowid (default `0`). Accepts the same `level` / `source` /
`q` / `since` / `until` filters as `GET /tama/v1/logs`. Full contract in
`docs/api/logs.md`.

| Event | Data |
|-------|------|
| `entry` | one `LogEntryDto` (compact single-line JSON) |
| `keepalive` | `{"keepalive": true}` (empty poll tick) |

## Log Store Events (GET /tama/v1/logs/events)

SSE of the structured-log **writer's** degraded / restored transitions —
the read endpoints and the drop-marker rows show the *results* of a backlog
building up; this signal makes the *event* visible (UI banner). Self-describing
JSON frames on a single `log_store` SSE event; keep-alive comment lines as
usual. No parameters.

| Event (`data.event`) | Data |
|-------|------|
| `log_store_degraded` | `{ event, since, channel_len, ring_len }` — fired when the writer starts dropping (`since` = unix ms the episode began; `channel_len`/`ring_len` = backlog sizes at that moment) |
| `log_store_restored` | `{ event, had_entries, ring_flushed }` — fired on recovery (`had_entries` = entries held when degraded; `ring_flushed` = whether the overflow ring drained fully) |
| `Lagged` | `{ Lagged: n }` (client fell behind) |

The drop markers written in-band (`dropped: true` rows, see
`docs/api/logs.md`) let search show exactly what the writer dropped after
recovery; the health snapshot at any moment is `GET /tama/v1/logs/status`.

## GET /tama/v1/system/metrics/stream

Stream proxy metrics snapshots (~2s cadence). Each event carries the legacy
local snapshot (CPU/RAM/network gauges + `current`) plus an additive `hosts`
array — one entry per registered tamad with its freshest stats snapshot:

```json
{
  "buckets": [ /* ~31 pre-aggregated 30s windows for the bar charts */ ],
  "current": {
    "cpu_usage_pct": 3.1,
    "ram_used_mib": 812,
    "ram_total_mib": 16384,
    "gpus": [],
    "models": [ /* actual model state — see below */ ]
  },
  "hosts": [
    {
      "tamad_id": "uuid",
      "name": "gpu-box",
      "online": true,
      "version": "1.4.2",
      "cpu_percent": 12.5,
      "memory": { "total_bytes": 17179869184, "used_bytes": 8589934592 },
      "gpus": [
        {
          "index": 0,
          "name": "NVIDIA RTX 4090",
          "driver_version": "560.35",
          "vram_total_bytes": 25769803776,
          "vram_used_bytes": 4294967296,
          "utilization_percent": 78.0,
          "temperature_c": 63.0,
          "power_w": 320.0
        }
      ]
    }
  ]
}
```

Field notes:

- `current.gpus` (legacy, proxy-local): **always empty** since the tamad
  split (plan-191) — the proxy samples no local hardware; per-host GPU
  data lives on `hosts[].gpus`.
- `hosts[]` (additive): the dashboard's **Hosts** section renders one card
  per entry — name, online badge, version, CPU%, RAM, and per-GPU
  VRAM/utilization/temperature. Built from the pool's latest stats snapshot
  (~1s fresh) for each registered tamad:
  - `online` — the tamad's stats stream is currently connected
  - `version` — the tamad's self-reported version from its last
    `HealthCheck` (cached by the pool; `null` until the first check)
  - `memory`, `gpus` — from the latest stats snapshot, zeroed/empty when
    the host has never streamed
  - zero-tamad deployments: `hosts: []`
- `current.models` — the **actual** model state (one entry per
  tamad-hosted backend process, with `model_name`, `provider_name`,
  `gpu_device`, `state` in `starting | ready | failed | unloading`),
  converged toward the **desired** state from the proxy database by the
  reconciler loop (see `models.md`, "Desired vs Actual Model State").
