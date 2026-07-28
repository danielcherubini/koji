# Implementation Plans

Plans for features, refactors, and bug fixes in the Tama project.

## Status Legend

| Status | Meaning |
|--------|---------|
| **Backlog** | Plan written and ready to execute |
| ✅ **COMPLETED** | Fully implemented, verified via git history |
| 🔁 **SUPERSEDED** | Replaced by another plan |

## Quick Stats

- **Total Plans**: 76
- **Backlog**: 6
- **Completed**: 69 ✅

> **Note**: The Tama Management API Spec (2026-04-03) was removed as it was a design document, not an implementation plan.

---

## Backlog

Current plans ready for execution, ordered by dependency-first cascade priority:

| Plan | Description | Findings |
|------|-------------|----------|
| [Server SSE Consolidation](plan-175-server-sse-consolidation.md) | serde-tagged PullEvent/UpdateEvent + shared job_event_stream | F12 |
| [Leptos UI Consolidation](plan-176-leptos-ui-consolidation.md) | Shared wasm-safe types via #[path] inclusion, collapse mirror types | F29, F31 |
| [ProxyState Sub-structs](plan-177-proxystate-substructs.md) | RegistryState/MetricsState/PullState composition | F32 |
| [Test Coverage Wave 2](plan-178-test-coverage-wave2.md) | Compaction/TTS via lifecycle traits, tama-mock integration | F22–F24, F36 |
| [SSE Benchmark Analysis](plan-180-sse-benchmark-analysis.md) | Compare 5 plan-175 pi sessions (Qwen baseline + 4 sampling params) — metrics, subagent durations, greptile grading | — |
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
