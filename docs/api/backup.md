# Backup & Restore API

## GET /tama/v1/backup

Create a `backup.tar.gz` archive of the config directory and return it as a file download.

**Response:** `application/gzip` file with `Content-Disposition: attachment; filename="backup.tar.gz"`.

## POST /tama/v1/restore/preview

Upload a backup archive and return a manifest preview without applying changes.

**Request:** `multipart/form-data` with the `backup.tar.gz` file.

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
| `upload_id` | string | **Required.** From the preview response |
| `selected_models` | string[] | Optional. Filter which model quants to restore |
| `skip_backends` | bool | Skip restoring backends (default `false`) |
| `skip_models` | bool | Skip restoring models (default `false`) |

**Response (200 OK):**

```json
{ "jobId": "uuid-string" }
```
