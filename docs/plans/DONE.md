# Completed Plans

## Recently Completed

| Plan | Description | PR | Git References |
|------|-------------|-----|---------------------|
| [Model Identity ConfigKey](done/plan-162-model-identity-configkey.md) | Introduce `ConfigKey` newtype as single derivation site for `repo_id.to_lowercase().replace('/', "--")`; replace 9 open-coded sites in tama-core + 5 in tama crate (WASM-safe mirror); extract case-preserving `card_slug` for card filenames; rename ambiguous `resolve_model_id` to `resolve_config_key`/`resolve_db_id` | #163 ✅ COMPLETED | [`048d44bc`](../../commit/048d44bc), [`6eddb183`](../../commit/6eddb183), [`7bc4b236`](../../commit/7bc4b236), [`432d6880`](../../commit/432d6880), [`b19c1e09`](../../commit/b19c1e09), [`8782ed97`](../../commit/8782ed97) |
| [Sharded GGUF Pull](done/plan-179-sharded-gguf-pull.md) | Support pulling sharded GGUF models where multiple files in a subdirectory (e.g. `UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf`) belong to a single quant — add `shards` field to QuantEntry, `group_sharded_quants` grouping function, `is_primary_shard` detection in verification, `is_safe_relative_path` validation + subdir creation, recursive `untracked_ggufs`, and frontend wizard shard handling | ✅ COMPLETED | [`8609e514`](../../commit/8609e514), [`bbb833cf`](../../commit/bbb833cf), [`71baea8b`](../../commit/71baea8b), [`dee87639`](../../commit/dee87639), [`a2d13316`](../../commit/a2d13316), [`8b7dc63f`](../../commit/8b7dc63f), [`7d9ad132`](../../commit/7d9ad132), [`66ce7770`](../../commit/66ce7770) |
| [Error Contract Unification](done/plan-161-error-contract.md) | Unify management API error responses on nested `{"error":{"message":"...","type":"..."}}` shape — add shared `json_error` helper in tama-core, migrate 53 flat sites in `crates/tama` + 51 sites in tama-core (tts.rs, pull/handlers.rs, api_keys.rs), fix OpenAPI `ErrorResponse` schema, add `assert_error_shape` test helper + 6 shape-assertion tests across 6 modules, migrate `backends/jobs.rs` `get_job` from bare `StatusCode` to `error_response` | #162 ✅ COMPLETED | [`53a463cc`](../../commit/53a463cc), [`1765f700`](../../commit/1765f700), [`6cc04560`](../../commit/6cc04560), [`3624a233`](../../commit/3624a233), [`d275e688`](../../commit/d275e688) |
| [DB Access Consolidation](done/plan-160-db-access-consolidation.md) | Repository as single API-layer access point — absorb model writes, collapse DTO/Record duplication, seal escape hatches + ApiKeyStore, share one Repository via WebState, amend ADR-0017 | #161 ✅ COMPLETED | [`90a06e0b`](../../commit/90a06e0b) |
| [Quick Fixes](done/plan-159-quick-fixes.md) | Break config↔proxy cycle, restore 501 stopgap, CSRF route + middleware tests, route /logout, SvelteKit→Leptos docs fix | #160 ✅ COMPLETED | [`7412709b`](../../commit/7412709b) |
| [Improved Logging](done/plan-158-improved-logging.md) | Fix stale logs bug — two-layer tracing (pretty console + JSON file), non-blocking writes via tracing-appender, config-respecting log level, GPU structured fields on inference events, JSON→human-readable formatting in logs API | #158 ✅ COMPLETED | [`03fbd767`](../../commit/03fbd767) |
| [Langfuse Web UI](done/plan-157-langfuse-web-ui.md) | Wire Langfuse config into web UI — WASM mirror types, structured config API, config editor form with credential validation | #157 ✅ COMPLETED | [`4a2fee2e`](../../commit/4a2fee2e), [`c6f2b2f8`](../../commit/c6f2b2f8), [`8fa8880e`](../../commit/8fa8880e) |
| [Langfuse Core Integration](done/plan-156-langfuse-core.md) | Langfuse observability — config persistence (migration 0037), telemetry structs, `langfuse-ergonomic` client wrapper, non-streaming + streaming request interception with energy cost | #156 ✅ COMPLETED | [`9e72c8b8`](../../commit/9e72c8b8) |
| [PATCH Endpoints](done/plan-155-patch-endpoints.md) | Add PATCH endpoints for models, config, and backends — surgical partial updates with deep recursive config merge | #155 ✅ COMPLETED | [`2c6a9119`](../../commit/2c6a9119), [`9c49a2a8`](../../commit/9c49a2a8), [`f5fa655e`](../../commit/f5fa655e), [`22556e3d`](../../commit/22556e3d), [`38acc38d`](../../commit/38acc38d) |
| [Fix PUT Model Wipes Optional Fields](done/plan-154-fix-put-model-wipes-optional-fields.md) | Fix `context_length`, `cache_type_k`, `cache_type_v` to use `.or(base.field)` merge in `apply_model_body()` — matches documented partial-update semantics | #154 ✅ COMPLETED | [`cc660fb5`](../../commit/cc660fb5), [`13366178`](../../commit/13366178) |
| [API Keys Web UI](done/plan-153-api-keys-web-ui.md) | Web UI page at `/tama/keys` for managing API keys — create, view, edit scopes, revoke — with one-time key reveal modal, active-only filter, and sidebar integration | #152 ✅ COMPLETED | [`98e20d36`](../../commit/98e20d36), [`a7f1850`](../../commit/a7f1850), [`b3934606`](../../commit/b3934606), [`d2c29b17`](../../commit/d2c29b17), [`8fcadfc`](../../commit/8fcadfc) |
| [API Keys](done/plan-152-api-keys.md) | Named, scoped API keys (`tama_XXXX`) stored as SHA-256 hashes, with auth + scope middleware and CRUD management API | #151 ✅ COMPLETED | [`a087b344`](../../commit/a087b344), [`c4f4d979`](../../commit/c4f4d979), [`21e593e`](../../commit/21e593e), [`c4f4d979`](../../commit/c4f4d979), [`db4eab95`](../../commit/db4eab95) |
| [OAuth2/OIDC Login](done/plan-151-oauth2-login.md) | Native OAuth2 login flow with session cookies, replacing Caddy forward_auth dependency | #150 ✅ COMPLETED | [`c6f2b2f8`](../../commit/c6f2b2f8), [`8fa8880e`](../../commit/8fa8880e) |
| [Naming and Cleanup](done/plan-150-naming-and-cleanup.md) | Rename download→pull in model subsystem (60+ files, DB migration _0034), typed handler responses, remove deprecated `loaded` field, ModelConfig design note | #149 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Web UI Splits](done/plan-149-web-ui-splits.md) | Split 3 large web UI files (types/config.rs, api/backends/manage.rs, pages/config_editor.rs) into focused modules | #148 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Testability](done/plan-148-testability.md) | Backend lifecycle trait abstractions (HealthChecker, ProcessSpawner, PortAllocator, ProcessChecker) + tests, update checker + download queue processor tests | #147 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Type Safety](done/plan-14[0-7]-type-safety.md) | GpuVendor + ModelState enums, DB tuples → typed records, config enums (RestartPolicy, LogLevel, CompactionDevice) | #146 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Architectural Refactoring](done/plan-14[0-7]-architectural-refactoring.md) | DB repository layer, ProxyState encapsulation + WebState extraction, move web_types to tama crate | #145 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [File Splits — tama-core](done/plan-14[0-7]-file-splits.md) | Split 5 god files (config/types.rs 1407, models.rs 1303, checker.rs 1079, forward.rs 1119, gpu/system.rs 1121) into focused modules | #144 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Model Editor Redesign](done/plan-14[0-7]-model-editor-redesign.md) | Replace side nav with pill-style tabs, sticky save bar, compact expandable sampling, reorganized sections (Settings/Hardware/Sampling/Files/Advanced), preset management | #142 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Dashboard Bar Charts](done/plan-14[0-7]-dashboard-bar-charts.md) | Replace dense sparkline area charts with vertical bar charts on dashboard stat cards (CPU, Memory, Network) — 30s buckets, avg aggregation, opacity-scaled bars | #141 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Model Sort + Group](done/plan-14[0-7]-model-sort-group.md) | Add sort and group controls to the dashboard Models section (GPU, Family, Vendor, Status) with localStorage persistence | #136–#140 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Model Editor GPU Isolation Polish](done/plan-1[0-3][0-9]-model-editor-gpu-isolation-polish.md) | Rename "GPU Device" to "GPU Isolation", change default to "None", style refresh button | #131 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [GPU Env-Var Isolation (UUID)](done/plan-1[0-3][0-9]-gpu-env-var-isolation.md) | Replace --device CLI flag with driver-level GPU isolation via env vars (ROCR/CUDA_VISIBLE_DEVICES) using hardware UUIDs; also fix mtp_model API response | #130 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [GPU Card Name + Separators](done/plan-1[0-3][0-9]-gpu-card-name-separators.md) | Add GPU product name + total VRAM subtitle under GPU label, horizontal separators in left/middle columns | #129 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Config TOML to SQLite](done/plan-1[0-3][0-9]-config-to-db.md) | Eliminate config.toml — move all global settings to typed SQLite tables, unified TOML→DB migration, remove loaded_from, 410 Gone for raw TOML endpoints | #128 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Config Page Missing Fields](done/plan-1[0-3][0-9]-config-page-missing-fields.md) | Add 4 missing config fields to web UI config editor: update_check_interval, download_queue_poll_interval_secs, authenticator_url, authenticator_skip_paths | #127 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Per-Model Inference Stats](done/plan-1[0-3][0-9]-per-model-inference-stats.md) | Per-model tok/s on GPU cards (HashMap keyed by server_name) + always show 0 tok/s when idle | #126 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Cancel Loading Model](done/plan-1[0-3][0-9]-cancel-loading-model.md) | Add Cancel button to model cards during loading state — kills backend process group and returns to idle | #125 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Split Large Files](done/plan-1[0-3][0-9]-split-large-files.md) | Split 4 files exceeding 1,000 LOC (lifecycle 1410, crud 1222, server 1172, download 1096) into focused sub-modules | #124 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [GPU Card Responsive Layout](done/plan-1[0-3][0-9]-gpu-card-responsive-layout.md) | Horizontal strip cards for 1-2 GPUs, portrait grid for 3+ — single internal structure reconfigured by CSS `:has()` | #123 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Network Dashboard Card](done/plan-1[0-3][0-9]-network-dashboard-card.md) | Replace GPU/VRAM stat cards with Network throughput card (↓/↑ MiB/s, dual-line sparkline) | #121 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [GPU Overview Dashboard](done/plan-1[0-3][0-9]-gpu-overview-dashboard.md) | Per-GPU device cards on the dashboard (util, VRAM, loaded model, telemetry) powered by per-device nvidia-smi/AMD sysfs queries | #120 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [GPU Device Selection](done/plan-1[0-3][0-9]-gpu-device-selection.md) | Add per-model GPU device assignment (gpu_device field, --device flag injection, --gpu-device CLI flag) for multi-GPU setups | #119 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Compaction Backend Card](done/plan-1[0-3][0-9]-compaction-backend-card.md) | Add compaction card to backends page with status and enable/disable toggle | #117 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Compaction Backend Lifecycle](done/plan-1[0-3][0-9]-compaction-backend-lifecycle.md) | Route compaction server through existing backend lifecycle (Kokoro TTS pattern) instead of custom subprocess management | #116 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [LLMLingua-2 Compaction Endpoint](done/plan-1[0-3][0-9]-llmlingua-compaction.md) | Add `/v1/compaction` endpoint that compresses prompts via Microsoft LLMLingua-2 (XLM-RoBERTa-large) before they hit the main LLM | #111–#114 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [SSE-Powered Updates Page](done/plan-1[0-3][0-9]-sse-updates-page.md) | Replace fire-and-forget refresh buttons with SSE-driven real-time updates on the updates page | #109 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Unified Dashboard Models](done/plan-1[0-3][0-9]-unified-dashboard-models.md) | Merge "Active Models" and "Inactive Models" sections into a single "Models" section on the dashboard | #108 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Backend Build Method Toggle](done/plan-1[0-3][0-9]-backend-build-method-toggle.md) | Add "Build from source" toggle on backend cards that persists to DB, letting users switch between prebuilt and source for updates | #107 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [/v1/opencode/models Capability Enrichment](done/plan-1[0-3][0-9]-opencode-models-capabilities.md) | Add tool_call, reasoning, attachment, temperature fields to /v1/opencode/models from backend /props | #105 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Shared Components Consolidation](done/plan-1[0-3][0-9]-list-card-refactor.md) | Consolidate duplicated UI patterns into 4 shared components: ListCard, SectionCard, AlertBanner, TabButtons | #102 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Extended Model Card Pips](done/plan-1[0-3][0-9]-model-card-pips.md) | Add GPU variant (combined with backend), KV cache quant, and speculative decoding indicator pips to model cards | #103 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Aliases Page Redesign](done/plan-1[0-3][0-9]-aliases-redesign.md) | Redesign aliases page with compact card layout, enabled dot indicator, proper page header, and dedicated CSS | #101 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Updates Center Fixes](done/plan-1[0-3][0-9]-updates-center-fixes.md) | Fix 5 issues: page header layout, Tama card consistency, missing variant badges, stale entries for deleted items, no refresh after backend update | #100 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Faster HF Downloads](done/plan-1[0-3][0-9]-faster-hf-downloads.md) | Replace hf-hub's slow downloader with enhanced parallel downloader + jitter backoff + auth headers; fix HF token passthrough for CLI | #99 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Split Remaining Long Files](done/plan-1[0-3][0-9]-split-remaining-files.md) | Split args_building.rs (2,256), handlers/tests.rs (1,530), and db/backfill.rs (1,023) into focused sub-modules | #97 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Move Web UI from /ui to /tama](done/plan-1[0-3][0-9]-move-ui-to-tama.md) | Consolidate all non-bearer-token endpoints under /tama — web UI at /tama, API at /tama/v1/* | #96 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Split proxy/handlers/mod.rs](done/plan-1[0-3][0-9]-split-proxy-handlers.md) | Split 2109 LOC file into 6 focused modules by responsibility | #93 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [/v1/models Meta Enrichment](done/plan-1[0-3][0-9]-v1-models-meta.md) | Forward /v1/models to backends for full GGUF meta, merge and inject ready | #92 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Wildcard Model Routing (whatevers-hot-n-fresh)](done/plan-1[0-3][0-9]-whatevers-hot-n-fresh.md) | Virtual model alias that routes to most-recently-accessed loaded LLM, or loads last-used model from DB as fallback | 🔁 SUPERSEDED by model-aliases | [`a7f1850`](../../commit/a7f1850) |
| [Model Aliases](done/plan-1[0-3][0-9]-model-aliases.md) | Replace hardcoded wildcard with user-managed global alias registry — DB table, ProxyState cache, handler integration, web API, and web UI | #95 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Authentik Auth Middleware](done/plan-1[0-3][0-9]-tama-authentik-auth.md) | Add Authentik API token validation middleware to tama proxy, supporting bearer tokens and Caddy forward_auth headers | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Merged /metrics Endpoint](done/plan-1[0-3][0-9]-merged-metrics.md) | Merge Tama proxy, backend (llama.cpp), and system (CPU/RAM/GPU/VRAM) metrics into Prometheus-format /metrics for Grafana | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [GGUF Metadata Parsing](done/plan-1[0-3][0-9]-gguf-metadata-parsing.md) | Parse GGUF file headers for authoritative model metadata, download queue with sequential processing, pull wizard rewrite with global SSE events, KV cache quantization in wizard | #90 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [MTP Benchmark](done/plan-1[0-3][0-9]-mtp-benchmark.md) | Add "MTP Testing" tab to Benchmarks page — sweep --spec-draft-n-max with --spec-type draft-mtp, 9 diverse prompts, per-prompt + aggregate metrics | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Spec Decoding Config](done/plan-1[0-3][0-9]-spec-decoding-config.md) | Add "Spec Decoding" section to model editor — checkboxes for draft-mtp/ngram-simple, n-max/n-min/draft-ngl params, injected as CLI flags | #91 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Remove llama.cpp Hardcoded Defaults](done/plan-1[0-3][0-9]-remove-llama-defaults.md) | Remove hardcoded llama_cpp and ik_llama backend entries from default config and template, making tama backend-agnostic from first boot | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Model Manager Centralization](done/plan-1[0-3][0-9]-model-manager-centralization.md) | Centralize all model DB access into a single ModelManager struct, replacing 29+ scattered db::open() calls across web, CLI, and proxy | #89 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Backend Manager Centralization](done/plan-0[0-9][0-9]-backend-manager-centralization.md) | Centralize all backend data access into a single BackendManager struct, replacing scattered db::queries calls and absorbing BackendRegistry | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Backend Config to Database](done/plan-0[0-9][0-9]-backend-config-to-db.md) | Move backend config (default_args, health_check_url) from config.toml to SQLite backend_configs table, keyed by (name, gpu_variant) with unique DB id | #88 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Startup Detection & Orphan Cleanup](done/plan-0[0-9][0-9]-startup-detection-and-orphan-cleanup.md) | Fix startup detection (2-consecutive health checks) and orphaned child process cleanup on startup failure | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Model Card Redesign](done/plan-0[0-9][0-9]-model-card-redesign.md) | Shared ModelCard component with accent strip, badge pills, and icon actions; replaces ModelRow and inline rendering | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [HF Metadata for Models](done/plan-0[0-9][0-9]-hf-metadata.md) | Add 9 HF metadata columns, populate from HF API + README parsing, display architecture on model cards | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Backend GPU Variant Restructure](done/plan-0[0-9][0-9]-backend-gpu-variant-restructure.md) | Restructure backend folders to type/variant/version, add gpu_variant to DB and queries, support multiple GPU variants per backend | #85 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Split pull.rs Into Submodules](done/plan-0[0-9][0-9]-split-pull-module.md) | Split 1,693-line models/pull.rs into 5 focused modules: api.rs, download.rs, metadata.rs, quant.rs | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Split config/resolve/tests.rs](done/plan-0[0-9][0-9]-split-resolve-tests.md) | Split 2,214-line test file into 4 topic-grouped modules | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Inference Stats Dashboard Cards](done/plan-0[0-9][0-9]-inference-stats-dashboard.md) | Surface llama_cpp timings (Processing Speed, Gen Speed, Cache Hits, Spec Accept) as 4 sparkline stat cards | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Shared Activity Panel + SSE Core](done/plan-0[0-9][0-9]-shared-activity-panel-and-sse-core.md) | Extract duplicated SSE reconnection logic into shared utility, create generic ActivityPanel UI shell | ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Metrics Snapshot Stream](done/plan-0[0-9][0-9]-metrics-snapshot-stream.md) | Replace delta SSE with full snapshot delivery every 2s, unify inference stats into same pipeline, eliminate frontend desync | #86 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |
| [Remove Windows Support](done/plan-0[0-9][0-9]-remove-windows-support.md) | Remove all Windows-specific code, CI, build targets, dependencies, and documentation | #87 ✅ COMPLETED | [`a7f1850`](../../commit/a7f1850) |

## Older Plans (Archived)

### Core Infrastructure
| Plan | Description |
|------|-------------|
| [Process Health Monitor](done/plan-0[0-9][0-9]-process-health-monitor.md) | Detect dead backend PIDs after Proxmox suspend/resume, auto-restart with max_restarts guard |
| [KV Unified Support](done/plan-0[0-9][0-9]-kv-unified-support.md) | Add --kv-unified flag support for llama-server shared KV cache pools |
| [Rename Kronk to Tama](done/plan-0[0-9][0-9]-rename-kronk-to-tama.md) | Complete rename across README, crates, routes, service names |
| [Split Large Files (Wave 1–4)](done/plan-0[0-9][0-9]-split-large-files.md) | Split CLI and core files into focused modules (multiple waves) |
| [Split Server Handler](done/plan-0[0-9][0-9]-split-server-handler.md) | Split handlers/server.rs and proxy/server.rs into submodules |

### CLI & Commands
| Plan | Description |
|------|-------------|
| [Bench Command](done/plan-0[0-9][0-9]-bench-command.md) | LLM inference benchmarking CLI command |
| [Status Command Redesign](done/plan-0[0-9][0-9]-status-command-plan.md) | Unified status command with /status endpoint, removed model ps |
| [Server Add/Edit Flag Extraction](done/plan-0[0-9][0-9]-server-add-flag-extraction-plan.md) | Extract tama flags from args, validate model cards |
| [Self-Update](done/plan-0[0-9][0-9]-self-update.md) | CLI `tama self-update` and web UI update button with GitHub release download |
| [Move Self-Update to Updates Center](done/plan-0[0-9][0-9]-move-self-update-to-updates-center.md) | Move self-update UI from sidebar to /updates page |

### Database & Storage
| Plan | Description |
|------|-------------|
| [SQLite DB and Model Update](done/plan-0[0-9][0-9]-sqlite-db-and-model-update.md) | SQLite database foundation with migration system |
| [DB Autobackfill and Process Tracking](done/plan-0[0-9][0-9]-db-autobackfill-and-process-tracking.md) | Active models table, backfill detection |
| [Backend Registry to DB](done/plan-0[0-9][0-9]-backend-registry-to-db.md) | Migrate from TOML to SQLite, add migration v3 |
| [Backup & Restore](done/plan-0[0-9][0-9]-backup-restore.md) | Backup config + DB archive, restore with model re-download and backend re-install |

### Backend Management
| Plan | Description |
|------|-------------|
| [Backend Naming and Version Pinning](done/plan-0[0-9][0-9]-backend-naming-and-config-version-pinning.md) | Canonical backend names, version pin field |
| [Backends Install/Update UI](done/plan-0[0-9][0-9]-backends-install-update-ui-spec.md) | Install, update, and check-updates for backends from web UI |
| [Fix Backend Default Args](done/plan-0[0-9][0-9]-fix-backend-default-args-spec.md) | Fix default_args display bug and add page-level save button |
| [ROCm Build Flags](done/plan-0[0-9][0-9]-rocm-build-flags.md) | Detect AMDGPU_TARGETS via rocminfo; add rocWMMA FA, FA_ALL_QUANTS, LLAMA_CURL |
| [Backend Version Cards](done/plan-0[0-9][0-9]-backend-version-cards.md) | Multiple backend versions with visual cards, activate/switch, version-specific remove |
| [TTS Backend Support](done/plan-0[0-9][0-9]-tts-backend.md) | Add Kokoro and Piper TTS engines with OpenAI-compatible `/v1/audio/*` endpoints |

### Model Management
| Plan | Description |
|------|-------------|
| [Unified Model Config](done/plan-0[0-9][0-9]-unified-model-config.md) | Merge model cards into ModelConfig with unified fields |
| [Integrate hf-hub for Authenticated Parallel Downloads](done/plan-0[0-9][0-9]-integrate-hf-hub-for-downloads.md) | Use hf-hub's authenticated client for gated/private repos |
| [Interactive Model Pull Wizard](done/plan-0[0-9][0-9]-interactive-model-pull-wizard.md) | Multi-step HF pull wizard with SSE progress |
| [Pull Quant from Model Editor](done/plan-0[0-9][0-9]-pull-quant-from-model-editor-spec.md) | Pull new quants via modal on model edit page |
| [mmproj Support](done/plan-0[0-9][0-9]-mmproj-support-spec.md) | Vision projector file support in pull wizard and model config |
| [API Name for Models](done/plan-0[0-9][0-9]-api-name-for-models.md) | Use HF repo names as model identifiers in OpenAI API |
| [Model Grid Separation](done/plan-0[0-9][0-9]-model-grid-separation.md) | Split model grid into loaded and unloaded sections |
| [Quant File Deletion](done/plan-0[0-9][0-9]-quant-file-deletion.md) | Delete GGUF files on quant removal, `tama model prune` command |
| [Preserve GGUF in Names](done/plan-0[0-9][0-9]-preserve-gguf-in-names.md) | Preserve -GGUF suffix in model IDs and paths |

### Web UI
| Plan | Description |
|------|-------------|
| [Web UI Redesign](done/plan-0[0-9][0-9]-web-ui-redesign.md) | Dark theme, nav bar, sparkline charts, dashboard polish |
| [Config Page Redesign](done/plan-0[0-9][0-9]-config-page-redesign-spec.md) | Real functional config editor with editable forms |
| [Model Editor Redesign](done/plan-0[0-9][0-9]-model-editor-redesign.md) | Side-nav layout, consolidated state, modular structure |
| [Collapsible Sidebar Navigation](done/plan-0[0-9][0-9]-sidebar-navigation.md) | Replace topbar with collapsible left sidebar |
| [Dashboard Metrics Redesign](done/plan-0[0-9][0-9]-dashboard-redesign.md) | Interactive sparkline cards with hover, history API |
| [Pull Model Modal Refactor](done/plan-0[0-9][0-9]-pull-model-modal-refactor.md) | Replace /pull page with modal on Models tab |
| [Pull Wizard Improvements](done/plan-0[0-9][0-9]-pull-wizard-improvements.md) | Consolidate quant/vision selection, smart KV cache dropdown, APEX/UD support |
| [Context Length Selector](done/plan-0[0-9][0-9]-context-length-selector.md) | Shared component for context length input with dropdown and custom value fallback |
| [KV Cache Quantization Dropdowns](done/plan-0[0-9][0-9]-kv-cache-quants.md) | Add K and V cache quantization dropdown selectors to model editor form |
| [Dashboard: Show All Models + Pull Model + Check All](done/plan-0[0-9][0-9]-dashboard-all-models.md) | Extend dashboard to show inactive models section, add Pull Model and Check all buttons |
| [Models Page Horizontal Layout](done/plan-0[0-9][0-9]-models-page-horizontal-layout.md) | Replace models page vertical card grid with horizontal row layout |
| [Benchmarks Page](done/plan-0[0-9][0-9]-benchmarks.md) | Web UI benchmarking page with llama-bench integration, SSE progress streaming |
| [Config Hot Reload](done/plan-0[0-9][0-9]-config-hot-reload.md) | Config sync from web UI to proxy without restart |

### Metrics & Dashboard
| Plan | Description |
|------|-------------|
| [Fix Dashboard Stale Stats](done/plan-0[0-9][0-9]-fix-dashboard-stale-stats.md) | Backfill metrics on SSE lag, tab visibility change, and SSE reconnect |
| [System Metrics](done/plan-0[0-9][0-9]-system-metrics.md) | CPU%, RAM, GPU metrics with background collection task |
| [Persist Dashboard Metrics](done/plan-0[0-9][0-9]-persist-dashboard-metrics.md) | SQLite persistence + SSE streaming for dashboard |
| [Dashboard Time Series Graphs](done/plan-0[0-9][0-9]-dashboard-time-series-graphs.md) | Sparkline SVG charts for metrics visualization |
| [Dashboard Filter Loaded Models](done/plan-0[0-9][0-9]-dashboard-filter-loaded-models.md) | Filter Active Models section to show only loaded (ready) models |

### Configuration, Lifecycle & Code Quality
| Plan | Description |
|------|-------------|
| [Grouped Args Formats](done/plan-0[0-9][0-9]-grouped-args-formats.md) | shlex helpers, grouped args format, auto-migration |
| [Proxy Shutdown](done/plan-0[0-9][0-9]-proxy-shutdown.md) | Graceful shutdown method for ProxyState |
| [System Restart](done/plan-0[0-9][0-9]-system-restart.md) | Process-level restart handler with graceful exit |
| [Updates Center](done/plan-0[0-9][0-9]-updates-center-plan.md) | Centralized `/updates` page with background checker, DB-cached results, and apply flows |
| [Test Coverage Improvements](done/plan-0[0-9][0-9]-core-test-coverage.md) | Add 98 unit tests across workspace |
| [Code Quality Improvements](done/plan-0[0-9][0-9]-code-quality-improvements.md) | Dead code cleanup, unused imports, formatting |
| [Fix Download Progress Bar](done/plan-0[0-9][0-9]-fix-download-progress-bar.md) | Content-Length parsing, finish_and_clear fixes |
| [Fix Review Bugs](done/plan-0[0-9][0-9]-fix-review-bugs.md) | Fix 40+ bugs from comprehensive code review |

### Discovery & Integration
| Plan | Description |
|------|-------------|
| [OpenCode Tama Plugin](done/plan-0[0-9][0-9]-opencode-tama-plugin.md) | Auto-discover models via /v1/models, provide modalities and config |
| [Proxy API Endpoints](done/plan-0[0-9][0-9]-proxy-api-endpoints.md) | Add all missing llama.cpp-compatible API endpoints using wildcard forwarding |
| [Max Loaded Models with LRU Eviction](done/plan-0[0-9][0-9]-max-loaded-models.md) | Add `max_loaded_models` config field (default=1) that auto-evicts least-recently-used model |
| [Speculative Decoding Benchmark](done/plan-0[0-9][0-9]-spec-decode-bench.md) | llama-cli based spec-decoding benchmark with sweep presets |
| [Backend Log Viewing](done/plan-0[0-9][0-9]-backend-log-viewing.md) | Grouped logs endpoint GET /tama/v1/logs returning all sources in one response |

---

## Superseded Plans

| Plan | Status |
|------|--------|
| [Dashboard Time Series Graphs](done/plan-028-dashboard-time-series-graphs.md) | 🔁 SUPERSEDED by persist-dashboard-metrics and dashboard-redesign |
| [Wildcard Model Routing (whatevers-hot-n-fresh)](done/plan-105-whatevers-hot-n-fresh.md) | 🔁 SUPERSEDED by Model Aliases |
| [Split Remaining Long Files (draft)](done/plan-094-split-remaining-files-spec.md) | 🔁 SUPERSEDED by updated plan |
| [Dashboard Model Management Spec](done/plan-002-dashboard-model-management-spec.md) | 🔁 SUPERSEDED by later plans |
| [Dashboard Model Management Plan](done/plan-001-dashboard-model-management-implementation-plan.md) | 🔁 SUPERSEDED by later plans |
| [MTP Draft Model Fixes](done/plan-137-mtp-draft-model-fixes.md) | 🔁 SUPERSEDED by GPU Env-Var Isolation (UUID) plan |

---

## Early Drafts & Specs

Companion specs absorbed into their associated implementation plans:

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
