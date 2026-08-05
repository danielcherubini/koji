# Docker config is separate from BackendSource

`BackendSource` (Prebuilt / SourceCode) describes *how a native binary was obtained*. Docker backends use a completely different runtime model — they don't have a local binary, they run containers. Nesting docker config inside `BackendSource` would conflate two orthogonal concerns: acquisition method vs runtime type.

Instead, `DockerConfig` is its own struct stored in a dedicated `docker_config` JSON column on `backend_installations`. Native backends leave it NULL; docker backends populate it. `BackendSource` remains NULL for docker installations.

**Considered Options:**
- Added a `Docker { ... }` variant to `BackendSource` — rejected because it mixes "where did this come from?" with "how does it run?", and would require every caller that inspects source (installer, updater) to handle a docker case that doesn't apply to them.
- Put docker config in `backend_configs` — rejected because it belongs with the installation record (image version, devices, volumes are per-installation, not per-backend-defaults like args/env).
