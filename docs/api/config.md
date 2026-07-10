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

## PATCH /tama/v1/config/structured

Update config with deep recursive field-level merge. Only provided fields change.

**Request body:** `ConfigPatchBody` — each section is `Option<SectionPatch>`, each `*Patch` has all fields as `Option<T>`.

```json
{
  "general": { "log_level": "debug" },
  "proxy": { "port": 18910 },
  "sampling_templates": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `general` | object \| null | General config patch — all fields optional |
| `supervisor` | object \| null | Supervisor config patch — all fields optional |
| `proxy` | object \| null | Proxy config patch — all fields optional |
| `sampling_templates` | object \| null | Sampling templates patch |
| `compaction` | object \| null | Compaction settings patch |

Note: `backends` section is omitted (read-only — managed through [Backends API](backends.md)).

**Response (200 OK):**

```json
{ "ok": true }
```

**Errors:**
- `422 Unprocessable Entity` — Validation failure

