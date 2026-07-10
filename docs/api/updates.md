# Updates API

Check for and apply updates to backends and models.

## GET /tama/v1/updates

Return cached update check results for all backends and models.

**Response:**

```json
{
  "backends": [
    {
      "item_type": "backend",
      "item_id": "llama_cpp",
      "variant": "cuda",
      "repo_id": null,
      "displayName": null,
      "current_version": "b5900",
      "latest_version": "b5950",
      "update_available": true,
      "status": "update_available",
      "error_message": null,
      "details_json": null,
      "checked_at": 1700000000
    }
  ],
  "models": [
    {
      "item_type": "model",
      "item_id": "1",
      "variant": null,
      "repo_id": "bartowski/Llama-3.1-8B-Instruct-GGUF",
      "displayName": "My Model",
      "current_version": "abc123",
      "latest_version": "def456",
      "update_available": false,
      "status": "up_to_date",
      "error_message": null,
      "details_json": {
        "quants": [
          {
            "quant_name": "Q4_K_M",
            "filename": "model-q4.gguf",
            "current_hash": "abc",
            "latest_hash": "abc",
            "update_available": false,
            "status": "up_to_date"
          }
        ]
      },
      "checked_at": 1700000000
    }
  ]
}
```

**Status values:** `"up_to_date"`, `"update_available"`, `"error"`, `"no_prior_record"`

## POST /tama/v1/updates/check

Trigger a full re-check of all backends and models in the background. Returns immediately.

**Response (200 OK):**

```json
{ "triggered": true, "message": "Update check started" }
```

## POST /tama/v1/updates/check/:item_type/:item_id

Check a single item for updates.

**Path params:**
- `item_type` — `"backend"` or `"model"`
- `item_id` — Backend name (e.g. `"llama_cpp"`) or model config_key/ID

**Query params:**
- `gpu_variant` — Optional GPU variant for backends

**Response (200 OK):**

```json
{ "ok": true }
```

## POST /tama/v1/updates/apply/backend/:name

Download and install the latest version of a backend. Runs asynchronously.

**Query params:**
- `gpu_variant` — Optional GPU variant (default: active variant)

**Response (200 OK):**

```json
{ "jobId": "uuid-string", "kind": "update" }
```

## POST /tama/v1/updates/apply/model/:id

Enqueue selected quants through the pull queue for download. Returns immediately with job IDs.

**Request body:**

```json
{ "quants": ["Q4_K_M", "Q8_0"] }
```

**Response (200 OK):**

```json
{ "job_ids": ["uuid-1", "uuid-2"], "total": 2 }
```

**Errors:**
- `409 Conflict` — Download already in progress for a quant
- `422 Unprocessable Entity` — Invalid quant keys
