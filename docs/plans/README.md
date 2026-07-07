# Implementation Plans Overview

This directory contains implementation plans for the Tama project. Each plan documents a feature or refactor with clear goals, architecture, tasks, and verification steps.

## Status Legend

| Status | Meaning |
|--------|---------|
| **Backlog** | Plan written and ready to execute |
| ✅ **COMPLETED** | Fully implemented, verified via git history |
| 🔁 **SUPERSEDED** | Replaced by another plan |

## Quick Stats

- **Total Plans**: 47
- **Backlog**: 2
- **Completed**: 41 ✅

> **Note**: The Tama Management API Spec (2026-04-03) was removed as it was a design document, not an implementation plan. The functionality it describes is already implemented via other plans.

---

## Backlog

| Plan | Description |
|------|-------------|
| [Web UI Splits](plan-149-web-ui-splits.md) | Split 3 large web UI files (types/config.rs 986, api/backends/manage.rs 1013, pages/config_editor.rs 937) into focused modules |
| [Naming and Cleanup](plan-150-naming-and-cleanup.md) | Rename download→pull in model subsystem (15+ locations + DB migration), typed handler responses, remove deprecated fields, set rename_legacy deadline |

## Completed Plans

### Recently Completed

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Testability](done/plan-148-testability.md) | Backend lifecycle trait abstractions (HealthChecker, ProcessSpawner, PortAllocator, ProcessChecker) + tests, update checker + download queue processor tests | #147 ✅ COMPLETED |
| [Type Safety](done/plan-14[0-7]-type-safety.md) | GpuVendor + ModelState enums, DB tuples → typed records, config enums (RestartPolicy, LogLevel, CompactionDevice) | #146 ✅ COMPLETED |
| [Architectural Refactoring](done/plan-14[0-7]-architectural-refactoring.md) | DB repository layer, ProxyState encapsulation + WebState extraction, move web_types to tama crate | #145 ✅ COMPLETED |
| [File Splits — tama-core](done/plan-14[0-7]-file-splits.md) | Split 5 god files (config/types.rs 1407, models.rs 1303, checker.rs 1079, forward.rs 1119, gpu/system.rs 1121) into focused modules | #144 ✅ COMPLETED |
| [Model Editor Redesign](done/plan-14[0-7]-model-editor-redesign.md) | Replace side nav with pill-style tabs, sticky save bar, compact expandable sampling, reorganized sections (Settings/Hardware/Sampling/Files/Advanced), preset management | #142 ✅ COMPLETED |
| [Dashboard Bar Charts](done/plan-14[0-7]-dashboard-bar-charts.md) | Replace dense sparkline area charts with vertical bar charts on dashboard stat cards (CPU, Memory, Network) — 30s buckets, avg aggregation, opacity-scaled bars | #141 ✅ COMPLETED |
| [Model Sort + Group](done/plan-14[0-7]-model-sort-group.md) | Add sort and group controls to the dashboard Models section (GPU, Family, Vendor, Status) with localStorage persistence | #136, #137, #138, #139, #140 ✅ COMPLETED |
| [Model Editor GPU Isolation Polish](done/plan-1[0-3][0-9]-model-editor-gpu-isolation-polish.md) | Rename "GPU Device" to "GPU Isolation", change default to "None", style refresh button | #131 ✅ COMPLETED |
| [GPU Env-Var Isolation (UUID)](done/plan-1[0-3][0-9]-gpu-env-var-isolation.md) | Replace --device CLI flag with driver-level GPU isolation via env vars (ROCR/CUDA_VISIBLE_DEVICES) using hardware UUIDs; also fix mtp_model API response | #130 ✅ COMPLETED |
| [GPU Card Name + Separators](done/plan-1[0-3][0-9]-gpu-card-name-separators.md) | Add GPU product name + total VRAM subtitle under GPU label, horizontal separators in left/middle columns | #129 ✅ COMPLETED |
| [Config TOML to SQLite](done/plan-1[0-3][0-9]-config-to-db.md) | Eliminate config.toml — move all global settings to typed SQLite tables, unified TOML→DB migration, remove loaded_from, 410 Gone for raw TOML endpoints | #128 ✅ COMPLETED |
| [Config Page Missing Fields](done/plan-1[0-3][0-9]-config-page-missing-fields.md) | Add 4 missing config fields to web UI config editor: update_check_interval, download_queue_poll_interval_secs, authenticator_url, authenticator_skip_paths | #127 ✅ COMPLETED |
| [Per-Model Inference Stats](done/plan-1[0-3][0-9]-per-model-inference-stats.md) | Per-model tok/s on GPU cards (HashMap keyed by server_name) + always show 0 tok/s when idle | #126 `36456320`, `ec459321`, `b04ec5b9`, `858ac05e` ✅ COMPLETED |
| [Cancel Loading Model](done/plan-1[0-3][0-9]-cancel-loading-model.md) | Add Cancel button to model cards during loading state — kills backend process group and returns to idle | #125 `a12532b1`, `fe5570f6`, `c36b6120`, `c2bc9f7a`, `596a4233` ✅ COMPLETED |
| [Split Large Files](done/plan-1[0-3][0-9]-split-large-files.md) | Split 4 files exceeding 1,000 LOC (lifecycle 1410, crud 1222, server 1172, download 1096) into focused sub-modules | #124 `bbca22cb`, `ed3fe8a6`, `55bf5769`, `3625b42f`, `5821ef43` ✅ COMPLETED |
| [GPU Card Responsive Layout](done/plan-1[0-3][0-9]-gpu-card-responsive-layout.md) | Horizontal strip cards for 1-2 GPUs, portrait grid for 3+ — single internal structure reconfigured by CSS `:has()` | #123 `40b2969b` ✅ COMPLETED |
| [Network Dashboard Card](done/plan-1[0-3][0-9]-network-dashboard-card.md) | Replace GPU/VRAM stat cards with Network throughput card (↓/↑ MiB/s, dual-line sparkline) | #121 `d3f8ded5`, `64b16b55`, `faf483e8`, `b9847704`, `2cc6e44d` ✅ COMPLETED |
| [GPU Overview Dashboard](done/plan-1[0-3][0-9]-gpu-overview-dashboard.md) | Per-GPU device cards on the dashboard (util, VRAM, loaded model, telemetry) powered by per-device nvidia-smi/AMD sysfs queries | #120 ✅ COMPLETED |
| [GPU Device Selection](done/plan-1[0-3][0-9]-gpu-device-selection.md) | Add per-model GPU device assignment (gpu_device field, --device flag injection, --gpu-device CLI flag) for multi-GPU setups | #119 `006638e3`, `6950c964`, `791457cf`, `3ac05a3a` ✅ COMPLETED |
| [Compaction Backend Card](done/plan-1[0-3][0-9]-compaction-backend-card.md) | Add compaction card to backends page with status and enable/disable toggle | #117 ✅ COMPLETED |
| [Compaction Backend Lifecycle](done/plan-1[0-3][0-9]-compaction-backend-lifecycle.md) | Route compaction server through existing backend lifecycle (Kokoro TTS pattern) instead of custom subprocess management | #116 ✅ COMPLETED |
| [LLMLingua-2 Compaction Endpoint](done/plan-1[0-3][0-9]-llmlingua-compaction.md) | Add `/v1/compaction` endpoint that compresses prompts via Microsoft LLMLingua-2 (XLM-RoBERTa-large) before they hit the main LLM | #111, #112, #113, #114 ✅ COMPLETED |
| [SSE-Powered Updates Page](done/plan-1[0-3][0-9]-sse-updates-page.md) | Replace fire-and-forget refresh buttons with SSE-driven real-time updates on the updates page | #109 `7a1a39dc`, `6faed2bf`, `03dbbcfe`, `efefdcf`, `eef6916` ✅ COMPLETED |
| [Unified Dashboard Models](done/plan-1[0-3][0-9]-unified-dashboard-models.md) | Merge "Active Models" and "Inactive Models" sections into a single "Models" section on the dashboard | #108 `1f6cc9f` ✅ COMPLETED |
| [Backend Build Method Toggle](done/plan-1[0-3][0-9]-backend-build-method-toggle.md) | Add "Build from source" toggle on backend cards that persists to DB, letting users switch between prebuilt and source for updates | #107 ✅ COMPLETED |
| [/v1/opencode/models Capability Enrichment](done/plan-1[0-3][0-9]-opencode-models-capabilities.md) | Add tool_call, reasoning, attachment, temperature fields to /v1/opencode/models from backend /props | #105 ✅ COMPLETED |
| [Shared Components Consolidation](done/plan-1[0-3][0-9]-list-card-refactor.md) | Consolidate duplicated UI patterns into 4 shared components: ListCard, SectionCard, AlertBanner, TabButtons | #102 ✅ COMPLETED |
| [Extended Model Card Pips](done/plan-1[0-3][0-9]-model-card-pips.md) | Add GPU variant (combined with backend), KV cache quant, and speculative decoding indicator pips to model cards | #103 ✅ COMPLETED |
| [Aliases Page Redesign](done/plan-1[0-3][0-9]-aliases-redesign.md) | Redesign aliases page with compact card layout, enabled dot indicator, proper page header, and dedicated CSS | #101 ✅ COMPLETED |
| [Updates Center Fixes](done/plan-1[0-3][0-9]-updates-center-fixes.md) | Fix 5 issues: page header layout, Tama card consistency, missing variant badges, stale entries for deleted items, no refresh after backend update | #100 ✅ COMPLETED |
| [Faster HF Downloads](done/plan-1[0-3][0-9]-faster-hf-downloads.md) | Replace hf-hub's slow downloader with enhanced parallel downloader + jitter backoff + auth headers; fix HF token passthrough for CLI | #99 ✅ COMPLETED |
| [Split Remaining Long Files](done/plan-1[0-3][0-9]-split-remaining-files.md) | Split args_building.rs (2,256), handlers/tests.rs (1,530), and db/backfill.rs (1,023) into focused sub-modules | #97 ✅ COMPLETED |
| [Move Web UI from /ui to /tama](done/plan-1[0-3][0-9]-move-ui-to-tama.md) | Consolidate all non-bearer-token endpoints under /tama — web UI at /tama, API at /tama/v1/* | #96 ✅ COMPLETED |
| [Split proxy/handlers/mod.rs](done/plan-1[0-3][0-9]-split-proxy-handlers.md) | Split 2109 LOC file into 6 focused modules by responsibility | #93 ✅ COMPLETED |
| [/v1/models Meta Enrichment](done/plan-1[0-3][0-9]-v1-models-meta.md) | Forward /v1/models to backends for full GGUF meta, merge and inject ready | #92 ✅ COMPLETED |
| [Wildcard Model Routing (whatevers-hot-n-fresh)](done/plan-1[0-3][0-9]-whatevers-hot-n-fresh.md) | Virtual model alias that routes to most-recently-accessed loaded LLM, or loads last-used model from DB as fallback | `2048fb97`, `44c50a06`, `947f46b2`, `bcc95694`, `8e112e0a` 🔁 SUPERSEDED by model-aliases |
| [Model Aliases](done/plan-1[0-3][0-9]-model-aliases.md) | Replace hardcoded wildcard with user-managed global alias registry — DB table, ProxyState cache, handler integration, web API, and web UI | `c07193f2`, `cc9bdccc`, `6dff30f2`, `f21fcf4c`, `c11a7120`, `d122d403`, `4afee36f`, `fedd5789`, `16dee7ba` ✅ COMPLETED (#95) |
| [Authentik Auth Middleware](done/plan-1[0-3][0-9]-tama-authentik-auth.md) | Add Authentik API token validation middleware to tama proxy, supporting bearer tokens and Caddy forward_auth headers | `2c2fa70c` ✅ COMPLETED |
| [Merged /metrics Endpoint](done/plan-1[0-3][0-9]-merged-metrics.md) | Merge Tama proxy, backend (llama.cpp), and system (CPU/RAM/GPU/VRAM) metrics into Prometheus-format /metrics for Grafana | `340c7954` ✅ COMPLETED |
| [GGUF Metadata Parsing](done/plan-1[0-3][0-9]-gguf-metadata-parsing.md) | Parse GGUF file headers for authoritative model metadata, download queue with sequential processing, pull wizard rewrite with global SSE events, KV cache quantization in wizard | #90 ✅ COMPLETED |
| [MTP Benchmark](done/plan-1[0-3][0-9]-mtp-benchmark.md) | Add "MTP Testing" tab to Benchmarks page — sweep --spec-draft-n-max with --spec-type draft-mtp, 9 diverse prompts, per-prompt + aggregate metrics | `1ba9510` ✅ COMPLETED |
| [Spec Decoding Config](done/plan-1[0-3][0-9]-spec-decoding-config.md) | Add "Spec Decoding" section to model editor — checkboxes for draft-mtp/ngram-simple, n-max/n-min/draft-ngl params, injected as CLI flags | #91 ✅ COMPLETED |
| [Remove llama.cpp Hardcoded Defaults](done/plan-1[0-3][0-9]-remove-llama-defaults.md) | Remove hardcoded llama_cpp and ik_llama backend entries from default config and template, making tama backend-agnostic from first boot | `94184d8`, `41bd8b2`, `725758c` ✅ COMPLETED |
| [Model Manager Centralization](done/plan-1[0-3][0-9]-model-manager-centralization.md) | Centralize all model DB access into a single ModelManager struct, replacing 29+ scattered db::open() calls across web, CLI, and proxy | #89 ✅ COMPLETED |
| [Backend Manager Centralization](done/plan-0[0-9][0-9]-backend-manager-centralization.md) | Centralize all backend data access into a single BackendManager struct, replacing scattered db::queries calls and absorbing BackendRegistry | `e6b163c` ✅ COMPLETED |
| [Backend Config to Database](done/plan-0[0-9][0-9]-backend-config-to-db.md) | Move backend config (default_args, health_check_url) from config.toml to SQLite backend_configs table, keyed by (name, gpu_variant) with unique DB id | #88 ✅ COMPLETED |
| [Startup Detection & Orphan Cleanup](done/plan-0[0-9][0-9]-startup-detection-and-orphan-cleanup.md) | Fix startup detection (2-consecutive health checks) and orphaned child process cleanup on startup failure | `17baa64` ✅ COMPLETED |
| [Model Card Redesign](done/plan-0[0-9][0-9]-model-card-redesign.md) | Shared ModelCard component with accent strip, badge pills, and icon actions; replaces ModelRow and inline rendering | `85c75a5` ✅ COMPLETED |
| [HF Metadata for Models](done/plan-0[0-9][0-9]-hf-metadata.md) | Add 9 HF metadata columns, populate from HF API + README parsing, display architecture on model cards | `925efde` ✅ COMPLETED |
| [Backend GPU Variant Restructure](done/plan-0[0-9][0-9]-backend-gpu-variant-restructure.md) | Restructure backend folders to type/variant/version, add gpu_variant to DB and queries, support multiple GPU variants per backend | #85 `18c5d18` ✅ COMPLETED |
| [Split pull.rs Into Submodules](done/plan-0[0-9][0-9]-split-pull-module.md) | Split 1,693-line models/pull.rs into 5 focused modules: api.rs, download.rs, metadata.rs, quant.rs | `bb6c8f5` ✅ COMPLETED |
| [Split config/resolve/tests.rs](done/plan-0[0-9][0-9]-split-resolve-tests.md) | Split 2,214-line test file into 4 topic-grouped modules | `bb6c8f5` ✅ COMPLETED |
| [Inference Stats Dashboard Cards](done/plan-0[0-9][0-9]-inference-stats-dashboard.md) | Surface llama_cpp timings (Processing Speed, Gen Speed, Cache Hits, Spec Accept) as 4 sparkline stat cards | `4a88d10` ✅ COMPLETED |
| [Shared Activity Panel + SSE Core](done/plan-0[0-9][0-9]-shared-activity-panel-and-sse-core.md) | Extract duplicated SSE reconnection logic into shared utility, create generic ActivityPanel UI shell | `ca711f2` ✅ COMPLETED |
| [Metrics Snapshot Stream](done/plan-0[0-9][0-9]-metrics-snapshot-stream.md) | Replace delta SSE with full snapshot delivery every 2s, unify inference stats into same pipeline, eliminate frontend desync | #86 `309c895`, `5d920b7`, `aff3c15`, `b024266` ✅ COMPLETED |
| [Remove Windows Support](done/plan-0[0-9][0-9]-remove-windows-support.md) | Remove all Windows-specific code, CI, build targets, dependencies, and documentation | #87 `091b11f`, `5f6a1c4`, `91559b3`, `918e2dd`, `9d7dbf4`, `f1af925`, `8f30f52`, `3b8419f` ✅ COMPLETED |

### Completed Plans

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Process Health Monitor](done/plan-0[0-9][0-9]-process-health-monitor.md) | Detect dead backend PIDs after Proxmox suspend/resume, auto-restart with max_restarts guard, catch stuck Starting states | #80 `1af210f`, `a19b4a2`, `02bd651`, `59cac4c` |

### Core Infrastructure

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [KV Unified Support](done/plan-0[0-9][0-9]-kv-unified-support.md) | Add --kv-unified flag support for llama-server shared KV cache pools | #73 `b3e535a`, `ab3ea8a`, `341dd66`, `c48f4a9` |
| [Rename Kronk to Tama](done/plan-0[0-9][0-9]-rename-kronk-to-koji.md) | Complete rename across README, crates, routes, service names | `6d3a220`, `8281739`, `ab25016`, `bb8b734`, `d731eab` |
| [Split Large Files (Wave 1 & 2)](done/plan-0[0-9][0-9]-split-large-files.md) | Split CLI and core files into focused modules | #20 `9915565`, `57b1fe2`, `3ee005e` |
| [Split Large Files (Wave 3)](done/plan-0[0-9][0-9]-split-large-files.md) | Split remaining large files into domain submodules | #48 `b1e2f7d`, `8705ad0`, `7c6d50c` |
| [Split Large Files (Wave 4)](done/plan-0[0-9][0-9]-file-size-refactor.md) | Split remaining files >400 lines: model.rs, backends.rs, api.rs, gpu.rs, source.rs, backend.rs, model_editor/mod.rs | `69b7889` ✅ COMPLETED |
| [Split Server Handler](done/plan-0[0-9][0-9]-split-server-handler.md) | Split handlers/server.rs and proxy/server.rs into submodules | `a9b3a84`, `92c110f` |
| [Split Windows Platform](done/plan-0[0-9][0-9]-split-windows-platform.md) | Split platform/windows.rs into install, service, firewall, permissions | `5d20835` |

### CLI & Commands

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Bench Command](done/plan-0[0-9][0-9]-bench-command.md) | LLM inference benchmarking CLI command | `4bf65f7`, `5d54245`, `7549b2c` |
| [Status Command Redesign](done/plan-0[0-9][0-9]-status-command-plan.md) | Unified status command with /status endpoint, removed model ps | `4de3b5a`, `b077271`, `7a49b44` |
| [Server Add/Edit Flag Extraction](done/plan-0[0-9][0-9]-server-add-flag-extraction-plan.md) | Extract tama flags from args, validate model cards | `c8327c8`, `4de3b5a` |
| [Self-Update](done/plan-0[0-9][0-9]-self-update.md) | CLI `tama self-update` and web UI update button with GitHub release download | #56 `efd5459`, `0b47435`, `cc51c83`, `1bf5ee8`, `5587df1` |
| [Move Self-Update to Updates Center](done/plan-0[0-9][0-9]-move-self-update-to-updates-center.md) | Move self-update UI from sidebar to /updates page, keep minimal version indicator in sidebar | #62 `fa2cc94` ✅ COMPLETED |

### Database & Storage

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [SQLite DB and Model Update](done/plan-0[0-9][0-9]-sqlite-db-and-model-update.md) | SQLite database foundation with migration system | `e7e73e0`, `8d01ccb` |
| [DB Autobackfill and Process Tracking](done/plan-0[0-9][0-9]-db-autobackfill-and-process-tracking.md) | Active models table, backfill detection | `fe9efcb`, `1fa1f9d` |
| [Backend Registry to DB](done/plan-0[0-9][0-9]-backend-registry-to-db.md) | Migrate from TOML to SQLite, add migration v3 | `998256c`, `d9aa88f`, `e3565e9`, `e954552` |
| [Backup & Restore](done/plan-0[0-9][0-9]-backup-restore.md) | Backup config + DB archive, restore with model re-download and backend re-install | `ad77da6`, `b225b8c`, `58f13b3`, `07643e9` ✅ COMPLETED |

### Backend Management

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Backend Naming and Version Pinning](done/plan-0[0-9][0-9]-backend-naming-and-config-version-pinning.md) | Canonical backend names, version pin field | `bce6928`, `90898b4`, `211546d` |
| [Backends Install/Update UI](done/plan-0[0-9][0-9]-backends-install-update-ui-spec.md) | Install, update, and check-updates for backends from web UI | #43 `f500c27`, `89f71ed`, `32ae3f6`, `9a70c1e` |
| [Fix Backend Default Args](done/plan-0[0-9][0-9]-fix-backend-default-args-spec.md) | Fix default_args display bug and add page-level save button | #49 `aefe2fe`, `29b26fc`, `6bee43d` |
| [ROCm Build Flags](done/plan-0[0-9][0-9]-rocm-build-flags.md) | Detect AMDGPU_TARGETS via rocminfo; add rocWMMA FA, FA_ALL_QUANTS, LLAMA_CURL; export HIPCXX/HIP_PATH | `e862ab6`, `69d492a`, `c99304a`, `7698a11` ✅ COMPLETED |
| [Backend Version Cards](done/plan-0[0-9][0-9]-backend-version-cards.md) | Multiple backend versions with visual cards, activate/switch, version-specific remove | #61 |
| [TTS Backend Support](done/plan-0[0-9][0-9]-tts-backend.md) | Add Kokoro and Piper TTS engines with OpenAI-compatible `/v1/audio/*` endpoints, SQLite config, CLI commands, web UI integration | #70 `26c6a9d`, `79ea29b`, `38b072c`, `4738059`, `e1f63e7`, `88de610`, `3bb5c42`, `8c0c91c`, `f0277eb`, `cd7acfc`, `2e4c7c6`, `8ebfaa6` ✅ COMPLETED |

### Model Management

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Unified Model Config](done/plan-0[0-9][0-9]-unified-model-config.md) | Merge model cards into ModelConfig with unified fields | `95c8e01`, `13bc2d3`, `0be825a` |
| [Integrate hf-hub for Authenticated Parallel Downloads](done/plan-0[0-9][0-9]-integrate-hf-hub-for-downloads.md) | Use hf-hub's authenticated client for gated/private repos, fix slow start | `eac40cb` |
| [Interactive Model Pull Wizard](done/plan-0[0-9][0-9]-interactive-model-pull-wizard.md) | Multi-step HF pull wizard with SSE progress | `04d609d`, `abe6aff`, `1114a13` |
| [Pull Quant from Model Editor](done/plan-0[0-9][0-9]-pull-quant-from-model-editor-spec.md) | Pull new quants via modal on model edit page | #39 `d39e3e4`, `4b2803b`, `113da31` |
| [mmproj Support](done/plan-0[0-9][0-9]-mmproj-support-spec.md) | Vision projector file support in pull wizard and model config | #40 `0489cc0`, `d58aa67`, `492dd1a` |
| [API Name for Models](done/plan-0[0-9][0-9]-api-name-for-models.md) | Use HF repo names as model identifiers in OpenAI API | #47 `d659b9f`, `8edb7d9`, `0cf3ef6` |
| [Model Grid Separation](done/plan-0[0-9][0-9]-model-grid-separation.md) | Split model grid into loaded and unloaded sections | `43b5678`, `405632b`, `329be36` |
| [Quant File Deletion](done/plan-0[0-9][0-9]-quant-file-deletion.md) | Delete GGUF files on quant removal, `tama model prune` command | #50 `a160eb3`, `f350293`, `f6461d1`, `78c3feb` |
| [Preserve GGUF in Names](done/plan-0[0-9][0-9]-preserve-gguf-in-names.md) | Preserve -GGUF suffix in model IDs and paths | `c102bd0`, `58ad0b4` |
| [Num Parallel Slots](done/plan-0[0-9][0-9]-num-parallel-slots.md) | Add num_parallel field to model configs that multiplies effective context length at inference time | #66
| [Updates Center Fix](done/plan-0[0-9][0-9]-updates-center-fix.md) | Backend update progress (JobLogPanel), per-quant LFS hash checking, download queue integration, expandable quant UI with selection | #65
| [Migrate Profiles to Model Cards Tests](done/plan-0[0-9][0-9]-migrate_profiles_to_model_cards_tests.md) | Tests integrated into unified model config | `95c8e01` |
| [Model Card Cleanup](done/plan-0[0-9][0-9]-model-card-cleanup.md) | Part of unified model config | `95c8e01` |
| [Remove Profiles.d](done/plan-0[0-9][0-9]-remove-profiles-d.md) | Part of unified model config | `95c8e01` |

### Web UI

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Web UI Redesign](done/plan-0[0-9][0-9]-web-ui-redesign.md) | Dark theme, nav bar, sparkline charts, dashboard polish | `734623d`, `d585ba4`, `9dc78d3`, `502e2f6` |
| [Config Page Redesign](done/plan-0[0-9][0-9]-config-page-redesign-spec.md) | Real functional config editor with editable forms | #41 `0504eef`, `f28c104`, `519e9a2` |
| [Model Editor Redesign](done/plan-0[0-9][0-9]-model-editor-redesign.md) | Side-nav layout, consolidated state, modular structure | #51 `a7f1850`, `bdadc68`, `1666050` |
| [Collapsible Sidebar Navigation](done/plan-0[0-9][0-9]-sidebar-navigation.md) | Replace topbar with collapsible left sidebar | #55 `9fa3e67`, `f5046a4`, `592a40c`, `d9af7ad` |
| [Dashboard Metrics Redesign](done/plan-0[0-9][0-9]-dashboard-redesign.md) | Interactive sparkline cards with hover, history API | #54 `858bf61`, `34ce619`, `502e2f6` |
| [Pull Model Modal Refactor](done/plan-0[0-9][0-9]-pull-model-modal-refactor.md) | Replace /pull page with modal on Models tab | #44 `0907a4e`, `ec3abc3`, `8dc2a8f` |
| [Pull Wizard Improvements](done/plan-0[0-9][0-9]-pull-wizard-improvements.md) | Consolidate quant/vision selection, smart KV cache dropdown, APEX/UD support, HF cache cleanup | #58 `10a9d7f`, `603c403`, `3be54a8`, `db955e0`, `6af6423`, `ae1c8f1` |
| [Wizard & Cache Improvements](done/plan-0[0-9][0-9]-wizard-cache-improvements.md) | Fix KV dropdown, add APEX/UD quant support, implement HF cache cleanup | #58 `3be54a8`, `db955e0`, `6af6423`, `ae1c8f1` |
| [Context Length Selector](done/plan-0[0-9][0-9]-context-length-selector.md) | Shared component for context length input with dropdown and custom value fallback | #59 |
| [KV Cache Quantization Dropdowns](done/plan-0[0-9][0-9]-kv-cache-quants.md) | Add K and V cache quantization dropdown selectors to model editor form, wired through all layers to llama-server CLI flags | #77 ✅ COMPLETED |
| [Dashboard: Show All Models + Pull Model + Check All](done/plan-0[0-9][0-9]-dashboard-all-models.md) | Extend dashboard to show inactive models section, add Pull Model and Check all for updates buttons, hide Models from sidebar | #82 `75543f0`, `e273fa2`, `5d1794d`, `fc860f0`, `eec050f`, `bd969b7`, `4500d30` ✅ COMPLETED
| [Models Page Horizontal Layout](done/plan-0[0-9][0-9]-models-page-horizontal-layout.md) | Replace models page vertical card grid with horizontal row layout matching dashboard | #81 `fe94160` ✅ COMPLETED |
| [Benchmarks Page](done/plan-0[0-9][0-9]-benchmarks.md) | Web UI benchmarking page with llama-bench integration, SSE progress streaming, preset configs (Quick/VRAM Sweet Spot/Thread Scaling), and benchmark history | `dd869b8`–`4be90f7` ✅ COMPLETED |
| [Config Hot Reload](done/plan-0[0-9][0-9]-config-hot-reload.md) | Config sync from web UI to proxy without restart | `69cbb68`, `54298dc`, `219c749` |
| [Tama Web Control Plane](done/plan-0[0-9][0-9]-koji-web-control-plane.md) | Core UI — initial implementation | ✅ PARTIALLY COMPLETED |

### Metrics & Dashboard

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Fix Dashboard Stale Stats](done/plan-0[0-9][0-9]-fix-dashboard-stale-stats.md) | Backfill metrics on SSE lag, tab visibility change, and SSE reconnect to prevent stale stats after browser idle | #84 `21f1a65` ✅ COMPLETED |
| [System Metrics](done/plan-0[0-9][0-9]-system-metrics.md) | CPU%, RAM, GPU metrics with background collection task | `67029b2`, `2465a4d`, `11d9287` |
| [Persist Dashboard Metrics](done/plan-0[0-9][0-9]-persist-dashboard-metrics.md) | SQLite persistence + SSE streaming for dashboard | `b657e22`, `8e6a5b5`, `fd12bf8`, `4c6d6e2`, `2892764` |
| [Dashboard Time Series Graphs](done/plan-0[0-9][0-9]-dashboard-time-series-graphs.md) | Sparkline SVG charts for metrics visualization | `404f3be`, `6b651cf`, `9dc78d3`, `502e2f6` |
| [Dashboard Filter Loaded Models](done/plan-0[0-9][0-9]-dashboard-filter-loaded-models.md) | Filter Active Models section to show only loaded (ready) models with proper empty-state UX | #78 `8a20bff` ✅ COMPLETED |

### Configuration

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Grouped Args Formats](done/plan-0[0-9][0-9]-grouped-args-formats.md) | shlex helpers, grouped args format, auto-migration | `5c8fac1`, `3fbf27b`, `ae67a0b` |

### Lifecycle & Shutdown

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Proxy Shutdown](done/plan-0[0-9][0-9]-proxy-shutdown.md) | Graceful shutdown method for ProxyState | `6c83743`, `82ec8ab` |
| [System Restart](done/plan-0[0-9][0-9]-system-restart.md) | Process-level restart handler with graceful exit | `3a1b7a0`, `eea20ef`, `ec0fc08`, `0fe3ab5` |
| [Updates Center](done/plan-0[0-9][0-9]-updates-center-plan.md) | Centralized `/updates` page with background checker, DB-cached results, and apply flows | `2099edb`, `29fb946`, `9db8ccf`, `e2bbec8` ✅ COMPLETED |

### Code Quality

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Test Coverage Improvements](done/plan-0[0-9][0-9]-core-test-coverage.md) | Add 98 unit tests across workspace — proxy, lifecycle, downloads, updates, API DTOs, CLI handlers | #63 `7180eb6` ✅ COMPLETED |
| [Code Quality Improvements](done/plan-0[0-9][0-9]-code-quality-improvements.md) | Dead code cleanup, unused imports, formatting | `a93e639`, `423ec0b` |
| [Fix Download Progress Bar](done/plan-0[0-9][0-9]-fix-download-progress-bar.md) | Content-Length parsing, finish_and_clear fixes | `bc35068`, `bd9ea75`, `f052bba` |
| [Fix Review Bugs](done/plan-0[0-9][0-9]-fix-review-bugs.md) | Fix 40+ bugs from comprehensive code review: security vulnerabilities, data integrity, reactivity bugs, concurrency issues | #67 `8190b31` ✅ COMPLETED

### Discovery & Integration

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [OpenCode Tama Plugin](done/plan-0[0-9][0-9]-opencode-koji-plugin.md) | Auto-discover models via /v1/models, provide modalities and config | `f4530d6`, `dbf1e51`, `b1260e4` |
| [Proxy API Endpoints](done/plan-0[0-9][0-9]-proxy-api-endpoints.md) | Add all missing llama.cpp-compatible API endpoints using wildcard forwarding | #68 `3e1d180` ✅ COMPLETED |
| [Max Loaded Models with LRU Eviction](done/plan-0[0-9][0-9]-max-loaded-models.md) | Add `max_loaded_models` config field (default=1) that automatically evicts the least-recently-used model when capacity is reached | #69 ✅ COMPLETED |
| [Speculative Decoding Benchmark](done/plan-0[0-9][0-9]-spec-decode-bench.md) | llama-cli based spec-decoding benchmark with sweep presets (ngram-simple/mod/map-k/k4v), delta vs baseline results table | #71 `dd9c1c1` ✅ COMPLETED |
| [Backend Log Viewing](done/plan-0[0-9][0-9]-backend-log-viewing.md) | Grouped logs endpoint GET /tama/v1/logs returning all sources (tama + backends) in one response, Logs page with source dropdown selector, auto-refresh every 5s | #72 ✅ COMPLETED |

---

## Code Quality Backlog

Ideas from codebase review (2026-06-27) — architectural improvements and refactors, not bugs:

| Idea | Scope | Priority |
|------|-------|----------|
| **Use `strum`/`derive_more` for `BackendType`** | `backends/types.rs` — auto-derive `Display`, `FromStr`, `EnumString` to eliminate manual match boilerplate | Low |
| **Split `ProxyState` god struct** | `proxy/types.rs` — 20+ public fields mixing config, runtime, web UI, and DB access. Split into focused sub-structs (`ModelRegistry`, `MetricsCollector`, `DownloadManager`) for encapsulation and testability | Medium |
| **Extract `ensure_model_loaded` helper** | `proxy/handlers/` — The pattern `evict_lru_if_needed → get_model_card → load_model` appears in `chat.rs`, `forward.rs`, and `handle_forward_post`. Single shared function would centralize the flow | Medium |
| **Track spawned tasks with `JoinSet`** | `proxy/lifecycle/mod.rs` — stdout/stderr readers and reaper are spawned but not tracked. `JoinSet` per model would allow clean cancellation on `unload_model` | Low |

## Roadmap

Longer-term features that don't yet have implementation plans:

- **TUI Dashboard** — `tama-tui` crate with ratatui
- **System tray** — Windows tray icon for quick service toggle
- **Tauri GUI** — Lightweight desktop frontend for non-CLI users

## Superseded Plans

| Plan | Description | Status |
|------|-------------|--------|
| [Dashboard Time Series Graphs](done/plan-028-dashboard-time-series-graphs.md) | Superseded by persist-dashboard-metrics and dashboard-redesign | 🔁 SUPERSEDED |
| [Wildcard Model Routing (whatevers-hot-n-fresh)](done/plan-105-whatevers-hot-n-fresh.md) | Superseded by Model Aliases (2026-05-26) | 🔁 SUPERSEDED |
| [Split Remaining Long Files (draft)](done/plan-094-split-remaining-files-spec.md) | Superseded by updated plan (2026-05-27) | 🔁 SUPERSEDED |
| [Dashboard Model Management Spec](done/plan-002-dashboard-model-management-spec.md) | Early 2024 spec, superseded by later plans | 🔁 SUPERSEDED |
| [Dashboard Model Management Plan](done/plan-001-dashboard-model-management-implementation-plan.md) | Early 2024 plan, superseded by later plans | 🔁 SUPERSEDED |
| [MTP Draft Model Fixes](done/plan-137-mtp-draft-model-fixes.md) | Superseded by GPU Env-Var Isolation (UUID) plan (2026-07-01) | 🔁 SUPERSEDED |

## Early Drafts & Specs

These files are companion specs or early drafts that were absorbed into their associated implementation plans:

| File | Context |
|------|---------|
| [Dashboard Model Management Spec](done/plan-002-dashboard-model-management-spec.md) | Early 2024 spec, superseded by later plans |
| [Dashboard Model Management Plan](done/plan-001-dashboard-model-management-implementation-plan.md) | Early 2024 plan, superseded by later plans |
| [Status Command Spec](done/plan-006-status-command-spec.md) | Spec for status command redesign |
| [Server Add Flag Extraction Spec](done/plan-004-server-add-flag-extraction-spec.md) | Spec for flag extraction |
| [Config Page Implementation Plan](done/plan-034-config-page-implementation-plan.md) | Companion to config page spec |
| [mmproj Implementation Plan](done/plan-036-mmproj-support-plan.md) | Companion to mmproj spec |
| [Pull Quant from Model Editor Plan](done/plan-039-pull-quant-from-model-editor-plan.md) | Companion to pull-quant spec |
| [Backends Install/Update UI Plan](done/plan-041-backends-install-update-ui-plan.md) | Companion to backends spec |
| [Fix Backend Default Args Plan](done/plan-045-fix-backend-default-args-plan.md) | Companion to backend args spec |

---

## Directory Structure

- `docs/plans/` — Backlog plans (ready to execute) + this README
- `docs/plans/done/` — Completed plans (archived)

## How to Use This Directory

1. **Find a plan** — Browse the Backlog section above
2. **Read the plan** — Understand the goal, architecture, and tasks
3. **Verify implementation** — Follow PR numbers or git references to see commits
4. **Track backlog** — See "Backlog" section above

## Contributing

When implementing a new feature:

1. Create a new plan file as `docs/plans/plan-NNN-<feature>.md` (NNN is the next sequential number, zero-padded to 3 digits)
2. Follow the template: Goal, Architecture, Tech Stack, Tasks
3. Mark tasks as `[ ]` (not started) or `[x]` (completed)
4. Link to related plans when applicable
5. Add the plan to the Backlog table in this README
6. When complete, move the plan file to `done/` and update the README

## Related Files

- [`README.md`](../README.md) — Project overview
- [`AGENTS.md`](../AGENTS.md) — Development guide and conventions
- [`docs/openapi/tama-api.yaml`](../openapi/tama-api.yaml) — Machine-readable OpenAPI spec
- [`docs/openapi/openai-compat.yaml`](../openapi/openai-compat.yaml) — OpenAI-compatible API spec

---

**Last Updated**: 2026-07-07
