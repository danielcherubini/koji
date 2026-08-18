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

Register a tamad connection. This is an **idempotent upsert keyed by name**:
the first call with a new name creates the row (auto-generated UUID, `201`);
calling again with the same name updates its `url`/`protocol`/`token` and
returns the stored id (`200`). Tamads use this endpoint for self-registration
at startup and periodically, so repeated calls are expected.

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
| `name` | string | **Yes** | Human-readable name (unique, upsert key) |
| `url` | string | **Yes** | Server address with scheme (`grpc://` or `http://`) |
| `protocol` | string | **Yes** | `"grpc"` or `"http"` |
| `token` | string | No | Bearer token the proxy must send to this tamad |

**Response:**
- `201 Created` — Name did not exist; the connection object with the auto-generated `id`.
- `200 OK` — Name already existed; the same connection object (original `id`, updated `url`/`protocol`/`token`).

**Errors:**
- `400 Bad Request` — Empty name, empty url, or invalid protocol

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

Trigger a real health check: the proxy calls the tamad's `HealthCheck`
RPC (gRPC) or `/health` endpoint (http) using the stored connection
token, and reports the result.

**Response (200 OK):**

```json
{ "status": "online" }
```

`"offline"` (with an `error` message when the tamad could not be reached):

```json
{ "status": "offline", "error": "connection refused" }
```

Unreachability is reported as `offline`, not a server error — it is the
normal state of a tamad whose box is powered off.

**Errors:** `404 Not Found` — Tamad does not exist.
