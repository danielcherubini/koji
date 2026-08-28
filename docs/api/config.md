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
| `general` | object \| null | General config patch — all fields optional (see below) |
| `supervisor` | object \| null | Supervisor config patch — all fields optional |
| `proxy` | object \| null | Proxy config patch — all fields optional |
| `sampling_templates` | object \| null | Sampling templates patch |
| `compaction` | object \| null | Compaction settings patch |

### `general` patch fields

| Field | Type | Description |
|-------|------|-------------|
| `log_level` | string \| null | Floor/default log level: `trace`, `debug`, `info`, `warn`, `error` |
| `log_directives` | string \| null | Target-specific directives, RUST_LOG syntax (`target=level` pairs, comma-separated). Validated before persist: a directive-looking entry that fails to parse → `400` and the row is **not** modified |
| `log_retention_days` | number \| null | Persisted log store retention: max entry age in days (default 7) |
| `log_retention_rows` | number \| null | Persisted log store retention: max row count (default 50,000) |
| `log_retention_max_mb` | number \| null | Persisted log store retention: max estimated size in MiB (default 256) |

The `log_retention_*` values bound the structured log store (`tama-logs.db`):
the writer applies them as a prune (age + rows + estimated bytes, at most once
per hour; the last prune's deleted count is reported on
`GET /tama/v1/logs/status` as `last_prune_deleted`). They are **boot-loaded**
— saving a new retention via PATCH persists it, but the running writer keeps
the bounds loaded at boot until the next restart (no live apply).

**Live apply (no restart):** when `log_level` or `log_directives` change, the
patch is applied to the running subscriber immediately through its reload
handle — no restart required. `RUST_LOG` is re-merged at every reload.

**`RUST_LOG` precedence (explicit).** The runtime filter is built in ONE
place for boot, `tama admin`, and the PATCH apply path above. The
`RUST_LOG` environment variable is read by the builder itself; config
`log_level` is the floor/default only. Target-specific entries from
`RUST_LOG` are merged in with the target-only rule (bare levels like
`RUST_LOG=info` are NOT directives and cannot override the floor), and the
durable `general.log_directives` are merged AFTER the env directives — so
**config directives win over RUST_LOG for the same target**
(last-addition-wins per target).

Note: `backends` section is omitted (read-only — managed through [Backends API](backends.md)).

Note: when `proxy.pull_backend` is set, its value must be the id of a tamad registered via [POST /tama/v1/tamads](tamads.md); saving an unregistered id is rejected with a foreign-key error ("not a registered tamad"). Register the tamad first, then set `pull_backend`.

**Response (200 OK):**

```json
{ "ok": true }
```

**Errors:**
- `400 Bad Request` — invalid `log_directives` (persisted nothing; the DB row is unchanged and the live filter is untouched)
- `422 Unprocessable Entity` — Validation failure

