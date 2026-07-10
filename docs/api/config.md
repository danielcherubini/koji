# Config API

Read and update the global TAMA configuration. Model configs are NOT stored here — they live in the SQLite database and are managed through the [Models API](models.md).

## GET /tama/v1/config/structured

Return the full Config object as JSON. Includes proxy settings, sampling templates, compaction settings, and other global configuration.

**Response:** Full `Config` object.

## POST /tama/v1/config/structured

Persist the config to the SQLite database and sync the proxy's in-memory config for hot-reload.

**Request body:**

```json
{ "config": { /* full Config object */ } }
```

**Response (200 OK):**

```json
{ "ok": true }
```
