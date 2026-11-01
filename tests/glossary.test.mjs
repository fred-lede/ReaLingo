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

test('exact duplicate and multiline entries are rejected before persistence', () => {
  const one = upsertGlossary([], lora, undefined, codes);
  assert.throws(() => upsertGlossary(one, lora, undefined, codes), /duplicate/i);
  assert.equal(upsertGlossary(one, { ...lora, source: 'LoRa', aliases: [] }, undefined, codes).length, 2);
  assert.throws(() => upsertGlossary([], { ...lora, target: 'bad\nline' }, undefined, codes), /line/i);
  assert.throws(() => upsertGlossary([], { ...lora, source: ' ' }, undefined, codes), /blank/i);
  assert.throws(() => upsertGlossary([], { ...lora, targetLang: 'xx' }, undefined, codes), /language/i);
});

test('migration keeps distinct canonical spellings', () => {
  const entries = migrateGlossary({ glossary: [lora, { ...lora, source: 'LoRa', aliases: [], target: '无线技术' }] });
  assert.equal(entries.length, 2);
});

test('case-only aliases are rejected because they can name a different term', () => {
  assert.throws(
    () => upsertGlossary([], { ...lora, aliases: ['LoRa'] }, undefined, codes),
    /ambiguousAlias/,
  );
  assert.deepEqual(upsertGlossary([], { ...lora, aliases: ['Low-Ri'] }, undefined, codes)[0].aliases, ['Low-Ri']);
  assert.deepEqual(
    migrateGlossary({ glossary: [{ ...lora, aliases: ['LoRa', 'Low-Ri'] }] })[0].aliases,
    ['Low-Ri'],
  );
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
