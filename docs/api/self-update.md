# Self-Update API

Update the TAMA binary itself.

## GET /tama/v1/self-update/check

Check GitHub Releases for a newer version of TAMA.

**Response:**

```json
{
  "update_available": true,
  "current_version": "1.26.2",
  "latest_version": "1.27.0",
  "release_notes": "Bug fixes and improvements",
  "published_at": "2025-01-01T00:00:00Z"
}
```

## POST /tama/v1/self-update/update

Download and install the latest TAMA version, then restart the process. Runs asynchronously — track via SSE at `GET /tama/v1/self-update/events`.

**Response (200 OK):**

```json
{ "ok": true, "message": "Update started" }
```

**Errors:**
- `409 Conflict` — An update is already in progress
