// The phone has no local mode, so the transcript script is a display choice taken from the
// language sheet: the same three options the desktop sidebar offers.
import assert from 'node:assert/strict';
import test from 'node:test';
import { computed, reactive } from 'vue';
import { readFileSync } from 'node:fs';
import { vueScript } from './helpers/vue-script.mjs';

const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8');

test('the language sheet offers the transcript script beside the UI language', () => {
  const settings = reactive({ uiLang: "system", transcriptStyle: "system" });
  const sheet = vueScript(new URL('../src/mobile/UiLangSheet.vue', import.meta.url), {
    computed, settings, t: (key) => key, tf: (key, vars) => `${key}:${vars.l}`, systemLocaleName: () => "繁體中文",
    defineEmits: () => {},
  }, ['options', 'scriptOptions', 'pick', 'pickScript']);

  assert.deepEqual(sheet.options.value.map((o) => o.value), ["system", "zh-TW", "zh", "en"]);
  assert.deepEqual(sheet.scriptOptions.value.map((o) => o.value), ["system", "zh", "zh-TW"]);
  // "Follow system" names the script it currently means, so the row is not a mystery.
  assert.equal(sheet.scriptOptions.value[0].description, "uiDetected:繁體中文");

  sheet.pickScript("zh-TW");
  assert.equal(settings.transcriptStyle, "zh-TW");
  sheet.pick("zh");
  assert.equal(settings.uiLang, "zh");
});

test('the phone overlay is sent the transcript in the chosen script', () => {
  const app = read('../src/mobile/MobileApp.vue');
  assert.match(app, /source: convertTranscript\(pair\?\.source \?\? "", settings\.transcriptStyle, locale\.value\)/);
  // The translation is never touched by the transcript script.
  assert.doesNotMatch(app, /target: convertTranscript/);
});
