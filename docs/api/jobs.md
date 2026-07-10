# Jobs API

Track async job progress and results. Jobs are created by backend install/update/restore operations and benchmark runs.

## GET /tama/v1/backends/jobs/:id

Return a snapshot of a job's current state, including log lines.

**Response:**

```json
{
  "id": "uuid-string",
  "kind": "install",
  "status": "Running",
  "backend_type": "llama_cpp",
  "started_at": 1700000000,
  "finished_at": null,
  "error": null,
  "log": [ "line 1", "line 2", ... ]
}
```

**Job kinds:** `"install"`, `"update"`, `"restore"`, `"benchmark"`

**Job statuses:** `"Running"`, `"Succeeded"`, `"Failed"`

**Errors:**
- `404 Not Found` — Job does not exist
- `500 Internal Server Error` — Job manager not configured
