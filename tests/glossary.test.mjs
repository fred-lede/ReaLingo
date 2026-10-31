import test from 'node:test';
import assert from 'node:assert/strict';
import { deleteGlossary, migrateGlossary, upsertGlossary } from '../src/glossary.ts';

const codes = new Set(['zh', 'en']);
const lora = { source: 'LoRA', aliases: ['Loari'], target: '低秩适配', targetLang: 'zh' };

test('old Qwen pairs migrate once and malformed saved data cannot crash startup', () => {
  assert.deepEqual(migrateGlossary({ hotwords: { LoRA: '低秩适配' } }), [
    { source: 'LoRA', aliases: [], target: '低秩适配', targetLang: '*' },
  ]);
  assert.deepEqual(migrateGlossary({ glossary: [lora], hotwords: {} }), [lora]);
  const both = { glossary: [{ ...lora, aliases: [], targetLang: '*' }], hotwords: { LoRA: '低秩适配' } };
  assert.equal(migrateGlossary(both).length, 1);
  assert.deepEqual(migrateGlossary({ glossary: 'bad', hotwords: null }), []);
});

test('add and delete return new arrays without changing unrelated entries', () => {
  const one = upsertGlossary([], lora, undefined, codes);
  assert.deepEqual(one, [lora]);
  const two = upsertGlossary(one, { source: 'GGUF', aliases: [], target: 'GGUF', targetLang: 'zh' }, undefined, codes);
  assert.deepEqual(deleteGlossary(two, 0), [two[1]]);
  assert.deepEqual(one, [lora]);
});

test('duplicate and multiline entries are rejected before persistence', () => {
  const one = upsertGlossary([], lora, undefined, codes);
  assert.throws(() => upsertGlossary(one, { ...lora, source: 'lora' }, undefined, codes), /duplicate/i);
  assert.throws(() => upsertGlossary([], { ...lora, target: 'bad\nline' }, undefined, codes), /line/i);
  assert.throws(() => upsertGlossary([], { ...lora, source: ' ' }, undefined, codes), /blank/i);
  assert.throws(() => upsertGlossary([], { ...lora, targetLang: 'xx' }, undefined, codes), /language/i);
});

test('a migrated wildcard can be edited but a new wildcard cannot be added', () => {
  const old = migrateGlossary({ hotwords: { LoRA: 'old' } });
  assert.deepEqual(upsertGlossary(old, { ...old[0], target: 'preferred' }, 0, codes), [
    { source: 'LoRA', aliases: [], target: 'preferred', targetLang: '*' },
  ]);
  assert.throws(() => upsertGlossary([], old[0], undefined, codes), /language/i);
});

test('editing and deleting one row leave neighboring terms alone', () => {
  const entries = [lora, { source: 'GGUF', aliases: [], target: 'GGUF', targetLang: 'en' }];
  const changed = upsertGlossary(entries, { ...entries[1], target: 'model file' }, 1, codes);
  assert.deepEqual(changed[0], lora);
  assert.equal(changed[1].target, 'model file');
  assert.deepEqual(deleteGlossary(changed, 1), [lora]);
});
