// The relay ASR switch: a router that loads its own ASR model can be aimed at another
// machine's worker, and the app offers that switch with the relay's own target list.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const { relayAsrBase, relayAsrConfig, relayAsrSelect } = await import('../src/core/relay-asr.ts');

const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8');

test('the admin route hangs off the endpoint, not off its realtime path', () => {
  assert.equal(relayAsrBase('ws://192.168.1.210:8001/v1'), 'http://192.168.1.210:8001');
  assert.equal(relayAsrBase('wss://relay.example:8443/v1/realtime?model=x'), 'https://relay.example:8443');
  assert.equal(relayAsrBase(' https://box.local/ '), 'https://box.local');
  // A built-in endpoint has no such route, and a blank URL has nothing to hang off.
  assert.equal(relayAsrBase(''), '');
  assert.equal(relayAsrBase('gpt-realtime-translate'), '');
});

test("the relay's own target list becomes the picker options", async () => {
  const sent = [];
  global.fetch = async (url, options) => {
    sent.push({ url, options });
    return {
      ok: true,
      json: () => ({
        state: 'active',
        active: { target_id: 'node:m4', provider: 'remote_asr', model: 'Qwen/Qwen3-ASR-0.6B' },
        targets: {
          local: [{ id: 'local:cuda:0', name: 'Apple M4 GPU' }, { id: 'local:cpu' }],
          nodes: [
            { id: 'node:m4', reachable: true, engine: 'qwen_asr', model: 'Qwen/Qwen3-ASR-0.6B' },
            { id: 'node:m9', reachable: false, engine: 'mlx_whisper' },
          ],
        },
      }),
    };
  };
  const found = await relayAsrConfig('ws://192.168.1.210:8001/v1', 'secret');
  assert.equal(sent.length, 1);
  assert.equal(sent[0].url, 'http://192.168.1.210:8001/api/asr/config');
  assert.equal(sent[0].options.headers.Authorization, 'Bearer secret');
  assert.deepEqual(
    found.targets,
    [
      { id: 'local:cuda:0', label: 'Apple M4 GPU' },
      { id: 'local:cpu', label: 'local:cpu' },
      { id: 'node:m4', label: 'm4 · qwen_asr Qwen/Qwen3-ASR-0.6B', disabled: false },
      // A node whose worker is down stays in the list, greyed out.
      { id: 'node:m9', label: 'm9 · mlx_whisper', disabled: true },
    ]
  );
  assert.equal(found.active, 'node:m4');
  assert.equal(found.model, 'Qwen/Qwen3-ASR-0.6B');
  assert.equal(found.state, 'active');
  assert.equal(found.ok, true);
});

test('a relay that cannot read the cluster says so instead of dropping the nodes', async () => {
  global.fetch = async () => ({
    ok: true,
    json: () => ({
      state: 'active',
      active: { target_id: 'local:cpu', model: 'faster_whisper large-v3' },
      targets: { local: [{ id: 'local:cpu', name: 'CPU' }], nodes: [], nodes_unreachable: true },
    }),
  });
  const found = await relayAsrConfig('ws://192.168.1.210:8001/v1', 'secret');
  assert.equal(found.nodesUnknown, true);
  assert.deepEqual(found.targets, [{ id: 'local:cpu', label: 'CPU' }]);
});

test('a relay that answers nothing leaves the row hidden, not broken', async () => {
  global.fetch = async () => ({ ok: false });
  const found = await relayAsrConfig('wss://api.example/v1', 'secret');
  assert.deepEqual(found, { ok: false, active: '', model: '', state: '', targets: [], nodesUnknown: false });
});

test('picking asks the relay to load that target', async () => {
  const sent = [];
  global.fetch = async (url, options) => {
    sent.push({ url, options });
    return { ok: true };
  };
  const ok = await relayAsrSelect('ws://192.168.1.210:8001/v1', 'secret', 'local:cuda:1');
  assert.equal(ok, true);
  assert.equal(sent[0].url, 'http://192.168.1.210:8001/api/asr/config');
  assert.equal(sent[0].options.method, 'POST');
  assert.equal(sent[0].options.body, JSON.stringify({ target_id: 'local:cuda:1' }));
  // Nothing to ask for: no endpoint, or no target chosen.
  assert.equal(await relayAsrSelect('', 'secret', 'local:cpu'), false);
  assert.equal(await relayAsrSelect('ws://box:1/v1', 'secret', ''), false);
});

test('the settings row offers the switch only when the relay answers', () => {
  const settings = read('../src/desktop/components/Settings.vue');
  assert.match(settings, /import \{ relayAsrConfig, relayAsrSelect, type RelayAsrTarget \} from "\.\.\/\.\.\/core\/relay-asr"/);
  assert.match(settings, /v-if="settings\.mode === 'remote' && relayPresent"/);
  assert.match(settings, /<Picker v-model="relayPick" :options="relayOptions"/);
  // The list is read when the endpoint changes, and the pick is applied when picked.
  assert.match(settings, /watch\(\(\) => settings\.url, async \(\) => await refreshRelayAsr\(\), \{ immediate: true \}\)/);
  assert.match(settings, /await relayAsrSelect\(settings\.url, activeKey\(\), v\)/);
  // A node without a reachable worker is shown, not offered.
  assert.match(settings, /disabled: o\.disabled,/);
  assert.match(settings, /note: o\.disabled \? t\("relayAsrUnreachable"\) : undefined,/);
});

test('a relay that picks its own model hides the client-side ASR model row', () => {
  const settings = read('../src/desktop/components/Settings.vue');
  assert.match(settings, /v-if="settings\.mode === 'remote' && asrDefault && !relayPresent && !relayLoading"/);
  // A picker option that cannot be chosen does not take a click.
  const picker = read('../src/desktop/components/Picker.vue');
  assert.match(picker, /if \(option\?\.disabled\) return;/);
  assert.match(picker, /:disabled="o\.disabled"/);
});

test('all three UI locales word the new row', () => {
  const i18n = read('../src/core/i18n.ts');
  for (const key of ['relayAsr', 'relayAsrHint', 'relayAsrUnreachable', 'relayAsrNodesUnknown']) {
    assert.equal(i18n.split(new RegExp(`\\b${key}:`)).length - 1, 3);
  }
});
