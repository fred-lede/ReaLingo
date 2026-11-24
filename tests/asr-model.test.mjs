// The cloud ASR-model picker: which protocols have a transcription field to name, and that
// the row the settings panel shows stays in step with the Rust payload builders.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import test from 'node:test';

// languages.ts value-imports ./s2t without the .ts extension; resolve that for Node.
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === './s2t' || specifier === './store') {
      const url = new URL(specifier + '.ts', context.parentURL);
      return { url: url.href, shortCircuit: true };
    }
    return next(specifier, context);
  },
});

const { ASR_MODELS } = await import('../src/core/languages.ts');

test('only the protocols with a transcription field name an ASR model', () => {
  assert.equal(ASR_MODELS.qwen35, 'qwen3-asr-flash-realtime');
  assert.equal(ASR_MODELS.openai, 'gpt-realtime-whisper');
  // 3.8 supplies its own, Gemini and Volc transcribe inside the live model.
  assert.equal(ASR_MODELS.qwen38, '');
  assert.equal(ASR_MODELS.gemini, '');
  assert.equal(ASR_MODELS.volc, '');
});

test('the Rust payload uses the same defaults', () => {
  const rust = readFileSync(new URL('../src-tauri/src/config.rs', import.meta.url), 'utf8');
  assert.match(rust, /pub const ASR_MODEL: &str = "qwen3-asr-flash-realtime"/);
  assert.match(rust, /pub const OPENAI_ASR_MODEL: &str = "gpt-realtime-whisper"/);
  // Both call sites go through the accessor, so a chosen model reaches the wire.
  assert.match(rust, /"model": self\.asr_model\(\)/);
  assert.match(rust, /fn asr_model\(&self\) -> String/);
});

test('the settings row appears only where a model can be named', () => {
  const settings = readFileSync(new URL('../src/desktop/components/Settings.vue', import.meta.url), 'utf8');
  // A relay that loads its own ASR picks the model itself, so the row steps aside for it.
  assert.match(settings, /v-if="settings\.mode === 'remote' && asrDefault && !relayPresent && !relayLoading"/);
  assert.match(settings, /const asrDefault = computed\(\(\) => ASR_MODELS\[settings\.protocol\]\)/);
  // Switching protocol drops a model that belonged to the previous payload.
  assert.match(settings, /settings\.asrModel = "";\n\s*customAsr\.value = false;/);
});

test('the chosen model is persisted and its 自定 state rebuilt on load', () => {
  const store = readFileSync(new URL('../src/core/store.ts', import.meta.url), 'utf8');
  // Part of the Settings object, so the whole-settings save carries it.
  assert.match(store, /^\s{2}asrModel: string;$/m);
  const settings = readFileSync(new URL('../src/desktop/components/Settings.vue', import.meta.url), 'utf8');
  // The flag is in memory only, so it is derived from the saved id rather than started false.
  assert.match(settings, /const customAsr = ref\(!asrOptions\.value\.some\(\(o\) => o\.value === \(settings\.asrModel \|\| asrDefault\.value\)\)\)/);
  assert.match(settings, /v-if="customAsr"/);
});

test('all three UI locales word the new row', () => {
  const i18n = readFileSync(new URL('../src/core/i18n.ts', import.meta.url), 'utf8');
  for (const key of ['asrModel', 'asrHint', 'asrCustomPh']) {
    assert.equal(i18n.split(new RegExp(`\\b${key}:`)).length - 1, 3);
  }
});
