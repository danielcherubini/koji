//! Dynamic OpenAPI 3.1.0 spec generation from registered routes.
//! Served at `GET /tama/v1/docs` as JSON.

#[cfg(feature = "ssr")]
use axum::{http::StatusCode, response::IntoResponse, Json};

/// Returns the full OpenAPI 3.1.0 specification as a JSON value.
pub fn spec() -> serde_json::Value {
    let paths: std::collections::HashMap<String, serde_json::Value> = [
        // ── System ──────────────────────────────────────────────────────────────
        (
            "/tama/v1/system/capabilities",
            op(
                "get",
                "getCapabilities",
                "Get system capabilities",
                "Returns supported GPU architectures, CUDA/ROCm versions, and platform info.",
                &["system"],
                None,
                None,
            ),
        ),
        // ── Backends ────────────────────────────────────────────────────────────
        (
            "/tama/v1/installations",
            post_op(
                "registerBackend",
                "Register a backend",
                "Register a backend directly without binary install. For docker backends, requires docker_config in the request body.",
                &["backends"],
                Some(("RegisterBackendRequest", "application/json")),
                Some("RegisterBackendResponse"),
            ),
        ),
        (
            "/tama/v1/installations",
            op(
                "get",
                "listBackends",
                "List all backends",
                "Returns configured and installed backends with their versions and states.",
                &["backends"],
                None,
                None,
            ),
        ),
        (
            "/tama/v1/installations/install",
            post_op(
                "installBackend",
                "Install a backend",
                "Installs a new backend version. Body limit: 16MB.",
                &["backends"],
                Some(("InstallRequest", "multipart/form-data")),
                Some("JobResponse"),
            ),
        ),
        (
            "/tama/v1/installations/{name}/update",
            post_op_p(
                "updateBackend",
                "Update a backend",
                "Updates an existing backend to its latest version. Body limit: 16MB.",
                &["backends"],
                &[("name", "path")],
                Some(("UpdateRequest", "application/json")),
                Some("JobResponse"),
            ),
        ),
        (
            "/tama/v1/installations/{name}",
            delete_op_p(
                "removeBackend",
                "Remove a backend",
                "Removes an installed backend.",
                &["backends"],
                &[("name", "path")],
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/installations/{name}",
            patch_op_p(
                "patchBackend",
                "Update backend config (partial)",
                "Update backend config fields (default_args, default_env, health_check_url) with partial merge.",
                &["backends"],
                &[("name", "path")],
                Some(("BackendPatchBody", "application/json")),
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/installations/{name}/default-args",
            post_op_p(
                "updateBackendDefaultArgs",
                "Update default args",
                "Sets the default CLI arguments for a backend.",
                &["backends"],
                &[("name", "path")],
                Some(("DefaultArgsRequest", "application/json")),
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/installations/{name}/versions/{version}",
            delete_op_pp(
                "removeBackendVersion",
                "Remove a backend version",
                "Removes a specific version of a backend.",
                &["backends"],
                &[("name", "path"), ("version", "path")],
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/installations/check-updates",
            post_op(
                "checkBackendUpdates",
                "Check for backend updates",
                "Triggers a check for new versions of all backends.",
                &["backends"],
                None,
                None,
            ),
        ),
        (
            "/tama/v1/installations/{name}/versions",
            op_p(
                "get",
                "listBackendVersions",
                "List backend versions",
                "Returns all installed versions of a backend.",
                &["backends"],
                &[("name", "path")],
                Some("BackendVersion"),
            ),
        ),
        (
            "/tama/v1/installations/{name}/activate",
            post_op_p(
                "activateBackendVersion",
                "Activate a backend version",
                "Activates a specific version of a backend.",
                &["backends"],
                &[("name", "path")],
                Some(("ActivateRequest", "application/json")),
                Some("OkResponse"),
            ),
        ),
        // ── Jobs ────────────────────────────────────────────────────────────────
        (
            "/tama/v1/installations/jobs/{id}",
            op_p(
                "get",
                "getJob",
                "Get backend job status",
                "Returns the current status of a backend installation/update job.",
                &["jobs"],
                &[("id", "path")],
                Some("JobStatus"),
            ),
        ),
        (
            "/tama/v1/installations/jobs/{id}/events",
            op_p(
                "get",
                "jobEventsSse",
                "Stream job events (SSE)",
                "Server-sent events stream for real-time job progress and log lines.",
                &["jobs"],
                &[("id", "path")],
                None,
            ),
        ),
        // ── Updates ─────────────────────────────────────────────────────────────
        (
            "/tama/v1/updates",
            op(
                "get",
                "getUpdates",
                "Get cached update results",
                "Returns previously checked update status for backends and models.",
                &["updates"],
                None,
                Some("UpdatesListResponse"),
            ),
        ),
        (
            "/tama/v1/updates/check",
            post_op(
                "triggerUpdateCheck",
                "Trigger full update check",
                "Starts a new update check for all backends and models.",
                &["updates"],
                None,
                Some("CheckResponse"),
            ),
        ),
        (
            "/tama/v1/updates/check/{item_type}/{item_id}",
            post_op_pp(
                "checkSingleUpdate",
                "Check a single item",
                "Checks only one backend or model for available updates.",
                &["updates"],
                &[("item_type", "path"), ("item_id", "path")],
                None,
                Some("CheckResponse"),
            ),
        ),
        (
            "/tama/v1/updates/apply/backend/{name}",
            post_op_p(
                "applyBackendUpdate",
                "Apply backend update",
                "Triggers installation of the latest version for a backend.",
                &["updates"],
                &[("name", "path")],
                None,
                Some("JobResponse"),
            ),
        ),
        (
            "/tama/v1/updates/apply/model/{id}",
            post_op_p(
                "applyModelUpdate",
                "Apply model updates",
                "Enqueues selected quant pulls for a model through the pull queue.",
                &["updates"],
                &[("id", "path")],
                Some(("ModelUpdateRequest", "application/json")),
                Some("ModelUpdateResponse"),
            ),
        ),
        // ── Pulls ───────────────────────────────────────────────────────────────
        (
            "/tama/v1/pulls/active",
            op(
                "get",
                "getActivePulls",
                "Get active pulls",
                "Returns currently running and queued pull jobs.",
                &["pulls"],
                None,
                Some("PullJob"),
            ),
        ),
        (
            "/tama/v1/pulls/history",
            op_q(
                "get",
                "getPullHistory",
                "Get pull history",
                "Returns completed and failed pull jobs with pagination.",
                &["pulls"],
                &[("limit", "query"), ("offset", "query")],
                Some("PullJob"),
            ),
        ),
        (
            "/tama/v1/pulls/{job_id}/cancel",
            post_op_p(
                "cancelPull",
                "Cancel a pull job",
                "Cancels a queued or active pull. Does not affect completed jobs.",
                &["pulls"],
                &[("job_id", "path")],
                None,
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/pulls/events",
            op(
                "get",
                "pullEventsSse",
                "Stream pull events (SSE)",
                "Server-sent events stream for pull lifecycle events.",
                &["pulls"],
                None,
                None,
            ),
        ),
        // ── Self-Update ─────────────────────────────────────────────────────────
        (
            "/tama/v1/self-update/check",
            op(
                "get",
                "checkSelfUpdate",
                "Check for self-update",
                "Checks if a newer version of the tama binary is available.",
                &["self-update"],
                None,
                Some("SelfUpdateCheck"),
            ),
        ),
        (
            "/tama/v1/self-update/update",
            post_op(
                "triggerSelfUpdate",
                "Trigger self-update",
                "Starts downloading and installing the latest tama binary.",
                &["self-update"],
                None,
                Some("SelfUpdateTrigger"),
            ),
        ),
        (
            "/tama/v1/self-update/events",
            op(
                "get",
                "selfUpdateEventsSse",
                "Stream self-update progress (SSE)",
                "Server-sent events stream showing self-update download and install progress.",
                &["self-update"],
                None,
                None,
            ),
        ),
        // ── Restore ─────────────────────────────────────────────────────────────
        (
            "/tama/v1/restore/preview",
            post_op(
                "restorePreview",
                "Preview restore archive",
                "Uploads a backup archive and returns its manifest for review before restoring.",
                &["restore"],
                Some(("RestorePreviewRequest", "multipart/form-data")),
                Some("RestorePreviewResponse"),
            ),
        ),
        (
            "/tama/v1/restore",
            post_op(
                "startRestore",
                "Start restore job",
                "Restores from a previously uploaded backup archive.",
                &["restore"],
                Some(("RestoreRequest", "application/json")),
                Some("JobResponse"),
            ),
        ),
        // ── Models (config CRUD) ────────────────────────────────────────────────
        (
            "/tama/v1/models",
            op2(
                "get",
                "listModels",
                "List all model configs",
                "Returns all model entries from the database plus available backends.",
                &["models"],
                None,
                Some("ModelsResponse"),
            ),
        ),
        (
            "/tama/v1/models",
            post_op(
                "createModel",
                "Create a new model config",
                "Adds a new `[models.<id>]` entry to the database.",
                &["models"],
                Some(("ModelBody", "application/json")),
                Some("ModelMutationResponse"),
            ),
        ),
        (
            "/tama/v1/models/{id}",
            op_p(
                "get",
                "getModel",
                "Get a model config",
                "Returns one model entry plus available backends.",
                &["models"],
                &[("id", "path")],
                Some("ModelConfig"),
            ),
        ),
        (
            "/tama/v1/models/{id}",
            put_op_p(
                "updateModel",
                "Update a model config",
                "Replaces the `[models.<id>]` entry in the database.",
                &["models"],
                &[("id", "path")],
                Some(("ModelBody", "application/json")),
                Some("ModelMutationResponse"),
            ),
        ),
        (
            "/tama/v1/models/{id}",
            patch_op_p(
                "patchModel",
                "Update a model (partial/surgical)",
                "Surgical partial update — only provided fields change, all others preserved. `backend` optional, `args` is `Option` (None=preserve, Some([])=clear).",
                &["models"],
                &[("id", "path")],
                Some(("ModelPatchBody", "application/json")),
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/models/{id}",
            delete_op_p(
                "deleteModel",
                "Delete a model config",
                "Removes the `[models.<id>]` entry from the database.",
                &["models"],
                &[("id", "path")],
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/models/{id}/rename",
            post_op_p(
                "renameModel",
                "Rename a model config",
                "Renames a model config entry in the database.",
                &["models"],
                &[("id", "path")],
                Some(("RenameRequest", "application/json")),
                Some("ModelMutationResponse"),
            ),
        ),
        (
            "/tama/v1/models/{id}/refresh",
            post_op_p(
                "refreshModelMetadata",
                "Refresh model metadata",
                "Re-queries HuggingFace for the current commit hash of a model.",
                &["models"],
                &[("id", "path")],
                None,
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/models/{id}/verify",
            post_op_p(
                "verifyModelFiles",
                "Verify model files",
                "Recomputes SHA-256 checksums for all tracked files of a model.",
                &["models"],
                &[("id", "path")],
                None,
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/models/{id}/quants/{quant_key}",
            delete_op_pp(
                "deleteQuant",
                "Delete a quant file",
                "Deletes a specific quant file from disk and its config entry.",
                &["models"],
                &[("id", "path"), ("quant_key", "path")],
                Some("OkResponse"),
            ),
        ),
        // ── Benchmarks ──────────────────────────────────────────────────────────
        (
            "/tama/v1/benchmarks/run",
            post_op(
                "runBenchmark",
                "Run a benchmark",
                "Starts a new benchmark run against a model.",
                &["benchmarks"],
                Some(("BenchmarkRequest", "application/json")),
                Some("JobResponse"),
            ),
        ),
        (
            "/tama/v1/benchmarks/jobs/{id}",
            op_p(
                "get",
                "getBenchmarkResult",
                "Get benchmark result",
                "Returns the results of a completed benchmark run.",
                &["benchmarks"],
                &[("id", "path")],
                Some("BenchmarkResult"),
            ),
        ),
        (
            "/tama/v1/benchmarks/jobs/{id}/events",
            op_p(
                "get",
                "benchmarkEventsSse",
                "Stream benchmark events (SSE)",
                "Server-sent events stream for benchmark progress.",
                &["benchmarks"],
                &[("id", "path")],
                None,
            ),
        ),
        (
            "/tama/v1/benchmarks/history",
            op(
                "get",
                "listBenchmarkHistory",
                "List benchmark history",
                "Returns all completed and failed benchmark runs.",
                &["benchmarks"],
                None,
                Some("BenchmarkResult"),
            ),
        ),
        (
            "/tama/v1/benchmarks/history/{id}",
            delete_op_p(
                "deleteBenchmark",
                "Delete benchmark result",
                "Removes a benchmark result from history.",
                &["benchmarks"],
                &[("id", "path")],
                Some("OkResponse"),
            ),
        ),
        // ── Web API (logs, config, backup) ──────────────────────────────────────
        (
            "/tama/v1/logs",
            op_q(
                "get",
                "getLogs",
                "Query structured logs",
                "Pages the structured log store (default 200, max 1000 per page, newest-first by default). `source` is a single label: `proxy` for proxy-generated lines, `backend:{id}` for proxy-side backend lifecycle, or a tamad label (exact or delimiter-aware prefix; `tamad:`-shaped sources are served by the legacy tail adapter). `level` filters by minimum level, `q` is full-text, `since`/`until` are unix ms. Use `next_cursor` to page.",
                &["web-api", "logs"],
                &[("level", "query"), ("source", "query"), ("q", "query"), ("since", "query"), ("until", "query"), ("limit", "query"), ("cursor", "query"), ("order", "query")],
                Some("LogsQueryResult"),
            ),
        ),
        (
            "/tama/v1/logs",
            serde_json::json!({
                "operationId": "deleteLogs",
                "summary": "Delete logs",
                "description": "Deletes structured log entries (all rows by default, or one `id` with `?id=`). Truncates the table + reclaims the vacuum; the ongoing app is unaffected. CSRF-protected.",
                "tags": ["web-api", "logs"],
                "parameters": [{"name": "id", "in": "query", "required": false, "schema": {"type": "integer"}}],
                "responses": {"200": {"description": "Deletion summary with `deleted` row count."}}
            }),
        ),
        (
            "/tama/v1/logs/sources",
            op(
                "get",
                "getLogSources",
                "List log sources",
                "Lists the sources that exist in the structured log store (label + row count + latest ts) plus the on-demand legacy tamad tail sources.",
                &["web-api", "logs"],
                None,
                Some("LogSources"),
            ),
        ),
        (
            "/tama/v1/logs/summary",
            op_q(
                "get",
                "getLogSummary",
                "Summarize log counts",
                "Row counts by level within the optional `since` unix ms window.",
                &["web-api", "logs"],
                &[("since", "query")],
                Some("LogSummary"),
            ),
        ),
        (
            "/tama/v1/logs/status",
            op(
                "get",
                "getLogStatus",
                "Log store status",
                "Writer channel/ring backlog, degraded flag + degraded-since unix ms. `503` when the log runtime is not wired (non-SSR boot / tests).",
                &["web-api", "logs"],
                None,
                Some("LogStoreStatusDto"),
            ),
        ),
        (
            "/tama/v1/logs/stream",
            op_q(
                "get",
                "logStreamSse",
                "Stream new log entries (SSE)",
                "Server-sent events tailing the structured log store: one `event: entry` frame per row with `id > after` (default 0 = from the beginning, paged 200 rows at a time), then a 1 s heartbeat poll; idle ticks emit `event: keepalive`. Accepts the same filters + the `after` rowid anchor as `GET /tama/v1/logs`.",
                &["web-api", "logs"],
                &[("level", "query"), ("source", "query"), ("q", "query"), ("since", "query"), ("until", "query"), ("after", "query")],
                None,
            ),
        ),
        (
            "/tama/v1/logs/events",
            op(
                "get",
                "logEventsSse",
                "Log store status events (SSE)",
                "Server-sent status transitions of the structured log writer. `event: log_store_degraded` when the writer starts dropping (with `since`/backlog counters) and `event: log_store_restored` on recovery (`had_entries`/`ring_flushed`). Self-describing JSON frames; the endpoint exists so degraded storage is visible from the UI instead of being silent.",
                &["web-api", "logs"],
                None,
                None,
            ),
        ),
        (
            "/tama/v1/logs/export",
            op_q(
                "get",
                "exportLogs",
                "Export logs as CSV",
                "Exports the full query result set as CSV (content type `text/csv`, blob download). `format` must be `csv`; `413 PayloadTooLarge` when the window exceeds the hard cap (50,000 rows) — check the count with `/tama/v1/logs/summary` first.",
                &["web-api", "logs"],
                &[("level", "query"), ("source", "query"), ("q", "query"), ("since", "query"), ("until", "query"), ("format", "query")],
                None,
            ),
        ),
        (
            "/tama/v1/backup",
            op(
                "get",
                "createBackup",
                "Create backup archive",
                "Creates a tar.gz archive of config files and returns it as a download.",
                &["web-api"],
                None,
                None,
            ),
        ),
        (
            "/tama/v1/config",
            serde_json::json!({
                "operationId": "getConfig",
                "summary": "Get config file contents",
                "description": "Removed — use /tama/v1/config/structured",
                "deprecated": true,
                "tags": ["web-api"],
                "responses": {"200": {"description": "Success"}}
            }),
        ),
        (
            "/tama/v1/config",
            serde_json::json!({
                "operationId": "saveConfig",
                "summary": "Save config file contents",
                "description": "Removed — use /tama/v1/config/structured",
                "deprecated": true,
                "tags": ["web-api"],
                "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ConfigBody"}}}},
                "responses": {"200": {"description": "Success"}}
            }),
        ),
        (
            "/tama/v1/config/structured",
            op(
                "get",
                "getStructuredConfig",
                "Get structured config",
                "Returns the parsed config as a JSON object with typed sections.",
                &["web-api"],
                None,
                None,
            ),
        ),
        (
            "/tama/v1/config/structured",
            post_op(
                "saveStructuredConfig",
                "Save structured config",
                "Validates and saves the provided JSON as the Tama config file.",
                &["web-api"],
                Some(("StructuredConfigBody", "application/json")),
                None,
            ),
        ),
        (
            "/tama/v1/config/structured",
            patch_op_p(
                "patchStructuredConfig",
                "Update config (deep recursive merge)",
                "Update config with deep recursive field-level merge. Only provided fields change. `backends` section omitted (read-only).",
                &["web-api"],
                &[],
                Some(("ConfigPatchBody", "application/json")),
                None,
            ),
        ),
        // ── Providers ─────────────────────────────────────────────────────────
        (
            "/tama/v1/providers",
            op(
                "get",
                "listProviders",
                "List all providers",
                "Returns all registered inference providers (local and remote).",
                &["providers"],
                None,
                Some("Provider"),
            ),
        ),
        (
            "/tama/v1/providers",
            post_op(
                "createProvider",
                "Create a provider",
                "Registers a new inference provider. Local providers require tamad_id; remote providers require base_url.",
                &["providers"],
                Some(("CreateProviderRequest", "application/json")),
                Some("Provider"),
            ),
        ),
        (
            "/tama/v1/providers/{name}",
            op_p(
                "get",
                "getProvider",
                "Get a provider",
                "Returns a single provider by name.",
                &["providers"],
                &[("name", "path")],
                Some("Provider"),
            ),
        ),
        (
            "/tama/v1/providers/{name}",
            patch_op_p(
                "updateProvider",
                "Update a provider",
                "Updates a provider's base_url and/or api_key.",
                &["providers"],
                &[("name", "path")],
                Some(("UpdateProviderRequest", "application/json")),
                Some("Provider"),
            ),
        ),
        (
            "/tama/v1/providers/{name}",
            delete_op_p(
                "deleteProvider",
                "Delete a provider",
                "Removes a provider by name.",
                &["providers"],
                &[("name", "path")],
                Some("OkResponse"),
            ),
        ),
        // ── Tamads ─────────────────────────────────────────────────────────
        (
            "/tama/v1/tamads",
            op(
                "get",
                "listTamads",
                "List all tamads",
                "Returns all registered tamad (tamad daemon) connections.",
                &["tamads"],
                None,
                Some("TamadConnection"),
            ),
        ),
        (
            "/tama/v1/tamads",
            post_op(
                "createTamad",
                "Create a tamad",
                "Registers a new tamad connection. Auto-generates a UUID for the tamad id.",
                &["tamads"],
                Some(("CreateTamadRequest", "application/json")),
                Some("TamadConnection"),
            ),
        ),
        (
            "/tama/v1/tamads/{id}",
            op_p(
                "get",
                "getTamad",
                "Get a tamad",
                "Returns a single tamad connection by id.",
                &["tamads"],
                &[("id", "path")],
                Some("TamadConnection"),
            ),
        ),
        (
            "/tama/v1/tamads/{id}",
            patch_op_p(
                "updateTamad",
                "Update a tamad",
                "Updates a tamad's url and/or token.",
                &["tamads"],
                &[("id", "path")],
                Some(("UpdateTamadRequest", "application/json")),
                Some("TamadConnection"),
            ),
        ),
        (
            "/tama/v1/tamads/{id}",
            delete_op_p(
                "deleteTamad",
                "Delete a tamad",
                "Unregisters a tamad connection.",
                &["tamads"],
                &[("id", "path")],
                Some("OkResponse"),
            ),
        ),
        (
            "/tama/v1/tamads/{id}/health",
            post_op_p(
                "triggerHealthCheck",
                "Trigger tamad health check",
                "Stub endpoint — returns {\"status\": \"unknown\"} until tamad client is wired.",
                &["tamads"],
                &[("id", "path")],
                None,
                Some("HealthCheckResponse"),
            ),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Tama Web API",
            "description": "Endpoints served natively by the `tama-web` process (port 11435). All endpoints are prefixed with `/tama/v1/`.",
            "version": env!("CARGO_PKG_VERSION"),
            "license": {"name": "MIT"}
        },
        "servers": [{"url": "http://localhost:11435", "description": "Local Tama web UI (default port)"}],
        "tags": [
            {"name": "system", "description": "System health, capabilities, restart, and config reload"},
            {"name": "backends", "description": "Backend lifecycle — install, update, remove, versions, activate"},
            {"name": "jobs", "description": "Backend job status and SSE event streams"},
            {"name": "updates", "description": "Update checking and application for backends and models"},
            {"name": "pulls", "description": "Pull queue management — active, history, cancel, events"},
            {"name": "self-update", "description": "Self-update check, trigger, and progress streaming"},
            {"name": "restore", "description": "Backup/restore archive preview and restoration"},
            {"name": "models", "description": "Model config CRUD — create, read, update, delete, rename, verify"},
            {"name": "benchmarks", "description": "Benchmark runs, results, and history"},
            {"name": "web-api", "description": "Log viewing, config editing, and backup download"},
            {"name": "providers", "description": "Provider registry — create, list, update, delete inference providers"},
            {"name": "tamads", "description": "Tamad daemon connections — register, list, update, delete tamad instances"}
        ],
        "paths": paths,
        "components": {
            "schemas": schemas(),
            "securitySchemes": {
                "csrf": {
                    "type": "apiKey",
                    "name": "X-CSRF-Token",
                    "in": "header",
                    "description": "CSRF double-submit token. GET requests return the token in Set-Cookie and X-CSRF-Token header; POST/PUT/PATCH must include it in both cookie and header."
                }
            }
        },
        "security": [{"csrf": []}]
    })
}

fn schemas() -> serde_json::Value {
    let mut map = serde_json::Map::new();

    // Core responses
    map.insert(
        "OkResponse".into(),
        serde_json::json!({"type": "object", "required": ["ok"], "properties": {"ok": {"type": "boolean", "example": true}}}),
    );
    map.insert(
        "ModelMutationResponse".into(),
        serde_json::json!({"type": "object", "required": ["ok", "id"], "properties": {"ok": {"type": "boolean", "example": true}, "id": {"type": "integer", "format": "int64"}}}),
    );
    map.insert(
        "ErrorResponse".into(),
        serde_json::json!({
            "type": "object",
            "required": ["error"],
            "properties": {
                "error": {
                    "type": "object",
                    "required": ["message"],
                    "properties": {
                        "message": {"type": "string"},
                        "type": {"type": "string"}
                    }
                }
            }
        }),
    );
    // Log store read API (plan-195 task 4)
    map.insert(
        "LogsQueryResult".into(),
        serde_json::json!({
            "type": "object",
            "required": ["entries", "next_cursor"],
            "properties": {
                "entries": {"type": "array", "items": {"$ref": "#/components/schemas/LogEntryDto"}},
                "next_cursor": {"type": ["integer", "null"], "format": "int64"}
            }
        }),
    );
    map.insert(
        "LogEntryDto".into(),
        serde_json::json!({
            "type": "object",
            "required": ["id", "ts", "level", "source", "message"],
            "properties": {
                "id": {"type": "integer", "format": "int64", "description": "Store rowid; negative for on-demand legacy tail lines"},
                "ts": {"type": "integer", "format": "int64", "description": "unix ms"},
                "level": {"type": "string", "enum": ["trace", "debug", "info", "warn", "error"]},
                "level_known": {"type": "boolean", "description": "false only for legacy tail lines"},
                "source": {"type": "string"},
                "message": {"type": "string"},
                "fields": {"type": "object", "description": "Structured fields without the top-level message"},
                "legacy": {"type": "boolean", "description": "true = on-demand tail line, not a stored row"}
            }
        }),
    );
    map.insert(
        "LogSources".into(),
        serde_json::json!({
            "type": "object",
            "required": ["source_summary", "legacy_sources"],
            "properties": {
                "source_summary": {"type": "array", "items": {"type": "object", "required": ["source", "row_count", "latest_ts"], "properties": {"source": {"type": "string"}, "row_count": {"type": "integer", "format": "int64"}, "latest_ts": {"type": "integer", "format": "int64"}}}},
                "legacy_sources": {"type": "array", "items": {"type": "string"}, "description": "On-demand legacy tail sources (tamad hosts/models)"}
            }
        }),
    );
    map.insert(
        "LogSummary".into(),
        serde_json::json!({
            "type": "object",
            "required": ["total", "by_level"],
            "properties": {
                "total": {"type": "integer", "format": "int64"},
                "by_level": {"type": "array", "items": {"type": "object", "required": ["level", "count"], "properties": {"level": {"type": "string"}, "count": {"type": "integer", "format": "int64"}}}}
            }
        }),
    );
    map.insert(
        "LogStoreStatusDto".into(),
        serde_json::json!({
            "type": "object",
            "required": ["degraded", "degraded_since", "channel_len", "ring_len"],
            "properties": {
                "degraded": {"type": "boolean"},
                "degraded_since": {"type": ["integer", "null"], "format": "int64", "description": "unix ms"},
                "channel_len": {"type": "integer"},
                "ring_len": {"type": "integer"}
            }
        }),
    );
    map.insert(
        "JobResponse".into(),
        serde_json::json!({"type": "object", "required": ["job_id"], "properties": {"job_id": {"type": "string"}, "message": {"type": "string"}}}),
    );
    map.insert(
        "CheckResponse".into(),
        serde_json::json!({"type": "object", "required": ["triggered", "message"], "properties": {"triggered": {"type": "boolean"}, "message": {"type": "string"}}}),
    );

    // Backend schemas
    map.insert(
        "BackendEntry".into(),
        serde_json::json!({"type": "object", "required": ["name", "backend_type", "version"], "properties": {"name": {"type": "string"}, "backend_type": {"type": "string"}, "version": {"type": "string"}, "is_active": {"type": "boolean"}}}),
    );
    map.insert(
        "BackendVersion".into(),
        serde_json::json!({"type": "object", "required": ["version"], "properties": {"version": {"type": "string"}, "is_active": {"type": "boolean"}}}),
    );
    map.insert(
        "InstallRequest".into(),
        serde_json::json!({"type": "object", "required": ["backend_type", "version"], "properties": {"backend_type": {"type": "string"}, "version": {"type": "string"}}}),
    );
    map.insert(
        "UpdateRequest".into(),
        serde_json::json!({"type": "object", "required": ["backend_type"], "properties": {"backend_type": {"type": "string"}}}),
    );
    map.insert(
        "DefaultArgsRequest".into(),
        serde_json::json!({"type": "object", "required": ["default_args"], "properties": {"default_args": {"type": "array", "items": {"type": "string"}}}}),
    );
    map.insert(
        "ActivateRequest".into(),
        serde_json::json!({"type": "object", "required": ["version"], "properties": {"version": {"type": "string"}}}),
    );
    map.insert(
        "RegisterBackendRequest".into(),
        serde_json::json!({"type": "object", "required": ["name", "backend_type", "version"], "properties": {"name": {"type": "string"}, "backend_type": {"type": "string"}, "version": {"type": "string"}, "gpu_variant": {"type": "string", "default": "cpu"}, "docker_config": {"$ref": "#/components/schemas/DockerConfig"}}}),
    );
    map.insert(
        "RegisterBackendResponse".into(),
        serde_json::json!({"type": "object", "required": ["name", "backend_type", "version", "path", "installedAt"], "properties": {"name": {"type": "string"}, "backend_type": {"type": "string"}, "version": {"type": "string"}, "path": {"type": "string"}, "installed_at": {"type": "integer", "format": "int64"}, "gpu_variant": {"type": "string"}, "source": {"$ref": "#/components/schemas/InstallationSourceDto"}, "docker_config": {"$ref": "#/components/schemas/DockerConfigDto"}}}),
    );
    map.insert(
        "DockerConfig".into(),
        serde_json::json!({"type": "object", "required": ["image", "model_mount"], "properties": {"image": {"type": "string"}, "container_port": {"type": "integer", "default": 8000}, "model_mount": {"$ref": "#/components/schemas/DockerVolume"}, "volumes": {"type": "array", "items": {"$ref": "#/components/schemas/DockerVolume"}}, "devices": {"type": "array", "items": {"type": "string"}}, "gpus": {"type": ["string", "null"]}, "shm_size": {"type": ["string", "null"]}, "cap_adds": {"type": "array", "items": {"type": "string"}}, "security_opts": {"type": "array", "items": {"type": "string"}}, "group_adds": {"type": "array", "items": {"type": "string"}}}}),
    );
    map.insert(
        "DockerVolume".into(),
        serde_json::json!({"type": "object", "required": ["host_path", "container_path"], "properties": {"host_path": {"type": "string"}, "container_path": {"type": "string"}, "read_only": {"type": "boolean", "default": false}}}),
    );
    map.insert(
        "DockerConfigDto".into(),
        serde_json::json!({"type": "object", "properties": {"image": {"type": "string"}, "container_port": {"type": "integer"}, "model_mount": {"$ref": "#/components/schemas/DockerVolumeDto"}, "volumes": {"type": "array", "items": {"$ref": "#/components/schemas/DockerVolumeDto"}}, "devices": {"type": "array", "items": {"type": "string"}}, "gpus": {"type": ["string", "null"]}, "shm_size": {"type": ["string", "null"]}, "cap_adds": {"type": "array", "items": {"type": "string"}}, "security_opts": {"type": "array", "items": {"type": "string"}}, "group_adds": {"type": "array", "items": {"type": "string"}}}}),
    );
    map.insert(
        "DockerVolumeDto".into(),
        serde_json::json!({"type": "object", "properties": {"host_path": {"type": "string"}, "container_path": {"type": "string"}, "read_only": {"type": "boolean"}}}),
    );
    map.insert(
        "InstallationSourceDto".into(),
        serde_json::json!({"type": "object", "properties": {"kind": {"type": "string"}, "version": {"type": "string"}, "git_url": {"type": ["string", "null"]}, "commit": {"type": ["string", "null"]}}}),
    );

    // Job/Update schemas
    map.insert(
        "JobStatus".into(),
        serde_json::json!({"type": "object", "required": ["id", "status"], "properties": {"id": {"type": "string"}, "status": {"type": "string"}, "progress": {"type": "number"}, "error_message": {"type": ["string", "null"]}}}),
    );
    map.insert(
        "UpdatesListResponse".into(),
        serde_json::json!({"type": "object", "required": ["backends", "models"], "properties": {"backends": {"type": "array", "items": {"$ref": "#/components/schemas/UpdateCheckDto"}}, "models": {"type": "array", "items": {"$ref": "#/components/schemas/UpdateCheckDto"}}}}),
    );
    map.insert(
        "UpdateCheckDto".into(),
        serde_json::json!({"type": "object", "required": ["item_type", "item_id", "status"], "properties": {"item_type": {"type": "string"}, "item_id": {"type": "string"}, "update_available": {"type": "boolean"}, "status": {"type": "string"}}}),
    );

    // Download/Update schemas
    map.insert(
        "ModelUpdateRequest".into(),
        serde_json::json!({"type": "object", "required": ["quants"], "properties": {"quants": {"type": "array", "items": {"type": "string"}}}}),
    );
    map.insert(
        "ModelUpdateResponse".into(),
        serde_json::json!({"type": "object", "required": ["job_ids", "total"], "properties": {"job_ids": {"type": "array", "items": {"type": "string"}}, "total": {"type": "integer"}}}),
    );
    map.insert(
        "DownloadJob".into(),
        serde_json::json!({"type": "object", "required": ["id", "status"], "properties": {"id": {"type": "string"}, "status": {"type": "string"}, "progress": {"type": "number"}, "speed_mbps": {"type": ["number", "null"]}}}),
    );
    map.insert(
        "SelfUpdateCheck".into(),
        serde_json::json!({"type": "object", "required": ["current_version"], "properties": {"current_version": {"type": "string"}, "latest_version": {"type": ["string", "null"]}, "update_available": {"type": "boolean"}}}),
    );
    map.insert(
        "SelfUpdateTrigger".into(),
        serde_json::json!({"type": "object", "required": ["triggered"], "properties": {"triggered": {"type": "boolean"}, "message": {"type": "string"}}}),
    );

    // Restore schemas
    map.insert(
        "RestorePreviewResponse".into(),
        serde_json::json!({"type": "object", "properties": {"archive_name": {"type": "string"}, "tama_version": {"type": "string"}}}),
    );
    map.insert(
        "RestoreRequest".into(),
        serde_json::json!({"type": "object", "properties": {"upload_id": {"type": "string"}}}),
    );
    map.insert(
        "RestorePreviewRequest".into(),
        serde_json::json!({"type": "object", "properties": {"file": {"type": "string", "format": "binary"}}}),
    );

    // System schemas
    map.insert(
        "Capabilities".into(),
        serde_json::json!({"type": "object", "properties": {"cuda_versions": {"type": "array", "items": {"type": "string"}}, "rocm_versions": {"type": "array", "items": {"type": "string"}}, "vulkan_support": {"type": "boolean"}}}),
    );

    // Model schemas
    map.insert(
        "ModelConfig".into(),
        serde_json::json!({"type": "object", "required": ["id", "backend", "args", "enabled"], "properties": {"id": {"type": "string"}, "backend": {"type": "string"}, "model": {"type": ["string", "null"]}, "quant": {"type": ["string", "null"]}, "args": {"type": "array", "items": {"type": "string"}}, "enabled": {"type": "boolean"}}}),
    );
    map.insert(
        "ModelBody".into(),
        serde_json::json!({"type": "object", "required": ["id", "backend"], "properties": {"id": {"type": "string"}, "backend": {"type": "string"}, "model": {"type": ["string", "null"]}, "quant": {"type": ["string", "null"]}, "args": {"type": "array", "items": {"type": "string"}}, "enabled": {"type": "boolean"}}}),
    );
    map.insert(
        "ModelsResponse".into(),
        serde_json::json!({"type": "object", "required": ["models", "backends"], "properties": {"models": {"type": "array", "items": {"$ref": "#/components/schemas/ModelConfig"}}, "backends": {"type": "array", "items": {"type": "string"}}}}),
    );
    map.insert(
        "RenameRequest".into(),
        serde_json::json!({"type": "object", "required": ["new_id"], "properties": {"new_id": {"type": "string"}}}),
    );

    // Benchmark schemas
    map.insert(
        "BenchmarkRequest".into(),
        serde_json::json!({"type": "object", "required": ["model_id"], "properties": {"model_id": {"type": "string"}, "quant": {"type": ["string", "null"]}}}),
    );
    map.insert(
        "BenchmarkResult".into(),
        serde_json::json!({"type": "object", "required": ["id", "status"], "properties": {"id": {"type": "string"}, "model_id": {"type": "string"}, "status": {"type": "string"}, "results": {"type": ["object", "null"]}}}),
    );

    // Config schemas
    map.insert(
        "ConfigBody".into(),
        serde_json::json!({"type": "object", "required": ["content"], "properties": {"content": {"type": "string"}}}),
    );
    map.insert(
        "StructuredConfigBody".into(),
        serde_json::json!({"type": "object"}),
    );

    // PATCH schemas
    map.insert(
        "ModelPatchBody".into(),
        serde_json::json!({"type": "object", "properties": {"repo_id": {"type": ["string", "null"]}, "backend": {"type": ["string", "null"]}, "gpu_variant": {"type": ["string", "null"]}, "gpu_device": {"type": ["string", "null"]}, "model": {"type": ["string", "null"]}, "quant": {"type": ["string", "null"]}, "mmproj": {"type": ["string", "null"]}, "mtp_model": {"type": ["string", "null"]}, "args": {"type": ["array", "null"], "items": {"type": "string"}}, "sampling": {"type": ["object", "null"]}, "enabled": {"type": ["boolean", "null"]}, "context_length": {"type": ["integer", "null"]}, "num_parallel": {"type": ["integer", "null"]}, "port": {"type": ["integer", "null"]}, "api_name": {"type": ["string", "null"]}, "display_name": {"type": ["string", "null"]}, "gpu_layers": {"type": ["integer", "null"]}, "quants": {"type": ["object", "null"]}, "modalities": {"type": ["object", "null"]}, "kv_unified": {"type": ["boolean", "null"]}, "cache_type_k": {"type": ["string", "null"]}, "cache_type_v": {"type": ["string", "null"]}, "spec_decoding": {"type": ["object", "null"]}, "vllm": {"type": ["object", "null"]}, "metadata": {"type": ["object", "null"]}}}),
    );
    map.insert(
        "ConfigPatchBody".into(),
        serde_json::json!({"type": "object", "properties": {"general": {"type": ["object", "null"]}, "lifecycle": {"type": ["object", "null"]}, "proxy": {"type": ["object", "null"]}, "sampling_templates": {"type": ["object", "null"]}, "compaction": {"type": ["object", "null"]}}}),
    );
    map.insert(
        "BackendPatchBody".into(),
        serde_json::json!({"type": "object", "properties": {"default_args": {"type": ["array", "null"], "items": {"type": "string"}}, "default_env": {"type": ["array", "null"], "items": {"type": "string"}}, "health_check_url": {"type": ["string", "null"]}}}),
    );

    // Provider schemas
    map.insert(
        "Provider".into(),
        serde_json::json!({"type": "object", "required": ["id", "name", "provider_type", "engine", "created_at"], "properties": {"id": {"type": "integer", "format": "int64"}, "name": {"type": "string"}, "provider_type": {"type": "string", "enum": ["local", "remote"]}, "engine": {"type": "string"}, "tamad_id": {"type": ["string", "null"]}, "base_url": {"type": ["string", "null"]}, "api_key": {"type": ["string", "null"]}, "created_at": {"type": "integer", "format": "int64"}}}),
    );
    map.insert(
        "CreateProviderRequest".into(),
        serde_json::json!({"type": "object", "required": ["name", "provider_type", "engine"], "properties": {"name": {"type": "string"}, "provider_type": {"type": "string", "enum": ["local", "remote"]}, "engine": {"type": "string"}, "tamad_id": {"type": ["string", "null"]}, "base_url": {"type": ["string", "null"]}, "api_key": {"type": ["string", "null"]}}}),
    );
    map.insert(
        "UpdateProviderRequest".into(),
        serde_json::json!({"type": "object", "properties": {"base_url": {"type": ["string", "null"]}, "api_key": {"type": ["string", "null"]}}}),
    );

    // Tamad schemas
    map.insert(
        "TamadConnection".into(),
        serde_json::json!({"type": "object", "required": ["id", "name", "url", "protocol", "status"], "properties": {"id": {"type": "string"}, "name": {"type": "string"}, "url": {"type": "string"}, "protocol": {"type": "string", "enum": ["grpc", "http"]}, "token": {"type": ["string", "null"]}, "status": {"type": "string", "enum": ["unknown", "connected", "disconnected"]}}}),
    );
    map.insert(
        "CreateTamadRequest".into(),
        serde_json::json!({"type": "object", "required": ["name", "url", "protocol"], "properties": {"name": {"type": "string"}, "url": {"type": "string"}, "protocol": {"type": "string", "enum": ["grpc", "http"]}, "token": {"type": ["string", "null"]}}}),
    );
    map.insert(
        "UpdateTamadRequest".into(),
        serde_json::json!({"type": "object", "properties": {"url": {"type": ["string", "null"]}, "token": {"type": ["string", "null"]}}}),
    );
    map.insert(
        "HealthCheckResponse".into(),
        serde_json::json!({"type": "object", "required": ["status"], "properties": {"status": {"type": "string"}, "message": {"type": ["string", "null"]}}}),
    );

    serde_json::Value::Object(map)
}

// ── Path item builders ────────────────────────────────────────────────────────

fn param(name: &str, loc: &str) -> serde_json::Value {
    serde_json::json!({"name": name, "in": loc, "required": true, "schema": {"type": "string"}})
}

fn op(
    _method: &str,
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn op_p(
    _method: &str,
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn op_q(
    _method: &str,
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert("parameters".into(), serde_json::json!(params.iter().map(|(n, _)| {
        serde_json::json!({"name": n, "in": "query", "required": false, "schema": {"type": "string"}})
    }).collect::<Vec<_>>()));
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn op2(
    _method: &str,
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn post_op(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    op("post", op_id, summary, desc, tags, request, response)
}

fn post_op_p(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn post_op_pp(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn put_op_p(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn patch_op_p(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn delete_op_p(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn delete_op_pp(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn schema_ref(name: &str) -> serde_json::Value {
    serde_json::json!({"$ref": format!("#/components/schemas/{}", name)})
}

fn responses_map<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (code, schema) in entries {
        let mut resp = serde_json::Map::new();
        resp.insert("description".into(), "Success".into());
        resp.insert(
            "content".into(),
            serde_json::json!({"application/json": {"schema": schema_ref(schema)}}),
        );
        map.insert(code.to_string(), serde_json::Value::Object(resp));
    }
    if map.is_empty() {
        let mut default = serde_json::Map::new();
        default.insert("description".into(), "Success".into());
        map.insert("200".to_string(), serde_json::Value::Object(default));
    }
    serde_json::Value::Object(map)
}

/// Serves the OpenAPI 3.1.0 specification as JSON at `GET /tama/v1/docs`.
#[cfg(feature = "ssr")]
pub async fn serve_spec() -> impl IntoResponse {
    let spec = spec();
    (StatusCode::OK, Json(spec)).into_response()
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;
    use crate::api::error::error_response;

    /// OkResponse schema must match the struct shape used by handlers:
    /// `required == ["ok"]` (id absent on plain-site responses).
    /// ModelMutationResponse has both ok and id.
    #[tokio::test]
    async fn test_openapi_ok_response_schema_matches_struct() {
        let schemas = schemas();

        // OkResponse must require only "ok" — id is absent on plain-site responses.
        let required: Vec<&str> = schemas["OkResponse"]["required"]
            .as_array()
            .expect("OkResponse.required should be an array")
            .iter()
            .map(|v| v.as_str().expect("required items should be strings"))
            .collect();
        assert_eq!(
            required,
            vec!["ok"],
            "OkResponse.required should be [\"ok\"] — id is absent on plain-site responses"
        );

        // OkResponse must NOT have an id property — only ModelMutationResponse does.
        assert!(
            schemas["OkResponse"]["properties"]["id"].is_null(),
            "OkResponse should not have an id property — only ModelMutationResponse does"
        );

        // ModelMutationResponse must require both ok and id.
        let mutation_required: Vec<&str> = schemas["ModelMutationResponse"]["required"]
            .as_array()
            .expect("ModelMutationResponse.required should be an array")
            .iter()
            .map(|v| v.as_str().expect("required items should be strings"))
            .collect();
        assert_eq!(
            mutation_required,
            vec!["ok", "id"],
            "ModelMutationResponse.required should be [\"ok\", \"id\"]"
        );

        // ModelMutationResponse id must also be integer/int64.
        assert_eq!(
            schemas["ModelMutationResponse"]["properties"]["id"]["type"], "integer",
            "ModelMutationResponse.id.type should be \"integer\", not \"string\""
        );
    }

    /// The `ErrorResponse` schema must describe the nested error shape
    /// `{"error":{"message":"...","type":"..."}}` — not the flat
    /// `{"error":"..."}` shape.
    #[tokio::test]
    async fn test_error_response_schema_is_nested() {
        let schemas = schemas();

        // The "error" property must be an object, not a string.
        assert_eq!(
            schemas["ErrorResponse"]["properties"]["error"]["type"], "object",
            "ErrorResponse.error should be an object, not a string"
        );

        // The nested "message" property must be a string.
        assert_eq!(
            schemas["ErrorResponse"]["properties"]["error"]["properties"]["message"]["type"],
            "string",
            "ErrorResponse.error.message should be a string"
        );

        // "message" must be required inside the nested error object.
        let required = schemas["ErrorResponse"]["properties"]["error"]["required"]
            .as_array()
            .expect("required should be an array");
        assert!(
            required.contains(&serde_json::Value::String("message".to_string())),
            "message should be required in ErrorResponse.error"
        );

        // The schema must validate against an actual error body produced by
        // error_response().
        let response = error_response(
            axum::http::StatusCode::NOT_FOUND,
            "model not found",
            Some("NotFoundError"),
        );
        let body = response.into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("body should be readable");
        let error_json: serde_json::Value =
            serde_json::from_slice(&bytes).expect("body should be valid JSON");

        // The body should have the nested structure described by the schema.
        assert_eq!(error_json["error"]["message"], "model not found");
        assert_eq!(error_json["error"]["type"], "NotFoundError");
    }
}
