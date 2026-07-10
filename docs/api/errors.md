# Error Responses

All API errors follow a consistent structure:

```json
{
  "error": {
    "message": "Human-readable error message",
    "type": "ErrorType"
  }
}
```

## HTTP Status Codes

| Code | Meaning |
|------|---------|
| `400` | Bad Request — invalid params or path traversal |
| `404` | Not Found — resource does not exist |
| `409` | Conflict — duplicate resource, or job already running |
| `422` | Unprocessable Entity — validation failure (bad field values, length limits) |
| `500` | Internal Server Error |
| `502` | Bad Gateway — upstream fetch failed (HuggingFace, etc.) |
| `503` | Service Unavailable — required service not configured |

## Error Types

| Type | Description |
|------|-------------|
| `NotFoundError` | Resource does not exist (404) |
| `ValidationError` | Request body or params failed validation (400/422) |
| `ConflictError` | Resource conflict — duplicate, job already running (409) |
| `ServiceUnavailableError` | Required service not configured (503) |
