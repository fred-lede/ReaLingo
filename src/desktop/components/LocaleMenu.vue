<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { locale, setLocale, systemLocaleName, t, type Locale } from "../../core/i18n";
import { settings } from "../../core/store";

type Choice = "system" | Locale;

const open = ref(false);
const box = ref<HTMLElement | null>(null);
const pop = ref<HTMLElement | null>(null);
const popStyle = ref<Record<string, string>>({});

/** The same four choices the phone offers, so both surfaces pick a language the same way. */
const options = computed(() => [
  { value: "system" as const, label: t("uiSystem"), note: t("uiDetected").replace("{l}", systemLocaleName()) },
  { value: "zh-TW" as const, label: "繁體中文" },
  { value: "zh" as const, label: "简体中文" },
  { value: "en" as const, label: "English" },
]);

/** What the button wears: the language the UI is in right now, system resolved. */
const short = computed(() => (locale.value === "zh-TW" ? "繁" : locale.value === "zh" ? "简" : "EN"));

/**
 * The popup is teleported to <body> and anchored by hand — same reasoning as Picker:
 * every ancestor here is a stacking context that could clip or bury it. It is right-
 * anchored because the button lives at the window's right edge.
 */
function place() {
  const r = box.value?.getBoundingClientRect();
  if (!r) return;
  const below = window.innerHeight - r.bottom;
  const flipUp = below < 220 && r.top > below;
  popStyle.value = {
    right: `${window.innerWidth - r.right}px`,
    ...(flipUp ? { bottom: `${window.innerHeight - r.top + 6}px` } : { top: `${r.bottom + 6}px` }),
  };
}

function onPointerDown(e: PointerEvent) {
  const target = e.target as Node;
  if (!box.value?.contains(target) && !pop.value?.contains(target)) open.value = false;
}
const onKeydown = (e: KeyboardEvent) => {
  if (e.key === "Escape") open.value = false;
};
function bind(on: boolean) {
  const fn = on ? window.addEventListener : window.removeEventListener;
  const doc = on ? document.addEventListener : document.removeEventListener;
  doc.call(document, "pointerdown", onPointerDown as EventListener, true);
  fn.call(window, "resize", place);
  fn.call(window, "scroll", place, true);
  fn.call(window, "keydown", onKeydown as EventListener);
}

watch(open, async (v) => {
  bind(v);
  if (!v) return;
  place();
  await nextTick();
  place();
});
onBeforeUnmount(() => bind(false));

function pick(value: Choice) {
  setLocale(value);
  open.value = false;
}
</script>

<template>
  <div ref="box" class="lm">
    <button
      class="btn-icon lm-btn"
      :aria-label="t('uiLang')"
      :title="t('uiLang')"
      aria-haspopup="menu"
      :aria-expanded="open"
      @click="open = !open"
    >{{ short }}</button>

    <Teleport to="body">
      <div v-if="open" ref="pop" class="lm-pop" :style="popStyle" role="menu">
        <button
          v-for="o in options"
          :key="o.value"
          class="lm-opt"
          :class="{ on: o.value === settings.uiLang }"
          role="menuitemradio"
          :aria-checked="o.value === settings.uiLang"
          @click="pick(o.value)"
        >
          <span>{{ o.label }}</span>
          <small v-if="o.note" class="lm-note">{{ o.note }}</small>
        </button>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.lm { position: relative; }
/* App.vue's scoped `.locale` sizing cannot reach in here, so the glyph wears it locally. */
.lm-btn { font-size: 12px; font-weight: 700; }
</style>

<!-- Teleported to <body>, so the popup cannot be scoped to this component. -->
<style>
.lm-pop {
  position: fixed;
  z-index: 120;
  min-width: 210px;
  padding: 8px;
  border-radius: var(--r-md);
  background: var(--glass);
  backdrop-filter: blur(34px) saturate(185%);
  box-shadow: 0 20px 44px -14px rgba(10, 16, 40, 0.5), inset 0 1px 0 var(--glass-hi);
  display: flex;
  flex-direction: column;
  gap: 1px;
  animation: lm-pop-in 0.18s var(--ease);
}
@keyframes lm-pop-in {
  from { opacity: 0; transform: translateY(-6px) scale(0.97); }
}
.lm-pop .lm-opt {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
  padding: 8px 10px;
  border-radius: 9px;
  font-size: 13px;
  text-align: left;
  transition: background 0.14s;
}
.lm-pop .lm-opt:hover { background: var(--shade); }
.lm-pop .lm-opt.on { background: var(--accent); color: #fff; }
.lm-pop .lm-note { font-size: 11px; color: var(--ink-3); }
.lm-pop .lm-opt.on .lm-note { color: rgba(255, 255, 255, 0.7); }
</style>
