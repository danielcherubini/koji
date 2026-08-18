# Pulls API

Monitor file pull progress for model GGUF files.

## Tamad-hosted pulls

When the proxy config's `proxy.pull_backend` is set to a registered tamad
connection id, queued model pulls (the per-file queue below) are executed on
that tamad instead of the proxy: the proxy dispatches `PullModel` over gRPC,
the download runs on the tamad's disk (the file is written to the proxy's own
`models_dir/<repo_id>` path), and the proxy relays the tamad's job progress
into the same PullJob / queue-item / SSE tracking used for local pulls. The
post-download verification (SHA-256 vs upstream LFS hash, GGUF/transformers
metadata parse, model registration) runs proxy-side either way.

This is an internal routing detail — the endpoints, response shapes, and SSE
events above are unchanged. Failure semantics (fail loud, no silent local
fallback):

- `pull_backend` names an unregistered tamad → the pull fails with
  `pull_backend '<id>' is not a registered tamad`.
- The tamad is unreachable or rejects the dispatch → the pull fails with the
  transport error.
- The job stream ends before a terminal event (tamad died) → the pull fails
  with `tamad disconnected mid-pull (no terminal job event)`.
- No progress event for 120 s → the pull fails with `pull stalled`.
- Tamad-side verification mismatch → the tamad deletes the corrupt file and
  fails the job; the proxy relay fails the pull with the tamad's error.

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

Whole-repo pulls run through the `hf` CLI (huggingface_hub ≥ 1.x) **on the
pull host** (the tamad named by `proxy.pull_backend`); the proxy relays
dispatch, progress, and cancellation — it never runs the CLI itself
(ADR-0010). Jobs are tracked in memory on the proxy — they do not appear in
the per-file pulls queue above. Jobs live until the server restarts.

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
| `422` | `ValidationError` | Invalid `repo_id` (charset / path traversal) |
| `404` | `NotFoundError` | Model id not found (when `model_id` given) |
| `409` | `ConflictError` | A repo pull for this repo is already running |
| `502` | `UpstreamError` | No pull host configured (`proxy.pull_backend` missing/offline), HF API / network failure, or the host rejected the dispatch (e.g. missing `hf` CLI on the host) |

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
