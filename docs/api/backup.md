# Backup & Restore API

Create and restore full configuration backups (config directory, model cards, SQLite database).

---

## GET /tama/v1/backup

Create a `backup.tar.gz` archive of the config directory and return it as a file download.

**Response:** `application/gzip` file with `Content-Disposition: attachment; filename="backup.tar.gz"`.

**Errors:**
- `500 Internal Server Error` — Archive cannot be created (e.g., config dir not accessible, I/O failure). Body is the nested error shape `{"error":{"message":"...", "type":"ServerError"}}`.

---

## POST /tama/v1/restore/preview

Upload a backup archive and return a manifest preview without applying changes.

**Request:** `multipart/form-data` with the `backup.tar.gz` file.

The uploaded file is stored under `<config_dir>/uploads/<upload_id>.tar.gz` until consumed by a subsequent restore call (or the process restarts).

**Response (200 OK):**

```json
{
  "upload_id": "uuid-string",
  "created_at": "2025-01-01T00:00:00Z",
  "tama_version": "1.26.2",
  "models": [
    {
      "repo_id": "bartowski/Llama-3.1-8B-Instruct-GGUF",
      "quants": ["Q4_K_M", "Q8_0"],
      "total_size_bytes": 9000000000
    }
  ],
  "backends": [
    {
      "name": "llama_cpp",
      "version": "b5900",
      "backend_type": "llama_cpp",
      "source": "prebuilt"
    }
  ]
}
```

**Errors:**
- `400 Bad Request` — No file uploaded, upload too large, or manifest extraction fails. Body: `{"error":{"message":"...", "type":"ValidationError"}}`.
- `500 Internal Server Error` — Failed to create temp directory or write upload file.

---

## POST /tama/v1/restore

Start a restore job from a previously uploaded backup.

**Request body:**

```json
{
  "upload_id": "uuid-string",
  "selected_models": ["Q4_K_M", "Q8_0"],
  "skip_backends": false,
  "skip_models": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `upload_id` | string | **Required.** From the preview response. |
| `selected_models` | string[] | Accepted for forward compatibility; restore currently always performs the full additive merge (local data wins). |
| `skip_backends` | bool | Accepted for forward compatibility; restore currently always performs the full additive merge. |
| `skip_models` | bool | Accepted for forward compatibility; restore currently performs the full additive merge. |

**Success response (200 OK):**

```json
{ "job_id": "j_<uuid>" }
```

The uploaded archive is deleted after the job finishes, on success or failure.

**Errors:**
- `404 Not Found` — Upload not found or expired (unknown/expired `upload_id`). Body: `{"error":{"message":"Upload not found or expired", "type":"NotFoundError"}}`.
- `409 Conflict` — Another restore job is already running. Body: `{"error":{"message":"another restore job is already running", "type":"ConflictError"}}`.
- `503 Service Unavailable` — Job manager not configured.

---

## Tracking the restore job

Restore runs asynchronously. Track progress using the Jobs API:

**Poll for status:**

```
GET /tama/v1/backends/jobs/:id
```

Returns a snapshot of the job's current state, including log lines. See [Jobs API](jobs.md) for the full response schema.

**Stream events (SSE):**

```
GET /tama/v1/backends/jobs/:id/events
```

The stream emits:
- `log` events — `{ line: "..." }` for each merge step (extracting, validating, merging model cards, database merge, config merge).
- `status` event — `{ status: "Succeeded" }` on success or `{ status: "Failed" }` on failure.
- `error` event — `{ error: "..." }` only when the job fails, with the full error message.

The stream closes on the terminal status event (`Succeeded` or `Failed`).

**Failure guarantee:** If archive validation fails (bad manifest, unsupported version, SHA-256 mismatch), the job transitions to `Failed` without modifying local config or the database. All extraction and validation completes in a temporary directory before any mutation is applied.
