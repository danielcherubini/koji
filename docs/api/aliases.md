# Aliases API

Create short, memorable names that resolve to model configurations.

## GET /tama/v1/aliases

List all aliases (enabled and disabled).

**Response:** Array of alias objects.

```json
[
  {
    "id": 1,
    "name": "my-llama",
    "model_id": 1,
    "description": "My main model",
    "enabled": true,
    "created_at": "2025-01-01T00:00:00Z",
    "updated_at": "2025-01-01T00:00:00Z"
  }
]
```

## GET /tama/v1/aliases/:id

Get a single alias by ID.

**Response:** Single alias object.

**Errors:** `404 Not Found`

## POST /tama/v1/aliases

Create a new alias.

**Request body:**

```json
{
  "name": "my-llama",
  "model_id": 1,
  "description": "My main model"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | **Yes** | Starts with alphanumeric, then alphanumeric/`_`/`-`, max 128 chars |
| `model_id` | int | **Yes** | Integer ID of an existing model |
| `description` | string | No | Optional description |

**Response (201 Created):** The created alias object.

**Errors:**
- `400 Bad Request` — Model does not exist
- `422 Unprocessable Entity` — Invalid alias name format

## PUT /tama/v1/aliases/:id

Update an alias. All fields are optional — only provided fields change.

**Request body:**

```json
{
  "name": "new-name",
  "model_id": 2,
  "description": "Updated description",
  "enabled": true
}
```

**Response (200 OK):** The updated alias object.

## DELETE /tama/v1/aliases/:id

Delete an alias.

**Response (200 OK):**

```json
{ "deleted": true }
```
