# Downloads API

Monitor file download progress for model GGUF files.

## GET /tama/v1/downloads/active

List currently active download items.

**Response:**

```json
{
  "items": [
    {
      "job_id": "uuid-string",
      "repo_id": "bartowski/Llama-3.1-8B-Instruct-GGUF",
      "filename": "llama-3.1-8b-instruct-q4_k_m.gguf",
      "displayName": null,
      "status": "pulling",
      "bytes_pulled": 1500000000,
      "total_bytes": 4500000000,
      "error_message": null,
      "started_at": "2025-01-01T00:00:00Z",
      "completed_at": null,
      "queued_at": "2025-01-01T00:00:00Z",
      "kind": "model"
    }
  ]
}
```

## GET /tama/v1/downloads/history

List completed download history.

**Query params:**
- `limit` — Max items (default `50`)
- `offset` — Pagination offset (default `0`)

**Response:**

```json
{
  "items": [ /* same PullQueueItemDto shape as active */ ],
  "total": 150
}
```

## POST /tama/v1/downloads/:job_id/cancel

Cancel an active download.

**Response (200 OK):**

```json
{ "ok": true, "message": null }
```
