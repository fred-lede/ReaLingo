/**
 * Character-level Simplified → Traditional conversion for the fixed display strings
 * that live outside the translation dictionaries (language names, voice notes).
 * Only the characters those strings actually contain are mapped — it is not a
 * general-purpose converter, so it is never applied to free-form text.
 */
const MAP: Record<string, string> = {
  语: "語", 韩: "韓", 罗: "羅", 亚: "亞", 乌: "烏", 尔: "爾", 兰: "蘭",
  马: "馬", 粤: "粵", 爱: "愛", 维: "維", 顿: "頓", 麦: "麥", 来: "來",
  宾: "賓", 腊: "臘", 务: "務", 图: "圖", 纳: "納", 达: "達", 萨: "薩",
  脱: "脫", 声: "聲", 仅: "僅", 说: "說", 话: "話", 复: "複",
};
const RE = new RegExp(`[${Object.keys(MAP).join("")}]`, "g");

export function s2t(s: string): string {
  return s.replace(RE, (c) => MAP[c]);
}

/** `s` as the given UI locale shows it: zh-TW gets the Traditional form, zh/en keep theirs. */
export function tw(s: string, locale: string): string {
  return locale === "zh-TW" ? s2t(s) : s;
}
