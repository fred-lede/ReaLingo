// Traditional Chinese ("zh-TW") as a translation target: which protocols offer it and how
// the name reads. Only Gemini's API takes a script split (target_code maps it to zh-Hant)
// and local mode is free text, so the pickers of Qwen/OpenAI/Volc — whose APIs accept a
// single "zh" — must keep it out.
import assert from 'node:assert/strict';
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

const { LANGUAGES, langName, languageCodes } = await import('../src/core/languages.ts');

test('zh-TW is one table row, speakable, and named in every UI language', () => {
  const rows = LANGUAGES.filter((l) => l.code === 'zh-TW');
  assert.equal(rows.length, 1);
  assert.equal(rows[0].tts, true);
  // The three UI locales: zh (Simplified) keeps the Traditional name as-is,
  // zh-TW s2t-normalizes it, en uses the English label.
  assert.equal(langName('zh-TW', 'zh'), '繁體中文');
  assert.equal(langName('zh-TW', 'zh-TW'), '繁體中文');
  assert.equal(langName('zh-TW', 'en'), 'Traditional Chinese');
});

test('only the protocols whose API accepts a script split offer zh-TW', () => {
  // The model argument only matters for the retired Qwen3 narrowing; '' is its default.
  for (const protocol of ['qwen35', 'qwen38', 'openai', 'volc']) {
    assert.ok(!languageCodes(protocol, '').includes('zh-TW'), `${protocol} must not offer zh-TW`);
  }
  assert.ok(languageCodes('gemini', '').includes('zh-TW'));
});
