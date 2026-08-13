# Providers API

Register and manage inference providers — local (tamad-managed) and remote (HTTP-based).

## GET /tama/v1/providers

List all registered providers (enabled and disabled), ordered by name.

**Response:** Array of provider objects.

```json
[
  {
    "id": 1,
    "name": "local-llama",
    "provider_type": "local",
    "engine": "llama_cpp",
    "tamad_id": "uuid-123",
    "base_url": null,
    "api_key": null,
    "created_at": 1700000000
  },
  {
    "id": 2,
    "name": "openai-proxy",
    "provider_type": "remote",
    "engine": "openai",
    "tamad_id": null,
    "base_url": "https://api.openai.com/v1",
    "api_key": "sk-xxx",
    "created_at": 1700000001
  }
]
```

## GET /tama/v1/providers/:name

Get a single provider by name.

**Response:** Single provider object.

**Errors:** `404 Not Found` — Provider does not exist.

## POST /tama/v1/providers

Create a new provider.

**Request body:**

```json
{
  "name": "local-llama",
  "provider_type": "local",
  "engine": "llama_cpp",
  "tamad_id": "uuid-123"
}
```

For remote providers:

```json
{
  "name": "openai-proxy",
  "provider_type": "remote",
  "engine": "openai",
  "base_url": "https://api.openai.com/v1",
  "api_key": "sk-xxx"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | **Yes** | Unique provider name |
| `provider_type` | string | **Yes** | `"local"` (tamad-managed) or `"remote"` (HTTP endpoint) |
| `engine` | string | **Yes** | Engine identifier (e.g. `"llama_cpp"`, `"openai"`, `"anthropic"`) |
| `tamad_id` | string | Local only | UUID of the managing tamad |
| `base_url` | string | Remote only | Base URL of the remote API |
| `api_key` | string | No | API key for the remote provider (stored encrypted) |

**Response (201 Created):** The created provider object.

**Errors:**
- `400 Bad Request` — Invalid provider_type, empty name, empty engine, or missing required field (local needs tamad_id, remote needs base_url)
- `409 Conflict` — Provider name already exists

## PATCH /tama/v1/providers/:name

Update a provider's base_url and/or api_key.

**Request body:**

```json
{
  "base_url": "https://new.api/v1",
  "api_key": "sk-new-key"
}
```

All fields are optional — only provided fields are updated.

**Response (200 OK):** The updated provider object.

**Errors:** `404 Not Found` — Provider does not exist.

## DELETE /tama/v1/providers/:name

Delete a provider by name.

**Response (200 OK):**

```json
{ "deleted": true }
```

**Errors:** `404 Not Found` — Provider does not exist.
