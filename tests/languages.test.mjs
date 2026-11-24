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

test('every protocol can still be aimed at 繁體, the split by coercion plus a display rewrite', async () => {
  const { targetCodes } = await import('../src/core/languages.ts');
  for (const protocol of ['qwen35', 'qwen38', 'openai', 'volc', 'gemini']) {
    const targets = targetCodes(protocol, '');
    assert.ok(targets.includes('zh-TW'), `${protocol} must offer 繁體中文 as a target`);
    // A source is what the ASR hears: one Chinese, no script split to offer.
    assert.ok(!languageCodes(protocol, '').includes('zh-TW') || protocol === 'gemini');
    // The extra code sits next to the Chinese it is a split of, so the list stays grouped.
    assert.equal(targets[targets.indexOf('zh') + 1], 'zh-TW');
  }
  // Volc speaks only its own list, and 繁體 is spoken in the Chinese voice it coerces to.
  const { canSpeak } = await import('../src/core/languages.ts');
  assert.ok(canSpeak('volc', 'zh-TW'));
  assert.ok(!canSpeak('volc', 'cs'));
});
