<script setup lang="ts">
import { computed, ref } from "vue";
import { deleteGlossary, upsertGlossary, type GlossaryEntry } from "../glossary";
import { locale, t, type Key } from "../i18n";
import { LANGUAGES } from "../languages";
import { settings, type ExecutionMode, type Protocol } from "../store";
import Picker from "./Picker.vue";

const props = defineProps<{ disabled: boolean; mode: ExecutionMode; protocol: Protocol }>();
const languageCodes = new Set(LANGUAGES.map((language) => language.code));
const editingIndex = ref<number | null>(null);
const formOpen = ref(false);
const aliasText = ref("");
const error = ref("");
const draft = ref<GlossaryEntry>({ source: "", aliases: [], target: "", targetLang: settings.targetLang });

const languageOptions = computed(() => [
  ...(editingIndex.value !== null && draft.value.targetLang === "*"
    ? [{ value: "*", label: t("glossaryAllTargets") }]
    : []),
  ...LANGUAGES.map((language) => ({
    value: language.code,
    label: locale.value === "zh" ? language.zh : language.en,
    note: language.code,
    keywords: `${language.zh} ${language.en}`,
  })),
]);
const activeCount = computed(() => settings.glossary.filter((entry) => entry.targetLang === "*" || entry.targetLang === settings.targetLang).length);
const availability = computed(() => {
  if (props.mode === "local") return t("glossaryLocalHint");
  return props.protocol === "qwen35" || props.protocol === "qwen38"
    ? t("glossaryQwenHint")
    : t("glossaryUnavailable");
});
const validationKeys: Record<string, Key> = {
  blank: "glossaryBlank",
  line: "glossaryLine",
  language: "glossaryLanguageError",
  duplicate: "glossaryDuplicate",
};

function reset() {
  editingIndex.value = null;
  formOpen.value = false;
  aliasText.value = "";
  error.value = "";
}

function add() {
  reset();
  draft.value = { source: "", aliases: [], target: "", targetLang: settings.targetLang };
  formOpen.value = true;
}

function edit(index: number) {
  const entry = settings.glossary[index];
  editingIndex.value = index;
  draft.value = { ...entry, aliases: [...entry.aliases] };
  aliasText.value = entry.aliases.join(", ");
  error.value = "";
  formOpen.value = true;
}

function save() {
  try {
    const entry = { ...draft.value, aliases: aliasText.value.split(",").map((value) => value.trim()).filter(Boolean) };
    settings.glossary = upsertGlossary(settings.glossary, entry, editingIndex.value ?? undefined, languageCodes);
    reset();
  } catch (cause) {
    error.value = t(validationKeys[(cause as Error).message] ?? "glossaryInvalid");
  }
}

function remove(index: number) {
  settings.glossary = deleteGlossary(settings.glossary, index);
  if (editingIndex.value === index) reset();
  else if (editingIndex.value !== null && editingIndex.value > index) editingIndex.value--;
}

function targetLabel(code: string): string {
  if (code === "*") return t("glossaryAllTargets");
  const language = LANGUAGES.find((item) => item.code === code);
  return language ? (locale.value === "zh" ? language.zh : language.en) : code;
}
</script>

<template>
  <div class="glossary">
    <p class="hint">{{ availability }} {{ t("glossaryNextSession") }}</p>
    <p v-if="mode === 'remote' && (protocol === 'qwen35' || protocol === 'qwen38')" class="hint">{{ t("glossaryQwenLimit") }}</p>
    <div class="toolbar">
      <span>{{ activeCount }} {{ t("glossaryActive") }} · {{ settings.glossary.length }} {{ t("glossaryStored") }}</span>
      <button class="link" :disabled="disabled" @click="add">{{ t("glossaryAdd") }}</button>
    </div>
    <div v-if="settings.glossary.length" class="entries">
      <div v-for="(entry, index) in settings.glossary" :key="index" class="entry">
        <div class="term">
          <strong>{{ entry.source }}</strong> <span>→ {{ entry.target }}</span>
          <small>{{ targetLabel(entry.targetLang) }}<template v-if="entry.aliases.length"> · {{ t("glossaryAliases") }}: {{ entry.aliases.join(", ") }}</template></small>
        </div>
        <button class="link" :disabled="disabled" @click="edit(index)">{{ t("glossaryEdit") }}</button>
        <button class="link danger" :disabled="disabled" @click="remove(index)">{{ t("glossaryDelete") }}</button>
      </div>
    </div>
    <p v-else class="hint">{{ t("glossaryEmpty") }}</p>

    <div v-if="formOpen" class="editor">
      <label><span>{{ t("glossarySource") }}</span><input v-model="draft.source" :disabled="disabled" type="text" spellcheck="false" /></label>
      <label><span>{{ t("glossaryAliases") }}</span><input v-model="aliasText" :disabled="disabled" type="text" :placeholder="t('glossaryAliasesHint')" spellcheck="false" /></label>
      <label><span>{{ t("glossaryTarget") }}</span><input v-model="draft.target" :disabled="disabled" type="text" spellcheck="false" /></label>
      <div class="language"><span>{{ t("glossaryLanguage") }}</span><Picker v-model="draft.targetLang" :options="languageOptions" :disabled="disabled" /></div>
      <p v-if="error" class="error" role="alert">{{ error }}</p>
      <div class="actions">
        <button class="link" :disabled="disabled" @click="reset">{{ t("glossaryCancel") }}</button>
        <button class="link" :disabled="disabled" @click="save">{{ t("save") }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.glossary { display: flex; flex-direction: column; gap: 10px; }
.hint, .toolbar, .term small { color: var(--ink-3); font-size: 11.5px; line-height: 1.5; }
.hint { margin: 0; }
.toolbar { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.entries { max-height: 220px; overflow-y: auto; border: 1px solid var(--hairline); border-radius: var(--r-sm); }
.entry { display: flex; align-items: center; gap: 10px; padding: 8px 10px; }
.entry + .entry { border-top: 1px solid var(--hairline); }
.term { flex: 1; min-width: 0; overflow-wrap: anywhere; font-size: 12px; }
.term small { display: block; }
.link { padding: 0; font: inherit; color: var(--accent); text-decoration: underline; text-underline-offset: 2px; }
.link:hover { filter: brightness(1.15); }
.danger { color: #d44; }
.editor { display: flex; flex-direction: column; gap: 9px; padding: 12px; border-radius: var(--r-sm); background: var(--shade); }
.editor label, .language { display: grid; grid-template-columns: 96px minmax(0, 1fr); align-items: center; gap: 8px; font-size: 12px; }
.editor input { min-width: 0; }
.actions { display: flex; justify-content: flex-end; gap: 14px; }
.error { margin: 0; color: #d44; font-size: 12px; }
</style>
