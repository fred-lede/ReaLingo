import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyLocalDiagnostics, updateLocalDiagnostics, localBackendsReady } from '../src/core/local-diagnostics.ts';

test('local diagnostics converts queued 100 ms chunks and retains the latest stage timings', () => {
  let stats = emptyLocalDiagnostics();
  stats = updateLocalDiagnostics(stats, { type: 'local.asr', audioQueueChunks: 14, elapsedMs: 2082 });
  stats = updateLocalDiagnostics(stats, { type: 'local.translation', queueWaitMs: 340, elapsedMs: 560 });
  stats = updateLocalDiagnostics(stats, { type: 'local.asr', audioQueueChunks: 3, elapsedMs: 910 });
  assert.deepEqual(stats, {
    audioQueueMs: 300,
    asrElapsedMs: 910,
    translationQueueMs: 340,
    translationElapsedMs: 560,
    droppedChunks: 0,
    asrBackend: 'detecting',
    translationBackend: 'detecting',
  });
});

test('backend events update only the named stage and reset for each run', () => {
  let stats = emptyLocalDiagnostics();
  stats = updateLocalDiagnostics(stats, { type: 'local.backend', stage: 'asr', backend: 'metal' });
  stats = updateLocalDiagnostics(stats, { type: 'local.backend', stage: 'translation', backend: 'cpu' });
  assert.equal(stats.asrBackend, 'metal');
  assert.equal(stats.translationBackend, 'cpu');
  assert.deepEqual(updateLocalDiagnostics(stats, { type: 'local.backend', stage: 'asr', backend: 'gpu' }), stats);
  assert.deepEqual(updateLocalDiagnostics(stats, { type: 'local.backend', stage: 'other', backend: 'cuda' }), stats);
  assert.equal(emptyLocalDiagnostics().translationBackend, 'detecting');
});

test('drop count follows cumulative capture total and unrelated events do not change diagnostics', () => {
  let stats = updateLocalDiagnostics(emptyLocalDiagnostics(), { type: 'audio.capture_dropped', chunks: 4, totalChunks: 4 });
  stats = updateLocalDiagnostics(stats, { type: 'audio.capture_dropped', chunks: 2, totalChunks: 6 });
  assert.equal(stats.droppedChunks, 6);
  assert.deepEqual(updateLocalDiagnostics(stats, { type: 'session.updated' }), stats);
  assert.deepEqual(updateLocalDiagnostics(stats, { type: 'local.asr', elapsedMs: -1, audioQueueChunks: 'bad' }), stats);
});

test('local start is gated on every backend leaving "detecting"', () => {
  let stats = emptyLocalDiagnostics();
  // Fresh: both still probing — the start button stays disabled.
  assert.equal(localBackendsReady(stats, false), false);
  stats = updateLocalDiagnostics(stats, { type: 'local.backend', stage: 'asr', backend: 'metal' });
  assert.equal(localBackendsReady(stats, false), false);
  // Transcribe-only: the translator never runs, so only the ASR backend matters.
  stats = updateLocalDiagnostics(stats, { type: 'local.backend', stage: 'asr', backend: 'cpu' });
  assert.equal(localBackendsReady(stats, true), true);
  assert.equal(localBackendsReady(stats, false), false);
  stats = updateLocalDiagnostics(stats, { type: 'local.backend', stage: 'translation', backend: 'metal' });
  assert.equal(localBackendsReady(stats, false), true);
});
