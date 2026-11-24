// 僅轉錄 works in cloud mode too: the endpoint still answers with a translation, so the app
// drops it where the stream files a row and where an export reads one. Only OS I/O and
// persisted settings are stubbed; exportTxt/exportSrt run unmodified.
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import test from 'node:test';
import { reactive, ref } from 'vue';

const settings = reactive({
  mode: 'remote', protocol: 'openai', model: 'm', sourceLang: 'auto', targetLang: 'zh-TW',
  transcriptStyle: 'system', transcribeOnly: true, sub: { mode: 'both' },
});
globalThis.__onlyTest = { settings, locale: ref('en') };
registerHooks({
  resolve(specifier, context, next) {
    const name = specifier === '@tauri-apps/api/core' ? 'core'
      : specifier === '@tauri-apps/api/event' ? 'event'
      : specifier === '@tauri-apps/plugin-dialog' ? 'dialog'
      : /(?:^|\/)store(?:\.ts)?$/.test(specifier) ? 'store'
      : /(?:^|\/)i18n(?:\.ts)?$/.test(specifier) ? 'i18n' : null;
    if (name) return { url: `only-test:${name}`, shortCircuit: true };
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
      store: 'export const { settings } = globalThis.__onlyTest;',
      i18n: 'export const { locale } = globalThis.__onlyTest; export const t = (s) => s;',
    };
    if (url.startsWith('only-test:')) return { format: 'module', shortCircuit: true, source: sources[url.split(':')[1]] };
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

test('a cloud session in 僅轉錄 exports only the transcript, whatever the subtitle mode says', () => {
  settings.sub.mode = 'both';
  assert.equal(stream.exportTxt(rows), 'hello\n\nworld');
  assert.deepEqual(cueTexts(stream.exportSrt(rows)), ['hello', 'world']);

  settings.sub.mode = 'target';
  assert.equal(stream.exportTxt(rows), 'hello\n\nworld');
  assert.deepEqual(cueTexts(stream.exportSrt(rows)), ['hello', 'world']);

  settings.transcribeOnly = false;
  settings.sub.mode = 'both';
  assert.equal(stream.exportTxt(rows), 'hello\n你好\n\nworld\n世界');
});

const read = (path) => readFileSync(path, 'utf8');

test('the switch is offered in cloud mode and means the same in both modes', () => {
  const app = read('src/desktop/App.vue');
  // The row is no longer gated on local mode: the label right above the switch opens bare.
  assert.match(app, /<label class="line">\s*<span>\{\{ t\("transcribeOnly"\) \}\}<\/span>/);
  assert.match(app, /const transcribingOnly = computed\(\(\) => settings\.transcribeOnly\)/);
  assert.match(app, /transcribeOnly: settings\.transcribeOnly,/);
  assert.match(read('src/desktop/Subtitle.vue'), /transcribingOnly\.value = settings\.transcribeOnly;/);
});

test('the stream files no translation while the switch is on', () => {
  const streamSource = read('src/core/stream.ts');
  assert.match(streamSource, /const filed = \(text: string\) => settings\.transcribeOnly \? "" : text;/);
  assert.match(streamSource, /target: filed\(t\.target\), at: t\.at/);
  assert.match(streamSource, /row\.target = filed\(t\.target\)/);
  assert.match(streamSource, /entry: \{ at: t\.at, source: t\.source, target: filed\(t\.target\)/);
  assert.match(streamSource, /current\.target = settings\.transcribeOnly \? "" : t\?\.target/);
  // The history header calls such a session transcription-only in either mode.
  assert.match(streamSource, /target: settings\.transcribeOnly \? "" : settings\.targetLang,/);
});

test('all three UI dictionaries word the cloud hint', () => {
  const i18n = read('src/core/i18n.ts');
  const hits = i18n.split('\n').filter((line) => line.trim().startsWith('transcribeOnlyRemoteHint:'));
  assert.equal(hits.length, 3);
});
