# System API

Proxy-local management endpoints. **Since the tamad split (ADR-0010), the
proxy hosts no inference workloads and presents no local hardware** — every
hardware fact (GPUs, per-host CPU/RAM) comes from the registered tamad hosts
(`tamad_pool`). Endpoints that used to report proxy-local hardware now
aggregate the per-tamad results; zero-tamad deployments get empty arrays /
`null` fields (back-compat, no resolver).

## GET /tama/v1/system/capabilities

Reports the proxy host's **toolchain** facts (OS/arch/git/cmake/compiler) as
build-from-source hints for the install wizard. It does **not** probe local
GPU hardware — backend builds run on a tamad host. The flags describe the
*reporting* (proxy) host only; in multi-host topologies build execution
happens on the provider's tamad, which probes its own host before building,
so treat these values as informational wizard hints there.

Cached for 5 seconds.

**Response:**

```json
{
  "os": "linux",
  "arch": "x86_64",
  "cmake_available": true,
  "compiler_available": true,
  "git_available": true
}
```

> The old `detected_cuda_version` / `supported_cuda_versions` fields are gone:
> CUDA facts belong to the tamad hosts (see `gpu-devices` below).

## GET /tama/v1/system/health

Proxy process health **plus** one entry per registered tamad host.

**Response:**

```json
{
  "status": "ok",
  "service": "tama",
  "models_loaded": 2,
  "cpu_usage_pct": 3.1,
  "ram_used_mib": 812,
  "ram_total_mib": 16384,
  "gpu_utilization_pct": null,
  "vram": null,
  "version": "2.1.0",
  "uptime_seconds": 3641.2,
  "hosts": [
    {
      "tamad_id": "uuid-1",
      "name": "host-a",
      "online": true,
      "version": "9.9.9-stub",
      "cpu_percent": 42.5,
      "memory_used_pct": 50.0,
      "gpus_online": 2
    }
  ]
}
```

Field notes:

- Top-level fields describe the **proxy process** (its own CPU/RAM usage,
  uptime since start, binary version). `gpu_utilization_pct` and `vram` are
  legacy fields and are always `null` now (the proxy no longer samples local
  GPUs).
- `hosts[]` — one entry per tamad in the pool, built from the pool's latest
  stats snapshot (~1s fresh) + the cached `HealthCheck`:
  - `online` — the tamad's stats stream is currently connected
  - `version` — the tamad's self-reported version (last `HealthCheck`; `null`
    until the first successful check)
  - `cpu_percent`, `memory_used_pct`, `gpus_online` — from the latest stats
    snapshot; zeroed when the host has never streamed
- Zero-tamad deployments: `hosts: []` with the legacy top-level shape intact.

## GET /tama/v1/system/metrics/stream

SSE stream of proxy metrics snapshots (~2s cadence) with an additive
`hosts[]` field (per-tamad cpu/memory/gpus). See
[sse.md](./sse.md) for the full event and `hosts[]` schema.

## GET /tama/v1/system/gpu-devices

Lists the GPUs reported by **every registered tamad host**, each entry tagged
with the tamad name that owns it. The per-tamad stats streams keep this list
continuously fresh (~1s cadence).

Query params are kept for client compatibility but no longer select a local
device set:

- `backend` (required)
- `gpu_variant` (required)

> The previous 404 `Backend binary not found` behavior is gone: the endpoint
> no longer invokes local backend binaries, so it never fails on the proxy
> for that reason.

**Response:** JSON array of devices (one per GPU across all tamads; `[]` when
no tamads are registered or none report GPUs):

```json
[
  {
    "device_id": "GPU0",
    "name": "gfx-a",
    "vendor": "",
    "vram_total_mib": 16384,
    "vram_free_mib": 12288,
    "utilization_pct": 12.0,
    "temperature_c": 45.0,
    "tamad": "host-a"
  }
]
```

> **Multi-host note:** `device_id` is the index *on the owning tamad* — two
> hosts can each report `GPU0`. Disambiguate with the `tamad` tag. The model
> editor shows the host name in each option's label for exactly this reason.
>
> `vendor` is always `""` for per-tamad devices today: the gRPC `GpuInfo`
> message carries no vendor field yet (kept for response-shape stability;
> the frontend ignores it).

## POST /tama/v1/system/gpu-devices/refresh

Returns the current per-tamad GPU union — same response shape and query
params as `GET /tama/v1/system/gpu-devices`. Since per-tamad stats streams
keep device data fresh continuously, no local re-scan happens on this call.

## GET /tama/v1/system/restart

Triggers a graceful shutdown of the proxy process, then exits.

**Response:** `200` with body `Tama is shutting down`.
