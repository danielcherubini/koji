#!/usr/bin/env node
/**
 * benchmark-greptile.test.js
 *
 * Tests for benchmark-greptile.js. Pure-stdlib (node:assert + node:test).
 * Run with: node --test scripts/benchmark-greptile.test.js
 *
 * These tests load the module via require() and exercise its exported pure
 * functions. They do NOT spawn the CLI / touch the real sessions directory —
 * we feed in synthetic JSONL text and verify the extractors.
 *
 * The CLI is exercised separately in the self-test step in the plan.
 */

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');

const bg = require('./benchmark-greptile.js');

// ---------------------------------------------------------------------------
// extractGreptileSummary
// ---------------------------------------------------------------------------

test('extractGreptileSummary: parses the canonical Summary block (indented)', () => {
  // The actual data has 2-space indentation before each `- ` line.
  const text = `
some preamble
  **Summary:**
  - Iterations: 2
  - Findings resolved: 2 (P0 wire-format regression)
  - Remaining: 0
  - Final confidence: 5/5

  **Note:** ...
`;
  const out = bg.extractGreptileSummary(text);
  assert.equal(out.found, true);
  assert.equal(out.iterations, 2);
  assert.equal(out.findings_resolved, 2);
  assert.equal(out.remaining, 0);
  assert.equal(out.confidence, '5/5');
});

test('extractGreptileSummary: uses LAST match (greptile runs multiple iterations)', () => {
  const text = `
  **Summary:**
  - Iterations: 1
  - Findings resolved: 3
  - Remaining: 2
  - Final confidence: 2/5

  ... lots of stuff between iterations ...

  **Summary:**
  - Iterations: 2
  - Findings resolved: 2
  - Remaining: 0
  - Final confidence: 5/5
`;
  const out = bg.extractGreptileSummary(text);
  assert.equal(out.iterations, 2);
  assert.equal(out.findings_resolved, 2);
  assert.equal(out.remaining, 0);
  assert.equal(out.confidence, '5/5');
});

test('extractGreptileSummary: found=false when no Summary block is present', () => {
  const out = bg.extractGreptileSummary('no greptile output here at all');
  assert.equal(out.found, false);
  assert.equal(out.iterations, 0);
  assert.equal(out.findings_resolved, 0);
  assert.equal(out.remaining, 0);
  assert.equal(out.confidence, null);
});

test('extractGreptileSummary: confidence is always N/5 format', () => {
  const text = '- Final confidence: 4/5\n';
  const out = bg.extractGreptileSummary(text);
  assert.equal(out.confidence, '4/5');
});

// ---------------------------------------------------------------------------
// extractRawGreptileJson
// ---------------------------------------------------------------------------

test('extractRawGreptileJson: extracts the LAST raw JSON block (multi-iteration)', () => {
  // First iteration has 3 comments; final iteration has 0 comments (clean).
  const text1 = '{"summary":"first","comments":[{"severity":"P0"},{"severity":"P0"},{"severity":"P2"}]}';
  const text2 = '{"summary":"final","comments":[]}';
  const out = bg.extractRawGreptileJson(text1 + '\n...\n' + text2);
  assert.ok(out, 'expected a parsed JSON object');
  assert.equal(out.summary, 'final');
  assert.equal(out.comments.length, 0);
});

test('extractRawGreptileJson: returns null when no greptile JSON block is present', () => {
  const out = bg.extractRawGreptileJson('no greptile raw output here');
  assert.equal(out, null);
});

test('tallyIssueCounts: counts P0, P1, P2, P3 correctly', () => {
  const obj = {
    comments: [
      { severity: 'P0' },
      { severity: 'P0' },
      { severity: 'P1' },
      { severity: 'P2' },
      { severity: 'P2' },
      { severity: 'P2' },
      { severity: 'P3' },
    ],
  };
  const out = bg.tallyIssueCounts(obj);
  assert.deepEqual(out, { p0: 2, p1: 1, p2: 3, p3: 1 });
});

test('tallyIssueCounts: handles missing comments key', () => {
  const out = bg.tallyIssueCounts({});
  assert.deepEqual(out, { p0: 0, p1: 0, p2: 0, p3: 0 });
});

test('tallyIssueCounts: ignores unrecognized severities', () => {
  const out = bg.tallyIssueCounts({ comments: [{ severity: 'P0' }, { severity: 'BOGUS' }] });
  assert.deepEqual(out, { p0: 1, p1: 0, p2: 0, p3: 0 });
});

// ---------------------------------------------------------------------------
// extractPhase3Table
// ---------------------------------------------------------------------------

test('extractPhase3Table: counts **Actionable** and Informational rows in the LAST table', () => {
  const text = `
| # | Severity | File | Lines | Classification |
|---|----------|------|-------|----------------|
| 1 | **P0** | a.rs | 1-2 | **Actionable** — one |
| 2 | **P0** | b.rs | 3-4 | **Actionable** — two |
| 3 | P2 | c.rs | 5 | Informational — three |

  ... other stuff ...

| # | Severity | File | Lines | Classification |
|---|----------|------|-------|----------------|
| 1 | P2 | d.rs | 6 | Informational — four
`;
  const out = bg.extractPhase3Table(text);
  assert.equal(out.found, true);
  assert.equal(out.actionable_count, 0);
  assert.equal(out.informational_count, 1);
});

test('extractPhase3Table: handles the only/only table', () => {
  const text = `
| # | Severity | File | Lines | Classification |
|---|----------|------|-------|----------------|
| 1 | **P0** | a.rs | 1-2 | **Actionable** — one |
| 2 | P2 | b.rs | 3 | Informational — two |
`;
  const out = bg.extractPhase3Table(text);
  assert.equal(out.found, true);
  assert.equal(out.actionable_count, 1);
  assert.equal(out.informational_count, 1);
});

test('extractPhase3Table: found=false when no Phase 3 table is present', () => {
  const out = bg.extractPhase3Table('no table here, just prose');
  assert.equal(out.found, false);
  assert.equal(out.actionable_count, 0);
  assert.equal(out.informational_count, 0);
});

// ---------------------------------------------------------------------------
// computeVerdict
// ---------------------------------------------------------------------------

test('computeVerdict: clean when remaining === 0', () => {
  assert.equal(bg.computeVerdict(0), 'clean');
});

test('computeVerdict: issues remain when remaining > 0', () => {
  assert.equal(bg.computeVerdict(1), 'issues remain');
  assert.equal(bg.computeVerdict(5), 'issues remain');
});

// ---------------------------------------------------------------------------
// Stream-JSONL helper (concurrent-safe synthetic file content)
// ---------------------------------------------------------------------------

test('concatenateAssistantText: joins assistant text + ask descriptions + toolResult text', () => {
  const events = [
    { type: 'message', message: { role: 'user', content: [{ type: 'text', text: 'user says hi' }] } },
    { type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: 'first assistant reply' }, { type: 'thinking', thinking: 'private' }] } },
    { type: 'message', message: { role: 'toolResult', toolName: 'bash', content: [{ type: 'text', text: 'tool output' }] } },
    { type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: 'second assistant reply' }] } },
  ];
  // thinking is excluded; user is excluded; toolResult text is INCLUDED so
  // the raw greptile JSON and bash captures can be parsed. Assistant text
  // blocks are the primary source.
  const out = bg.concatenateAssistantText(events);
  assert.equal(out, 'first assistant reply\nsecond assistant reply\ntool output');
});

test('concatenateAssistantText: includes ask toolCall arguments descriptions', () => {
  const events = [
    {
      type: 'message',
      message: {
        role: 'assistant',
        content: [
          { type: 'text', text: 'summary below' },
          {
            type: 'toolCall',
            name: 'ask',
            arguments: {
              questions: [
                { description: '**Summary:**\n- Iterations: 2\n- Findings resolved: 2\n- Remaining: 0\n- Final confidence: 5/5' },
              ],
            },
          },
        ],
      },
    },
  ];
  const out = bg.concatenateAssistantText(events);
  assert.ok(out.includes('**Summary:**'));
  assert.ok(out.includes('Final confidence: 5/5'));
});

test('concatenateAssistantText: skips events with non-array content', () => {
  const events = [
    { type: 'message', message: { role: 'assistant', content: 'scalar' } },
    { type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: 'array form' }] } },
  ];
  const out = bg.concatenateAssistantText(events);
  assert.equal(out, 'array form');
});
