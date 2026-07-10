# HuggingFace API

Fetch model metadata and quant file listings from HuggingFace.

## GET /tama/v1/hf/:owner/:repo/metadata

Fetch HuggingFace repo metadata (README + API info) for a model.

**Response:** `HfModelMetadata` object.

```json
{
  "hf_format": "gguf",
  "hf_base_model": "meta-llama/Llama-3.1-8B-Instruct",
  "hf_pipeline_tag": "text-generation",
  "hf_total_params": 8030261248,
  "hf_active_params": 8030261248,
  "hf_architecture_type": "text-generation",
  "hf_context_length": 131072,
  "hf_num_layers": 32,
  "hf_last_modified": "2025-01-01T00:00:00Z"
}
```

**Errors:** `400 Bad Request` (invalid repo_id), `502 Bad Gateway` (upstream fetch failed)

## GET /tama/v1/hf/:owner/:repo

List GGUF files (quants) available in the repo.

**Response:**

```json
[
  {
    "filename": "llama-3.1-8b-instruct-q4_k_m.gguf",
    "quant": "Q4_K_M",
    "size_bytes": 4500000000,
    "kind": "Q4_K_M"
  },
  {
    "filename": "llama-3.1-8b-instruct-q8_0.gguf",
    "quant": "Q8_0",
    "size_bytes": 9000000000,
    "kind": "Q8_0"
  }
]
```

**Errors:** `400 Bad Request` (invalid repo_id — SSRF protection blocks `..` and null bytes), `502 Bad Gateway`
