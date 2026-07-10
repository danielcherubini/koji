# Models API

Manage model configurations. Each model maps a HuggingFace repo (`repo_id`) to a backend and runtime settings.

## GET /tama/v1/models

List all model configs plus available backends and sampling templates.

**Response:**

```json
{
  "models": [
    {
      "id": 1,
      "repo_id": "bartowski/Llama-3.1-8B-Instruct-GGUF",
      "backend": "llama_cpp",
      "gpu_variant": "cuda",
      "gpu_device": null,
      "model": null,
      "quant": "Q4_K_M",
      "mmproj": null,
      "mtp_model": null,
      "args": [],
      "sampling": null,
      "enabled": true,
      "context_length": null,
      "num_parallel": null,
      "port": null,
      "api_name": null,
      "display_name": null,
      "kv_unified": true,
      "gpu_layers": null,
      "cache_type_k": null,
      "cache_type_v": null,
      "hf_context_length": null,
      "hf_architecture_type": null,
      "hf_base_model": null,
      "quants": {
        "Q4_K_M": {
          "file": "llama-3.1-8b-instruct-q4_k_m.gguf",
          "kind": "Q4_K_M",
          "size_bytes": 4500000000,
          "context_length": null,
          "lfs_oid": "sha256:abc...",
          "db_size_bytes": 4500000000,
          "last_verified_at": "2025-01-01T00:00:00Z",
          "verified_ok": true,
          "verify_error": null
        }
      },
      "modalities": null,
      "spec_decoding": {},
      "repo_commit_sha": null,
      "repo_pulled_at": null
    }
  ],
  "backends": [
    {
      "name": "llama_cpp",
      "type": "LlamaCpp",
      "path": "/path/to/llama_cpp/..."
    }
  ],
  "sampling_templates": {}
}
```

## GET /tama/v1/models/:id

Get a single model config.

**Path params:**
- `id` — Integer ID or config_key (double-dash format, e.g. `bartowski--llama-3.1-8b-instruct-gguf`)

**Response:** Same shape as a single entry from the list endpoint, plus a `"backends"` array with available backend options.

**Errors:** `404 Not Found`

## POST /tama/v1/models

Create a new model config.

**Request body:**

```json
{
  "repo_id": "bartowski/Llama-3.1-8B-Instruct-GGUF",
  "backend": "llama_cpp",
  "gpu_variant": "cuda",
  "gpu_device": null,
  "model": null,
  "quant": null,
  "mmproj": null,
  "mtp_model": null,
  "args": [],
  "sampling": null,
  "enabled": true,
  "context_length": null,
  "num_parallel": null,
  "port": null,
  "api_name": null,
  "display_name": null,
  "gpu_layers": null,
  "quants": {},
  "modalities": null,
  "kv_unified": true,
  "cache_type_k": null,
  "cache_type_v": null,
  "spec_decoding": null,
  "metadata": null
}
```

**Field reference:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `repo_id` | string | **Yes** | HuggingFace repo `owner/repo` (max 256 chars, alphanumeric + `.` `_` `-` `/`) |
| `backend` | string | **Yes** | Backend name: `"llama_cpp"`, `"ik_llama"`, etc. |
| `gpu_variant` | string | No | `"cpu"`, `"cuda"`, `"vulkan"`, `"rocm"`, `"metal"` |
| `gpu_device` | string | No | Specific GPU device identifier |
| `model` | string | No | Internal model name override |
| `quant` | string | No | Default quant key (e.g. `"Q4_K_M"`) |
| `mmproj` | string | No | Multi-modal projector quant key |
| `mtp_model` | string | No | Multi-token prediction model quant key |
| `args` | string[] | No | CLI arguments passed to the backend |
| `sampling` | object | No | Sampling parameters |
| `enabled` | bool | No | Whether model is active (default `true`) |
| `context_length` | int | No | Override context length |
| `num_parallel` | int | No | Number of parallel requests |
| `port` | int | No | Custom backend port |
| `api_name` | string | No | Custom name for OpenAI-compatible routes |
| `display_name` | string | No | Human-readable display name |
| `gpu_layers` | int | No | Number of layers on GPU |
| `quants` | object | No | Map of quant key → `QuantEntry` |
| `modalities` | object | No | e.g. `{"text": true, "image": true}` |
| `kv_unified` | bool | No | Use unified KV cache (default `true`) |
| `cache_type_k` | string | No | KV cache type for keys (e.g. `"f8"`, `"q4"`) |
| `cache_type_v` | string | No | KV cache type for values |
| `spec_decoding` | object | No | Speculative decoding config |
| `metadata` | object | No | `HfModelMetadata` to pre-populate HF fields |

**Response (201 Created):**

```json
{ "ok": true, "id": 1 }
```

**Errors:**
- `409 Conflict` — `repo_id` already exists
- `422 Unprocessable Entity` — Validation failure

## PUT /tama/v1/models/:id

Update an existing model. Partial update — only provided fields change.

**Request body:** Any subset of the `POST /tama/v1/models` body (minus `repo_id` and `metadata`).

**Response (200 OK):**

```json
{ "ok": true, "id": 1 }
```

**Errors:** `404 Not Found`, `422 Unprocessable Entity`

## PATCH /tama/v1/models/:id

Update an existing model. Surgical partial update — only provided fields change, all others preserved.

**Path params:**
- `id` — Integer ID or config_key (double-dash format, e.g. `bartowski--llama-3.1-8b-instruct-gguf`)

**Request body:** `ModelPatchBody` — all fields optional.

```json
{
  "backend": "llama_cpp",
  "args": null,
  "enabled": true
}
```

| Field | Type | Description |
|-------|------|-------------|
| `backend` | string \| null | Backend name (optional — unlike PUT where it was required) |
| `args` | string[] | null | CLI arguments — `null` preserves current value, `[]` clears |
| All other fields | Various | Same as `POST /tama/v1/models` body — all optional, `null` = preserve |

**Response (200 OK):**

```json
{ "ok": true, "id": 1 }
```

**Errors:**
- `404 Not Found` — Model does not exist
- `422 Unprocessable Entity` — Validation failure

## POST /tama/v1/models/:id/rename

Rename a model (change its `repo_id`). The integer `id` is preserved.

**Request body:**

```json
{ "new_repo_id": "new-owner/new-repo-name" }
```

**Response (200 OK):**

```json
{ "ok": true, "id": 1 }
```

**Errors:**
- `404 Not Found` — Model does not exist
- `409 Conflict` — Target `repo_id` already exists
- `422 Unprocessable Entity` — Invalid `new_repo_id` format

## DELETE /tama/v1/models/:id

Delete a model config and all associated files from disk. Removes the model directory, model card, and database records.

**Response (200 OK):**

```json
{ "ok": true }
```

**Errors:** `404 Not Found`

## DELETE /tama/v1/models/:id/quants/:quant_key

Delete a single quant entry from a model and its GGUF file. If the deleted quant was the active `quant` or `mmproj`, those fields are cleared to `null`.

**Response (200 OK):**

```json
{
  "ok": true,
  "id": 1,
  "quant_key": "Q4_K_M",
  "deleted_file": "llama-3.1-8b-instruct-q4_k_m.gguf"
}
```

**Errors:** `404 Not Found` (model or quant does not exist)

## POST /tama/v1/models/:id/refresh

Re-query HuggingFace for the current commit SHA and per-file LFS hashes/sizes, and write them into the local database. Only updates metadata for files already tracked locally.

**Response (200 OK):**

```json
{
  "ok": true,
  "id": 1,
  "repo_id": "bartowski/Llama-3.1-8B-Instruct-GGUF",
  "repo_commit_sha": "abc123...",
  "repo_pulled_at": "2025-01-01T00:00:00Z",
  "files": [
    {
      "filename": "llama-3.1-8b-instruct-q4_k_m.gguf",
      "quant": "Q4_K_M",
      "lfs_oid": "sha256:...",
      "size_bytes": 4500000000,
      "downloaded_at": null,
      "last_verified_at": "2025-01-01T00:00:00Z",
      "verified_ok": true,
      "verify_error": null
    }
  ]
}
```

## POST /tama/v1/models/:id/verify

Recompute SHA-256 for every tracked file and compare against stored LFS hashes. CPU-bound and potentially slow for large GGUF files.

**Response (200 OK):**

```json
{
  "ok": true,
  "any_unknown": false,
  "id": 1,
  "repo_id": "bartowski/Llama-3.1-8B-Instruct-GGUF",
  "results": [
    {
      "filename": "llama-3.1-8b-instruct-q4_k_m.gguf",
      "ok": true,
      "error": null
    }
  ],
  "files": [ /* same file format as refresh */ ]
}
```
