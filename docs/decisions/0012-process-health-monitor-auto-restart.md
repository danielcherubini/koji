# Process health monitor with auto-restart

The proxy runs a periodic health monitor (every 30s or `idle_timeout_secs / 2`) that verifies PID liveness for all `Ready` models and detects `Starting` models stuck beyond a timeout. When a dead PID is detected, the proxy auto-restarts the backend process (spawned, not awaited, to keep the health check tick fast). A `restart_count` field on `ModelState::Ready`/`Unloading` tracks auto-restarts; when it reaches `supervisor.max_restarts`, the model transitions to `Failed` instead of restarting again.

This replaced the earlier circuit breaker approach, which marked backends as unhealthy after consecutive failures but did not auto-restart. The health monitor was motivated by Proxmox LXC suspend/resume scenarios where backend PIDs became orphaned — the process was gone but the DB and in-memory state still thought it was running.

The monitor also handles stuck `Starting` states (e.g. backend binary crashed during model load) by checking `start_time: Instant` on the `Starting` variant. A TOCTOU revalidation under write lock ensures the PID is still alive before transitioning states.

**Status:** accepted

**Considered Options:**

- **Circuit breaker only** (status quo) — marked backends unhealthy after N failures but required manual restart
- **PID-based health monitor with auto-restart** (chosen) — periodic PID check + automatic restart with max limit. Handles LXC suspend/resume, OOM kills, and stuck startups
- **OS-level watchdog** (systemd Restart=on-failure) — only works for the Tama process itself, not child backend processes
- **Heartbeat from backend** — would require backend modifications; not feasible for third-party binaries like `llama-server`

**Consequences:**

- Good, because backends recover automatically from crashes, LXC suspend/resume, and OOM kills
- Good, because max_restarts limit prevents restart loops on permanently broken backends
- Good, because stuck `Starting` models are detected and cleaned up (no orphaned DB rows)
- Bad, because the 30s check interval means up to 30s of downtime before restart
- Bad, because TOCTOU between PID check and state transition requires write lock revalidation
