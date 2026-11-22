export interface GlossaryEntry {
  source: string;
  aliases: string[];
  target: string;
  targetLang: string;
}

/**
 * Whether a glossary entry's target language is the session's target. The two Chinese
 * script variants are one language (only the output characters differ), so entries
 * tagged for either apply to both; "*" (untagged) applies everywhere. Mirrors the
 * matcher on the Rust side so the editor's active count stays honest.
 */
export function targetLangMatches(entryLang: string, targetLang: string): boolean {
  if (entryLang === "*" || entryLang === targetLang) return true;
  return (entryLang === "zh" || entryLang === "zh-TW") && (targetLang === "zh" || targetLang === "zh-TW");
}

function clean(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function savedEntry(value: unknown): GlossaryEntry | null {
  if (!value || typeof value !== "object") return null;
  const row = value as Record<string, unknown>;
  const source = clean(row.source);
  const target = clean(row.target);
  const targetLang = clean(row.targetLang);
  if (!source || !target || !targetLang || /[\r\n]/.test(source + target + targetLang)) return null;
  const aliases = Array.isArray(row.aliases)
    ? row.aliases.map(clean).filter((alias) => alias && alias.toLowerCase() !== source.toLowerCase())
    : [];
  if (aliases.some((alias) => /[\r\n]/.test(alias))) return null;
  return { source, aliases, target, targetLang };
}

export function migrateGlossary(saved: { glossary?: unknown; hotwords?: unknown }): GlossaryEntry[] {
  const entries: GlossaryEntry[] = [];
  const add = (entry: GlossaryEntry) => {
    if (!entries.some((item) => item.targetLang === entry.targetLang && item.source === entry.source)) {
      entries.push(entry);
    }
  };
  if (Array.isArray(saved.glossary)) {
    for (const value of saved.glossary) {
      const entry = savedEntry(value);
      if (entry) add(entry);
    }
  }
  if (saved.hotwords && typeof saved.hotwords === "object" && !Array.isArray(saved.hotwords)) {
    for (const [source, target] of Object.entries(saved.hotwords)) {
      const entry = savedEntry({ source, aliases: [], target, targetLang: "*" });
      if (entry) add(entry);
    }
  }
  return entries;
}

export function upsertGlossary(
  entries: GlossaryEntry[],
  draft: GlossaryEntry,
  index: number | undefined,
  languageCodes: ReadonlySet<string>,
): GlossaryEntry[] {
  if (index !== undefined && (!Number.isInteger(index) || index < 0 || index >= entries.length)) throw new Error("index");
  const source = clean(draft.source);
  const target = clean(draft.target);
  const targetLang = clean(draft.targetLang);
  const aliases = Array.isArray(draft.aliases) ? draft.aliases.map(clean).filter(Boolean) : [];
  if (!source || !target) throw new Error("blank");
  if ([source, target, targetLang, ...aliases].some((value) => /[\r\n]/.test(value))) throw new Error("line");
  if (aliases.some((alias) => alias.toLowerCase() === source.toLowerCase())) throw new Error("ambiguousAlias");
  if (!languageCodes.has(targetLang) && !(targetLang === "*" && index !== undefined && entries[index].targetLang === "*")) {
    throw new Error("language");
  }
  if (entries.some((entry, at) => at !== index && entry.targetLang === targetLang && entry.source === source)) {
    throw new Error("duplicate");
  }
  const next = entries.slice();
  const entry = { source, aliases, target, targetLang };
  if (index === undefined) next.push(entry);
  else next[index] = entry;
  return next;
}

export function deleteGlossary(entries: GlossaryEntry[], index: number): GlossaryEntry[] {
  if (!Number.isInteger(index) || index < 0 || index >= entries.length) throw new Error("index");
  return entries.filter((_, at) => at !== index);
}
