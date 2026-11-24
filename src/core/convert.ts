import { ref } from "vue";

export type TranscriptStyle = "system" | "zh" | "zh-TW";

/**
 * The cn→twp tables are a megabyte of character and phrase dictionaries. A Simplified UI
 * never needs them, so they are fetched the first time a Traditional display is asked for.
 * `ready` is reactive on purpose: a render that hits the unloaded converter shows the source
 * as-is, and when the tables arrive the flip re-renders the line.
 */
const ready = ref(false);
let cn2t: ((text: string) => string) | undefined;
let loading: Promise<void> | undefined;

/** Load the one-way converter. Safe to call repeatedly; the second call awaits the first. */
export function ensureTraditional(): Promise<void> {
  loading ??= import("js-opencc/cn2t").then(({ Converter }) => {
    cn2t = Converter({ to: "twp" });
    ready.value = true;
  });
  return loading;
}

/** Traditional Han characters for `text`, whatever script came in. Lazy: a render that beats
 * the tables gets the text as-is and re-renders once `ready` flips. */
function traditional(text: string): string {
  if (!ready.value) void ensureTraditional();
  return ready.value ? cn2t!(text) : text;
}

/**
 * Whether the pick, given the UI locale, wants Traditional Han characters. "system" follows
 * the interface: a zh-TW interface means Traditional, anything else the source's own script.
 */
export function wantsTraditional(style: TranscriptStyle, uiLocale: string): boolean {
  return style === "zh-TW" || (style === "system" && uiLocale === "zh-TW");
}

/**
 * Show `text` the way the transcript script says. ASR emits Simplified Chinese, so this is
 * one-way only; Latin, digits and kana pass through untouched, so a mixed transcript is safe.
 * The stored line keeps what the engine emitted; the exports apply this same rule.
 */
export function convertTranscript(text: string | undefined, style: TranscriptStyle, uiLocale: string): string {
  if (!text) return "";
  return wantsTraditional(style, uiLocale) ? traditional(text) : text;
}

/**
 * A translation line in the script its own language asks for. A 繁體中文 target has to read
 * Traditional even when the model answers in Simplified — a small local model and every cloud
 * API that only knows one `zh` do that often — so the guarantee is made here, not in the prompt.
 * A plain `zh` target keeps whatever the model wrote: the app never rewrites Simplified away.
 */
export function convertTarget(text: string | undefined, targetLang: string): string {
  if (!text) return "";
  return targetLang === "zh-TW" ? traditional(text) : text;
}
