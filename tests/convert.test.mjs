// Transcript display conversion: ASR emits Simplified Chinese, so "Traditional" is a
// one-way cn→twp conversion; "Simplified" and "System" (non-zh-TW UI) show the source
// as-is. Only Han characters move; Latin/digits/kana pass through untouched.
import assert from 'node:assert/strict';
import test from 'node:test';
import { convertTranscript, ensureTraditional, wantsTraditional } from '../src/core/convert.ts';

test('the dictionaries are fetched only when a Traditional display is asked for', async () => {
  // A Simplified UI never touches them, and the first Traditional request cannot block.
  assert.equal(convertTranscript('出租车', 'zh', 'zh-TW'), '出租车');
  assert.equal(convertTranscript('出租车', 'system', 'zh'), '出租车');
  assert.equal(convertTranscript('出租车', 'zh-TW'), '出租车');
  await ensureTraditional();
  assert.equal(convertTranscript('出租车', 'zh-TW'), '計程車');
});

test('conversion moves Han characters only', async () => {
  await ensureTraditional();
  assert.equal(convertTranscript('软件更新完成', 'zh-TW'), '軟體更新完成');
  assert.equal(convertTranscript('Hello 123 中文', 'zh-TW'), 'Hello 123 中文');
  // Already Traditional: the character set is unchanged, only regional variants may move.
  assert.equal(convertTranscript('計程車', 'zh-TW'), '計程車');
});

test('empty and undefined text never throws and stays empty', async () => {
  await ensureTraditional();
  assert.equal(convertTranscript('', 'zh-TW'), '');
  assert.equal(convertTranscript(undefined, 'zh-TW'), '');
});

test('follow system means Traditional only for a Traditional interface', () => {
  assert.equal(wantsTraditional('system', 'zh-TW'), true);
  assert.equal(wantsTraditional('system', 'zh'), false);
  assert.equal(wantsTraditional('system', 'en'), false);
  assert.equal(wantsTraditional('zh', 'zh-TW'), false);
  assert.equal(wantsTraditional('zh-TW', 'en'), true);
});
