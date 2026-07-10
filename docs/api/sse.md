# SSE Streams

All SSE endpoints return `text/event-stream`. Use the browser `EventSource` API or an equivalent library.

## GET /tama/v1/downloads/events

Stream download lifecycle events.

| Event | Data |
|-------|------|
| `Queued` | `{ job_id, repo_id, filename }` |
| `Started` | `{ job_id, repo_id, filename, total_bytes }` |
| `Progress` | `{ job_id, bytes_pulled, total_bytes }` |
| `Verifying` | `{ job_id, filename }` |
| `Completed` | `{ job_id, filename, size_bytes, duration_ms }` |
| `Failed` | `{ job_id, filename, error }` |
| `Cancelled` | `{ job_id, filename }` |
| `Lagged` | `{ lagged: N }` (client fell behind) |

## GET /tama/v1/updates/events

Stream update check lifecycle events.

| Event | Data |
|-------|------|
| `CheckStarted` | `{ item_type, item_id, variant }` |
| `CheckCompleted` | `{ item_type, item_id, variant, dto }` |
| `CheckError` | `{ item_type, item_id, variant, error }` |
| `CheckSkipped` | `{ item_type, reason }` |

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
