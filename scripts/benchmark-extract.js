#!/usr/bin/env node
/**
 * benchmark-extract.js
 *
 * Deterministic parser for pi session JSONL logs. Reads
 * `benchmark-sessions.json` (an explicit session-id -> variant map at the
 * project root), parses each session's JSONL line-by-line, computes
 * session-level metrics, and writes `benchmark-metrics.json`.
 *
 * Pure stdlib: fs, path, readline. No npm dependencies.
 *
 * Run from the project root:
 *   node scripts/benchmark-extract.js
 *
 * Exit non-zero on:
 *   - map file missing/unreadable
 *   - any session id in the map not resolving to exactly one *.jsonl file
 *   - any session file being unreadable
 *   - first event in any session not being a `session` event
 *   - fewer than 5 sessions processed successfully
 */

'use strict';

const fs = require('fs');
const path = require('path');
const readline = require('readline');

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const PROJECT_ROOT = process.cwd();
const SESSIONS_DIR = path.join(
  process.env.HOME || '/root',
  '.pi/agent/sessions/--home-daniel-Coding-Rust-tama--'
);
const MAP_PATH = path.join(PROJECT_ROOT, 'benchmark-sessions.json');
const OUTPUT_PATH = path.join(PROJECT_ROOT, 'benchmark-metrics.json');

const EXPECTED_SESSIONS = 5;

// ---------------------------------------------------------------------------
// Tiny helpers
// ---------------------------------------------------------------------------

function err(msg) {
  process.stderr.write(`benchmark-extract: ${msg}\n`);
}

function fail(msg) {
  err(msg);
  process.exit(1);
}

/** Parse a line as JSON; return null on failure (with a warning). */
function tryParse(line) {
  if (!line || !line.trim()) return null;
  try {
    return JSON.parse(line);
  } catch (e) {
    err(`warning: could not parse JSONL line (${e.message}); skipping`);
    return null;
  }
}

/** Compute modal value of a list (most frequent; tie-break = last occurrence). */
function modalValue(list) {
  if (!list || list.length === 0) return null;
  const counts = new Map();
  let lastIdx = new Map();
  for (let i = 0; i < list.length; i++) {
    const v = list[i];
    if (v == null) continue;
    counts.set(v, (counts.get(v) || 0) + 1);
    lastIdx.set(v, i);
  }
  let bestVal = null;
  let bestCount = -1;
  let bestLastIdx = -1;
  for (const [v, c] of counts) {
    const li = lastIdx.get(v);
    if (c > bestCount || (c === bestCount && li > bestLastIdx)) {
      bestVal = v;
      bestCount = c;
      bestLastIdx = li;
    }
  }
  return bestVal;
}

// ---------------------------------------------------------------------------
// Streamed JSONL read
// ---------------------------------------------------------------------------

/** Read a JSONL file with readline; return all parsed events in order. */
function readJsonlStream(filePath) {
  return new Promise((resolve, reject) => {
    const events = [];
    const rl = readline.createInterface({
      input: fs.createReadStream(filePath, { encoding: 'utf8' }),
      crlfDelay: Infinity,
    });
    rl.on('line', (line) => {
      const ev = tryParse(line);
      if (ev !== null) events.push(ev);
    });
    rl.on('close', () => resolve(events));
    rl.on('error', (e) => reject(e));
  });
}

// ---------------------------------------------------------------------------
// Per-session metrics
// ---------------------------------------------------------------------------

/**
 * Extract the `<new>` part of a `git branch -m ...` command. Handles both:
 *   - `git branch -m <oldname> <newname> ...`
 *   - `git branch -m <newname> ...`  (rename current to <newname>)
 * followed by `&& ...` chains.
 *
 * Returns the new branch name, or null if no `git branch -m` is present.
 */
function extractBranchRename(command) {
  if (typeof command !== 'string') return null;
  // Match `git branch -m` and capture everything up to the first `&&`, `;`,
  // or end-of-string after it.
  const re = /git\s+branch\s+-m\s+([^\n;&]+?)(?=\s*(?:&&|;|$))/;
  const m = command.match(re);
  if (!m) return null;
  const tail = m[1].trim();
  if (!tail) return null;
  // Split on whitespace; if 2+ tokens, second is the new name (git branch -m old new);
  // if 1 token, that is the new name (git branch -m new, where current is renamed).
  const tokens = tail.split(/\s+/);
  return tokens[tokens.length - 1] || null;
}

/** Strip a leading "model:" prefix and surrounding whitespace. */
function trimModel(s) {
  if (typeof s !== 'string') return s;
  return s.trim();
}

function computeSessionMetrics(variant, sessionId, sessionPath, events) {
  // Validate first event
  if (!events.length || events[0].type !== 'session') {
    throw new Error(
      `session ${sessionId}: first event is not a 'session' event (got ${events[0]?.type})`
    );
  }

  const sessionStartTs = events[0].timestamp || null;

  // First-pass aggregations
  let lastEventTs = null;
  let thinkingLevel = null;
  let firstRename = null; // first observed `git branch -m <new>` result
  const assistantModels = []; // for modal computation
  const modelUsage = Object.create(null); // tally
  let totalTokens = 0;
  let toolCallsTotal = 0;
  let toolFailures = 0;
  let thinkingBlockCount = 0;
  let thinkingTotalChars = 0;
  let thinkingTotalLines = 0;
  let textTotalChars = 0;

  // Subagent dispatch records (one per subagent toolCall)
  const subagents = [];

  // For thinking_pct_time
  let thinkingTimeSumMs = 0;

  // First-pass: walk events chronologically
  for (let i = 0; i < events.length; i++) {
    const ev = events[i];
    const evTs = ev.timestamp;
    if (evTs) lastEventTs = evTs;

    if (ev.type === 'thinking_level_change') {
      if (thinkingLevel == null && typeof ev.thinkingLevel === 'string') {
        thinkingLevel = ev.thinkingLevel;
      }
    }

    if (ev.type !== 'message') continue;
    const msg = ev.message || {};
    const role = msg.role;
    const content = msg.content;

    // Tool failures (only message events with role === 'toolResult')
    if (role === 'toolResult' && msg.isError === true) {
      toolFailures++;
    }

    // Only assistant messages contribute to model/text/thinking/tool-call
    // accounting.
    if (role !== 'assistant' || !Array.isArray(content)) {
      continue;
    }

    // Model tally (modal later)
    const m = trimModel(msg.model);
    if (m) {
      assistantModels.push(m);
      modelUsage[m] = (modelUsage[m] || 0) + 1;
    }

    // Token usage
    if (msg.usage && typeof msg.usage.totalTokens === 'number') {
      totalTokens += msg.usage.totalTokens;
    }

    // Determine if this assistant message has a thinking block; we need the
    // prev_event timestamp for thinking_pct_time. The `prev_event` is the
    // immediately preceding event in the JSONL (any type), not the previous
    // assistant message.
    let hasThinking = false;
    for (const blk of content) {
      if (!blk || typeof blk !== 'object') continue;
      if (blk.type === 'toolCall') {
        toolCallsTotal++;
        // bash + git branch -m
        if (blk.name === 'bash') {
          const args = blk.arguments || {};
          const cmd = args.command;
          if (firstRename == null) {
            const newName = extractBranchRename(cmd);
            if (newName != null) firstRename = newName;
          }
        }
        // subagent dispatch record
        if (blk.name === 'subagent') {
          const args = blk.arguments || {};
          // subagent_intended_model: the model the main agent EXPLICITLY asked
          // the subagent to use (read from `arguments.model`). This is the
          // anomaly signal for the temp-0.7-style "main agent dispatched
          // subagents with the wrong model" bug. Null when missing/empty.
          let subagentIntendedModel = null;
          if (typeof args.model === 'string' && args.model.length > 0) {
            subagentIntendedModel = args.model;
          }
          const sub = {
            subagent_call_id: blk.id,
            agent_name: args.agent || null,
            task_preview: typeof args.task === 'string' ? args.task.slice(0, 80) : '',
            dispatched_at: evTs,
            is_async: args.async === true,
            model_at_dispatch: m || null,
            subagent_intended_model: subagentIntendedModel,
            completed_at: null,
            duration_ms: null,
            is_error: null,
          };
          subagents.push(sub);
        }
      } else if (blk.type === 'thinking') {
        hasThinking = true;
        const t = typeof blk.thinking === 'string' ? blk.thinking : '';
        thinkingBlockCount++;
        thinkingTotalChars += t.length;
        thinkingTotalLines += t.split('\n').length;
      } else if (blk.type === 'text') {
        const t = typeof blk.text === 'string' ? blk.text : '';
        textTotalChars += t.length;
      }
    }

    if (hasThinking && i > 0) {
      const prevTs = events[i - 1].timestamp;
      if (prevTs && evTs) {
        const dt = Date.parse(evTs) - Date.parse(prevTs);
        if (dt > 0) thinkingTimeSumMs += dt;
      }
    }
  }

  // Second pass: match subagent toolResults by toolCallId
  for (const ev of events) {
    if (ev.type !== 'message') continue;
    const msg = ev.message || {};
    if (msg.role !== 'toolResult') continue;
    if (msg.toolName !== 'subagent') continue;
    const tcId = msg.toolCallId;
    if (!tcId) continue;
    // Find the subagent record (first match by call_id).
    const sub = subagents.find((s) => s.subagent_call_id === tcId && s.completed_at == null);
    if (!sub) continue;
    sub.completed_at = ev.timestamp || null;
    if (sub.dispatched_at && ev.timestamp) {
      const dt = Date.parse(ev.timestamp) - Date.parse(sub.dispatched_at);
      sub.duration_ms = dt >= 0 ? dt : null;
    } else {
      sub.duration_ms = null;
    }
    sub.is_error = msg.isError === true;
  }

  // Wall clock + thinking pct
  const wallClockMs =
    sessionStartTs && lastEventTs
      ? Math.max(0, Date.parse(lastEventTs) - Date.parse(sessionStartTs))
      : 0;

  const thinkingPctTime = wallClockMs > 0 ? (100 * thinkingTimeSumMs) / wallClockMs : 0;

  const thinkingPctChars =
    thinkingTotalChars + textTotalChars > 0
      ? (100 * thinkingTotalChars) / (thinkingTotalChars + textTotalChars)
      : 0;

  const completion = firstRename != null ? 'Yes' : 'Aborted';
  const mainModel = modalValue(assistantModels);

  return {
    variant,
    session_id: sessionId,
    session_path: sessionPath,
    session_start_ts: sessionStartTs,
    last_event_ts: lastEventTs,
    wall_clock_ms: wallClockMs,
    completion,
    main_model: mainModel,
    model_usage: modelUsage,
    thinking_level: thinkingLevel,
    total_tokens: totalTokens,
    tool_calls_total: toolCallsTotal,
    tool_failures: toolFailures,
    branch_renamed_to: firstRename,
    thinking_block_count: thinkingBlockCount,
    thinking_total_chars: thinkingTotalChars,
    thinking_total_lines: thinkingTotalLines,
    text_total_chars: textTotalChars,
    avg_thinking_chars: thinkingBlockCount > 0 ? thinkingTotalChars / thinkingBlockCount : 0,
    avg_thinking_lines: thinkingBlockCount > 0 ? thinkingTotalLines / thinkingBlockCount : 0,
    thinking_pct_chars: round1(thinkingPctChars),
    thinking_pct_time: round1(thinkingPctTime),
    subagent_count: subagents.length,
    subagents,
  };
}

function round1(n) {
  return Math.round(n * 10) / 10;
}

// ---------------------------------------------------------------------------
// Model correctness
// ---------------------------------------------------------------------------

function computeModelCorrectness(session) {
  const variant = session.variant;
  let mainExpected; // string or RegExp
  let subagentExpected; // string or RegExp — what model the subagent SHOULD be told to use
  let mainMatches; // (model) => boolean
  let subagentMatches; // (model) => boolean
  if (variant === 'Qwen baseline') {
    mainExpected = /qwen3\.6-27b/;
    subagentExpected = /qwen3\.6-35b-a3b/;
    mainMatches = (m) => typeof m === 'string' && /qwen3\.6-27b/.test(m);
    subagentMatches = (m) => typeof m === 'string' && /qwen3\.6-35b-a3b/.test(m);
  } else {
    mainExpected = 'laguna-s-2.1';
    subagentExpected = 'laguna-s-2.1';
    mainMatches = (m) => m === 'laguna-s-2.1';
    subagentMatches = (m) => m === 'laguna-s-2.1';
  }

  const modelCorrect = mainMatches(session.main_model);
  const out = {
    main_expected: mainExpected instanceof RegExp ? mainExpected.source : mainExpected,
    subagent_expected:
      subagentExpected instanceof RegExp ? subagentExpected.source : subagentExpected,
    model_correct: modelCorrect,
    subagent_model_unverifiable: true,
    non_expected_models_used: [],
    had_model_anomaly: false,
  };
  if (!modelCorrect) {
    out.main_model_mismatch = { actual: session.main_model, expected: out.main_expected };
  }

  // Independent non-expected scan: TWO sources, merged into one sorted array.
  //
  // 1) Main-agent models: tally of `message.model` on assistant messages.
  // 2) Subagent-intended models: every non-null value in
  //    `subagents[].subagent_intended_model` (the model the main agent ASKED
  //    the subagent to use via `arguments.model`). Nulls (the main agent did
  //    not override the subagent's model) are skipped.
  const offenders = [];

  for (const [model, count] of Object.entries(session.model_usage)) {
    if (!mainMatches(model)) {
      offenders.push({ model, count, source: 'main_agent' });
    }
  }

  // Tally subagent-intended model usage.
  const subagentIntendedUsage = Object.create(null);
  if (Array.isArray(session.subagents)) {
    for (const sub of session.subagents) {
      const m = sub && sub.subagent_intended_model;
      if (typeof m === 'string' && m.length > 0) {
        subagentIntendedUsage[m] = (subagentIntendedUsage[m] || 0) + 1;
      }
    }
  }
  out.subagent_intended_usage = subagentIntendedUsage;

  for (const [model, count] of Object.entries(subagentIntendedUsage)) {
    if (!subagentMatches(model)) {
      offenders.push({ model, count, source: 'subagent_dispatch' });
    }
  }

  // Sort: count desc, then source alphabetically (main_agent < subagent_dispatch).
  offenders.sort((a, b) => {
    if (b.count !== a.count) return b.count - a.count;
    if (a.source < b.source) return -1;
    if (a.source > b.source) return 1;
    return 0;
  });
  out.non_expected_models_used = offenders;
  if (offenders.length > 0) out.had_model_anomaly = true;

  return out;
}

// ---------------------------------------------------------------------------
// Summary line
// ---------------------------------------------------------------------------

function fmtDuration(ms) {
  if (ms == null) return '?';
  const totalSec = Math.round(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  const s = totalSec % 60;
  return `${m}m ${s}s`;
}

function fmtTokens(n) {
  if (n == null || n === 0) return '0';
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${Math.round(n / 1e3)}k`;
  return String(n);
}

function printSummaryLine(session) {
  const dur = fmtDuration(session.wall_clock_ms);
  const tok = fmtTokens(session.total_tokens);
  const fails = session.tool_failures;
  const subs = session.subagent_count;
  const model = session.main_model || '?';
  let line = `  ${session.variant}: ${dur}, ${tok} tokens, ${fails} tool fails, ${subs} subagents, model=${model}`;
  if (session.had_model_anomaly) {
    const pairs = session.non_expected_models_used
      .map((x) => {
        // 'main_agent' → 'main', 'subagent_dispatch' → 'subagent'.
        const src = x.source === 'main_agent' ? 'main' : x.source === 'subagent_dispatch' ? 'subagent' : x.source;
        return `${x.model}:${x.count} (${src})`;
      })
      .join(',');
    line += `, model-anomaly=${pairs}`;
  }
  process.stdout.write(line + '\n');
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  // 1. Read the map.
  if (!fs.existsSync(MAP_PATH)) {
    fail(
      `missing ${MAP_PATH}. Create it with the template shape:\n` +
        '  { "sessions": [\n' +
        '    { "variant": "Qwen baseline",         "session_id": "<ulid>" },\n' +
        '    { "variant": "temp 0.5",              "session_id": "<ulid>" },\n' +
        '    { "variant": "temp 0.3 (aborted #1)", "session_id": "<ulid>" },\n' +
        '    { "variant": "temp 0.3 (aborted #2)", "session_id": "<ulid>" },\n' +
        '    { "variant": "temp 0.7 / top_p 0.95", "session_id": "<ulid>" }\n' +
        '  ] }\n'
    );
  }

  let map;
  try {
    map = JSON.parse(fs.readFileSync(MAP_PATH, 'utf8'));
  } catch (e) {
    fail(`could not parse ${MAP_PATH}: ${e.message}`);
  }
  if (!map || !Array.isArray(map.sessions)) {
    fail(`${MAP_PATH} must have a top-level "sessions" array`);
  }
  if (map.sessions.length < EXPECTED_SESSIONS) {
    fail(
      `${MAP_PATH} has ${map.sessions.length} sessions, expected at least ${EXPECTED_SESSIONS}`
    );
  }

  // 2. Resolve file paths in one readdir pass.
  if (!fs.existsSync(SESSIONS_DIR)) {
    fail(`sessions dir not found: ${SESSIONS_DIR}`);
  }
  let dirEntries;
  try {
    dirEntries = fs.readdirSync(SESSIONS_DIR);
  } catch (e) {
    fail(`could not read ${SESSIONS_DIR}: ${e.message}`);
  }

  function resolveSessionFile(sessionId) {
    const matches = dirEntries.filter(
      (f) => f.endsWith(`_${sessionId}.jsonl`) && f.includes('_')
    );
    if (matches.length === 0) {
      throw new Error(`no file matching '*_${sessionId}.jsonl' in ${SESSIONS_DIR}`);
    }
    if (matches.length > 1) {
      throw new Error(
        `ambiguous: ${matches.length} files match '*_${sessionId}.jsonl' (${matches.join(', ')})`
      );
    }
    return path.join(SESSIONS_DIR, matches[0]);
  }

  // 3. Process each session.
  const results = [];
  for (const entry of map.sessions) {
    const { variant, session_id: sessionId } = entry;
    if (!variant || !sessionId) {
      fail(`malformed map entry: ${JSON.stringify(entry)}`);
    }
    let filePath;
    try {
      filePath = resolveSessionFile(sessionId);
    } catch (e) {
      fail(`session "${variant}" (${sessionId}): ${e.message}`);
    }

    let events;
    try {
      events = await readJsonlStream(filePath);
    } catch (e) {
      fail(`session "${variant}" (${sessionId}): could not read ${filePath}: ${e.message}`);
    }
    if (!events.length) {
      fail(`session "${variant}" (${sessionId}): file is empty`);
    }

    let metrics;
    try {
      metrics = computeSessionMetrics(variant, sessionId, filePath, events);
    } catch (e) {
      fail(`session "${variant}" (${sessionId}): ${e.message}`);
    }
    Object.assign(metrics, computeModelCorrectness(metrics));
    results.push(metrics);
  }

  if (results.length < EXPECTED_SESSIONS) {
    fail(`processed ${results.length} sessions, expected at least ${EXPECTED_SESSIONS}`);
  }

  // 4. Write output.
  try {
    fs.writeFileSync(OUTPUT_PATH, JSON.stringify(results, null, 2) + '\n', 'utf8');
  } catch (e) {
    fail(`could not write ${OUTPUT_PATH}: ${e.message}`);
  }

  // 5. Print summaries in map order.
  for (const r of results) printSummaryLine(r);
}

// Expose internals for testing when required as a module. When run as a CLI
// (require.main === module), kick off the real pipeline.
if (require.main === module) {
  main().catch((e) => {
    err(e && e.stack ? e.stack : String(e));
    process.exit(1);
  });
}

module.exports = {
  trimModel,
  extractBranchRename,
  modalValue,
  readJsonlStream,
  computeSessionMetrics,
  computeModelCorrectness,
  printSummaryLine,
  // Constants — useful for tests that need to assert on raw counts.
  EXPECTED_SESSIONS,
};
