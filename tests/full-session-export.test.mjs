// The main screen's export covers the whole session, not just the visible window: the stream
// keeps the last 500 rows on screen while the file carries every row since the session opened.
// Only OS I/O and persisted settings are stubbed; publish() and the export builders run unmodified.
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import test from 'node:test';
import { reactive, ref } from 'vue';

const settings = reactive({
  protocol: 'qwen38', model: 'm', mode: 'remote', sourceLang: 'auto', targetLang: 'en',
  transcriptStyle: 'system', sub: { mode: 'both' }, history: true,
});
globalThis.__sessionFullTest = { settings, locale: ref('en') };
registerHooks({
  resolve(specifier, context, next) {
    const name = specifier === '@tauri-apps/api/core' ? 'core'
      : specifier === '@tauri-apps/api/event' ? 'event'
      : specifier === '@tauri-apps/plugin-dialog' ? 'dialog'
      : /(?:^|\/)store(?:\.ts)?$/.test(specifier) ? 'store'
      : /(?:^|\/)i18n(?:\.ts)?$/.test(specifier) ? 'i18n' : null;
    if (name) return { url: `full-test:${name}`, shortCircuit: true };
    if (specifier.startsWith('.') && context.parentURL?.startsWith('file:')) {
      const url = new URL(`${specifier}.ts`, context.parentURL);
      if (existsSync(url)) return next(url.href);
    }
    return next(specifier);
  },
  load(url, context, next) {
    const sources = {
      core: 'export const invoke = async () => undefined;',
      event: 'export const listen = globalThis.__sessionFullTest.listen;',
      dialog: 'export const save = async () => null;',
      store: 'export const { settings } = globalThis.__sessionFullTest;',
      i18n: 'export const { locale } = globalThis.__sessionFullTest; export const t = (s) => s;',
    };
    if (url.startsWith('full-test:')) return { format: 'module', shortCircuit: true, source: sources[url.split(':')[1]] };
    return next(url, context);
  },
});

const events = new Map();
globalThis.__sessionFullTest.listen = async (name, cb) => { events.set(name, cb); return () => events.delete(name); };
const stream = await import('../src/core/stream.ts');

const emit = (kind, values = {}) =>
  events.get('rt://event')({ payload: { kind, text: '', stash: '', lang: '', id: '', done: false, ...values } });

/** One finished turn: the link that names it, then its transcript and translation. */
function say(i) {
  emit('link', { text: `in-${i}`, id: `turn-${i}` });
  emit('source', { text: `line ${i}`, id: `in-${i}`, done: true, startMs: i * 1000, endMs: i * 1000 + 800 });
  emit('target', { text: `trans ${i}`, id: `turn-${i}`, done: true, startMs: i * 1000, endMs: i * 1000 + 800 });
}

test('the export carries every row of the session, the screen keeps the last 500', () => {
  for (let i = 0; i < 600; i++) say(i);
  assert.equal(stream.lines.value.length, 500);
  assert.equal(stream.sessionLines.value.length, 600);
  const blocks = stream.exportTxt().split('\n\n');
  assert.equal(blocks.length, 600);
  assert.equal(blocks[0], 'line 0\ntrans 0');
  assert.equal(blocks[599], 'line 599\ntrans 599');
  assert.equal(stream.exportSrt().split('\n\n').filter(Boolean).length, 600);
});

test('a row scrolled out of the window is still updated by its own turn', () => {
  emit('link', { text: 'in-600', id: 'turn-600' });
  emit('source', { text: 'half a sentence', id: 'in-600', done: false, startMs: 600000, endMs: 600500 });
  for (let i = 601; i < 1101; i++) say(i); // the row leaves the visible window
  emit('source', { text: 'half a sentence that finished', id: 'in-600', done: true, startMs: 600000, endMs: 600900 });
  emit('target', { text: 'finished translation', id: 'turn-600', done: true, startMs: 600000, endMs: 600900 });
  const text = stream.exportTxt();
  assert.ok(text.includes('half a sentence that finished'), 'the finished transcript never reached the export');
  assert.ok(!text.includes('half a sentence\n'), 'the half-written transcript was left in the file');
  assert.equal(stream.sessionLines.value.length, 1101);
});

test('clearing the panel clears the export too', () => {
  stream.clearStream();
  assert.equal(stream.lines.value.length, 0);
  assert.equal(stream.sessionLines.value.length, 0);
  assert.equal(stream.exportTxt(), '');
  say(0);
  assert.equal(stream.exportTxt(), 'line 0\ntrans 0');
});

test('both front ends export the session, not the visible window', () => {
  const read = (p) => readFileSync(new URL(p, import.meta.url), 'utf8');
  for (const file of ['../src/desktop/App.vue', '../src/mobile/MobileApp.vue']) {
    const src = read(file);
    assert.match(src, /saveAs\(kind, sessionLines\.value/, `${file} still exports the visible window`);
    assert.match(src, /@click="clearStream\(\)"/, `${file} clears only the visible rows`);
    assert.doesNotMatch(src, /@click="lines = \[\]"/, `${file} still reassigns the ref in the template`);
  }
});
