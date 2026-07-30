# Implementation Plans

Plans for features, refactors, and bug fixes in the Tama project.

## Status Legend

| Status | Meaning |
|--------|---------|
| **Backlog** | Plan written and ready to execute |
| ✅ **COMPLETED** | Fully implemented, verified via git history |
| 🔁 **SUPERSEDED** | Replaced by another plan |

## Quick Stats

- **Total Plans**: 79
- **Backlog**: 4
- **Completed**: 74 ✅

> **Note**: The Tama Management API Spec (2026-04-03) was removed as it was a design document, not an implementation plan.

---

## Backlog

Current plans ready for execution, ordered by dependency-first cascade priority:

**Full execution order & dependencies**: [execution-order.md](execution-order.md)  
**All completed plans**: [done.md](done.md)

| Plan | Description | Depends on |
|------|-------------|------------|
| [plan-180 Benchmark Bug Fixes](plan-180-benchmark-bug-fixes.md) | Fix spec/MTP history conversion arms, gpu_variant backend dropdown, history refresh, draft_ngl, success/partial/failed status, silent submit errors | — |
| [plan-181 Model Batch/µ-batch Fields](plan-181-model-batch-ubatch-fields.md) | First-class `n_batch`/`n_ubatch` model fields (migration `_0041`, args normalization, `-b`/`-ub` spawn injection, editor dropdowns, bench prefill) | — |
| [plan-183 Benchmark Suite](plan-183-benchmark-suite.md) | One-button capability-aware benchmark suite: GGUF nextn detection, `POST /benchmarks/suite` sequential job (ADR-0004), `suite_id` grouping (migration `_0042`), Suite tab + models-page button | plan-180, plan-181, plan-182 |

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

**Last Updated**: 2026-07-28
