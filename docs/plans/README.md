# Implementation Plans

Plans for features, refactors, and bug fixes in the Tama project.

## Status Legend

| Status | Meaning |
|--------|---------|
| **Backlog** | Plan written and ready to execute |
| ✅ **COMPLETED** | Fully implemented, verified via git history |
| 🔁 **SUPERSEDED** | Replaced by another plan |

## Quick Stats

- **Total Plans**: 75
- **Backlog**: 15
- **Completed**: 59 ✅

> **Note**: The Tama Management API Spec (2026-04-03) was removed as it was a design document, not an implementation plan.

---

## Backlog

Current plans ready for execution, ordered by dependency-first cascade priority:

| Plan | Description | Findings |
|------|-------------|----------|
| [forward_request Tests](plan-165-forward-request-tests.md) | Dead-PID 502 + cleanup, circuit-breaker behavior | F9 |
| [Pull Handler Tests](plan-166-pull-handler-tests.md) | Validation, enqueue, job GET/SSE via wiremock HF | F10 |
| [Cleanup](plan-167-cleanup.md) | Delete ~1900 lines dead code, unused deps, style batch | F26, F34, F38, F39 |
| [API Handler Boilerplate](plan-168-api-handler-boilerplate.md) | Wire submit_benchmark_job, resolve_model_record, etc. | F13–F15, F20, F37 |
| [Router Consolidation](plan-169-router-consolidation.md) | Single-source route table (31 routes), fix shadowed /system/health | F33 |
| [Newtypes](plan-170-newtypes.md) | GpuType FromStr/serde, CompactionDevice 422, HfEndpoints | F16–F18 |
| [DB Query from_row](plan-171-db-query-from-row.md) | Per-record from_row + COLUMNS const for 6 record types | F30 |
| [File Splits Wave 2](plan-172-file-splits-wave2.md) | Split pull_queue.rs, api/updates.rs, auth.rs into module dirs | F11 |
| [Naming Domain Terms](plan-173-naming-domain-terms.md) | GpuType→GpuVariant, server→backend, download→pull, etc. | F27, F28, F40 |
| [Typed API Responses](plan-174-typed-api-responses.md) | StatusResponse/ModelEntry/OkResponse structs with golden shape tests | F19 |
| [Server SSE Consolidation](plan-175-server-sse-consolidation.md) | serde-tagged PullEvent/UpdateEvent + shared job_event_stream | F12 |
| [Leptos UI Consolidation](plan-176-leptos-ui-consolidation.md) | Shared wasm-safe types via #[path] inclusion, collapse mirror types | F29, F31 |
| [ProxyState Sub-structs](plan-177-proxystate-substructs.md) | RegistryState/MetricsState/PullState composition | F32 |
| [Test Coverage Wave 2](plan-178-test-coverage-wave2.md) | Compaction/TTS via lifecycle traits, tama-mock integration | F22–F24, F36 |
**Full execution order & dependencies**: [execution-order.md](execution-order.md)  
**All completed plans**: [done.md](done.md)

---

## Directory Structure

- `docs/plans/` — Backlog plans + this README
- `docs/plans/done/` — Completed plans (archived)
- `docs/plans/backlog.md` — Full backlog with phase descriptions
- `docs/plans/done.md` — All completed plans organized by category
- `docs/plans/execution-order.md` — Dependency-first cascade with phases and coordination flags

## How to Use This Directory

1. **Find a plan** — Browse the Backlog table above or read [backlog.md](backlog.md) for phase descriptions
2. **Check execution order** — See [execution-order.md](execution-order.md) for dependencies and priority
3. **Read the plan** — Understand the goal, architecture, and tasks
4. **Verify implementation** — Follow PR numbers or git references in [done.md](done.md)

## Contributing

When implementing a new feature:

1. Create a new plan file as `docs/plans/plan-NNN-<feature>.md` (NNN is the next sequential number)
2. Follow the template: Goal, Architecture, Tech Stack, Tasks
3. Mark tasks as `[ ]` (not started) or `[x]` (completed)
4. Link to related plans when applicable
5. When complete, move the plan file to `done/` and update [done.md](done.md)

## Related Files

- [`README.md`](../README.md) — Project overview
- [`AGENTS.md`](../AGENTS.md) — Development guide and conventions
- [`docs/openapi/tama-api.yaml`](../openapi/tama-api.yaml) — Machine-readable OpenAPI spec
- [`docs/openapi/openai-compat.yaml`](../openapi/openai-compat.yaml) — OpenAI-compatible API spec

---

**Last Updated**: 2026-07-18
