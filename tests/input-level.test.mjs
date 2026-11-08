import test from 'node:test';
import assert from 'node:assert/strict';
import { isInputTooQuiet } from '../src/input-level.ts';

test('a quiet input warns while a remote session runs', () => {
  assert.equal(isInputTooQuiet(0.0067, { running: true, remote: true }), true);
});

test('a loud input does not warn', () => {
  assert.equal(isInputTooQuiet(0.09, { running: true, remote: true }), false);
});

test('nothing is said when no session runs', () => {
  assert.equal(isInputTooQuiet(0.001, { running: false, remote: true }), false);
});

test('the local mode does not warn: its Whisper hears a quiet microphone', () => {
  assert.equal(isInputTooQuiet(0.001, { running: true, remote: false }), false);
});

test('a silent input counts as quiet, since a silent microphone is quiet too', () => {
  assert.equal(isInputTooQuiet(0, { running: true, remote: true }), true);
});
