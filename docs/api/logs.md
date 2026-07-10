# Logs API

## GET /tama/logs

Return the last N lines of `tama.log`.

**Query params:**
- `lines` — Number of lines (default `200`)

**Response:**

```json
{ "lines": ["line 1", "line 2", ...] }
```

## GET /tama/v1/logs/:backend

Return logs for a specific backend.
