# Benchmarks API

Run llama-bench benchmarks and manage benchmark history. All runs are asynchronous — track via job IDs.

## POST /tama/v1/benchmarks/run

Run a standard llama-bench benchmark.

**Request body:**

```json
{
  "model_id": "1",
  "quant": "Q4_K_M",
  "backend_name": "llama_cpp",
  "pp_sizes": [512, 1024, 2048],
  "tg_sizes": [128, 256],
  "runs": 3,
  "warmup": 1,
  "threads": [],
  "ngl_range": null,
  "ctx_override": null,
  "batch_sizes": [],
  "ubatch_sizes": [],
  "kv_cache_type": null,
  "depth": [],
  "flash_attn": null,
  "benchmark_type": "baseline"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `model_id` | string | Model ID (accepts integer or config_key) |
| `quant` | string | Optional quant label (e.g. `"Q6_K"`) |
| `backend_name` | string | Optional backend name override |
| `pp_sizes` | int[] | Prompt processing sizes to benchmark |
| `tg_sizes` | int[] | Token generation sizes to benchmark |
| `runs` | int | Number of runs per configuration |
| `warmup` | int | Warmup iterations |
| `threads` | int[] | Optional thread counts to test |
| `ngl_range` | string | GPU layer range (e.g. `"0-35"`) |
| `ctx_override` | int | Override context length |
| `batch_sizes` | int[] | Batch size array |
| `ubatch_sizes` | int[] | Unbatch size array |
| `kv_cache_type` | string | KV cache type override |
| `depth` | int[] | Depth values for sweep |
| `flash_attn` | bool | Enable flash attention |
| `benchmark_type` | string | Label for categorization (e.g. `"baseline"`, `"pp_sweep"`) |

**Response (202 Accepted):**

```json
{ "jobId": "uuid-string" }
```

Track via [GET /tama/v1/benchmarks/jobs/:id](#get-tamav1benchmarksjobsid).

## POST /tama/v1/benchmarks/spec-run

Run a speculative decoding benchmark.

**Request body:**

```json
{
  "model_id": "1",
  "quant": "Q4_K_M",
  "backend_name": "llama_cpp",
  "gpu_variant": "cuda",
  "spec_types": ["ngram", "extern"],
  "draft_max_values": [4, 8, 16],
  "ngram_n_values": [2, 3, 4],
  "ngram_m_values": [1, 2],
  "ngram_min_values": [1],
  "ngram_max_values": [4, 8],
  "ngram_min_hits": 1,
  "gen_tokens": 256,
  "runs": 3,
  "ngl": null,
  "flash_attn": true,
  "benchmark_type": "spec_scan"
}
```

**Response (202 Accepted):** `{ "jobId": "uuid-string" }`

## POST /tama/v1/benchmarks/mtp-run

Run a Multi-Token Prediction benchmark. Uses the same async job pattern.

**Request body:**

```json
{
  "model_id": "1",
  "quant": "Q4_K_M",
  "backend_name": "llama_cpp",
  "gpu_variant": "cuda",
  "draft_max_values": [0, 1, 2, 3, 4, 5, 6, 7, 8],
  "ngl": 99,
  "draft_ngl": 99,
  "flash_attn": true,
  "context_size": 32768,
  "benchmark_type": "mtp_scan"
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model_id` | string | — | Model ID (accepts integer or config_key) |
| `quant` | string | — | Optional quant label |
| `backend_name` | string | — | Optional backend name override |
| `gpu_variant` | string | — | Optional GPU variant override |
| `draft_max_values` | int[] | `[0..8]` | Draft max values to test (must not be empty) |
| `ngl` | int | `99` | GPU layers for main model |
| `draft_ngl` | int | `99` | GPU layers for draft model |
| `flash_attn` | bool | `true` | Enable flash attention |
| `context_size` | int | `32768` | Context size |
| `benchmark_type` | string | — | Label for categorization |

**Errors:**
- `400 Bad Request` — `draft_max_values` is empty
- `409 Conflict` — Another job is already running

**Response (202 Accepted):** `{ "jobId": "uuid-string" }`

## GET /tama/v1/benchmarks/jobs/:id

Get the result of a benchmark job.

**Response:**

```json
{
  "job_id": "uuid-string",
  "status": "Succeeded",
  "error": null,
  "log_lines": [ "line 1", "line 2" ],
  "benchmark_results": "..."
}
```

## GET /tama/v1/benchmarks/history

List all benchmark history entries from the database.

**Response:** Array of `BenchmarkHistoryEntry` objects.

```json
[
  {
    "id": 1,
    "created_at": 1700000000,
    "model_id": "1",
    "displayName": null,
    "quant": "Q4_K_M",
    "backend": "llama_cpp",
    "engine": "llama-bench",
    "benchmark_type": "baseline",
    "pp_sizes": [512, 1024],
    "tg_sizes": [128],
    "runs": 3,
    "results_count": 2,
    "status": "completed",
    "results": [ /* llama-bench summary objects */ ]
  }
]
```

## POST /tama/v1/benchmarks/suite

Run multiple benchmark types sequentially under a single job with shared log/SSE stream and continue-on-error semantics.

**Request body:**

```json
{
  "model_id": "1",
  "quant": "Q4_K_M",
  "backend_name": "llama_cpp",
  "gpu_variant": "cuda",
  "types": ["llama_bench", "spec", "mtp"]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `model_id` | string | Model ID (accepts integer or config_key) |
| `quant` | string | Optional quant label (e.g. `"Q6_K"`) — benchmarks use this GGUF file instead of the default |
| `backend_name` | string | Optional backend name override |
| `gpu_variant` | string | Optional GPU variant override (`"cpu"`, `"cuda"`, `"rocm"`, `"vulkan"`) |
| `types` | string[] | Optional list of benchmark types to run. Supported: `"llama_bench"`, `"spec"`, `"mtp"`. When omitted, auto-selects based on model capabilities. |

**Advanced override fields (all optional — when absent, sub-run builders use sensible defaults):**

| Field | Type | Description |
|-------|------|-------------|
| `pp_sizes` | int[] | Override prompt sizes for llama_bench sub-run |
| `tg_sizes` | int[] | Override generation lengths for llama_bench sub-run |
| `runs` | int | Override runs per type (default: 3) |
| `warmup` | int | Override warmup runs for llama_bench sub-run (default: 1) |
| `threads` | int[] | Override thread counts for llama_bench sub-run |
| `batch_sizes` | int[] | Override batch sizes for llama_bench sub-run |
| `ubatch_sizes` | int[] | Override micro-batch sizes for llama_bench sub-run |
| `kv_cache_type` | string | Override KV cache type for llama_bench sub-run |
| `depth` | int[] | Override depth (pre-fill tokens) for llama_bench sub-run |
| `flash_attn` | bool | Override flash attention flag for llama_bench sub-run |

**Auto-selection behavior:** When `types` is omitted, the endpoint always includes `"llama_bench"` and `"spec"`, then adds `"mtp"` when the model supports multi-token prediction (detected from GGUF `nextn_predict_count` and config heuristics).

**Response (202 Accepted):** `{ "jobId": "uuid-string" }`

Track via [GET /tama/v1/benchmarks/jobs/:id](#get-tamav1benchmarksjobsid). Errors from individual sub-benchmarks do not abort the suite — the final job status is `Failed` only when ALL sub-runs fail.

## DELETE /tama/v1/benchmarks/history/:id

Delete a benchmark history entry.

**Response (200 OK):**

```json
{ "ok": true }
```
