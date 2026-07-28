#!/usr/bin/env node
/**
 * Tests for scripts/benchmark-extract.js — focused on the subagent_intended_model
 * field and the extended non-expected-model scan added to capture the
 * "main agent dispatched a subagent with the wrong model" anomaly class
 * (e.g. the temp-0.7 session dispatched 5 subagents with `arguments.model:
 * "gemini-2.5-flash"` while its own `message.model` was always `laguna-s-2.1`).
 *
 * Run: node test/benchmark-extract.test.js
 *
 * Pure stdlib (assert), no test framework. Exits 0 on success, 1 on any
 * assertion failure.
 */

'use strict';

const assert = require('assert');
const path = require('path');

const {
  computeSessionMetrics,
  computeModelCorrectness,
  printSummaryLine,
} = require('../scripts/benchmark-extract.js');

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    passed++;
    process.stdout.write(`  ok  ${name}\n`);
  } catch (e) {
    failed++;
    process.stdout.write(`  FAIL ${name}\n`);
    if (e && e.stack) process.stdout.write(e.stack + '\n');
    else process.stdout.write(String(e) + '\n');
  }
}

/** Build a minimal valid events array for a synthetic session. */
function buildEvents({ sessionId, subagentDispatchModel, withSecondDispatch = true, withMatchResults = true } = {}) {
  const events = [
    {
      type: 'session',
      id: sessionId,
      timestamp: '2026-07-28T12:00:00.000Z',
      cwd: '/tmp',
      version: 3,
    },
  ];

  // First assistant message containing a subagent toolCall.
  const firstToolCall = {
    type: 'toolCall',
    id: 'tc_gemini_1',
    name: 'subagent',
    arguments: {
      agent: 'general',
      task: 'task 1 with wrong model',
    },
  };
  if (subagentDispatchModel !== undefined) {
    firstToolCall.arguments.model = subagentDispatchModel;
  }
  events.push({
    type: 'message',
    id: 'm1',
    parentId: sessionId,
    timestamp: '2026-07-28T12:00:01.000Z',
    message: {
      role: 'assistant',
      content: [{ type: 'text', text: 'dispatching' }, firstToolCall],
      model: 'laguna-s-2.1',
      usage: { totalTokens: 100 },
    },
  });
  if (withMatchResults) {
    events.push({
      type: 'message',
      id: 'r1',
      parentId: 'm1',
      timestamp: '2026-07-28T12:00:02.000Z',
      message: {
        role: 'toolResult',
        toolCallId: 'tc_gemini_1',
        toolName: 'subagent',
        content: [{ type: 'text', text: 'completed' }],
        isError: false,
      },
    });
  }

  if (withSecondDispatch) {
    // Second assistant message with a subagent toolCall that has NO model arg.
    events.push({
      type: 'message',
      id: 'm2',
      parentId: 'r1',
      timestamp: '2026-07-28T12:00:03.000Z',
      message: {
        role: 'assistant',
        content: [
          { type: 'text', text: 'dispatching again' },
          {
            type: 'toolCall',
            id: 'tc_default_2',
            name: 'subagent',
            arguments: { agent: 'general', task: 'task 2 with no model arg' },
          },
        ],
        model: 'laguna-s-2.1',
        usage: { totalTokens: 100 },
      },
    });
    if (withMatchResults) {
      events.push({
        type: 'message',
        id: 'r2',
        parentId: 'm2',
        timestamp: '2026-07-28T12:00:04.000Z',
        message: {
          role: 'toolResult',
          toolCallId: 'tc_default_2',
          toolName: 'subagent',
          content: [{ type: 'text', text: 'completed' }],
          isError: false,
        },
      });
    }
  }

  return events;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

process.stdout.write('benchmark-extract.js — subagent_intended_model + non-expected scan\n');

test('subagent record: subagent_intended_model captured from arguments.model (non-empty string)', () => {
  const events = buildEvents({ sessionId: 's1', subagentDispatchModel: 'gemini-2.5-flash' });
  const m = computeSessionMetrics('temp 0.7 / top_p 0.95', 's1', '/tmp/s1.jsonl', events);
  assert.strictEqual(m.subagents.length, 2, 'expected 2 subagent records');
  assert.strictEqual(
    m.subagents[0].subagent_intended_model,
    'gemini-2.5-flash',
    'first subagent should record arguments.model verbatim'
  );
  assert.strictEqual(
    m.subagents[1].subagent_intended_model,
    null,
    'second subagent (no model arg) should be null'
  );
});

test('subagent record: subagent_intended_model is null when arguments.model is missing', () => {
  const events = buildEvents({ sessionId: 's1', subagentDispatchModel: undefined });
  const m = computeSessionMetrics('temp 0.5', 's1', '/tmp/s1.jsonl', events);
  assert.strictEqual(m.subagents.length, 2);
  for (const s of m.subagents) {
    assert.strictEqual(s.subagent_intended_model, null);
  }
});

test('subagent record: subagent_intended_model is null when arguments.model is empty string', () => {
  // The spec says "non-empty string" — empty string should be treated as null.
  const events = buildEvents({ sessionId: 's1', subagentDispatchModel: '' });
  const m = computeSessionMetrics('temp 0.5', 's1', '/tmp/s1.jsonl', events);
  assert.strictEqual(m.subagents[0].subagent_intended_model, null);
});

test('non-expected scan: gemini-2.5-flash dispatched to subagent in a laguna session → anomaly', () => {
  const events = buildEvents({ sessionId: 's1', subagentDispatchModel: 'gemini-2.5-flash' });
  const m = computeSessionMetrics('temp 0.7 / top_p 0.95', 's1', '/tmp/s1.jsonl', events);
  Object.assign(m, computeModelCorrectness(m));

  // main agent always used laguna-s-2.1; no main-agent anomaly.
  // (model_usage is built with Object.create(null); assert via Object.entries.)
  assert.deepStrictEqual(Object.entries(m.model_usage), [['laguna-s-2.1', 2]]);
  assert.strictEqual(m.had_model_anomaly, true, 'had_model_anomaly must be true');
  assert.strictEqual(m.non_expected_models_used.length, 1, 'one non-expected entry');
  assert.deepStrictEqual(m.non_expected_models_used[0], {
    model: 'gemini-2.5-flash',
    count: 1,
    source: 'subagent_dispatch',
  });
});

test('non-expected scan: no anomaly when all subagent dispatches use the expected model (or null)', () => {
  // All dispatches are "laguna-s-2.1" or null — matches the variant's expected.
  const events = buildEvents({ sessionId: 's1', subagentDispatchModel: 'laguna-s-2.1' });
  // Second dispatch has no model — also fine.
  const m = computeSessionMetrics('temp 0.5', 's1', '/tmp/s1.jsonl', events);
  Object.assign(m, computeModelCorrectness(m));
  assert.strictEqual(m.had_model_anomaly, false, 'no anomaly when everything matches');
  assert.deepStrictEqual(m.non_expected_models_used, []);
});

test('non-expected scan: main-agent + subagent-dispatch anomalies are merged in one array', () => {
  // Build a session where the main agent also briefly used the wrong model.
  // We assemble events by hand for full control.
  const events = [
    { type: 'session', id: 's2', timestamp: '2026-07-28T12:00:00.000Z', cwd: '/tmp', version: 3 },
    {
      type: 'message',
      id: 'm1',
      parentId: 's2',
      timestamp: '2026-07-28T12:00:01.000Z',
      message: {
        role: 'assistant',
        content: [{ type: 'text', text: 'hi' }],
        model: 'gemini-2.5-flash', // main-agent wrong model
        usage: { totalTokens: 50 },
      },
    },
    {
      type: 'message',
      id: 'm2',
      parentId: 'm1',
      timestamp: '2026-07-28T12:00:02.000Z',
      message: {
        role: 'assistant',
        content: [
          {
            type: 'toolCall',
            id: 'tc_sub1',
            name: 'subagent',
            arguments: { agent: 'general', task: 't', model: 'haiku' }, // subagent-dispatch wrong model
          },
        ],
        model: 'laguna-s-2.1',
        usage: { totalTokens: 50 },
      },
    },
    {
      type: 'message',
      id: 'r1',
      parentId: 'm2',
      timestamp: '2026-07-28T12:00:03.000Z',
      message: {
        role: 'toolResult',
        toolCallId: 'tc_sub1',
        toolName: 'subagent',
        content: [{ type: 'text', text: 'done' }],
        isError: false,
      },
    },
  ];
  const m = computeSessionMetrics('temp 0.5', 's2', '/tmp/s2.jsonl', events);
  Object.assign(m, computeModelCorrectness(m));

  // model_usage should have BOTH the main-agent and the (hypothetical) subagent-side counts.
  // The assistant message at m2 is the one that contains the subagent toolCall — its
  // message.model is "laguna-s-2.1", so model_usage will record 1 × laguna + 1 × gemini.
  assert.strictEqual(m.model_usage['gemini-2.5-flash'], 1, 'main-agent gemini turn tallied');
  assert.strictEqual(m.model_usage['laguna-s-2.1'], 1, 'main-agent laguna turn tallied');

  // Now the non-expected array should have BOTH entries, sorted by count desc, source asc.
  assert.strictEqual(m.non_expected_models_used.length, 2);
  // Both have count=1; tie-break by source alphabetically → main_agent before subagent_dispatch.
  assert.deepStrictEqual(m.non_expected_models_used[0], {
    model: 'gemini-2.5-flash',
    count: 1,
    source: 'main_agent',
  });
  assert.deepStrictEqual(m.non_expected_models_used[1], {
    model: 'haiku',
    count: 1,
    source: 'subagent_dispatch',
  });
  assert.strictEqual(m.had_model_anomaly, true);
});

test('Qwen baseline: subagent expected is qwen3.6-35b-a3b (substring); matches "qwen3.6-35b-a3b"', () => {
  // Build a Qwen session with one subagent dispatched with qwen3.6-35b-a3b and one with gemini.
  const events = [
    { type: 'session', id: 's3', timestamp: '2026-07-28T12:00:00.000Z', cwd: '/tmp', version: 3 },
    {
      type: 'message',
      id: 'm1',
      parentId: 's3',
      timestamp: '2026-07-28T12:00:01.000Z',
      message: {
        role: 'assistant',
        content: [
          { type: 'text', text: 'go' },
          {
            type: 'toolCall',
            id: 'tc_a',
            name: 'subagent',
            arguments: { agent: 'general', task: 'a', model: 'qwen3.6-35b-a3b' },
          },
          {
            type: 'toolCall',
            id: 'tc_b',
            name: 'subagent',
            arguments: { agent: 'general', task: 'b', model: 'gemini-2.5-flash' },
          },
        ],
        model: 'qwen3.6-27b',
        usage: { totalTokens: 100 },
      },
    },
    {
      type: 'message',
      id: 'r_a',
      parentId: 'm1',
      timestamp: '2026-07-28T12:00:02.000Z',
      message: {
        role: 'toolResult',
        toolCallId: 'tc_a',
        toolName: 'subagent',
        content: [{ type: 'text', text: 'a ok' }],
        isError: false,
      },
    },
    {
      type: 'message',
      id: 'r_b',
      parentId: 'm1',
      timestamp: '2026-07-28T12:00:03.000Z',
      message: {
        role: 'toolResult',
        toolCallId: 'tc_b',
        toolName: 'subagent',
        content: [{ type: 'text', text: 'b ok' }],
        isError: false,
      },
    },
  ];
  const m = computeSessionMetrics('Qwen baseline', 's3', '/tmp/s3.jsonl', events);
  Object.assign(m, computeModelCorrectness(m));

  // Subagent #1 is expected (qwen3.6-35b-a3b), #2 is an anomaly.
  assert.strictEqual(m.subagents.length, 2);
  assert.strictEqual(m.subagents[0].subagent_intended_model, 'qwen3.6-35b-a3b');
  assert.strictEqual(m.subagents[1].subagent_intended_model, 'gemini-2.5-flash');
  assert.strictEqual(m.had_model_anomaly, true);
  assert.deepStrictEqual(m.non_expected_models_used, [
    { model: 'gemini-2.5-flash', count: 1, source: 'subagent_dispatch' },
  ]);
});

test('summary line: includes (subagent) suffix for subagent-dispatch anomaly', () => {
  const events = buildEvents({ sessionId: 's4', subagentDispatchModel: 'gemini-2.5-flash' });
  const m = computeSessionMetrics('temp 0.7 / top_p 0.95', 's4', '/tmp/s4.jsonl', events);
  Object.assign(m, computeModelCorrectness(m));

  // Capture stdout from printSummaryLine.
  const origWrite = process.stdout.write.bind(process.stdout);
  let captured = '';
  process.stdout.write = (chunk) => {
    captured += chunk;
    return true;
  };
  try {
    printSummaryLine(m);
  } finally {
    process.stdout.write = origWrite;
  }
  assert.ok(
    /model-anomaly=gemini-2\.5-flash:1 \(subagent\)/.test(captured),
    `summary line should include 'model-anomaly=gemini-2.5-flash:1 (subagent)'. Got: ${captured.trim()}`
  );
});

test('summary line: includes (main) suffix for main-agent anomaly', () => {
  // Synthesize a session where the main agent itself briefly used gemini-2.5-flash.
  const events = [
    { type: 'session', id: 's5', timestamp: '2026-07-28T12:00:00.000Z', cwd: '/tmp', version: 3 },
    {
      type: 'message',
      id: 'm1',
      parentId: 's5',
      timestamp: '2026-07-28T12:00:01.000Z',
      message: {
        role: 'assistant',
        content: [{ type: 'text', text: 'hi' }],
        model: 'gemini-2.5-flash',
        usage: { totalTokens: 50 },
      },
    },
  ];
  const m = computeSessionMetrics('temp 0.5', 's5', '/tmp/s5.jsonl', events);
  Object.assign(m, computeModelCorrectness(m));

  const origWrite = process.stdout.write.bind(process.stdout);
  let captured = '';
  process.stdout.write = (chunk) => {
    captured += chunk;
    return true;
  };
  try {
    printSummaryLine(m);
  } finally {
    process.stdout.write = origWrite;
  }
  assert.ok(
    /model-anomaly=gemini-2\.5-flash:1 \(main\)/.test(captured),
    `summary line should include 'model-anomaly=gemini-2.5-flash:1 (main)'. Got: ${captured.trim()}`
  );
});

test('summary line: anomaly line shows BOTH (main) and (subagent) when both classes present', () => {
  // Build a session with a main-agent anomaly AND a subagent-dispatch anomaly.
  const events = [
    { type: 'session', id: 's6', timestamp: '2026-07-28T12:00:00.000Z', cwd: '/tmp', version: 3 },
    {
      type: 'message',
      id: 'm1',
      parentId: 's6',
      timestamp: '2026-07-28T12:00:01.000Z',
      message: {
        role: 'assistant',
        content: [{ type: 'text', text: 'hi' }],
        model: 'gemini-2.5-flash',
        usage: { totalTokens: 50 },
      },
    },
    {
      type: 'message',
      id: 'm2',
      parentId: 'm1',
      timestamp: '2026-07-28T12:00:02.000Z',
      message: {
        role: 'assistant',
        content: [
          {
            type: 'toolCall',
            id: 'tc_sub1',
            name: 'subagent',
            arguments: { agent: 'general', task: 't', model: 'haiku' },
          },
        ],
        model: 'laguna-s-2.1',
        usage: { totalTokens: 50 },
      },
    },
    {
      type: 'message',
      id: 'r1',
      parentId: 'm2',
      timestamp: '2026-07-28T12:00:03.000Z',
      message: {
        role: 'toolResult',
        toolCallId: 'tc_sub1',
        toolName: 'subagent',
        content: [{ type: 'text', text: 'done' }],
        isError: false,
      },
    },
  ];
  const m = computeSessionMetrics('temp 0.5', 's6', '/tmp/s6.jsonl', events);
  Object.assign(m, computeModelCorrectness(m));

  const origWrite = process.stdout.write.bind(process.stdout);
  let captured = '';
  process.stdout.write = (chunk) => {
    captured += chunk;
    return true;
  };
  try {
    printSummaryLine(m);
  } finally {
    process.stdout.write = origWrite;
  }
  // main_agent entry should come first (count=1, source='main_agent' < 'subagent_dispatch' alphabetically)
  assert.ok(
    /model-anomaly=gemini-2\.5-flash:1 \(main\),haiku:1 \(subagent\)/.test(captured),
    `summary line should show both. Got: ${captured.trim()}`
  );
});

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

process.stdout.write(`\nbenchmark-extract: ${passed} passed, ${failed} failed\n`);
process.exit(failed > 0 ? 1 : 0);
