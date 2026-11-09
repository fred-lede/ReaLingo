import test from 'node:test';
import assert from 'node:assert/strict';
import { isInputTooQuiet } from '../src/input-level.ts';

test('an input the recogniser handles well does not warn', () => {
  // 0.0067 is a microphone at a low gain. The server judges speech by the input level
  // rather than a fixed cutoff, and the model transcribes this level perfectly — a
  // warning here would be crying wolf over audio that works.
  assert.equal(isInputTooQuiet(0.0067, { running: true, remote: true }), false);
  assert.equal(isInputTooQuiet(0.01, { running: true, remote: true }), false);
});

test('an input with no signal in it warns', () => {
  assert.equal(isInputTooQuiet(0.0001, { running: true, remote: true }), true);
  assert.equal(isInputTooQuiet(0, { running: true, remote: true }), true);
});

test('a loud input does not warn', () => {
  assert.equal(isInputTooQuiet(0.09, { running: true, remote: true }), false);
});

test('nothing is said when no session runs', () => {
  assert.equal(isInputTooQuiet(0.0001, { running: false, remote: true }), false);
});

test('the local mode does not warn: its Whisper hears a quiet microphone', () => {
  assert.equal(isInputTooQuiet(0.0001, { running: true, remote: false }), false);
});
