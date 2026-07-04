# Unified single-port server

The proxy server (port 11434) and web UI server (port 11435) were merged into a single Axum server on one port. The proxy (`tama-core`) became the sole entry point — when built with the `ssr` feature, it merges the web UI's API routes and static file serving into its own router. All web UI handlers accept `Arc<ProxyState>` instead of a separate `AppState`, eliminating inter-process HTTP proxying between the two servers.

The two-port architecture existed because the web UI was originally a separate Leptos/WASM crate with its own Axum server. After the SvelteKit migration (ADR-0003) and CLI removal (ADR-0005), maintaining two servers added unnecessary complexity — the web UI had to proxy requests to the proxy, doubling latency for management API calls and complicating error handling.

After the merge, all routes serve from a single port: `/v1/*` for OpenAI API, `/tama/v1/*` for management API, `/health`, `/status`, `/metrics` for system endpoints, and `/` for the embedded SPA.

**Status:** accepted

**Considered Options:**

- **Two separate servers** (status quo) — clean process isolation but doubled complexity, inter-process proxying, and two ports to manage
- **Single server with feature-gated routes** (chosen) — proxy owns the router; web routes are compiled in via `ssr` feature flag. Clean startup path: `tama` with no args starts the unified server
- **Reverse proxy** (nginx/Caddy in front) — adds external dependency and deployment complexity for a local app

**Consequences:**

- Good, because single port simplifies firewall rules, systemd config, and user setup
- Good, because web UI handlers share `ProxyState` directly — no HTTP round-trip to the proxy
- Good, because `AppState` was eliminated in favor of `ProxyState` with feature-gated web fields
- Bad, because the binary must be built with the `ssr` feature to include web routes (adds build step)
- Bad, because `tama-core` pulls in web dependencies when `ssr` is enabled (larger debug builds)
