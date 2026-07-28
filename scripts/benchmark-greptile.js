#!/usr/bin/env node
/**
 * benchmark-greptile.js
 *
 * Scrape greptile review results from completed pi session JSONL logs.
 * The user has already run greptile on each completed branch during their
 * normal workflow; the output is recorded as assistant text content in the
 * session logs. This script does NOT re-execute greptile — it reads the
 * recorded output deterministically.
 *
 * Reads:
 *   - benchmark-sessions.json (variant -> session_id map at project root)
 *   - 3 session JSONL files in ~/.pi/agent/sessions/--home-daniel-Coding-Rust-tama--/
 *
 * Writes:
 *   - benchmark-greptile.json (project root, pretty-printed)
 *
 * Pure stdlib: fs, path, readline. No npm dependencies.
 *
 * Run from the project root:
 *   node scripts/benchmark-greptile.js
 *
 * Exit non-zero on:
 *   - map file missing/unreadable
 *   - any completed-variant session id not resolving to exactly one *.jsonl file
 *   - any session file being unreadable
 *   - fewer than 3 completed branches processed successfully
 *   - any entry has verdict "greptile output not found in session log"
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
const OUTPUT_PATH = path.join(PROJECT_ROOT, 'benchmark-greptile.json');

// The 3 completed branches the user wants graded. The 2 aborted temp-0.3
// entries are skipped.
const COMPLETED_VARIANTS = new Set(['Qwen baseline', 'temp 0.5', 'temp 0.7 / top_p 0.95']);
const EXPECTED_COMPLETED = 3;

// ---------------------------------------------------------------------------
// Tiny helpers
// ---------------------------------------------------------------------------

function err(msg) {
  process.stderr.write(`benchmark-greptile: ${msg}\n`);
}

function warn(msg) {
  process.stderr.write(`benchmark-greptile: WARNING: ${msg}\n`);
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
// Text assembly
// ---------------------------------------------------------------------------

/**
 * Concatenate all relevant text content from `type: "message"` events into
 * a single string (newlines between blocks). Non-array content is skipped.
 * `type: "thinking"` blocks are intentionally EXCLUDED — they are private
 * reasoning, not visible to the user, and including them would let the
 * agent's own prose classification (e.g. "Finding 1 is P2 actionable")
 * pollute the table-counting logic.
 *
 * NOTE on the spec — the spec says to include only assistant text blocks.
 * In practice, the greptile output lives in three other places that must
 * also be in scope:
 *   1. `ask` toolCall arguments' `questions[].description` (where the
 *      Summary block lives)
 *   2. toolResult `details.description` (mirror of the ask description)
 *   3. toolResult text content (where raw `greptile review show --json`
 *      output and bash captures of greptile review text appear)
 * Without all of these, the script reports "greptile output not found" for
 * every session, which is the wrong default. Surfacing this format detail
 * to the caller is part of the script's design.
 */
function concatenateAssistantText(events) {
  const parts = [];
  for (const ev of events) {
    if (!ev || ev.type !== 'message') continue;
    const msg = ev.message || {};
    if (msg.role !== 'assistant') continue;
    const content = msg.content;
    if (!Array.isArray(content)) continue;
    for (const blk of content) {
      if (!blk || typeof blk !== 'object') continue;
      if (blk.type === 'text' && typeof blk.text === 'string') {
        parts.push(blk.text);
      } else if (blk.type === 'toolCall' && blk.name === 'ask') {
        collectAskDescriptions(blk.arguments, parts);
      }
    }
  }
  // toolResult: details.description (ask toolResult echo) + text content
  // (bash/read output that often contains raw greptile JSON).
  for (const ev of events) {
    if (!ev || ev.type !== 'message') continue;
    const msg = ev.message || {};
    if (msg.role !== 'toolResult') continue;
    if (msg.details && typeof msg.details.description === 'string') {
      parts.push(msg.details.description);
    }
    const content = msg.content;
    if (Array.isArray(content)) {
      for (const blk of content) {
        if (blk && blk.type === 'text' && typeof blk.text === 'string') {
          parts.push(blk.text);
        }
      }
    }
  }
  return parts.join('\n');
}

function collectAskDescriptions(args, parts) {
  if (!args || typeof args !== 'object') return;
  const qs = args.questions;
  if (!Array.isArray(qs)) return;
  for (const q of qs) {
    if (q && typeof q.description === 'string') parts.push(q.description);
  }
}

// ---------------------------------------------------------------------------
// Summary block (Iterations / Findings resolved / Remaining / Final confidence)
// ---------------------------------------------------------------------------

// The actual Summary blocks in the recorded session text are indented with
// 2 spaces (e.g. "  - Iterations: 2") because they appear inside a JSON
// description string. We tolerate any leading horizontal whitespace before
// the leading dash, not just the spec's exact `^-` form.
const RE_ITERATIONS = /^[ \t]*- Iterations:\s*(\d+)/m;
const RE_FINDINGS_RESOLVED = /^[ \t]*- Findings resolved:\s*(\d+)/m;
const RE_REMAINING = /^[ \t]*- Remaining:\s*(\d+)/m;
const RE_FINAL_CONFIDENCE = /^[ \t]*- Final confidence:\s*(\d+)\/5/m;

/**
 * Extract the four summary metrics from the concatenated assistant text.
 * Uses the LAST match for each field (greptile runs multiple iterations, the
 * last one is the final state). Returns {found, iterations, ...} with
 * `found: false` when no Summary block is detected.
 */
function extractGreptileSummary(text) {
  const iterations = lastIntMatch(text, RE_ITERATIONS);
  const findingsResolved = lastIntMatch(text, RE_FINDINGS_RESOLVED);
  const remaining = lastIntMatch(text, RE_REMAINING);
  const confidenceMatch = matchAll(text, RE_FINAL_CONFIDENCE);
  const confidence = confidenceMatch.length > 0 ? `${confidenceMatch[confidenceMatch.length - 1]}/5` : null;

  // "Found" iff we saw at least the iterations field (the canonical
  // "**Summary:**" block always has all four, but a malformed session might
  // have only some — the iterations line is the most distinctive).
  const found = iterations !== null;
  return {
    found,
    iterations: iterations != null ? iterations : 0,
    findings_resolved: findingsResolved != null ? findingsResolved : 0,
    remaining: remaining != null ? remaining : 0,
    confidence,
  };
}

function lastIntMatch(text, re) {
  const matches = matchAll(text, re);
  if (matches.length === 0) return null;
  return matches[matches.length - 1];
}

function matchAll(text, re) {
  const out = [];
  if (!text) return out;
  // Avoid stateful lastIndex bugs: build a fresh regex from the pattern.
  const flags = re.flags.includes('g') ? re.flags : re.flags + 'g';
  const gre = new RegExp(re.source, flags);
  let m;
  while ((m = gre.exec(text)) !== null) {
    out.push(parseInt(m[1], 10));
    if (m.index === gre.lastIndex) gre.lastIndex++; // safety against zero-width
  }
  return out;
}

// ---------------------------------------------------------------------------
// Raw greptile JSON (for issue_counts: p0, p1, p2, p3)
// ---------------------------------------------------------------------------

/**
 * The raw greptile review JSON is a single line in a text-block that starts
 * with `{"summary":` and contains `"comments":[`. We extract the LAST one
 * (final iteration) and JSON.parse it. Returns null when none is present.
 *
 * The JSON is embedded inside assistant or toolResult text content. We look
 * for the substring `{"summary":` as a starting marker, then attempt to find
 * the matching closing `}` by counting braces (since the JSON contains
 * nested objects/arrays and naive `\n` splits don't work — the JSON is on
 * one line but with escaped quotes inside the `summary` string).
 */
function extractRawGreptileJson(text) {
  if (!text) return null;
  const marker = '{"summary":';
  // Find every occurrence, take the last one.
  let lastStart = -1;
  let searchFrom = 0;
  while (true) {
    const idx = text.indexOf(marker, searchFrom);
    if (idx === -1) break;
    lastStart = idx;
    searchFrom = idx + 1;
  }
  if (lastStart === -1) return null;

  // Walk forward from lastStart, tracking string-vs-code state to find the
  // matching top-level closing `}`. This is needed because the JSON is on
  // one (very long) line and contains escaped quotes inside string values.
  const slice = text.slice(lastStart);
  let depth = 0;
  let inString = false;
  let escape = false;
  let endIdx = -1;
  for (let i = 0; i < slice.length; i++) {
    const c = slice[i];
    if (inString) {
      if (escape) {
        escape = false;
      } else if (c === '\\') {
        escape = true;
      } else if (c === '"') {
        inString = false;
      }
      continue;
    }
    if (c === '"') {
      inString = true;
    } else if (c === '{') {
      depth++;
    } else if (c === '}') {
      depth--;
      if (depth === 0) {
        endIdx = i + 1;
        break;
      }
    }
  }
  if (endIdx === -1) return null;
  const jsonStr = slice.slice(0, endIdx);
  try {
    return JSON.parse(jsonStr);
  } catch (e) {
    return null;
  }
}

/** Tally comments[].severity into {p0, p1, p2, p3}. Unrecognized severities
 *  are ignored. Missing `comments` is treated as zero comments. */
function tallyIssueCounts(obj) {
  const out = { p0: 0, p1: 0, p2: 0, p3: 0 };
  if (!obj || !Array.isArray(obj.comments)) return out;
  for (const c of obj.comments) {
    if (!c || typeof c !== 'object') continue;
    const sev = c.severity;
    if (sev === 'P0' || sev === 'P1' || sev === 'P2' || sev === 'P3') {
      const k = sev.toLowerCase();
      out[k]++;
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Phase 3 markdown table (for actionable_count + informational_count)
// ---------------------------------------------------------------------------

/**
 * The Phase 3 markdown table has rows of the form:
 *   `| <n> | <severity> | <file> | <lines> | <classification> |`
 * where classification is `**Actionable**` (with markdown emphasis) or
 * `Informational` (plain). We count occurrences of each across the LAST
 * such table in the text (greptile may re-classify in each iteration).
 *
 * Returns {found, actionable_count, informational_count}.
 */
function extractPhase3Table(text) {
  const out = { found: false, actionable_count: 0, informational_count: 0 };
  if (!text) return out;

  // Find every markdown table whose header is `| # | Severity | File | Lines | Classification |`.
  // We walk line by line to be robust to wrapping.
  const lines = text.split(/\r?\n/);
  const tableHeaderRe = /^\s*\|\s*#\s*\|\s*Severity\s*\|\s*File\s*\|\s*Lines\s*\|\s*Classification\s*\|\s*$/;
  const tableDividerRe = /^\s*\|[\s\-:|]+\|\s*$/;
  const tableRowRe = /^\s*\|\s*\d+\s*\|/;

  // Collect (start, end) index pairs for every table matching the Phase 3
  // header.
  const tables = [];
  let i = 0;
  while (i < lines.length) {
    if (tableHeaderRe.test(lines[i])) {
      // Expect a divider line next; if not present, still accept.
      let j = i + 1;
      if (j < lines.length && tableDividerRe.test(lines[j])) j++;
      const rowStart = j;
      while (j < lines.length && tableRowRe.test(lines[j])) j++;
      const rowEnd = j; // exclusive
      if (rowEnd > rowStart) {
        tables.push({ start: i, headerEnd: rowStart, rowStart, rowEnd });
      }
      i = j;
    } else {
      i++;
    }
  }
  if (tables.length === 0) return out;

  // Use the LAST table (final iteration).
  const last = tables[tables.length - 1];
  let actionable = 0;
  let informational = 0;
  for (let k = last.rowStart; k < last.rowEnd; k++) {
    const row = lines[k];
    // Count `**Actionable**` (markdown emphasis) and `Informational` (plain).
    // The classification is in the last column before the trailing `|`.
    // Be careful: a row could in principle mention both words; count each
    // once per row, prioritizing the column.
    if (row.includes('**Actionable**')) actionable++;
    else if (/\bInformational\b/.test(row)) informational++;
  }
  out.found = true;
  out.actionable_count = actionable;
  out.informational_count = informational;
  return out;
}

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

function computeVerdict(remaining) {
  if (remaining === 0) return 'clean';
  return 'issues remain';
}

// ---------------------------------------------------------------------------
// Per-session metrics assembly
// ---------------------------------------------------------------------------

function buildEntry(variant, sessionId, events) {
  const text = concatenateAssistantText(events);
  const summary = extractGreptileSummary(text);
  const raw = extractRawGreptileJson(text);
  const issueCounts = tallyIssueCounts(raw);
  const table = extractPhase3Table(text);

  if (!summary.found) {
    warn(
      `session ${sessionId} (${variant}): no greptile Summary block found in assistant text — ` +
        `verdict will be "greptile output not found in session log"`
    );
    return {
      variant,
      session_id: sessionId,
      iterations: 0,
      findings_resolved: 0,
      remaining: 0,
      confidence: null,
      issue_counts: { p0: 0, p1: 0, p2: 0, p3: 0 },
      actionable_count: 0,
      informational_count: 0,
      verdict: 'greptile output not found in session log',
    };
  }

  if (raw === null) {
    warn(
      `session ${sessionId} (${variant}): no raw greptile JSON block found — ` +
        `issue_counts will be all zeros`
    );
  }
  if (!table.found) {
    warn(
      `session ${sessionId} (${variant}): no Phase 3 markdown table found — ` +
        `actionable/informational counts will be 0`
    );
  }

  return {
    variant,
    session_id: sessionId,
    iterations: summary.iterations,
    findings_resolved: summary.findings_resolved,
    remaining: summary.remaining,
    confidence: summary.confidence,
    issue_counts: issueCounts,
    actionable_count: table.actionable_count,
    informational_count: table.informational_count,
    verdict: computeVerdict(summary.remaining),
  };
}

// ---------------------------------------------------------------------------
// Summary line
// ---------------------------------------------------------------------------

function printSummaryLine(entry) {
  const it = entry.iterations;
  const fr = entry.findings_resolved;
  const conf = entry.confidence || '—';
  const ac = entry.actionable_count;
  const ic = entry.informational_count;
  const v = entry.verdict;
  process.stdout.write(
    `  ${entry.variant}: ${it} iter, ${fr}/${fr} resolved, confidence ${conf}, ` +
      `${ac} actionable + ${ic} informational, ${v}\n`
  );
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

  // 2. Filter to the 3 completed variants, preserving the order in the map.
  const completed = map.sessions.filter((s) => COMPLETED_VARIANTS.has(s.variant));
  if (completed.length < EXPECTED_COMPLETED) {
    fail(
      `expected ${EXPECTED_COMPLETED} completed-variant sessions in ${MAP_PATH}, found ${completed.length}`
    );
  }
  // Order the completed entries deterministically: Qwen baseline, temp 0.5,
  // temp 0.7 / top_p 0.95 (the spec's required order).
  const order = ['Qwen baseline', 'temp 0.5', 'temp 0.7 / top_p 0.95'];
  completed.sort((a, b) => order.indexOf(a.variant) - order.indexOf(b.variant));

  // 3. Resolve file paths in one readdir pass.
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

  // 4. Process each completed session.
  const results = [];
  for (const entry of completed) {
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

    const built = buildEntry(variant, sessionId, events);
    results.push(built);
  }

  if (results.length < EXPECTED_COMPLETED) {
    fail(`processed ${results.length} completed sessions, expected ${EXPECTED_COMPLETED}`);
  }

  // 5. Write output.
  const output = { branches: results };
  try {
    fs.writeFileSync(OUTPUT_PATH, JSON.stringify(output, null, 2) + '\n', 'utf8');
  } catch (e) {
    fail(`could not write ${OUTPUT_PATH}: ${e.message}`);
  }

  // 6. Print summaries in the spec'd order.
  for (const r of results) printSummaryLine(r);

  // 7. Exit non-zero if anything looks wrong.
  const anyMissing = results.some(
    (r) => r.verdict === 'greptile output not found in session log'
  );
  if (anyMissing) {
    process.exit(1);
  }
}

// Expose internals for testing. When run as a CLI, kick off the real
// pipeline.
if (require.main === module) {
  main().catch((e) => {
    err(e && e.stack ? e.stack : String(e));
    process.exit(1);
  });
}

module.exports = {
  concatenateAssistantText,
  extractGreptileSummary,
  extractRawGreptileJson,
  tallyIssueCounts,
  extractPhase3Table,
  computeVerdict,
  buildEntry,
  printSummaryLine,
  // Constants — useful for tests.
  COMPLETED_VARIANTS,
  EXPECTED_COMPLETED,
};
