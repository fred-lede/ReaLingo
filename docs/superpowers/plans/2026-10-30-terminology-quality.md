# Terminology Quality Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an editable, persistent terminology list that corrects explicit ASR aliases in local subtitles, guides only matching local translations, and configures Qwen's native remote hotwords.

**Architecture:** Store language-scoped entries in the existing settings file and migrate the current Qwen hotword map. At local session start, compile the entries for the chosen target language into one matcher; each finalized ASR segment yields corrected source text and at most eight translation hints. Qwen receives applicable phrases in `session.update`; OpenAI and Gemini keep their current wire messages and display an honest availability note.

**Tech Stack:** Vue 3 / TypeScript, Tauri 2 / Rust, `aho-corasick`, `llama-cpp-2`, Node 26 built-in test runner.

**Spec:** `docs/superpowers/specs/2026-10-30-terminology-quality-design.md`

## Global Constraints

- Keep every entry in ReaLingo's current `LANGUAGES` list; do not narrow source or target choices.
- Existing Qwen `hotwords` must survive migration with their old all-target behavior. Only migrated entries may keep `targetLang: "*"`; new entries require one language code.
- Local mode remains subtitle-only. No TTS, cloud fallback, raw-audio upload, or change to the 16-second ASR window.
- At most eight matched entries enter one local prompt; at most 1,000 phrases enter one Qwen session update. No match means no extra prompt section.
- Do not claim glossary enforcement for dedicated OpenAI realtime translation or Gemini Live Translate; send neither provider an undocumented field and do not rewrite its target subtitles.
- Keep the existing Chinese and English UI dictionaries. Do not change model dependencies except the matcher crate.
- Leave the new work uncommitted and unpushed until the user explicitly requests Git publication.
- Metal scheduling and microphone drop/queue tuning belong to a later decision based on the pending two-minute `rt://raw` diagnostics.

## Review Focus

- **Old or malformed saved settings:** migration keeps valid old pairs, handles absent/invalid arrays without a startup exception, and does not duplicate entries on reload. Test in Task 1.
- **Ambiguous matching:** `LoRa` and `Loari` correct to `LoRA`, while `Flora` and a larger overlapping term stay intact; mixed CJK/Latin text remains valid UTF-8. Test in Task 3.
- **Conflicting or excessive Qwen phrases:** an exact target-language entry overrides a migrated all-target entry, and the wire map never exceeds 1,000 keys. Test in Task 2.
- **User-entered line breaks and duplicate terms:** the editor rejects them before persistence, including case-only duplicates for the same target language. Test in Task 1 and Task 5.
- **Unsupported remote providers:** OpenAI and Gemini session payloads stay unchanged, and Settings explains why glossary entries do not apply. Test in Task 2 and UI check in Task 5.

---

## File Structure

- `src/glossary.ts` — frontend entry type, legacy migration, validation, and immutable add/edit/delete helpers.
- `tests/glossary.test.mjs` — Node tests for migration and editor data operations.
- `src/store.ts` — persisted `glossary` array; one-time migration before the existing autosave watcher starts.
- `src-tauri/src/config.rs` — matching serde entry type and applicable Qwen phrase selection.
- `src-tauri/Cargo.toml` — direct `aho-corasick` dependency.
- `src-tauri/src/local/glossary.rs` — one-session matcher, alias correction, and per-segment translation hints.
- `src-tauri/src/local/mod.rs` — register the glossary module and update its existing model smoke test call.
- `src-tauri/src/local/session.rs` — run the matcher after raw ASR deduplication and pass hints with translation jobs.
- `src-tauri/src/local/translation.rs` — render matching terms in the GGUF user prompt, including retries.
- `src/components/GlossaryEditor.vue` — focused add/edit/delete UI and provider availability note.
- `src/components/Settings.vue` — replace the Qwen-only hotword textarea with the shared editor.
- `src/i18n.ts` — Chinese and English UI labels and validation messages.
- `README.md`, `README.en.md` — describe local/Qwen application and OpenAI/Gemini limitations.

### Task 1: Persist entries and migrate legacy hotwords

**Files:** Create `src/glossary.ts`, `tests/glossary.test.mjs`; modify `src/store.ts:80-115,163-183`.

**Interfaces:** Produce `GlossaryEntry = { source: string; aliases: string[]; target: string; targetLang: string }`, `migrateGlossary(saved: { glossary?: unknown; hotwords?: unknown }): GlossaryEntry[]`, `upsertGlossary(entries: GlossaryEntry[], draft: GlossaryEntry, index?: number): GlossaryEntry[]`, and `deleteGlossary(entries: GlossaryEntry[], index: number): GlossaryEntry[]`. An invalid draft throws a short error key consumed by the UI. Valid language codes are passed into `upsertGlossary` as an additional `languageCodes: ReadonlySet<string>` parameter; `"*"` is accepted only while editing an existing migrated entry.

- [ ] **Step 1: Add failing Node tests.** In `tests/glossary.test.mjs`, import the four functions from `../src/glossary.ts` and use `node:assert/strict`. Pin these concrete cases:

```js
assert.deepEqual(migrateGlossary({ hotwords: { LoRA: '低秩适配' } }), [
  { source: 'LoRA', aliases: [], target: '低秩适配', targetLang: '*' },
]);
const saved = { glossary: [{ source: 'LoRA', aliases: ['Loari'], target: '低秩适配', targetLang: 'zh' }], hotwords: {} };
assert.deepEqual(migrateGlossary(saved), saved.glossary);
const oldAndNew = { glossary: [{ source: 'LoRA', aliases: [], target: 'Low-rank adaptation', targetLang: '*' }], hotwords: { LoRA: 'Low-rank adaptation' } };
assert.equal(migrateGlossary(oldAndNew).length, 1);
assert.deepEqual(migrateGlossary({ glossary: 'bad', hotwords: null }), []);
const codes = new Set(['zh', 'en']);
const one = upsertGlossary([], { source: 'LoRA', aliases: ['Loari'], target: '低秩适配', targetLang: 'zh' }, undefined, codes);
assert.equal(one.length, 1);
assert.equal(deleteGlossary(one, 0).length, 0);
assert.throws(() => upsertGlossary(one, { ...one[0], source: 'lora' }, undefined, codes), /duplicate/i);
assert.throws(() => upsertGlossary([], { ...one[0], target: 'bad\nline' }, undefined, codes), /line/i);
```

- [ ] **Step 2: Run `node --test tests/glossary.test.mjs`.** Expect module-not-found failure before `src/glossary.ts` exists.
- [ ] **Step 3: Implement the pure helpers.** Trim source/target/aliases; reject blank fields, line breaks, invalid target codes, and same-source/same-target-language duplicates (case-insensitive). Preserve an existing `"*"` only at its edit index. `migrateGlossary` accepts only well-formed saved entries, maps old string-to-string hotwords to wildcard entries when no equivalent wildcard exists, and never mutates its input. Return new arrays from add/edit/delete.

```ts
export interface GlossaryEntry { source: string; aliases: string[]; target: string; targetLang: string }
export function migrateGlossary(saved: { glossary?: unknown; hotwords?: unknown }): GlossaryEntry[];
export function upsertGlossary(entries: GlossaryEntry[], draft: GlossaryEntry, index: number | undefined, languageCodes: ReadonlySet<string>): GlossaryEntry[];
export function deleteGlossary(entries: GlossaryEntry[], index: number): GlossaryEntry[];
```

- [ ] **Step 4: Add `glossary: GlossaryEntry[]` to `Settings` with default `[]`.** In `initSettings`, after `Object.assign` and before installing the autosave watcher, set `settings.glossary = migrateGlossary(saved)` and `settings.hotwords = {}`. Keep the old `hotwords` field in the type for one migration cycle; the new UI will no longer edit it.

```ts
settings.glossary = migrateGlossary(saved);
settings.hotwords = {};
```
- [ ] **Step 5: Run `node --test tests/glossary.test.mjs` and `npm run build`.** Expect all migration/validation tests and Vue type checking to pass. Check `git diff --check`.

### Task 2: Apply applicable terms to Qwen only

**Files:** Modify `src-tauri/src/config.rs:70-119,280-335,375-395,449-465`.

**Interfaces:** Produce Rust `GlossaryEntry` with serde camelCase fields `source`, `aliases`, `target`, `targetLang`; add `Settings.glossary: Vec<GlossaryEntry>` with `#[serde(default)]`. Produce `Settings::qwen_phrases(&self) -> BTreeMap<String, String>`; `session_update` adds `corpus.phrases` only when the resulting map is nonempty.

- [ ] **Step 1: Add failing Rust tests** in `config::tests`. Construct entries for `zh` and `en`, set target `zh`, and assert only `zh`/wildcard terms and aliases enter `session.translation.corpus.phrases`; assert a target-specific `LoRA` value beats a wildcard value. Create 1,005 distinct phrases and assert `phrases.as_object().unwrap().len() == 1_000`. Assert OpenAI and Gemini payloads contain no `corpus`. Assert deserializing old settings without `glossary` yields an empty vector.

```rust
let mut settings = qwen(Region::Beijing, "");
settings.target_lang = "zh".into();
settings.glossary.push(GlossaryEntry {
    source: "LoRA".into(), aliases: vec!["Loari".into()],
    target: "低秩适配".into(), target_lang: "zh".into(),
});
let phrases = &settings.session_update()["session"]["translation"]["corpus"]["phrases"];
assert_eq!(phrases["Loari"], "低秩适配");
```
- [ ] **Step 2: Run `cargo test --manifest-path src-tauri/Cargo.toml config::tests`.** Expect compile failure because `Settings.glossary` and `GlossaryEntry` are not defined.
- [ ] **Step 3: Implement the fields and `qwen_phrases`.** Start with legacy `hotwords`, then wildcard entries, then exact-target entries so the more specific entry wins. Include canonical source and each nonempty alias as source keys; trim keys/values and skip line breaks. Convert the final deterministic `BTreeMap` to at most 1,000 entries before serialization. Replace the existing direct `self.hotwords` corpus assignment with the resulting map. Leave the early OpenAI/Gemini return paths alone.

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryEntry {
    pub source: String,
    #[serde(default)] pub aliases: Vec<String>,
    pub target: String,
    pub target_lang: String,
}
// In Settings: #[serde(default)] pub glossary: Vec<GlossaryEntry>
let phrases = self.qwen_phrases();
if !phrases.is_empty() {
    translation["corpus"] = serde_json::json!({ "phrases": phrases });
}
```
- [ ] **Step 4: Run `cargo test --manifest-path src-tauri/Cargo.toml config::tests`.** Expect all existing and new config tests to pass. Run `cargo fmt --manifest-path src-tauri/Cargo.toml --check`.

### Task 3: Compile a bounded local alias matcher

**Files:** Modify `src-tauri/Cargo.toml`; create `src-tauri/src/local/glossary.rs`; modify `src-tauri/src/local/mod.rs:1-4`.

**Interfaces:** Produce `GlossaryMatcher::new(entries: &[GlossaryEntry], target_lang: &str) -> Result<Self, String>` and `GlossaryMatcher::apply(&self, text: &str) -> MatchResult`, where `MatchResult { text: String, hints: Vec<(String, String)> }`. Compile once at session start, not per ASR window. `hints` is unique by canonical source/target pair and contains at most eight entries.

- [ ] **Step 1: Add failing unit tests** in `local/glossary.rs` for `"LoRa and Loari" -> "LoRA and LoRA"` with one hint, `"Flora"` unchanged, `"人工智能Loari"` corrected without corrupting the Chinese characters, a literal `"人工智能"` entry matching inside a Chinese sentence, empty/no-match text with zero hints, longer overlapping alias winning, inactive target language ignored, and 20 matching terms yielding eight hints. Construct 5,000 entries for other target languages plus one active entry and assert only the active term becomes a compiled pattern.

```rust
let entry = GlossaryEntry {
    source: "LoRA".into(), aliases: vec!["Loari".into()],
    target: "低秩适配".into(), target_lang: "zh".into(),
};
let matcher = GlossaryMatcher::new(&[entry], "zh").unwrap();
let hit = matcher.apply("LoRa and Loari, not Flora");
assert_eq!(hit.text, "LoRA and LoRA, not Flora");
assert_eq!(hit.hints, vec![("LoRA".into(), "低秩适配".into())]);
```
- [ ] **Step 2: Run `cargo test --manifest-path src-tauri/Cargo.toml local::glossary::tests`.** Expect compile failure until the module is registered and its matcher exists.
- [ ] **Step 3: Add `aho-corasick` as a direct dependency and implement the matcher.** Build `AhoCorasick` from canonical terms and aliases with ASCII-insensitive matching. Gather overlapping byte-range matches, reject Latin matches adjacent to alphanumeric/underscore characters, select the longest valid nonoverlapping match at each start, and copy untouched UTF-8 slices around replacements. Replace an explicit alias or an ASCII case variant of the canonical term with canonical spelling; an exact canonical match only adds a hint. Do not create an automaton for zero active patterns.

```rust
pub struct MatchResult { pub text: String, pub hints: Vec<(String, String)> }
impl GlossaryMatcher {
    pub fn new(entries: &[GlossaryEntry], target_lang: &str) -> Result<Self, String>;
    pub fn apply(&self, text: &str) -> MatchResult;
}
```
- [ ] **Step 4: Run the focused matcher tests and `cargo fmt --manifest-path src-tauri/Cargo.toml --check`.** Expect every matching, boundary, overlap, and limit case to pass.

### Task 4: Feed corrected source and small hints to the local GGUF translator

**Files:** Modify `src-tauri/src/local/session.rs:105-111,113-150,195-310`, `src-tauri/src/local/translation.rs:75-115`, and the existing ignored smoke call in `src-tauri/src/local/mod.rs`.

**Interfaces:** Add `hints: Vec<(String, String)>` to `TranslationJob`. Extend `LocalTranslator::translate` and `generate` with `hints: &[(String, String)]`; keep the current four arguments in their current order and append `hints`. Produce a pure `translation_user_text(text: &str, context: &str, hints: &[(String, String)]) -> String` used by both initial generation and the Chinese retry.

- [ ] **Step 1: Add failing prompt tests** in `local::translation::tests`: no hints produces the current exact `Current segment to translate:\n...` body; one hint produces a `Terminology for current segment` section with `LoRA => 低秩适配`; context remains labeled `for meaning only`; 20 hints produce at most eight lines. Add a session test that raw deduplication still sees the uncorrected ASR spelling while the emitted/queued text uses the matcher result.
- [ ] **Step 2: Run `cargo test --manifest-path src-tauri/Cargo.toml local::translation::tests` and `cargo test --manifest-path src-tauri/Cargo.toml local::session::tests`.** Expect failure because the helper and job hints do not exist.
- [ ] **Step 3: Wire one matcher into `LocalSession::run`.** Pass it to `run_asr` and `process_window`. After `trim_repeated_prefix`, retain that raw text as `last_source` for overlap deduplication, then call `matcher.apply`; emit its corrected text as `source` and place corrected text plus hints in the translation job. In `translate_loop`, pass each job's hints to `translator.translate`; leave the two-segment context behavior and diagnostics fields intact.

```rust
let matcher = GlossaryMatcher::new(&settings.glossary, &settings.target_lang)?;
let raw_text = if segment.start_ms < *committed_until_ms {
    trim_repeated_prefix(last_source, &segment.text)
} else { segment.text.clone() };
*last_source = raw_text.clone();
let matched = matcher.apply(raw_text.trim());
let source_text = matched.text;
let hints = matched.hints;
```

- [ ] **Step 4: Implement `translation_user_text`.** With zero hints, return the existing user text byte-for-byte. Otherwise prepend only the matched `source => target` pairs, then the current context and segment labels. Sanitize line breaks already rejected by the UI again at the Rust boundary; apply the same hints during a Chinese-output retry. Update the ignored Metal smoke test call with an empty hint slice.

```rust
fn translation_user_text(text: &str, context: &str, hints: &[(String, String)]) -> String;
// The existing body remains unchanged when hints.is_empty().
// With a hit: "Terminology for current segment:\nLoRA => 低秩适配\n\nCurrent segment to translate:\nLoRA"
```
- [ ] **Step 5: Run `cargo test --manifest-path src-tauri/Cargo.toml` and `cargo fmt --manifest-path src-tauri/Cargo.toml --check`.** Expect the full Rust suite to pass, with only the existing model-dependent Metal test ignored.

### Task 5: Replace the textarea with a shared terminology editor

**Files:** Create `src/components/GlossaryEditor.vue`; modify `src/components/Settings.vue:1-70,229-244`, `src/i18n.ts:136-158,299-320`, `README.md`, `README.en.md`.

**Interfaces:** `GlossaryEditor` reads/writes `settings.glossary` through `upsertGlossary` and `deleteGlossary`, receives `disabled: boolean`, `mode: ExecutionMode`, and `protocol: Protocol`, and uses the complete `LANGUAGES` list for the target picker. Its form has canonical source, comma-separated aliases, preferred target, and one target language; an existing wildcard entry may retain `*` until edited to a specific language.

- [ ] **Step 1: Add failing frontend helper tests** for an existing wildcard entry edited without changing its language, for rejecting a newly added wildcard entry, and for editing/deleting exactly the selected row while other rows remain unchanged. Run `node --test tests/glossary.test.mjs`; expect these tests to fail until the helper behavior is complete.
- [ ] **Step 2: Build `GlossaryEditor.vue`.** Render a list with Edit and Delete buttons and a compact form using the existing `Picker` and Settings field styles. Use a local draft so incomplete typing never autosaves. On Save, call `upsertGlossary`; show its validation message next to the form and preserve the draft on error. Disable all edits during a running session. New entries default to `settings.targetLang`; show a legacy all-target option only for the row being edited.

```ts
const draft = ref<GlossaryEntry>({ source: "", aliases: [], target: "", targetLang: settings.targetLang });
const aliasText = ref("");
function save() {
  const entry = { ...draft.value, aliases: aliasText.value.split(",").map(x => x.trim()).filter(Boolean) };
  settings.glossary = upsertGlossary(settings.glossary, entry, editingIndex.value ?? undefined, new Set(LANGUAGES.map(l => l.code)));
}
```

- [ ] **Step 3: Replace the Qwen-only textarea section** with an always-visible translation group containing the editor. Add Chinese/English labels for terminology, source, aliases, preferred translation, target language, Add/Edit/Delete, duplicate/blank/line-break errors, Qwen's 1,000-phrase cap, and the OpenAI/Gemini unavailable notice. Keep `src/languages.ts` untouched so every existing language remains selectable.

```vue
<section class="group">
  <h3>{{ t("groupTranslate") }}</h3>
  <GlossaryEditor :disabled="isRunning()" :mode="settings.mode" :protocol="settings.protocol" />
</section>
```
- [ ] **Step 4: Document behavior** in both READMEs: local alias correction and matching prompt hints, Qwen native corpus, migrated old hotwords, and OpenAI/Gemini limitations. State that entries apply automatically on the next session and that stored count is not copied into every local prompt.
- [ ] **Step 5: Run `node --test tests/glossary.test.mjs`, `npm run build`, `cargo test --manifest-path src-tauri/Cargo.toml`, and `git diff --check`.** Expect all tests/builds to pass. In the app, add/edit/delete a term, close/reopen Settings, restart the app, and confirm persistence; switch target language and confirm applicable terms change; inspect Qwen/OpenAI/Gemini session payload tests rather than asserting unsupported providers apply entries.

## Final Acceptance

- Run the supplied speech sample with and without a `LoRA` entry (`Loari`, `LoRa` aliases). Compare source spelling, target spelling, missed phrases, and per-segment ASR/translation timings; report numbers and the fact that one sample cannot establish general accuracy.
- Do not change Metal scheduling until the user's microphone log shows queue growth or capture drops and identifies which stage is responsible. If that log is not yet available, report the performance decision as pending rather than declaring the drop issue fixed.
- Review the final diff against the spec; verify only glossary, local-prompt, Qwen config, documentation, and necessary tests changed. Leave the branch uncommitted and unpushed pending an explicit Git request.
