// A stray tag survives vue-tsc and vite (Vue warns, then parses the junk as a component
// name), so the desktop templates are compiled here and the warnings are asserted away.
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { compile } from 'vue';

function templateWarnings(path) {
  const source = readFileSync(new URL(path, import.meta.url), 'utf8');
  // Greedy: the outer template is the one that ends just before the first <style>.
  const template = source.match(/<template>([\s\S]*?)<\/template>\s*<style/)[1];
  const warnings = [];
  const warn = console.warn;
  console.warn = (message) => warnings.push(String(message));
  try {
    compile(template);
  } catch (error) {
    warnings.push(String(error));
  }
  console.warn = warn;
  return warnings;
}

for (const file of ['../src/desktop/App.vue', '../src/desktop/Subtitle.vue', '../src/shared/History.vue']) {
  test(`${file} parses without a stray or unclosed tag`, () => {
    assert.deepEqual(templateWarnings(file), []);
  });
}
