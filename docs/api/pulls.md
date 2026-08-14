# Pulls API

Monitor file pull progress for model GGUF files.

## GET /tama/v1/pulls/active

List currently active pull items.

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

## GET /tama/v1/pulls/history

List completed pull history.

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

## POST /tama/v1/pulls/:job_id/cancel

Cancel an active pull.

**Response (200 OK):**

```json
{ "ok": true, "message": null }
```

## Repo pulls (safetensors / transformers)

Whole-repo pulls run through the `hf` CLI (huggingface_hub ≥ 1.x) and are
tracked in memory — they do not appear in the per-file pulls queue above. Jobs
live until the server restarts.

### POST /tama/v1/pulls/repo

Start a whole-repo `hf` CLI pull into `<models_dir>/<repo_id>`.

**Request body:**

```json
{
  "repo_id": "Qwen/Qwen3-8B",
  "model_id": 306
}
```

- `repo_id` — Hugging Face repo id, required.
- `model_id` — optional pre-created stub model row; on completion the model
  row is updated with HF + config.json metadata.

**Response (200 OK):**

```json
{
  "job_id": "hfrepo-9f2b7e4a-1c6d-4e8a-9b0c-3d5f7a9e1c2b",
  "status": "running",
  "total_bytes": 16400000000
}
```

**Errors:**

| Status | Type | Meaning |
|--------|------|---------|
| `422` | `ValidationError` | Invalid `repo_id` (charset / path traversal), missing `hf` CLI (message includes `pip install -U huggingface_hub`), or repo not found on HuggingFace |
| `409` | `ConflictError` | A repo pull for this repo is already running |
| `502` | `UpstreamError` | HF API / network / spawn failure |

### GET /tama/v1/pulls/repo/:job_id

Live status of a repo pull job. `bytes_done` is the current on-disk size of
the destination directory, computed server-side.

**Response (200 OK):**

```json
{
  "job_id": "hfrepo-9f2b7e4a-1c6d-4e8a-9b0c-3d5f7a9e1c2b",
  "status": "running",
  "bytes_done": 8200000000,
  "total_bytes": 16400000000,
  "error": null,
  "context_length": null
}
```

- `status` — `running` | `completed` | `failed` | `cancelled`
- `total_bytes` — expected size from HF sibling sizes, `null` if unknown
  (progress becomes indeterminate)
- `error` — capped stderr tail for failed jobs, `null` otherwise
- `context_length` — `max_position_embeddings` from `config.json`, populated
  on completion, `null` otherwise

**Errors:**

| Status | Type | Meaning |
|--------|------|---------|
| `404` | `NotFoundError` | Unknown job id (or server restarted) |

### DELETE /tama/v1/pulls/repo/:job_id

Cancel + kill a running repo pull. The job transitions to `cancelled`.

**Response (200 OK):**

```json
{ "ok": true }
```

**Errors:**

| Status | Type | Meaning |
|--------|------|---------|
| `404` | `NotFoundError` | Unknown job id (or server restarted) |
| `409` | `ConflictError` | Job already finished (completed / failed / cancelled) |
