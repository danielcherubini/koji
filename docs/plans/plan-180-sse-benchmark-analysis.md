# SSE Consolidation Benchmark Analysis Plan

**Goal:** Produce a side-by-side comparison of 5 pi session runs that all executed the same task (plan-175 SSE consolidation) under different sampling parameters and a Qwen baseline, measuring execution quality, reasoning characteristics, and post-run greptile review outcomes.

**Architecture:** A two-stage hybrid pipeline. Stage 1 is a deterministic Node.js script (`scripts/benchmark-extract.js`) that parses the JSONL session logs and emits `benchmark-metrics.json`. Stage 2 spawns pi subagents: one runs the `greptile` skill on each completed branch and emits `benchmark-greptile.json`; a second reads both JSON files and produces a single markdown report at `docs/benchmarks/plan-175-sse-consolidation-benchmark.md`. No Rust code is touched — this is a pure data-analysis task.

**Tech Stack:** Node.js (built-in `fs`/`path`/`readline` only, no deps), pi subagents (sonnet for the report writer), the existing `greptile` skill.

---

## Background (read first)

The 5 benchmark runs, in chronological order:

1. **Qwen baseline** — model `qwen3.6-27b` (orchestrator) + `qwen3.6-35b-a3b` (general subagents). Branch: `feature/plan-175-server-sse-consolidation-qwen`.
2. **temp 0.5** — model `laguna-s-2.1` for main and all subagents. Branch: `feat/sse-consolidation-plan-175-temp-0.5`.
3. **temp 0.3 (aborted #1)** — model `laguna-s-2.1`. Aborted by user before `git branch -m` rename.
4. **temp 0.3 (aborted #2)** — model `laguna-s-2.1`. Aborted by user before `git branch -m` rename.
5. **temp 0.7 / top_p 0.95** — model `laguna-s-2.1` (eventually). Branch: `feature/plan-175-sse-consolidation-temp-0.7-top-p-0.95`. **Known anomaly:** the first several subagent dispatches in this session specified `model: "gemini-2.5-flash"` in their `subagent` toolCall `arguments` (the user reported "first 3 agent runs" — actual count in the data is 5). The main agent's own `message.model` was always `laguna-s-2.1`. The plan must detect and surface subagent-intended-model anomalies, not just main-agent-model anomalies.

Pi session logs live at `~/.pi/agent/sessions/--home-daniel-Coding-Rust-tama--/`. **Filenames are date-prefixed:** e.g. `2026-07-28T07-35-12-620Z_019fa7a6-29ec-7e33-9f87-05e16295440d.jsonl`. The bare ULID portion after the last `_` is the canonical session id (and may differ from the `id` field inside the first `session` event — do not assume they match). The script must resolve each mapped session id by globbing `*_<session_id>.jsonl` in the directory, not by appending `.jsonl` to the bare id. Subagent session logs as separate `<session-id>/<hex>/run-0/session.jsonl` files are **not** persisted for these July 2026 sessions — subagent data must be derived from `subagent` toolCall→toolResult pairs in the main log.

**Temperature / top_p are NOT recorded anywhere in the logs or provider config** — they were set per-request and never persisted. The sampling parameters can only be recovered from the branch name (when renamed) or from an explicit user-supplied session→variant map.

---

### Task 1: Write the extraction script `scripts/benchmark-extract.js`

**Context:**
The script must deterministically parse pi session JSONL logs and emit a single `benchmark-metrics.json` file with all 5 variants accounted for. The user wants this in pure Node.js (no extra dependencies) so it can be re-run on any future benchmark runs. The script runs from the `tama` project root and writes its output to the project root.

The script reads the explicit session-id→variant map from `benchmark-sessions.json` (a small file the user creates with one entry per variant). If a session in the map is missing or unreadable, the script exits non-zero with a clear error.

**Files:**
- Create: `scripts/benchmark-extract.js`
- Create: `benchmark-sessions.json` (template, see step 1 below)

**JSONL event schema** (every line is one event object — VERIFIED against actual logs):
- `{type: "session", id, timestamp, cwd, version: 3}` — first line of each session. The `id` is the session's internal id; the **filename ULID** (after the last `_`) is the canonical session id and may differ from this `id` field.
- `{type: "model_change", id, parentId, timestamp, provider, modelId}` — model in use after this point. The first `model_change` is the auto-selection cascade and is NOT the actual working model.
- `{type: "thinking_level_change", id, parentId, timestamp, thinkingLevel}` — `"off"` or `"high"` etc.
- `{type: "message", id, parentId, timestamp, message: {role, content, model, provider, usage, stopReason, api, isError, toolCallId, toolName}}` — the main event. `message.role` is `"user"`, `"assistant"`, or `"toolResult"`. For assistant messages, `message.model` is the model that produced this response (USE THIS for main_model and model_usage, not the first `model_change`). `content` is an array of typed blocks:
  - `{type: "text", text}` — visible text.
  - `{type: "thinking", thinking, thinkingSignature}` — reasoning block. `thinking` is a string.
  - `{type: "toolCall", id, name, arguments}` — `arguments` is an **object** (NOT a JSON string — do NOT `JSON.parse` it). For `name: "bash"`, read `arguments.command` (a string). For `name: "subagent"`, read `arguments.agent`, `arguments.task`, `arguments.async`.
- `{type: "toolResult", ...}` — **DOES NOT EXIST as a top-level event in current sessions.** Tool results are message events with `message.role === "toolResult"`. Use that to count `tool_failures`.
- `{type: "custom_message", customType, content, display, details, id, parentId, timestamp}` — subagent control signals, generally ignored.

**What to implement:**

A single-file Node.js script (use only `fs`/`path`/`readline` from stdlib — do NOT add npm dependencies). Run it with `node scripts/benchmark-extract.js` from the tama project root. It must:

1. **Read the session map.** Load `benchmark-sessions.json` from the project root. Expected shape:
   ```json
   {
     "sessions": [
       { "variant": "Qwen baseline",            "session_id": "019fa7a6-29ec-7e33-9f87-05e16295440d" },
       { "variant": "temp 0.5",                 "session_id": "..." },
       { "variant": "temp 0.3 (aborted #1)",    "session_id": "..." },
       { "variant": "temp 0.3 (aborted #2)",    "session_id": "..." },
       { "variant": "temp 0.7 / top_p 0.95",    "session_id": "..." }
     ]
   }
   ```
   The `session_id` is the bare ULID portion of the filename (after the date prefix and the last `_`). If the file is missing, print an error explaining how to create it (template provided) and exit 1. Session IDs can be discovered via `ls ~/.pi/agent/sessions/--home-daniel-Coding-Rust-tama--/ | awk -F_ '{print $NF}' | sed 's/.jsonl$//'` and cross-referencing with each session's first user message ("plan-175 SSE consolidation").

2. **Resolve file paths.** Read `fs.readdirSync(SESSIONS_DIR)` once. For each mapped `session_id`, find the unique file matching `*_<session_id>.jsonl` (the `*_` prefix is the date). If zero or more than one file matches, log a clear error and exit 1. Then read each resolved file line by line using `readline` for streaming (sessions can be large — the 0.3 loops may be megabytes). For each line, parse JSON and append to a per-session array. Skip lines that fail to parse (warn, don't crash). Set the per-session `session_id` to the bare ULID from the map (the canonical id), and `session_path` to the resolved absolute file path. Do not use the first `session` event's `id` field — it may differ from the filename ULID and is not what the user provided in the map.

3. **Compute session-level metrics:**
   - `variant` (from the map)
   - `session_id`, `session_path`, `session_start_ts` (from the `session` event's `timestamp`)
   - `last_event_ts` (max of all event `timestamp` values, top-level field)
   - `wall_clock_ms` (`Date.parse(last_event_ts) - Date.parse(session_start_ts)`)
   - `completion`:
     - "Yes" if a `git branch -m` bash toolCall is present in the session (i.e. the rename step was reached)
     - "Aborted" otherwise
   - `main_model`:
     - **Use the modal value of `message.model` across all assistant messages** (each assistant message has its own `message.model` field). Pick the value that appears most often; if there is a tie, pick the chronologically last one. This avoids the auto-selection cascade in the first `model_change`.
   - `model_usage` (object): a tally of how many assistant messages used each `message.model` value. Example: `{"laguna-s-2.1": 42, "gemini-2.5-flash": 3}`. Sum equals the total number of assistant messages. This captures the case where a session used the wrong model for a stretch of turns before switching — the modal value may still pass the correctness check, but the anomaly must remain visible in the report.
   - `thinking_level` (from the first `thinking_level_change.thinkingLevel`, or `null`)
   - `total_tokens` (sum of `usage.totalTokens` across all assistant messages; treat undefined `usage` as 0)
   - `tool_calls_total` (count of content blocks where `type === "toolCall"` in assistant messages)
   - `tool_failures` (**count of message events where `message.role === "toolResult"` AND `message.isError === true`** — NOT top-level toolResult events)
   - `branch_renamed_to` (the `<new>` part of the first matched `git branch -m` line, or `null`)
   - `thinking_block_count` (count of content blocks where `type === "thinking"` across all assistant messages)
   - `thinking_total_chars` (sum of `thinking.thinking.length` for every thinking block)
   - `thinking_total_lines` (sum of `thinking.thinking.split("\n").length`)
   - `text_total_chars` (sum of `text.text.length` for every `type: "text"` block in assistant messages only)
   - `avg_thinking_chars` (`thinking_total_chars / thinking_block_count`, or `0` if count is 0)
   - `avg_thinking_lines` (similar)
   - `thinking_pct_chars` (`100 * thinking_total_chars / max(1, thinking_total_chars + text_total_chars)`, 1 decimal)
   - `thinking_pct_time`: see step 4
   - `subagent_count` (count of `name === "subagent"` toolCall blocks in assistant messages)
   - `subagents` (array, see step 5)

4. **`thinking_pct_time` calculation:** Iterate ALL events (in chronological order) in the session. For every assistant message that contains at least one `type: "thinking"` content block, compute the delta using **top-level event `timestamp`** only (do not use `message.timestamp` — be consistent across event types): `current_event.timestamp - prev_event.timestamp`. Sum these deltas (in ms), then `100 * sum / wall_clock_ms`, rounded to 1 decimal. If `wall_clock_ms` is 0, output `0`.

5. **Subagent metrics from the main log.** Iterate assistant message events. For each `name === "subagent"` toolCall block, record:
   - `subagent_call_id` (the toolCall `id`)
   - `agent_name` (from `arguments.agent`, e.g. `"general"`)
   - `task_preview` (first 80 chars of `arguments.task` or `""`)
   - `dispatched_at` (the event's top-level `timestamp`)
   - `is_async` (from `arguments.async`, default false)
   - `model_at_dispatch` (the `message.model` of the assistant message that contained this toolCall — this is the main model's model at dispatch time, NOT the subagent's actual model)
   - `subagent_intended_model` (from `arguments.model` if present, else `null`. This is the model the main agent ASKED the subagent to use. An anomaly here means the main agent dispatched a subagent with the wrong model — e.g. the temp-0.7 run's first ~5 subagents were dispatched with `model: "gemini-2.5-flash"`.)
   - `completed_at`, `duration_ms`, `is_error` (filled in step 6)

6. **Match subagent calls to results.** Iterate the events again. For each `message.role === "toolResult"` event where `message.toolName === "subagent"` and `message.toolCallId` matches a `subagent_call_id` from step 5, set:
   - `completed_at` = the event's top-level `timestamp`
   - `duration_ms` = `Date.parse(completed_at) - Date.parse(dispatched_at)`
   - `is_error` = `message.isError === true`
   - Parse the `message.content` (it may be an array of typed blocks) for any status text — ignore for now.
   - If multiple results match (rare), keep the first chronologically.
   - If no result matches (subagent still running, or never completed), set `completed_at: null`, `duration_ms: null`, `is_error: null`.

7. **Model correctness check — main model only, with full-usage anomaly detection.** Subagent models are NOT directly verifiable from the main log (the `subagent` toolCall carries no `model` argument, only the dispatch-time main model). So:
   - For `"Qwen baseline"`: main expected substring `/qwen3\.6-27b/`. `model_correct: true` iff `main_model` matches the main expected. Set `subagent_model_unverifiable: true`.
   - For all other variants: main expected `laguna-s-2.1` (exact). `model_correct: true` iff `main_model` matches. Set `subagent_model_unverifiable: true`.
   - The subagent detail rows still record `model_at_dispatch` for informational display only — it is the main model at dispatch time, NOT the subagent's actual model. Do NOT compare it to any expected value.
   - **Non-expected model detection (independent of `model_correct`).** Two independent scans:
     1. **Main-agent models**: scan every distinct value in `model_usage` (tally of `message.model` on assistant messages). For each value that does NOT match the variant's `main_expected`, add to `non_expected_models_used[]` with `{model, count, source: "main_agent"}`. Sort by count descending.
     2. **Subagent-intended models**: scan every distinct value across `subagents[].subagent_intended_model` (skip nulls). For each value that does NOT match the variant's `subagent_expected` (e.g. for the 4 sampling runs, anything that isn't `laguna-s-2.1`; for Qwen, anything that doesn't contain `qwen3.6-35b-a3b`), add to `non_expected_models_used[]` with `{model, count, source: "subagent_dispatch"}`.
     - If `non_expected_models_used` is non-empty, also set `had_model_anomaly: true` on the session.
     - The temp-0.7 session MUST show `gemini-2.5-flash` with `source: "subagent_dispatch"` in `non_expected_models_used[]` (count will be 5, not 3 — the user's "first 3" recollection was approximate).
   - If `main_model` doesn't match expected, set `main_model_mismatch: {actual, expected}`. Do not populate `model_mismatches[]` (no subagent-vs-expected comparison is performed).

8. **Sort and write `benchmark-metrics.json`** in the order specified in the map (do NOT re-sort). Pretty-print with 2-space indent. Also print a one-line summary per session to stdout: `  Qwen baseline: 1h 12m, 1.2M tokens, 0 tool fails, 8 subagents, model=qwen3.6-27b`. If a session has `had_model_anomaly: true`, append `, model-anomaly=<comma-separated model:count pairs>` to the line.

9. **Exit non-zero** if any session in the map cannot be read, if its first event is not a `session` event, or if fewer than 5 sessions were processed. Print a clear error in that case.

**Steps:**
- [ ] Create `scripts/benchmark-extract.js` with the full implementation above.
- [ ] Create `benchmark-sessions.json` in the project root with the template shape (variant + session_id for each of the 5 sessions). Discover the temp-0.7 session_id from the new branch: `git log --all --pretty=format:"%H" | head -3` and cross-reference, or look for the most recent `019fa8b8-…` session. The other 4 session IDs are the same as on the original benchmark branches.
- [ ] Run `cd ~/Coding/Rust/tama && node scripts/benchmark-extract.js` from the project root.
  - Does it print 5 session summaries? Does `benchmark-metrics.json` exist and parse as valid JSON? Does it have all 5 entries in the order from the map?
  - If it crashes or produces wrong output, fix the script and re-run before continuing.
- [ ] Spot-check: open one identified session's source JSONL, pick 3 metrics (e.g. `wall_clock_ms`, `tool_failures`, `subagent_count`), and verify by hand that they match the script's output. If they don't, fix the script.
- [ ] Spot-check a subagent duration: find a `name: "subagent"` toolCall in the source log, find the matching toolResult by `toolCallId`, and verify `duration_ms` matches. If not, fix step 6.
- [ ] Verify the temp-0.7 session's `model_usage` includes `gemini-2.5-flash` with count ~3 and `laguna-s-2.1` with the bulk of the count, and that `had_model_anomaly: true` is set.
- [ ] Commit with message: `feat(scripts): add benchmark-extract.js for plan-175 session analysis`

**Acceptance criteria:**
- [ ] `scripts/benchmark-extract.js` runs without errors
- [ ] `benchmark-sessions.json` exists, has exactly 5 entries with valid session IDs that resolve to real files
- [ ] `benchmark-metrics.json` exists, is valid JSON, contains exactly 5 entries in the order from the map
- [ ] Every `subagents[]` entry has `dispatched_at`, `duration_ms` (or null), `agent_name`, `model_at_dispatch`
- [ ] `thinking_pct_time` is a number between 0 and 100 (or 0 if wall_clock is 0)
- [ ] `tool_failures` is a non-negative integer, manually verified to be > 0 for at least the Qwen baseline (it has 2 real failures)
- [ ] `main_model` matches the actual modal `message.model` for at least one session (spot-checked)
- [ ] `model_usage` is an object whose values sum to the total number of assistant messages in that session
- [ ] If a session used a non-expected model at any point, `non_expected_models_used[]` is non-empty and `had_model_anomaly: true` is set
- [ ] The temp-0.7 session specifically surfaces `gemini-2.5-flash` in `non_expected_models_used[]` with `source: "subagent_dispatch"` and count 5 (the actual data; the user's "first 3" was approximate)
- [ ] The script does not require any npm install (no `package.json` changes)
- [ ] If any mapped session is missing or unreadable, the script exits non-zero with a clear error message

---

### Task 2: Run greptile on each completed branch and capture results

**Context:**
The 3 completed branches (Qwen baseline, temp 0.5, temp 0.7 / top_p 0.95) need to be graded via the `greptile` skill. Greptile does not emit `confidence` (out of 5) or p0-p3 severity buckets — those are not in the skill's output. What it does emit (per the skill at `~/.agents/skills/greptile/SKILL.md`):
- An internal loop counter `iterations` (how many review→fix rounds ran)
- A list of findings classified as Actionable / Informational / Already-addressed
- A final state of "clean" (zero actionable findings) or "issues remain"

We capture the iterations, the actionable/informational counts, and the clean state. The user originally wanted a confidence score — that's documented as not available from the current skill.

**Files:**
- Create: `benchmark-greptile.json` (in the project root)
- (No code files created — this is a subagent task)

**What to implement:**

Spawn a single pi subagent with this exact task brief:

> For each of these 3 git branches, checkout the branch in this repo (`~/Coding/Rust/tama`), invoke the `greptile` skill (read `~/.agents/skills/greptile/SKILL.md` first to understand the exact output format and the loop control), let it loop review→fix until it reports clean or hits its abort condition, and capture the final state. **Important:** stop AFTER Phase 6 (the skill's exit decision) and BEFORE Phase 7's `ask()` — capture the summary and return. Do NOT proceed to open a PR or merge to main. Then write `benchmark-greptile.json` in the project root with this structure:
>
> ```json
> {
>   "branches": [
>     {
>       "branch": "feature/plan-175-server-sse-consolidation-qwen",
>       "variant": "Qwen baseline",
>       "iterations": 2,
>       "actionable_count": 0,
>       "informational_count": 5,
>       "verdict": "clean"
>     },
>     { "branch": "feat/sse-consolidation-plan-175-temp-0.5", "variant": "temp 0.5", ... },
>     { "branch": "feature/plan-175-sse-consolidation-temp-0.7-top-p-0.95", "variant": "temp 0.7 / top_p 0.95", ... }
>   ]
> }
> ```
>
> **Important:**
> - Discover branch names by listing `git branch -a` and matching against the variant labels (Qwen baseline, temp 0.5, temp 0.7 / top_p 0.95). If a branch cannot be found locally, try `git fetch --all` first, then list again. If still missing, write an entry with `"verdict": "branch not found"` and a clear `error` field, but continue with the other branches.
> - Read the greptile skill file at `~/.agents/skills/greptile/SKILL.md` to see exactly what fields it emits. The subagent running greptile must itself track: (a) the number of Phase 2→3→4 cycles it ran as `iterations` (the skill's internal loop guard caps this at 5); (b) the final counts of Actionable and Informational findings, obtained by parsing the last `greptile review show <ID> --json` output; (c) `verdict` = `"clean"` if zero actionable findings remain, `"issues remain"` otherwise. If the greptile CLI itself fails (not installed, auth error, review dispatch error), set `verdict` to `"greptile failed"` and record the error in an `error` field — do not fabricate counts in that case.

The subagent should return the contents of `benchmark-greptile.json` in its response so the calling agent can verify.

**Steps:**
- [ ] Read `~/.agents/skills/greptile/SKILL.md` to understand the skill's output format and how to invoke it.
- [ ] For each of the 3 completed branches: `git checkout <branch>` (or `git switch`), invoke the `greptile` skill, capture the final state, return to the original branch.
- [ ] Write `benchmark-greptile.json` in the project root with the structure above.
- [ ] Verify the JSON parses, has 3 entries (one per expected branch), each `iterations` is a non-negative integer, each `verdict` is in `{clean, issues remain, branch not found, greptile failed}`.
- [ ] Commit with message: `chore(benchmark): capture greptile results for 3 completed branches`

**Acceptance criteria:**
- [ ] `benchmark-greptile.json` exists, parses, has 3 branch entries
- [ ] All 3 entries have `iterations` (number ≥ 0), `actionable_count` (number), `informational_count` (number), `verdict` (string in `{clean, issues remain, branch not found, greptile failed}`)
- [ ] No `temp 0.3` branch was touched
- [ ] If a branch was missing, an explicit error is recorded (not a crash)
- [ ] NO `confidence` or `p0`-`p3` fields are fabricated (they do not exist in greptile's output)

---

### Task 3: Generate the final markdown report

**Context:**
The two JSON files from Tasks 1 and 2 need to be merged into a single human-readable report. The report must show the 5-variant comparison table, a subagent detail table (subagent durations are now recoverable from the main log), and an anomaly summary.

**Files:**
- Create: `docs/benchmarks/plan-175-sse-consolidation-benchmark.md`
- Create directory: `docs/benchmarks/` (if it doesn't exist)

**What to implement:**

Spawn a pi subagent with this exact task brief:

> Read `benchmark-metrics.json` and `benchmark-greptile.json` from the project root (`~/Coding/Rust/tama`). Both are well-formed JSON written by the previous tasks. Join them by `variant` label. Then produce a markdown report at `docs/benchmarks/plan-175-sse-consolidation-benchmark.md` with EXACTLY this structure:
>
> # Plan-175 SSE Consolidation Benchmark
>
> **Generated:** <ISO date>  
> **Sessions analyzed:** 5
>
> ## Main Comparison
>
> A markdown table with these columns IN THIS ORDER:
>
> | Variant | Completed | Duration | Tokens | Tool fails | Subs | Model OK | Think % (time) | Avg think latency | Avg think chars | Greptile iter | Confidence | P0 | P1 | P2 | P3 | Actionable | Informational | Verdict |
>
> Row formatting rules:
> - **Variant**: as it appears in the JSON, verbatim (e.g. `Qwen baseline`, `temp 0.5`, `temp 0.3 (aborted #1)`, `temp 0.3 (aborted #2)`, `temp 0.7 / top_p 0.95`)
> - **Completed**: `Yes` / `Aborted` (from `completion`)
> - **Duration**: format `wall_clock_ms` as `Xh Ym` (e.g. `1h 12m`, `8m 32s` if under an hour). Round to nearest minute for ≥1h, nearest second for <1h.
> - **Tokens**: format as `1.2M`, `250k`, etc. (divide by 1e6 for M, 1e3 for k, 0 decimals)
> - **Tool fails**: integer
> - **Subs**: integer (from `subagent_count`)
> - **Model OK**: `✅` if `model_correct` is `true`, else `❌` (followed by the expected model in parens if it fails, e.g. `❌ (expected qwen3.6-27b)`). If `had_model_anomaly` is `true`, append a footnote marker `†` and explain in the anomalies section below.
> - **Think % (time)**: `thinking_pct_time` as integer with `%` (e.g. `47%`)
> - **Avg think latency**: derive from `thinking_pct_time` and `wall_clock_ms` and `thinking_block_count` — compute `avg_thinking_latency_ms = (thinking_pct_time / 100) * wall_clock_ms / thinking_block_count` if `thinking_block_count > 0`, else `0`. Format as `X.Xs`.
> - **Avg think chars**: integer
> - **Greptile iter**: integer, or `—` if no greptile data
> - **Confidence**: `N/5` string (e.g. `5/5`), or `—`
> - **P0 / P1 / P2 / P3**: integers from `issue_counts`, or `—`
> - **Actionable / Informational**: integers, or `—`
> - **Verdict**: `clean` / `issues remain` / `N/A` / `—`
>
> ## Subagent Detail
>
> A markdown table with columns IN THIS ORDER:
>
> | Variant | Agent | Dispatched at | Duration | Model at dispatch | Error |
>
> One row per subagent across all 5 sessions. Order: by variant in the order listed above, then by `dispatched_at` ascending. Format `Duration` as `Xm Ys` (round to nearest second; e.g. `8m 12s`, or `—` if `duration_ms` is null). Format `Dispatched at` as `HH:MM:SS` (extract from the ISO timestamp; the user's local time is fine). Format `Error` as `✅` if `is_error` is false, `❌` if true, `—` if null.
>
> Below the table, add a single line: `Total subagent time: Xh Ym (sum of all completed subagent durations)`.
>
> ## Anomalies & Summary
>
> A free-form bullet list (3-7 bullets) covering:
> - **0.3 attempts**: confirm both were aborted, note their durations and any tool failures
> - **Subagent time**: total subagent time per variant and overall. The user cares most about this.
> - **Model correctness**: state which sessions had mismatches (likely none for the 4 sampling runs; the Qwen baseline uses short-form ids `qwen3.6-27b` / `qwen3.6-35b-a3b` so document this). **List every session with `had_model_anomaly: true`** — for each, list `non_expected_models_used[]` with model name and turn count. This is independent of `model_correct` (the modal may still be correct, but a stretch of wrong-model usage is a real anomaly worth surfacing).
> - **Greptile ranking**: order the 3 completed branches by composite score (p0 * 100 + p1 * 10 + actionable_count + (1 if issues remain else 0)). State the best (lowest score) and worst (highest score), and call out the confidence (out of 5) for each.
> - **Tool failure correlation**: if any aborted session has tool failures, note them as probable abort triggers.
> - **Reasoning patterns**: 1-2 sentences comparing avg think chars and latency across variants.
>
> After writing the file, PRINT THE FULL FILE CONTENTS to stdout (cat it) so the user sees it immediately.

**Steps:**
- [ ] Read both `benchmark-metrics.json` and `benchmark-greptile.json` from the project root.
- [ ] Create `docs/benchmarks/` if it doesn't exist.
- [ ] Generate the report file and print its contents.
- [ ] Verify the table has exactly 5 rows in the main table and one row per subagent in the subagent table.
- [ ] Verify that any session with `had_model_anomaly: true` is explicitly called out in the anomalies section.
- [ ] Commit with message: `docs(benchmark): plan-175 SSE consolidation benchmark report`

**Acceptance criteria:**
- [ ] `docs/benchmarks/plan-175-sse-consolidation-benchmark.md` exists
- [ ] Main table has exactly 5 rows, in the order from the session map
- [ ] All 14 columns are present in the main table
- [ ] Subagent table has one row per subagent (count = sum of `subagent_count` across the 5 sessions)
- [ ] Subagent duration total line is present and correctly summed
- [ ] Anomaly section has at least 3 bullets
- [ ] Any session with `had_model_anomaly: true` is called out in the anomaly section with model name and count
- [ ] Greptile columns show `—` for the 2 aborted variants
- [ ] The report is also printed to stdout so the user sees it without opening the file

---

### Task 4: Final verification

**Context:**
A final sanity check before declaring this done. We want to catch any silent failures (e.g. wrong session matched to wrong variant, missing data, etc.).

**Files:** none

**What to implement:**

Run these verifications and report results to the user:

1. Confirm all 5 expected variant labels from `benchmark-sessions.json` are present in `benchmark-metrics.json` in the same order.
2. Confirm `benchmark-greptile.json` has exactly 3 entries.
3. Confirm the report file at `docs/benchmarks/plan-175-sse-consolidation-benchmark.md` exists.
4. Sum the `duration_ms` of completed subagents (non-null) in the subagent table of the report and confirm it matches the sum from `benchmark-metrics.json`.
5. If any session has `had_model_anomaly: true`, confirm the anomaly section explicitly lists the model name and count for each.
6. Print a one-line summary: "Benchmark analysis complete: 5 sessions analyzed, 3 graded by greptile, 2 aborted (temp 0.3 ×2), N model-anomalies detected."

**Steps:**
- [ ] Run the 6 verifications above.
- [ ] If any fail, fix the underlying issue (re-run the appropriate task) and re-verify.
- [ ] Report final status to the user.

**Acceptance criteria:**
- [ ] All 6 verifications pass
- [ ] Final summary line printed with the correct count of model anomalies
- [ ] User can read the report file or see it inline from Task 3
