<script setup lang="ts">
import { computed } from "vue";
import Sheet from "./Sheet.vue";
import ChoiceList from "./ChoiceList.vue";
import { settings } from "../core/store";
import { t, tf, systemLocaleName, type Locale } from "../core/i18n";
import type { TranscriptStyle } from "../core/convert";

defineEmits<{ close: [] }>();

const options = computed(() => [
  { value: "system", label: t("uiSystem"), description: t("uiDetected").replace("{l}", systemLocaleName()) },
  { value: "zh-TW", label: "繁體中文" },
  { value: "zh", label: "简体中文" },
  { value: "en", label: "English" },
]);

/**
 * The ASR engines write Simplified Chinese whatever is spoken, so the script of the
 * transcript is a display choice, separate from the language being translated into.
 */
const scriptOptions = computed(() => [
  { value: "system", label: t("styleSystem"), description: tf("uiDetected", { l: systemLocaleName() }) },
  { value: "zh", label: t("styleSimplified") },
  { value: "zh-TW", label: t("styleTraditional") },
]);

function pick(v: "system" | Locale) {
  settings.uiLang = v;
}
function pickScript(v: string) {
  settings.transcriptStyle = v as TranscriptStyle;
}
</script>

<template>
  <Sheet :title="t('uiLang')" @close="$emit('close')">
    <p class="hintx">{{ t('uiLangHint') }}</p>
    <ChoiceList :label="t('uiLang')" :value="settings.uiLang" :options="options" @select="pick($event as 'system' | Locale)" />

    <h4 class="grp">{{ t('transcriptStyle') }}</h4>
    <p class="hintx">{{ t('transcriptStyleHint') }}</p>
    <ChoiceList :label="t('transcriptStyle')" :value="settings.transcriptStyle" :options="scriptOptions" @select="pickScript($event)" />
  </Sheet>
</template>

<style scoped>
.hintx { margin: -6px 0 0; font-size: 11.5px; line-height: 1.55; color: var(--ink-3); }
.grp { margin: 0; font-size: 14px; font-weight: 660; color: var(--ink-2); }
</style>
