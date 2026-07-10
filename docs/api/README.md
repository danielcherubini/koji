# TAMA Management API

REST API for managing TAMA — a local LLM proxy with OpenAI-compatible routing.

All endpoints are prefixed with `/tama/v1/`. The API uses JSON request/response bodies (`application/json`).

## Endpoints

| Section | File | Description |
|---------|------|-------------|
| [Models](models.md) | `models.md` | CRUD operations for model configurations |
| [Backends](backends.md) | `backends.md` | Install, manage, and update inference backends |
| [Aliases](aliases.md) | `aliases.md` | Short names that resolve to model configs |
| [Downloads](downloads.md) | `downloads.md` | Monitor file download progress |
| [HuggingFace](huggingface.md) | `huggingface.md` | Fetch model metadata and quant listings |
| [Config](config.md) | `config.md` | Read and update global TAMA settings |
| [Benchmarks](benchmarks.md) | `benchmarks.md` | Run and manage llama-bench benchmarks |
| [Updates](updates.md) | `updates.md` | Check and apply updates for backends and models |
| [Backup & Restore](backup.md) | `backup.md` | Archive and restore configurations |
| [Self-Update](self-update.md) | `self-update.md` | Update the TAMA binary itself |
| [System](system.md) | `system.md` | System capabilities and compaction backend |
| [Logs](logs.md) | `logs.md` | Retrieve TAMA and backend log output |

## Shared Resources

| Section | File | Description |
|---------|------|-------------|
| [SSE Streams](sse.md) | `sse.md` | Real-time event subscriptions (downloads, updates, jobs) |
| [Jobs](jobs.md) | `jobs.md` | Track async job progress and results |
| [Errors](errors.md) | `errors.md` | Error response format and common status codes |

## Base URL

By default TAMA listens on `http://127.0.0.1:18910`. All API paths below omit the base URL — prepend it to every request.

```bash
export TAMA=http://127.0.0.1:18910
curl $TAMA/tama/v1/models
```
