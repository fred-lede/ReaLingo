<script setup lang="ts">
import { computed, ref, watch, toRaw, onMounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { activeKey, keyringOk, REGION_ID, setActiveKey, settings, type Protocol, type Region, type TranslationPace } from "../store";
import { isRunning } from "../stream";
import { PRESETS, PROTOCOLS } from "../languages";
import { t } from "../i18n";
import Picker from "./Picker.vue";
import Sheet from "./Sheet.vue";
import GlossaryEditor from "./GlossaryEditor.vue";

defineEmits<{ close: [] }>();

const endpoint = ref("");
const hostOs = ref("");
const hostArch = ref("");
onMounted(async () => {
  const platform = await invoke<{ os: string; arch: string }>("platform");
  hostOs.value = platform.os;
  hostArch.value = platform.arch;
  if (platform.os === "linux" || (platform.os === "macos" && platform.arch !== "aarch64") || (platform.os === "windows" && platform.arch !== "x86_64")) settings.mode = "remote";
});
const localAllowed = computed(() => hostOs.value === "windows" && hostArch.value === "x86_64" || (hostOs.value === "macos" && hostArch.value === "aarch64"));
const modeOptions = computed(() => [
  { value: "remote", label: t("modeRemote") },
  ...(localAllowed.value ? [{ value: "local", label: t("modeLocal") }] : []),
]);
const modePick = computed({ get: () => settings.mode, set: (value: string) => { if (value !== "local" || localAllowed.value) settings.mode = value as "remote" | "local"; } });
const paceOptions = computed(() => [
  { value: "fast", label: t("paceFast") },
  { value: "complete", label: t("paceComplete") },
]);
const pacePick = computed({ get: () => settings.translationPace, set: (value: string) => { settings.translationPace = value as TranslationPace; } });
async function chooseModel(kind: "whisper" | "translation") {
  const path = await openDialog({ multiple: false, directory: false, filters: [{ name: kind === "whisper" ? "Whisper model" : "GGUF model", extensions: [kind === "whisper" ? "bin" : "gguf"] }] });
  if (typeof path === "string") settings[kind === "whisper" ? "whisperModelPath" : "translationModelPath"] = path;
}
// An explicit source list, not watchEffect: the call awaits before touching anything else,
// so only what is read synchronously is tracked — and a field missing from that list makes
// the preview silently lag one change behind.
watch(
  () => [settings.protocol, settings.url, settings.region, settings.workspaceId, settings.model],
  async () => {
    endpoint.value = await invoke<string>("endpoint_url", { settings: toRaw(settings) });
  },
  { immediate: true }
);

const vendor = computed(() => PROTOCOLS[settings.protocol].vendor);
/** Region, workspace and the Model Studio links only mean something for Qwen. */
const isQwen = computed(() => vendor.value === "qwen");

const presets = computed(() =>
  PRESETS[settings.protocol].map((p) => ({ value: p.id, label: p.name, note: `${p.langs} ${t("langsUnit")}` }))
);
const CUSTOM = "\0custom";
const modelOptions = computed(() => [...presets.value, { value: CUSTOM, label: t("modelCustom") }]);
// Its own flag rather than "model is not a preset": typing a preset's name into the custom
// field must not snap the picker back and take the field away mid-edit.
const customModel = ref(!presets.value.some((p) => p.value === settings.model));
const modelPick = computed({
  get: () => (customModel.value ? CUSTOM : settings.model),
  set: (v: string) => {
    customModel.value = v === CUSTOM;
    if (v !== CUSTOM) settings.model = v;
  },
});

const apiKey = computed({ get: activeKey, set: setActiveKey });

/**
 * A model belongs to one protocol and a URL to one vendor (the two Qwens share an endpoint),
 * so each is put away on the way out and brought back on return: a round trip through
 * another protocol no longer loses a custom model or proxy address.
 */
function pickProtocol(p: Protocol) {
  const from = settings.protocol;
  if (p === from) return;
  settings.savedModels[from] = settings.model;
  settings.savedUrls[PROTOCOLS[from].vendor] = settings.url;
  settings.protocol = p;
  settings.model = settings.savedModels[p] ?? PRESETS[p][0].id;
  settings.url = settings.savedUrls[PROTOCOLS[p].vendor] ?? "";
  customModel.value = !PRESETS[p].some((x) => x.id === settings.model);
}

// A dropdown, not a segmented row: the list grows with every protocol added.
const protocolOptions = computed(() => [
  { value: "qwen35", label: t("protocolQwen35"), note: t("vendorQwen") },
  { value: "qwen38", label: t("protocolQwen38"), note: t("vendorQwen") },
  { value: "openai", label: t("protocolOpenai") },
  { value: "gemini", label: t("protocolGemini"), note: "Google" },
]);
const protocolPick = computed({
  get: () => settings.protocol as string,
  set: (v: string) => pickProtocol(v as Protocol),
});

/** Model Studio console pages are per region; the region id is part of the path. */
const consoleUrl = (page: string) => `https://bailian.console.aliyun.com/${REGION_ID[settings.region]}/${page}`;
const keyConsole = computed(
  () =>
    ({
      qwen: consoleUrl("model/settings/api-key"),
      openai: "https://platform.openai.com/api-keys",
      gemini: "https://aistudio.google.com/apikey",
    })[vendor.value]
);
const KEY_LABEL = { qwen: "apiKeyGet", openai: "apiKeyGetOpenai", gemini: "apiKeyGetGemini" } as const;
const keyConsoleLabel = computed(() => t(KEY_LABEL[vendor.value]));
// The opener plugin, not an <a href>: a plain link navigates the app's own webview away.
const openUrl = (url: string) => invoke("plugin:opener|open_url", { url });

// A dropdown even with two entries: more regions will come once they carry the models.
const regionOptions = computed(() => [
  { value: "beijing", label: t("regionBJ") },
  { value: "singapore", label: t("regionSG") },
]);
const regionPick = computed({
  get: () => settings.region as string,
  set: (v: string) => (settings.region = v as Region),
});

/** Starts open when something in it is already set, so a configured override is never hidden. */
const advancedOpen = !!settings.url;
</script>

<template>
  <Sheet :title="t('settings')" @close="$emit('close')">
    <section class="group">
      <h3>{{ t("groupConn") }}</h3>

      <div class="field">
        <span class="label">{{ t("executionMode") }}</span>
        <Picker v-model="modePick" :options="modeOptions" :disabled="isRunning()" />
        <small v-if="settings.mode === 'local'">{{ t(settings.transcribeOnly ? "transcribeOnlyHint" : "localHint") }}</small>
        <small v-else-if="!localAllowed">{{ t("localUnsupported") }}</small>
      </div>

      <template v-if="settings.mode === 'local'">
        <div class="field">
          <span class="label">{{ t("whisperModel") }}</span>
          <code class="endpoint">{{ settings.whisperModelPath || t("modelNotSelected") }}</code>
          <button class="link" :disabled="isRunning()" @click="chooseModel('whisper')">{{ t("chooseModel") }}</button>
        </div>
        <div v-if="!settings.transcribeOnly" class="field">
          <span class="label">{{ t("translationModel") }}</span>
          <code class="endpoint">{{ settings.translationModelPath || t("modelNotSelected") }}</code>
          <button class="link" :disabled="isRunning()" @click="chooseModel('translation')">{{ t("chooseModel") }}</button>
        </div>
        <div v-if="!settings.transcribeOnly" class="field">
          <span class="label">{{ t("translationPace") }}</span>
          <Picker v-model="pacePick" :options="paceOptions" :disabled="isRunning()" />
          <small>{{ t("paceHint") }}</small>
        </div>
      </template>

      <template v-else>
      <div v-if="settings.mode === 'remote'" class="field">
        <span class="label">{{ t("protocol") }}</span>
        <Picker v-model="protocolPick" :options="protocolOptions" :disabled="isRunning()" />
        <small v-if="isQwen">{{ t("protocolHint") }}</small>
      </div>

      <label class="field">
        <span class="label">{{ t("apiKey") }}</span>
        <input v-model="apiKey" type="password" placeholder="sk-..." spellcheck="false" />
        <small>
          {{ keyringOk ? t("apiKeyHint") : t("apiKeyPlain") }}
          <button class="link" @click.prevent="openUrl(keyConsole)">
            {{ keyConsoleLabel }} ↗
          </button>
        </small>
      </label>
      </template>

      <div v-if="settings.mode === 'remote'" class="field">
        <span class="label">{{ t("model") }}</span>
        <Picker v-model="modelPick" :options="modelOptions" :disabled="isRunning()" />
        <input
          v-if="customModel"
          v-model.trim="settings.model"
          type="text"
          :placeholder="t('modelCustomPh')"
          :disabled="isRunning()"
          spellcheck="false"
        />
        <!-- Regions carry different model lists; worth checking before blaming the app. -->
        <small v-if="isQwen">
          <button class="link" @click.prevent="openUrl(consoleUrl('model/market'))">{{ t("modelsLink") }} ↗</button>
        </small>
      </div>

      <div v-if="settings.mode === 'remote' && isQwen" class="field">
        <span class="label">{{ t("region") }}</span>
        <Picker v-model="regionPick" :options="regionOptions" :disabled="isRunning()" />
      </div>

      <label v-if="settings.mode === 'remote' && isQwen" class="field">
        <span class="label">{{ t("workspace") }}</span>
        <!-- Beijing workspace IDs start with "llm-", every other region's with "ws-". -->
        <input
          v-model.trim="settings.workspaceId"
          type="text"
          :placeholder="settings.region === 'beijing' ? 'llm-xxxxxxxx' : 'ws-xxxxxxxx'"
          :disabled="isRunning()"
          spellcheck="false"
        />
        <small>
          {{ t("workspaceHint") }}
          <button class="link" @click.prevent="openUrl(consoleUrl('settings/workspace'))">{{ t("workspaceManage") }} ↗</button>
        </small>
      </label>
    </section>

    <section class="group">
      <h3>{{ t("groupTranslate") }} <span class="sub">{{ t("glossary") }}</span></h3>
      <GlossaryEditor :disabled="isRunning()" :mode="settings.mode" :protocol="settings.protocol" />
    </section>

    <!-- Native <details>: the open/closed state and keyboard handling come for free. -->
    <details v-if="settings.mode === 'remote'" class="group" :open="advancedOpen">
      <summary>{{ t("groupAdvanced") }}</summary>
      <div class="group-body">
        <label class="field">
          <span class="label">{{ t("url") }}</span>
          <input v-model.trim="settings.url" type="text" placeholder="wss://..." :disabled="isRunning()" spellcheck="false" />
          <small>{{ PROTOCOLS[settings.protocol].urlAsIs ? t("urlHintAsIs") : t("urlHint") }}</small>
        </label>

        <div class="field">
          <span class="label">{{ t("endpoint") }}</span>
          <code class="endpoint">{{ endpoint }}</code>
        </div>
      </div>
    </details>
  </Sheet>
</template>

<style scoped>
.group { display: flex; flex-direction: column; gap: 14px; }
.group + .group { padding-top: 16px; border-top: 1px solid var(--hairline); }
/* A step above the field labels, or the group titles read as just another field. */
.group > h3, .group > summary { margin: 0; font-size: 13px; font-weight: 700; color: var(--ink); }
summary { cursor: pointer; list-style-position: inside; }
/* <details> does not hand flex layout to its content, so the body spaces itself. */
.group-body { display: flex; flex-direction: column; gap: 14px; }

.group > h3 .sub { margin-left: 6px; font-weight: 500; color: var(--ink-3); }

/* Label left, control right, everything else (custom-model input, hint) under the control —
   one row per setting instead of a label line plus a full-width control line. */
.field {
  display: grid;
  grid-template-columns: 96px minmax(0, 1fr);
  align-items: center;
  column-gap: 12px;
  row-gap: 6px;
}
.field > :not(.label) { grid-column: 2; }
.field small { font-size: 11.5px; line-height: 1.5; color: var(--ink-3); }

.endpoint {
  font-family: "SF Mono", "Cascadia Code", ui-monospace, monospace;
  font-size: 11.5px;
  line-height: 1.5;
  padding: 9px 11px;
  border-radius: var(--r-sm);
  background: var(--shade);
  color: var(--ink-2);
  word-break: break-all;
  user-select: text;
}

.link {
  padding: 0;
  font: inherit;
  color: var(--accent);
  text-decoration: underline;
  text-underline-offset: 2px;
}
.link:hover { filter: brightness(1.15); }

</style>
