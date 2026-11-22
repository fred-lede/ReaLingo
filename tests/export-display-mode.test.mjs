// The export carries what the subtitle mode shows: source only, target only, or both.
// Only OS I/O and persisted settings are stubbed; exportTxt/exportSrt run unmodified.
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { registerHooks } from 'node:module';
import test from 'node:test';
import { reactive, ref } from 'vue';

const settings = reactive({
  protocol: 'qwen38', model: 'm', sourceLang: 'auto', targetLang: 'en',
  transcriptStyle: 'system', sub: { mode: 'both' },
});
globalThis.__modeTest = { settings, locale: ref('en') };
registerHooks({
  resolve(specifier, context, next) {
    const name = specifier === '@tauri-apps/api/core' ? 'core'
      : specifier === '@tauri-apps/api/event' ? 'event'
      : specifier === '@tauri-apps/plugin-dialog' ? 'dialog'
      : /(?:^|\/)store(?:\.ts)?$/.test(specifier) ? 'store'
      : /(?:^|\/)i18n(?:\.ts)?$/.test(specifier) ? 'i18n' : null;
    if (name) return { url: `mode-test:${name}`, shortCircuit: true };
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
      store: 'export const { settings } = globalThis.__modeTest;',
      i18n: 'export const { locale } = globalThis.__modeTest; export const t = (s) => s;',
    };
    if (url.startsWith('mode-test:')) return { format: 'module', shortCircuit: true, source: sources[url.split(':')[1]] };
    return next(url, context);
  },
});

const stream = await import('../src/core/stream.ts');

const rows = [
  { source: 'hello', target: '你好', start: 0, end: 2000, at: 1000 },
  { source: 'world', target: '世界', start: 2000, end: 4000, at: 2000 },
];

function cueTexts(file) {
  return file.split('\n\n').filter(Boolean).map((cue) => cue.replace(/\n$/, '').split('\n').slice(2).join('\n'));
}

test('the export carries the lines the subtitle mode shows', () => {
  settings.sub.mode = 'both';
  assert.equal(stream.exportTxt(rows), 'hello\n你好\n\nworld\n世界');
  assert.deepEqual(cueTexts(stream.exportSrt(rows)), ['hello\n你好', 'world\n世界']);

  settings.sub.mode = 'target';
  assert.equal(stream.exportTxt(rows), '你好\n\n世界');
  assert.deepEqual(cueTexts(stream.exportSrt(rows)), ['你好', '世界']);

  settings.sub.mode = 'source';
  assert.equal(stream.exportTxt(rows), 'hello\n\nworld');
  assert.deepEqual(cueTexts(stream.exportSrt(rows)), ['hello', 'world']);
});

test('a transcription-only row exports its transcript even in target mode, and empty rows are dropped', () => {
  settings.sub.mode = 'target';
  const mixed = [
    { source: 'first', target: '', start: 0, end: 1000, at: 0 },
    { source: '', target: '', start: 1000, end: 2000, at: 1000 },
    { source: 'third', target: 'third', start: 2000, end: 3000, at: 2000 },
  ];
  assert.equal(stream.exportTxt(mixed), 'first\n\nthird');
  const cues = cueTexts(stream.exportSrt(mixed));
  assert.deepEqual(cues, ['first', 'third']);
  // Numbering has no hole where the empty row was dropped.
  const numbers = stream.exportSrt(mixed).split('\n').filter((line) => /^\d+$/.test(line));
  assert.deepEqual(numbers, ['1', '2']);
});
