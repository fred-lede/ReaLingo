// Where the transcript script is applied: at display time and at export time. The stream keeps
// the text the ASR engine emitted, so history rows and the overlay's data stay as spoken.
// These exercise the overlay's real setup code and the main window's wiring.
import assert from 'node:assert/strict';
import test from 'node:test';
import { computed, nextTick, ref, watch } from 'vue';
import { readFileSync } from 'node:fs';
import { vueScript } from './helpers/vue-script.mjs';
import { convertTranscript, ensureTraditional } from '../src/core/convert.ts';

const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8');

test('the overlay renders the transcript in the script the main window sends it', async () => {
  const handlers = new Map();
  const state = vueScript(new URL('../src/desktop/Subtitle.vue', import.meta.url), {
    computed, ref, watch, nextTick,
    onMounted: () => {},
    listen: (name, cb) => { handlers.set(name, cb); return () => {}; },
    emitTo: () => {},
    settings: { sub: { mode: 'both', srcSize: 25, tgtSize: 40, opacity: 0.5, align: 'center', grow: false, locked: true }, transcriptStyle: 'system' },
    initSettings: async () => {},
    current: { source: '软件更新完成', target: '', sourceStash: '', targetStash: '' },
    lines: ref([{ id: 1, source: '出租车', target: '' }]),
    locale: ref('zh'),
    convertTranscript,
    t: (key) => key,
  }, ['src', 'showSource', 'transcribingOnly', 'transcriptStyle']);

  // Simplified source, Simplified UI: nothing to do.
  assert.equal(state.src.value, '软件更新完成');

  // The main window owns the setting; it pushes the resolved pick down with the style.
  handlers.get('sub://style')({
    payload: { mode: 'source', srcSize: 25, tgtSize: 40, opacity: 0.5, align: 'center', grow: false, locked: true, transcribeOnly: true, transcriptStyle: 'zh-TW' },
  });
  assert.equal(state.transcriptStyle.value, 'zh-TW');
  // The dictionaries are a megabyte, so the render that asks for them still shows the
  // source; the ready flip invalidates the computed and the line converts.
  assert.equal(state.src.value, '软件更新完成');
  await ensureTraditional();
  assert.equal(state.src.value, '軟體更新完成');
  assert.equal(state.showSource.value, true);

  // A Traditional pick is a display choice only: the stored line is still what ASR said.
  handlers.get('sub://style')({ payload: { transcriptStyle: 'zh' } });
  assert.equal(state.src.value, '软件更新完成');
});

test('the main window converts the transcript at render time, stored lines untouched', () => {
  const app = read('../src/desktop/App.vue');
  const stream = read('../src/core/stream.ts');
  assert.match(app, /convertTranscript\(l\.source, settings\.transcriptStyle, locale\)/);
  assert.match(app, /convertTranscript\(current\.source, settings\.transcriptStyle, locale\)/);
  assert.match(app, /convertTranscript\(current\.sourceStash, settings\.transcriptStyle, locale\)/);
  // The transcript script is not a translation target: only the source rows move.
  assert.doesNotMatch(app, /convertTranscript\((l|current)\.target/);
  // The stored line stays what the engine emitted; only the export builders convert.
  assert.doesNotMatch(stream, /source: convertTranscript/);
  assert.match(stream, /asShown\(l\.source\)/);
  assert.doesNotMatch(stream, /asShown\(l\.target/);
  assert.match(stream, /if \(wantsTraditional\(settings\.transcriptStyle, locale\.value\)\) await ensureTraditional\(\)/);
});

test('the overlay is told the script, not left to guess from its own stale settings', () => {
  const app = read('../src/desktop/App.vue');
  assert.match(app, /transcriptStyle: settings\.transcriptStyle/);
  assert.match(app, /watch\(\(\) => \[settings\.mode, settings\.transcribeOnly, settings\.transcriptStyle, locale\.value\], pushOverlayStyle\)/);
});

test('every UI dictionary names the control and its three choices', () => {
  const i18n = read('../src/core/i18n.ts');
  for (const key of ['transcriptStyle', 'transcriptStyleHint', 'styleSystem', 'styleSimplified', 'styleTraditional']) {
    assert.equal((i18n.match(new RegExp(`\\n  ${key}:`, 'g')) ?? []).length, 3, `${key} missing from a dictionary`);
  }
});
