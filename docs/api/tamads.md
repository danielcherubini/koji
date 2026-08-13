# Tamads API

Register and manage tamad (tama daemon) connections — remote inference servers that the proxy can route requests to.

## GET /tama/v1/tamads

List all registered tamad connections.

**Response:** Array of tamad connection objects.

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "name": "gpu-server-1",
    "url": "grpc://192.168.1.100:50051",
    "protocol": "grpc",
    "token": null,
    "status": "unknown"
  }
]
```

## GET /tama/v1/tamads/:id

Get a single tamad connection by its UUID.

**Response:** Single tamad connection object.

**Errors:** `404 Not Found` — Tamad does not exist.

## POST /tama/v1/tamads

Register a new tamad connection. Auto-generates a UUID for the tamad id.

**Request body:**

```json
{
  "name": "gpu-server-1",
  "url": "grpc://192.168.1.100:50051",
  "protocol": "grpc",
  "token": "secret-token"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | **Yes** | Human-readable name (unique) |
| `url` | string | **Yes** | Server address with scheme (`grpc://` or `http://`) |
| `protocol` | string | **Yes** | `"grpc"` or `"http"` |
| `token` | string | No | Authentication token for the tamad |

**Response (201 Created):** The created tamad connection object with auto-generated `id`.

**Errors:**
- `400 Bad Request` — Empty name, empty url, or invalid protocol
- `409 Conflict` — Tamad name already exists

## PATCH /tama/v1/tamads/:id

Update a tamad's url and/or token.

**Request body:**

```json
{
  "url": "grpc://192.168.1.101:50051",
  "token": "new-secret-token"
}
```

All fields are optional — only provided fields are updated. At least one field must be provided.

**Response (200 OK):** The updated tamad connection object.

**Errors:**
- `400 Bad Request` — Neither `url` nor `token` provided
- `404 Not Found` — Tamad does not exist

## DELETE /tama/v1/tamads/:id

Unregister a tamad connection.

**Response (200 OK):**

```json
{ "deleted": true }
```

**Errors:** `404 Not Found` — Tamad does not exist.

## POST /tama/v1/tamads/:id/health

Trigger a health check for a tamad connection.

**Note:** This is currently a stub endpoint. It verifies the tamad exists and returns `{"status": "unknown"}`. Real health check logic will be implemented when the tamad client is wired up.

**Response (200 OK):**

```json
{
  "status": "unknown",
  "message": "Health check not yet implemented — tamad client not wired"
}
```

**Errors:** `404 Not Found` — Tamad does not exist.
