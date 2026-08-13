# Backends API

Manage inference backends (llama.cpp, ik_llama, kokoro TTS).

## GET /tama/v1/installations

List all installed backends grouped by type + GPU variant, plus available (not yet installed) backend types.

**Response:**

```json
{
  "activeJob": {
    "id": "uuid",
    "kind": "install",
    "backend_type": "llama_cpp"
  },
  "backends": [
    {
      "type": "llama_cpp",
      "displayName": "llama.cpp",
      "installed": true,
      "gpuVariant": "cuda",
      "info": {
        "name": "llama_cpp",
        "version": "b5900",
        "path": "/path/to/llama_cpp/b5900",
        "installedAt": 1700000000,
        "gpuVariant": "cuda",
        "source": { "kind": "Prebuilt", "version": "b5900" }
      },
      "versions": [
        {
          "name": "llama_cpp",
          "version": "b5900",
          "path": "/path/to/llama_cpp/b5900",
          "installedAt": 1700000000,
          "gpuVariant": "cuda",
          "source": { "kind": "Prebuilt", "version": "b5900" },
          "isActive": true
        }
      ],
      "update": {
        "checked": true,
        "latestVersion": "b5950",
        "updateAvailable": true
      },
      "releaseNotesUrl": "https://github.com/ggml-org/llama.cpp/releases",
      "defaultArgs": [],
      "defaultEnv": [],
      "dockerConfig": null,
      "isActive": true
    }
  ],
  "custom": [],
  "available": ["ik_llama", "tts_kokoro"],
  "compaction": {
    "enabled": false,
    "device": "cpu",
    "port": null,
    "running": false,
    "requestTimeoutMs": 30000
  }
}
```

**Backend source kinds:**
- `Prebuilt` — `{ "kind": "Prebuilt", "version": "b5900" }`
- `SourceCode` — `{ "kind": "SourceCode", "version": "main", "gitUrl": "..." }`

## POST /tama/v1/installations

Register a backend directly (bypasses binary install). Used for docker-based backends.

**Request body:**

```json
{
  "name": "vllm",
  "backend_type": "docker",
  "version": "0.5.8",
  "gpu_variant": "cpu",
  "docker_config": {
    "image": "stilldeadcode/vllm-radiance:0.5.8",
    "container_port": 8000,
    "model_mount": {
      "host_path": "{{MODEL_DIR}}",
      "container_path": "/models",
      "read_only": true
    },
    "volumes": [],
    "devices": ["/dev/nvidia0", "/dev/nvidiactl", "/dev/nvidia-uvm"],
    "gpus": "all",
    "shm_size": "16G",
    "cap_adds": [],
    "security_opts": [],
    "group_adds": ["video"]
  }
}
```

**Validation:**
- `backend_type="docker"` requires non-null `docker_config` → 400 if missing
- Non-docker types reject non-null `docker_config` → 400
- `DockerConfig.validate()` runs: image non-empty, container_port 1-65535, absolute container paths
- `docker_available()` preflight at registration time → 400 if docker not available

**Response (201 Created):**

```json
{
  "name": "vllm",
  "backend_type": "docker",
  "version": "0.5.8",
  "path": "stilldeadcode/vllm-radiance:0.5.8",
  "installed_at": 1700000000,
  "gpu_variant": "cpu",
  "source": null,
  "docker_config": {
    "image": "stilldeadcode/vllm-radiance:0.5.8",
    "container_port": 8000,
    "model_mount": {
      "host_path": "{{MODEL_DIR}}",
      "container_path": "/models",
      "read_only": true
    },
    "volumes": [],
    "devices": ["/dev/nvidia0", "/dev/nvidiactl", "/dev/nvidia-uvm"],
    "gpus": "all",
    "shm_size": "16G",
    "cap_adds": [],
    "security_opts": [],
    "group_adds": ["video"]
  }
}
```

## GET /tama/v1/installations/:name/versions

List all installed versions for a specific backend.

**Response:**

```json
{
  "versions": [
    {
      "name": "llama_cpp",
      "version": "b5900",
      "path": "/path/to/llama_cpp/b5900",
      "installedAt": 1700000000,
      "gpuVariant": "cuda",
      "source": { "kind": "Prebuilt", "version": "b5900" },
      "isActive": true
    }
  ],
  "activeVersion": "b5900"
}
```

## POST /tama/v1/installations/install

Install a backend. Runs asynchronously — track via the returned `jobId`.

**Request body:**

```json
{
  "backend_type": "llama_cpp",
  "version": "b5900",
  "gpu_variant": "cuda",
  "build_from_source": false,
  "force": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `backend_type` | string | `"llama_cpp"`, `"ik_llama"`, `"tts_kokoro"` |
| `version` | string | Optional version tag (default `"latest"`) |
| `gpu_variant` | string | `"cpu"`, `"cuda"`, `"vulkan"`, `"rocm"`, `"metal"`, `"custom"` |
| `build_from_source` | bool | Build from git. `ik_llama` always builds from source. On Linux + CUDA, forced `true`. |
| `force` | bool | Allow overwriting existing installation |

**Response (200 OK):**

```json
{
  "jobId": "uuid-string",
  "kind": "install",
  "backendType": "llama_cpp",
  "notices": []
}
```

**Errors:**
- `400 Bad Request` — Invalid backend type, missing build prerequisites (git, cmake, compiler)
- `409 Conflict` — Another backend job is already running

## POST /tama/v1/installations/:name/update

Update a backend to its latest version. Runs asynchronously.

**Query params:**
- `gpu_variant` — Optional GPU variant (default: active variant)

**Response (200 OK):** Same shape as install — returns a `jobId`.

**Errors:**
- `404 Not Found` — Backend not found
- `409 Conflict` — Another job is already running

## POST /tama/v1/installations/:name/activate

Switch the active version for a backend.

**Query params:**
- `gpu_variant` — Optional. Auto-inferred if omitted (works when only one variant exists, or only one variant has the requested version).

**Request body:**

```json
{ "version": "b5900" }
```

**Response (200 OK):**

```json
{ "version": "b5900", "isActive": true }
```

## DELETE /tama/v1/installations/:name

Remove all versions of a backend (or a specific GPU variant). Deletes files and database records.

**Query params:**
- `gpu_variant` — Optional. If omitted, removes all variants.

**Response (200 OK):**

```json
{ "removed": true }
```

**Errors:**
- `404 Not Found` — Backend not found
- `409 Conflict` — A job is running for this backend, or path is outside managed directory

## PATCH /tama/v1/installations/:name

Update backend config fields (default_args, default_env, health_check_url) with partial merge.

**Query params:**
- `gpu_variant` — Optional GPU variant (default: active variant)

**Request body:** `BackendPatchBody` — all fields optional.

```json
{
  "default_args": ["--flash-attn", "--cache-type-k q8_0"],
  "default_env": ["CUDA_VISIBLE_DEVICES=0"],
  "health_check_url": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `default_args` | string[] \| null | CLI arguments — `null` preserves current value, `[]` clears |
| `default_env` | string[] \| null | Environment variables — `null` preserves current value, `[]` clears |
| `health_check_url` | string \| null | Health check URL — `null` preserves current value, `""` clears, `Some(value)` sets |

**Response (200 OK):**

```json
{ "success": true }
```

**Errors:**
- `404 Not Found` — Backend not found
- `422 Unprocessable Entity` — Validation failure

## POST /tama/v1/installations/:name/rename

Rename a backend across every table that carries its display name —
`backend_installations`, `backend_configs`, `model_configs.backend`, and
`active_models.backend` — in a single transaction.

The backend's stable `logical_id` is **preserved**, so its default args/env in
`backend_configs` (keyed by that logical id) and any models on the backend
survive the rename intact. Use this endpoint instead of editing the name by hand.

**Request body:**

```json
{ "name": "radiance" }
```

**Response (200 OK):**

```json
{ "success": true, "name": "radiance" }
```

**Errors:**
- `400 Bad Request` — Invalid new name (missing/empty, whitespace, or contains path separators/traversal sequences such as `/`, `\`, or `..`)
- `404 Not Found` — The backend to rename does not exist
- `409 Conflict` — Rename failed: renaming onto an existing, distinct backend or onto a name with overlapping `backend_configs` (merge/overlap conflict), or another rename error
- `500 Internal Server Error` — Internal/server error

## DELETE /tama/v1/installations/:name/versions/:version

Remove a specific version. If multiple variants share the same version, `gpu_variant` is required.

**Query params:**
- `gpu_variant` — Optional, required when version exists in multiple variants

**Response (200 OK):**

```json
{ "removed": true }
```

## POST /tama/v1/installations/check-updates

Check all installed backends for updates against upstream sources. Returns fresh update status.

**Response:** Same shape as `GET /tama/v1/installations` (with updated `update` objects on each card).

## POST /tama/v1/installations/:name/default-args

Set default CLI arguments for a backend. Existing `default_env` is preserved.

**Query params:**
- `gpu_variant` — GPU variant

**Request body:**

```json
{ "default_args": ["--flash-attn", "--cache-type-k q8_0"] }
```

**Response (200 OK):**

```json
{ "success": true }
```

## POST /tama/v1/installations/:name/default-env

Set default environment variables for a backend. Existing `default_args` is preserved.

**Query params:**
- `gpu_variant` — GPU variant

**Request body:**

```json
{ "default_env": ["CUDA_VISIBLE_DEVICES=0,1"] }
```

**Response (200 OK):**

```json
{ "success": true }
```

## POST /tama/v1/installations/:name/source

Change the build method (prebuilt vs source) for a backend. Affects future installs/updates.

**Query params:**
- `gpu_variant` — Optional. Auto-inferred if omitted.

**Request body:**

```json
{ "build_from_source": true }
```

**Response (200 OK):**

```json
{ "build_from_source": true }
```

## POST /tama/v1/installations/compaction

Enable/disable the compaction backend and update its configuration.

**Request body:**

```json
{
  "enabled": true,
  "device": "cuda",
  "port": null,
  "request_timeout_ms": 30000
}
```

**Response (200 OK):**

```json
{ "enabled": true, "running": true }
```
