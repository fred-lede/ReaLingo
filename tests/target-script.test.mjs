// A 繁體中文 target must read Traditional whatever the model answered in. Small local models
// and the cloud APIs that only know one `zh` drift to Simplified mid-session, so the script is
// enforced where the line is shown and written, not hoped for in the prompt.
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import test from 'node:test';
import { reactive, ref } from 'vue';
import { convertTarget, ensureTraditional } from '../src/core/convert.ts';

const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8');

test("a 繁體 target is rewritten Traditional, a plain zh target keeps the model's own script", async () => {
  await ensureTraditional();
  assert.equal(convertTarget('软件更新完成', 'zh-TW'), '軟體更新完成');
  // Already Traditional: the one-way converter leaves those characters alone.
  assert.equal(convertTarget('軟體更新完成', 'zh-TW'), '軟體更新完成');
  // Latin, digits and kana in a translation pass through untouched.
  assert.equal(convertTarget('GPT-4 和软件', 'zh-TW'), 'GPT-4 和軟體');
  assert.equal(convertTarget('软件更新完成', 'zh'), '软件更新完成');
  assert.equal(convertTarget('软件更新完成', 'en'), '软件更新完成');
  assert.equal(convertTarget('', 'zh-TW'), '');
  assert.equal(convertTarget(undefined, 'zh-TW'), '');
});

const settings = reactive({
  protocol: 'qwen38', model: 'm', sourceLang: 'auto', targetLang: 'zh-TW',
  transcriptStyle: 'system', sub: { mode: 'both' },
});
globalThis.__targetTest = { settings, locale: ref('en') };
registerHooks({
  resolve(specifier, context, next) {
    const name = specifier === '@tauri-apps/api/core' ? 'core'
      : specifier === '@tauri-apps/api/event' ? 'event'
      : specifier === '@tauri-apps/plugin-dialog' ? 'dialog'
      : /(?:^|\/)store(?:\.ts)?$/.test(specifier) ? 'store'
      : /(?:^|\/)i18n(?:\.ts)?$/.test(specifier) ? 'i18n' : null;
    if (name) return { url: `target-test:${name}`, shortCircuit: true };
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
      store: 'export const { settings } = globalThis.__targetTest;',
      i18n: 'export const { locale } = globalThis.__targetTest; export const t = (s) => s;',
    };
    if (url.startsWith('target-test:')) return { format: 'module', shortCircuit: true, source: sources[url.split(':')[1]] };
    return next(url, context);
  },
});

const stream = await import('../src/core/stream.ts');

function cueTexts(file) {
  return file.split('\n\n').filter(Boolean).map((cue) => cue.replace(/\n$/, '').split('\n').slice(2).join('\n'));
}

test('exports write the translation in the script the target language asks for', async () => {
  await ensureTraditional();
  const rows = [
    { source: 'The update is done', target: '软件更新完成', start: 0, end: 2000, at: 0 },
    { source: 'It landed', target: '计划已经落地', start: 2000, end: 4000, at: 2000 },
  ];
  // The model answered Simplified; a 繁體 target still reads Traditional.
  settings.targetLang = 'zh-TW';
  assert.equal(stream.exportTxt(rows), 'The update is done\n軟體更新完成\n\nIt landed\n計劃已經落地');
  assert.deepEqual(cueTexts(stream.exportSrt(rows)), ['The update is done\n軟體更新完成', 'It landed\n計劃已經落地']);

  // A plain Chinese target is left exactly as the model wrote it.
  settings.targetLang = 'zh';
  assert.equal(stream.exportTxt(rows), 'The update is done\n软件更新完成\n\nIt landed\n计划已经落地');

  // Target-only mode carries the converted translation, and the transcript is still what ASR said.
  settings.sub.mode = 'source';
  assert.equal(stream.exportTxt(rows), 'The update is done\n\nIt landed');
});

test("a saved session exports in its own target language, not the picker's", async () => {
  await ensureTraditional();
  settings.targetLang = 'en';
  settings.sub.mode = 'both';
  const rows = [{ source: 'hello', target: '你好世界', start: 0, end: 1000, at: 0 }];
  assert.equal(stream.exportTxt(rows, 'zh-TW'), 'hello\n你好世界');
  const simplified = [{ source: 'hello', target: '软件更新完成', start: 0, end: 1000, at: 0 }];
  assert.equal(stream.exportTxt(simplified, 'zh-TW'), 'hello\n軟體更新完成');
});

test('every view converts the translation line, and the export is told which language it was', () => {
  const app = read('../src/desktop/App.vue');
  assert.match(app, /convertTarget\(l\.target, settings\.targetLang\)/);
  assert.match(app, /convertTarget\(current\.target, settings\.targetLang\)/);
  assert.match(app, /convertTarget\(current\.targetStash, settings\.targetLang\)/);
  assert.match(app, /targetLang: settings\.targetLang,/);
  assert.match(app, /settings\.transcriptStyle, settings\.targetLang, locale\.value\], pushOverlayStyle\)/);

  const overlay = read('../src/desktop/Subtitle.vue');
  assert.match(overlay, /const tgt = computed\(\(\) => convertTarget\(live\.value \? current\.target : last\.value\?\.target/);
  assert.match(overlay, /const tgtStash = computed\(\(\) => convertTarget\(live\.value \? current\.targetStash/);
  assert.match(overlay, /targetLang\.value = payload\.targetLang \?\? targetLang\.value/);

  const history = read('../src/shared/History.vue');
  assert.match(history, /convertTarget\(l\.target, sessionTarget\)/);
  assert.match(history, /saveAs\(kind, shown\.value, openId\.value, sessionTarget\.value\)/);

  const mobile = read('../src/mobile/StreamList.vue');
  assert.match(mobile, /convertTarget\(l\.target, settings\.targetLang\)/);
  assert.match(mobile, /convertTarget\(current\.target, settings\.targetLang\)/);
  assert.match(read('../src/mobile/LiveCard.vue'), /convertTarget\(current\.target, settings\.targetLang\)/);
  const mobileApp = read('../src/mobile/MobileApp.vue');
  assert.match(mobileApp, /target: convertTarget\(pair\?\.target \?\? "", settings\.targetLang\)/);
  assert.match(mobileApp, /\(\) => settings\.transcriptStyle, \(\) => settings\.targetLang\], syncOverlay/);
});

test('the stream threads the target language through both export builders and its lazy guard', () => {
  const streamSrc = read('../src/core/stream.ts');
  assert.match(streamSrc, /function shownLines\(line: Line, targetLang = settings\.targetLang\)/);
  assert.match(streamSrc, /const target = convertTarget\(line\.target, targetLang\)/);
  assert.match(streamSrc, /export function exportTxt\(rows: Line\[\] = sessionLines\.value, targetLang = settings\.targetLang\)/);
  assert.match(streamSrc, /shownLines\(l, targetLang\)\.join\("\\n"\)/);
  assert.match(streamSrc, /export async function saveAs\(kind: "txt" \| "srt", rows: Line\[\], stem = "translation", targetLang = settings\.targetLang\)/);
  assert.match(streamSrc, /if \(wantsTraditional\(settings\.transcriptStyle, locale\.value\) \|\| targetLang === "zh-TW"\) await ensureTraditional\(\)/);
  // The stored line still keeps what the model emitted: nothing converts when a row is built.
  assert.doesNotMatch(streamSrc, /target: convertTarget/);
});
