# The proxy spawns nothing — all self-hosted concerns live in tamad

Tama originally ran everything in one process: the proxy spawned and supervised backend
processes, pulled models to local disk, benchmarked local GPUs, and read local
CPU/memory stats. As part of the provider abstraction (plan-088) we decided this is wrong
for multi-host deployments: we decided the tama proxy **never spawns a backend process or
reads local hardware, even for single-node deployments**. All self-hosted concerns —
backend install/upgrade, process lifecycle, pulls, benchmarks, host/GPU stats — live in
**tamad**, a daemon on the inference host. The proxy always talks to at least one tamad
(localhost:50051 for single-node) over the shared gRPC/HTTP protocol, and to remote
providers (OpenAI/Anthropic) directly.

**Why:** a single code path for "manage a local backend" — the proxy orchestrates, any
tamad executes. Single-node → multi-node becomes a pure config change (register another
tamad), and the proxy stays deployable on machines with no GPUs. The invariant "proxy
spawns nothing, ever" keeps the boundary airtight: no fallback path, no duplicated
lifecycle machinery.

**Considered Options:**
- Proxy keeps a local-spawn mode for single-node — rejected: two code paths for
  lifecycle, installs, pulls, and stats; the monolith comes back by the front door.
- Hybrid: local mode deprecated but functional — rejected: "deprecated but functional"
  paths never die, and every new feature (pulls, benchmarks, host stats) would need to
  land in both.

**Consequences:**
- A single-node deployment runs two processes (tama + local tamad) and depends on
  localhost:50051 being reachable; the first-boot UX must handle "no tamad registered".
- The proxy's former local machinery (lifecycle, installations, pull queue, bench, system
  stats) must be moved into tamad before it can be deleted from the proxy.
- Auth between proxy and tamad (the stored token) becomes security-critical the moment
  a tamad listens on a non-loopback address.
