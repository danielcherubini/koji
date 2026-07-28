# Plan-175 SSE Consolidation Benchmark

**Generated:** 2026-07-28
**Sessions analyzed:** 5

## Main Comparison

| Variant | Completed | Duration | Tokens | Tool fails | Subs | Model OK | Think % (time) | Avg think latency | Avg think chars | Greptile iter | Confidence | P0 | P1 | P2 | P3 | Actionable | Informational | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Qwen baseline | Yes | 1h 7m | 2.2M | 2 | 8 | ✅ | 16% | 12.3s | 240 | 2 | 5/5 | 2 | 0 | 1 | 0 | 2 | 1 | clean |
| temp 0.5 | Yes | 1h 51m | 3.5M | 4 | 7 | ✅† | 10% | 18.2s | 694 | 2 | 5/5 | 0 | 0 | 0 | 0 | 0 | 0 | clean |
| temp 0.3 (aborted #1) | Aborted | 32m 35s | 484k | 2 | 0 | ✅ | 15% | 22.1s | 1375 | — | — | — | — | — | — | — | — | — |
| temp 0.3 (aborted #2) | Aborted | 53m 53s | 1.3M | 0 | 3 | ✅† | 50% | 66.7s | 4340 | — | — | — | — | — | — | — | — | — |
| temp 0.7 / top_p 0.95 | Yes | 2h 23m | 7.6M | 4 | 12 | ✅† | 18% | 22.1s | 936 | 1 | 4/5 | 0 | 0 | 0 | 0 | 0 | 0 | clean |

† Modal main model is correct, but a stretch of subagent dispatches used a non-expected `model` in their arguments — see Anomalies & Summary.

## Subagent Detail

| Variant | Agent | Dispatched at | Duration | Model at dispatch | Error |
| --- | --- | --- | --- | --- | --- |
| Qwen baseline | general | 07:39:44 | 9m 29s | qwen3.6-27b | ✅ |
| Qwen baseline | general | 07:49:57 | 12m 17s | qwen3.6-27b | ✅ |
| Qwen baseline | general | 08:04:13 | 1m 8s | qwen3.6-27b | ✅ |
| Qwen baseline | general | 08:06:32 | 3m 25s | qwen3.6-27b | ✅ |
| Qwen baseline | general | 08:10:51 | 4m 23s | qwen3.6-27b | ✅ |
| Qwen baseline | general | 08:15:49 | 4m 42s | qwen3.6-27b | ✅ |
| Qwen baseline | general | 08:21:02 | 0m 47s | qwen3.6-27b | ✅ |
| Qwen baseline | general | 08:32:11 | 2m 7s | qwen3.6-27b | ✅ |
| temp 0.5 | general | 09:14:50 | 0m 5s | laguna-s-2.1 | ✅ |
| temp 0.5 | general | 09:15:50 | 19m 52s | laguna-s-2.1 | ✅ |
| temp 0.5 | general | 09:36:55 | 17m 50s | laguna-s-2.1 | ✅ |
| temp 0.5 | general | 09:55:58 | 10m 48s | laguna-s-2.1 | ✅ |
| temp 0.5 | general | 10:07:41 | 8m 7s | laguna-s-2.1 | ✅ |
| temp 0.5 | general | 10:16:41 | 4m 40s | laguna-s-2.1 | ✅ |
| temp 0.5 | general | 10:34:14 | 0m 25s | laguna-s-2.1 | ✅ |
| temp 0.3 (aborted #2) | general | 11:49:28 | 0m 5s | laguna-s-2.1 | ✅ |
| temp 0.3 (aborted #2) | general | 11:50:55 | 0m 5s | laguna-s-2.1 | ✅ |
| temp 0.3 (aborted #2) | general | 11:52:11 | 24m 48s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 12:42:50 | 0m 45s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 12:44:41 | 0m 29s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 12:47:43 | 0m 54s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 12:51:31 | 0m 4s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 12:53:22 | 0m 7s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 12:54:33 | 7m 11s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 13:03:20 | 8m 33s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 13:14:27 | 36m 45s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 13:53:58 | 28m 29s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 14:24:58 | 3m 20s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 14:30:41 | 2m 6s | laguna-s-2.1 | ✅ |
| temp 0.7 / top_p 0.95 | general | 14:45:15 | 5m 16s | laguna-s-2.1 | ✅ |

Total subagent time: 3h 39m (sum of all completed subagent durations)

## Anomalies & Summary

- **0.3 attempts (both aborted before `git branch -m` rename):** `temp 0.3 (aborted #1)` ran 32m 35s, dispatched 0 subagents, and accumulated 2 tool failures — the failures plus 0 subagent progress strongly suggest the agent got stuck in a tool-failure loop and the user pulled the plug. `temp 0.3 (aborted #2)` ran 53m 53s, dispatched 3 subagents (one of them taking 24m 48s), had **0 tool failures**, and burned ~50% of wall-clock on thinking blocks (avg 4340 chars, 66.7s per block) — the abort here looks like a user-tolerance issue driven by a runaway reasoning style at low temperature, not tool failures.

- **Subagent time (per variant / overall):** Qwen baseline 0h 38m, temp 0.5 1h 2m, temp 0.3 (aborted #1) 0h 0m, temp 0.3 (aborted #2) 0h 25m, temp 0.7 / top_p 0.95 **1h 34m** (largest by far — 12 dispatches, including one 36m 45s block on Task 3). Overall total subagent time across all 5 sessions is **3h 39m**, which is ~43% of the combined wall-clock of the 3 completed runs (1h 7m + 1h 51m + 2h 23m = 5h 21m). This is the dominant cost — the agent spends almost half its time waiting on subagents.

- **Model anomalies (3 sessions flagged `had_model_anomaly: true`):** All 4 sampling runs use the modal main model `laguna-s-2.1` correctly; the Qwen baseline uses `qwen3.6-27b` (orchestrator) with the subagent short-form id `qwen3.6-35b-a3b` — the short-form vs. dash form is a known naming quirk and not a real anomaly. The real anomalies are all on the `arguments.model` field of the `subagent` toolCall (NOT on the main agent's `message.model`):
  - **`temp 0.7 / top_p 0.95` — 5 subagent dispatches used `model: 'gemini-2.5-flash'` in their arguments** (a different field than the main agent's `message.model`); the main agent itself always used `laguna-s-2.1`. This is the headline anomaly: the model was meant to be `laguna-s-2.1` for the whole run, but the main agent's first 5 subagent toolCalls passed the wrong model id. The anomaly is invisible to a check that only looks at `message.model` on assistant messages.
  - **`temp 0.5` — 1 subagent dispatch used `arguments.model: 'general'`** (the *agent name*, not a model id — looks like a hallucination in the toolCall schema). The main agent's own `message.model` was always `laguna-s-2.1`.
  - **`temp 0.3 (aborted #2)` — 2 subagent dispatches used `arguments.model: 'general'`** (same agent-name-leaked-into-model-field bug, twice). Main agent `message.model` was always `laguna-s-2.1`.
  - **`Qwen baseline` and `temp 0.3 (aborted #1)` — no anomalies.**

- **Greptile data quality note:** The `temp 0.5` and `temp 0.7 / top_p 0.95` agents described their greptile findings in prose rather than rendering the standard `| # | Severity | ... | Classification |` Phase 3 table, so `actionable_count` and `informational_count` are 0 for those sessions even though the agents clearly acted on findings (the temp 0.5 dispatch at 10:34:14 was titled "Fix Greptile finding #2" and the temp 0.7 dispatch at 14:45:15 was titled "Fix Greptile findings"). The Qwen baseline rendered the table normally and has `actionable_count: 2, informational_count: 1`. **The summary numbers (`iterations`, `confidence`, `findings_resolved`, `p0`-`p1`-`p2`-`p3` counts) are correct for all 3 branches** — only the Actionable/Informational columns are noisy on the two prose-only runs.

- **Greptile ranking (3 completed branches, composite = `p0*100 + p1*10 + actionable_count + (1 if issues remain else 0)`, lower is better):**
  - **Best: `temp 0.5` (composite 0, confidence 5/5)** — tied on score with temp 0.7 but higher confidence; the agent's two P0 fix-rounds resolved cleanly with no leftover P0/P1/P2/P3 findings.
  - **Tied: `temp 0.7 / top_p 0.95` (composite 0, confidence 4/5)** — same zero-severity outcome but graded 4/5 (likely because of the 5 gemini-2.5-flash subagent dispatches, which are real defects in the run even if they didn't leak into the final diff).
  - **Worst: `Qwen baseline` (composite 202, confidence 5/5)** — 2 P0 findings on the initial run; both were addressed in two iterations, so the final diff is clean (`verdict: clean`), but the high composite score reflects the first-pass severity.

- **Tool failure correlation:** `temp 0.3 (aborted #1)` had 2 tool failures in 32m 35s with 0 subagent dispatches and 29 tool calls — the failure rate (~7%) is much higher than any completed run and is the most plausible abort trigger. `temp 0.3 (aborted #2)` had 0 tool failures despite 36 tool calls and 3 subagents, so its abort is not tool-failure-driven (see reasoning-patterns note). For reference, the 3 completed runs had: Qwen 2 fails / 52 calls (4%), temp 0.5 4 / 65 (6%), temp 0.7 4 / 108 (4%).

- **Reasoning patterns:** Qwen had `thinking_level: "off"` and still produced 51 thinking blocks (avg 240 chars, 12.3s per block) — the level setting is advisory only. The aborted temp 0.3 #2 stands out with `avg_thinking_chars: 4340` and `avg_thinking_latency: 66.7s` (roughly 4× the next-highest variant), and 50% of wall-clock spent in thinking — at low temperature the model is reasoning much harder and longer per turn, which is consistent with the runaway-style abort. The completed temp 0.5 and temp 0.7 runs have moderate thinking (694 / 936 avg chars) but delegate heavily to subagents (7 / 12 dispatches), while Qwen thinks less and delegates less.
