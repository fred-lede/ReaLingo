import test from 'node:test';
import assert from 'node:assert/strict';
import { visibleServerAsr } from '../src/server-asr.ts';

test('the server ASR model is shown for a custom endpoint', () => {
  assert.equal(visibleServerAsr('ws://122.116.209.1:8001/v1/realtime', 'Qwen/Qwen3-ASR-1.7B'), 'Qwen/Qwen3-ASR-1.7B');
});

test('the server ASR model is hidden without a custom endpoint', () => {
  // For the built-in protocols the transcription model is the vendor's own, and the
  // server does not know it — showing the server's own would name a different model.
  assert.equal(visibleServerAsr('', 'Qwen/Qwen3-ASR-1.7B'), '');
  assert.equal(visibleServerAsr('   ', 'Qwen/Qwen3-ASR-1.7B'), '');
});

test('nothing is shown until the server has said which model it uses', () => {
  assert.equal(visibleServerAsr('ws://122.116.209.1:8001/v1/realtime', ''), '');
});
