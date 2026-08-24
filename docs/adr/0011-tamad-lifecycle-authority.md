# The tamad is the source of truth for lifecycle; the proxy reads and steers

ADR-0010 fixed that the proxy never *spawns* processes, but the proxy kept *judges*
lifecycle central anyway: a `desired_models` table, a 1-second reconciler converging
actual to desire, and an in-memory `BackendState` "staging mirror" driving both the
UI's model state and request routing. On a real incident (2026-08-31) the proxy
restarted mid-load — the mirror went blank, so the UI showed `idle` while the tamad's
container served traffic, a hung load RPC showed nothing, and the empty `desired`
state pointed at a live container nobody owned. The central DB believed one thing,
the host another, and the UI showed a third.

We decided: **the tamad is the single source of truth per host** for what is desired
(models to keep alive), what is running, *how* each process was launched (its last
pushed `LoadSpec`, persisted on the host), and the crash-restart budget. The proxy
(**tama**) demotes to a **read/steer/route** plane: it consumes the tamad's already
streaming 1 Hz process snapshot, routes requests and renders states from it, and
steers via the existing `LoadModel`/`UnloadModel` RPCs. Desire has exactly one
owner; the UI shows only what a host reported.

**Why push, not pull:** configs already travel to the host — every `LoadModel` RPC
carries the full launch spec. Making the tamad durable therefore adds no protocol;
it adds one JSON file per model. Host reboot → systemd → tamad → boot sweep with
last-known specs, with zero dependency on the proxy.

**Considered Options:**
- *Desire stays in the proxy; only reporting becomes a projection* — rejected: a
  second source of truth remains, and a reconciler still arbitrates between them, so
  the UI can still be wrong while the host is right.
- *Tamad pulls fresh specs from the proxy on startup (always-current recovery)* —
  rejected: recovery gains a hard dependency on the control plane; the exact
  direction removed. Stale specs (recovered model served by an older backend
  install) are acceptable and self-heal on the next explicit load.
- *No auto-recovery after a tamad restart (loaded ≠ wanted)* — rejected: an
  always-on inference box must not lose every loaded model per host reboot.

**Consequences:**
- The proxy loses `reconciler.rs`, `desired_models`, the staging mirror, the
  `in_flight` registry, and the restart-budget tracker. Model state and routing
  derive from snapshots; `ensure_model_loaded` becomes three cases: route /
  503-fast with stage / dispatch-and-503.
- Tamad gains `<state_dir>/models/<config_key>.json` (`spec`, `desired`,
  `budget`) plus the boot sweep and budget enforcement. It becomes operable
  standalone (CLI over the existing gRPC health reopens the host) while the proxy
  is stopped.
- The proxy decides LRU evictions (only it sees request churn); tamads execute
  them. Install pinning, pulls, aliases stay in the proxy DB untouched.
- Zero new RPCs for v1. The wire is the `LoadSpec` payload and existing snapshot
  fields. The proxy's forward path keeps "503 model starting" until a snapshot
  flips ready; warm-up queues are a later feature, not this one.
- Rollout is order-sensitive: durable tamad first (fully compatible, boot sweep
  gated off), then the proxy flip, which is also individually roll-back.
