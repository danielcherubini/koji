# Logs API

## GET /tama/v1/logs

Return grouped logs from all configured sources (proxied from tama-core).

**Sources:**

- `tama` — the proxy's own `tama.log`
- `{backend}_{name}` — each loaded backend's local log file (proxy-spawned
  backends only)
- `{tamad-host}:{model}` — engine-log tails for **tamad-hosted models**.
  Those models' backends run on the remote tamad host; the proxy asks each
  online tamad for the last lines of the `tama-<model>` Docker container
  the model's engine runs in (e.g. `gpu-box:qwen--qwen3.8-27b-fp8`).
  Native host backends on a tamad write no container logs, so they produce
  no source. Tamads that are offline, unreachable, or time out are skipped
  silently — a flaky host never fails this endpoint. The dashboard's
  per-model “Open logs” link (📄, on active-model rows) deep-links here via
  `?source={tamad-host}:{model}`.

**Response:**

```json
{ "sources": [{ "name": "...", "lines": [...] }] }
```

Source names are the exact values for the `?source=` query param on the
`/tama/logs` page. No new routes; the response shape is unchanged.

## GET /tama/v1/logs/:backend

Return the last N lines of a specific backend's log file.

**Query params:**
- `lines` — Number of lines (default `200`, max `10000`)

**Response:**

```json
{ "lines": ["line 1", "line 2", ...] }
```

**Errors:**
- `400 Bad Request` — Invalid backend name (must be alphanumeric/`_`/`-`, max 64 chars)
- `404 Not Found` — No logs found for backend
