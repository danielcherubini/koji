# Logs API

## GET /tama/v1/logs

Return grouped logs from all configured sources (proxied from tama-core).

**Response:**

```json
{ "sources": [{ "name": "...", "lines": [...] }] }
```

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
