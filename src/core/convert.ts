import { ref } from "vue";

export type TranscriptStyle = "system" | "zh" | "zh-TW";

/**
 * The cn→twp tables are a megabyte of character and phrase dictionaries. A Simplified UI
 * never needs them, so they are fetched the first time a Traditional display is asked for.
 * `ready` is reactive on purpose: a render that hits the unloaded converter shows the source
 * as-is, and when the tables arrive the flip re-renders the line.
 */
const ready = ref(false);
let toTraditional: ((text: string) => string) | undefined;
let loading: Promise<void> | undefined;

/** Load the one-way converter. Safe to call repeatedly; the second call awaits the first. */
export function ensureTraditional(): Promise<void> {
  loading ??= import("js-opencc/cn2t").then(({ Converter }) => {
    toTraditional = Converter({ to: "twp" });
    ready.value = true;
  });
  return loading;
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
 * The stored line and every export stay what the engine produced.
 */
export function convertTranscript(text: string | undefined, style: TranscriptStyle, uiLocale: string): string {
  if (!text) return "";
  if (!wantsTraditional(style, uiLocale)) return text;
  if (!ready.value) void ensureTraditional();
  return ready.value ? toTraditional!(text) : text;
}
