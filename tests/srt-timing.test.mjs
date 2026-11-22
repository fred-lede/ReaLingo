// SRT cue timing: cues must run in audio order, never overlap, never have a negative or
// zero-length span, and survive rows the backend never timed. Only OS I/O and persisted
// settings are stubbed; exportSrt runs unmodified.
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { registerHooks } from 'node:module';
import test from 'node:test';
import { reactive, ref } from 'vue';

const settings = reactive({ protocol: 'qwen38', model: 'm', sourceLang: 'auto', targetLang: 'en' });
globalThis.__srtTest = { settings, locale: ref('zh') };
registerHooks({
  resolve(specifier, context, next) {
    const name = specifier === '@tauri-apps/api/core' ? 'core'
      : specifier === '@tauri-apps/api/event' ? 'event'
      : specifier === '@tauri-apps/plugin-dialog' ? 'dialog'
      : /(?:^|\/)store(?:\.ts)?$/.test(specifier) ? 'store'
      : /(?:^|\/)i18n(?:\.ts)?$/.test(specifier) ? 'i18n' : null;
    if (name) return { url: `srt-test:${name}`, shortCircuit: true };
    if (specifier.startsWith('.') && context.parentURL?.startsWith('file:')) {
      const url = new URL(`${specifier}.ts`, context.parentURL);
      if (existsSync(url)) return next(url.href);
    }
    return next(specifier);
  },
  load(url, context, next) {
    const sources = {
      core: 'export const invoke = async () => undefined;',
      event: 'export const listen = async () => () => {};',
      dialog: 'export const save = async () => null;',
      store: 'export const { settings } = globalThis.__srtTest;',
      i18n: 'export const { locale } = globalThis.__srtTest; export const t = (s) => s;',
    };
    if (url.startsWith('srt-test:')) return { format: 'module', shortCircuit: true, source: sources[url.split(':')[1]] };
    return next(url, context);
  },
});

const stream = await import('../src/core/stream.ts');

const parse = (stamp) => stamp.split(':').reduce((ms, part) => ms * 60000 + Number(part.split(',')[0]) * 1000 + Number(part.split(',')[1] ?? 0), 0);

/** Cue number, start, end, text lines — the shape a player reads. */
function cues(file) {
  return file.split('\n\n').filter(Boolean).map((cue) => {
    const [index, times, ...text] = cue.replace(/\n+$/, '').split('\n');
    const [start, end] = times.split(' --> ').map(parse);
    return { index: Number(index), start, end, text: text.join('\n') };
  });
}

function assertWellFormed(file) {
  const list = cues(file);
  assert.ok(list.length > 0, 'no cues at all');
  assert.ok(!file.endsWith('\n\n'), 'the file ends with a blank line');
  for (const [i, c] of list.entries()) {
    assert.ok(Number.isInteger(c.index) && c.index === i + 1, `cue ${i + 1} is numbered ${c.index}`);
    assert.ok(c.start >= 0, `cue ${c.index} starts before the audio: ${c.start}`);
    assert.ok(c.end > c.start, `cue ${c.index} ends at or before its start: ${c.start} --> ${c.end}`);
    assert.ok(c.text.trim() !== '', `cue ${c.index} has no text`);
    if (i + 1 < list.length) assert.ok(c.end <= list[i + 1].start, `cue ${c.index} overlaps cue ${c.index + 1}`);
  }
  return list;
}

// Windows the VAD kept open across a pause: each row's span is its own window.
test('sensevoice-style windows export cues in window order with no overlap', () => {
  const file = stream.exportSrt([
    { id: 1, source: '第一句', target: '', at: 1000, start: 0, end: 2600 },
    { id: 2, source: '第二句', target: '', at: 4000, start: 3900, end: 6200 },
    { id: 3, source: '第三句', target: '', at: 8000, start: 6100, end: 9500 },
  ]);
  const list = assertWellFormed(file);
  assert.deepEqual(list.map((c) => [c.start, c.end]), [[0, 2600], [3900, 6100], [6100, 9500]]);
});

// A transcript-only cue has no second line, so it must not carry an empty one.
test('a transcript-only cue has one text line, not a trailing empty one', () => {
  const file = stream.exportSrt([{ id: 1, source: '第一句', target: '', at: 0, start: 0, end: 2600 }]);
  assert.equal(file, '1\n00:00:00,000 --> 00:00:02,600\n第一句\n');
});

// Whisper's overlapping windows can start a segment before the previous one ended.
test('overlapping spans are cut at the next cue instead of overlapping it', () => {
  const file = stream.exportSrt([
    { id: 1, source: 'A', target: '', at: 1000, start: 0, end: 3000 },
    { id: 2, source: 'B', target: '', at: 3000, start: 2000, end: 5000 },
  ]);
  assertWellFormed(file);
});

// Rows sit in turn order, which is not audio order when the server times them separately.
test('rows out of audio order are sorted before the cues are numbered', () => {
  const file = stream.exportSrt([
    { id: 1, source: 'later', target: '', at: 10000, start: 20000, end: 24000 },
    { id: 2, source: 'earlier', target: '', at: 11000, start: 5000, end: 8000 },
  ]);
  const list = assertWellFormed(file);
  assert.equal(list[0].text, 'earlier');
  assert.equal(list[1].text, 'later');
});

// Silence: a cue keeps its own span instead of stretching across the gap.
test('a long silence leaves the cue at its own span, not padded to the next', () => {
  const file = stream.exportSrt([
    { id: 1, source: 'A', target: '', at: 1000, start: 0, end: 1500 },
    { id: 2, source: 'B', target: '', at: 30000, start: 30000, end: 33000 },
  ]);
  const list = assertWellFormed(file);
  assert.equal(list[0].end, 1500);
  assert.equal(list[1].start, 30000);
});

// Cues closer than a second apart still stay readable, and the next one starts on time.
test('cues closer than a second apart do not overlap', () => {
  const file = stream.exportSrt([
    { id: 1, source: 'A', target: '', at: 0, start: 0, end: 400 },
    { id: 2, source: 'B', target: '', at: 300, start: 300, end: 700 },
    { id: 3, source: 'C', target: '', at: 600, start: 600, end: 900 },
  ]);
  assertWellFormed(file);
});

// History written before spans were recorded: the timeline is the arrival clock.
test('untimed history rows fall back to arrival time without negative cues', () => {
  const file = stream.exportSrt([
    { id: 1, source: 'A', target: 'X', at: 100000 },
    { id: 2, source: 'B', target: 'Y', at: 104000 },
  ]);
  const list = assertWellFormed(file);
  assert.deepEqual(list.map((c) => [c.start, c.end]), [[0, 4000], [4000, 7000]]);
});

// The arrival clock is anchored at the earliest row, not the first one in the file.
test('the arrival clock is anchored at the earliest arrival, never a negative start', () => {
  const file = stream.exportSrt([
    { id: 1, source: 'A', target: 'X', at: 100000 },
    { id: 2, source: 'B', target: 'Y', at: 95000 },
  ]);
  const list = assertWellFormed(file);
  assert.equal(list[0].text, 'B\nY');
  assert.equal(list[0].start, 0);
  assert.deepEqual(list.map((c) => [c.start, c.end]), [[0, 5000], [5000, 8000]]);
});

// Bilingual cues keep both lines inside one cue.
test('a translated row is one cue with source then target', () => {
  const file = stream.exportSrt([{ id: 1, source: '你好', target: 'Hello', at: 0, start: 1000, end: 2000 }]);
  const [cue] = assertWellFormed(file);
  assert.equal(cue.text, '你好\nHello');
});
