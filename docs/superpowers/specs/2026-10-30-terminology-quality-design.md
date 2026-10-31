# Terminology and local translation quality

## Goal

Let users correct recurring names and technical terms without adding measurable work to every subtitle segment. Keep ReaLingo's complete language list and existing local and remote modes.

## Scope

- Add a terminology editor in Settings with add, edit, and delete actions. Each entry has a canonical source term, optional recognition aliases, a preferred translation, and a target language. The entry applies automatically when that target language is selected.
- Preserve existing Qwen hotwords. Migrate them into terminology entries without losing their previous all-target behavior; the editor can then assign a specific target language.
- For local mode, match only terms present in the current finalized ASR segment. Replace explicit aliases with the canonical source spelling in that segment, and pass only matched source-to-target preferences to the GGUF translation prompt. Do not alter unrelated text or append missing words.
- For Qwen remote mode, send applicable entries through the documented `session.translation.corpus.phrases` field. Bound the session payload to 1,000 phrases.
- The dedicated OpenAI realtime translation model does not accept glossaries. Gemini Live Translate documents no glossary field. Show these limitations in Settings; do not claim that entries are enforced for those protocols or rewrite their translated subtitles without reliable alignment.

## Data and matching

Store entries in the existing local settings file. A target language is a code from the existing `LANGUAGES` list; the legacy all-target code remains available only for migrated entries. Duplicates with the same source and target language are rejected in the editor. Build a matcher once when a session starts, using only entries for its target language, so the full stored list is not scanned for every segment. Alias matching uses word boundaries for Latin terms and literal phrase matching for CJK terms. The longer matching term wins when entries overlap. A segment contributes at most eight matching entries to the local prompt.

## Verification

- Add, edit, delete, persist, and reload entries without changing the full language selector.
- Reproduce a `Loari` / `LoRa` recognition alias corrected to `LoRA`, with an applicable preferred target spelling, while nearby unrelated words remain unchanged.
- Confirm a nonmatching entry adds no tokens to a local translation prompt; more stored entries do not increase the number of prompt terms for a segment.
- Confirm Qwen session configuration contains only applicable terms, OpenAI and Gemini receive no undocumented glossary fields, and existing hotwords survive migration.
- Run Rust tests and the frontend build. Use the supplied speech sample for a manual before/after quality and latency comparison when Metal diagnostics can run.

## Performance follow-up

Inspect the pending two-minute microphone diagnostics before changing GPU scheduling. Compare audio queue depth, capture drops, ASR elapsed time, translation queue wait, and translation elapsed time. Keep the 16-second ASR window unless a replay shows a faster configuration with equal or better phrase recall.

## References

- Qwen corpus: https://help.aliyun.com/en/model-studio/qwen3-5-livetranslate-flash-realtime
- OpenAI realtime translation guide: https://developers.openai.com/cookbook/examples/voice_solutions/realtime_translation_guide
- Gemini Live Translate: https://ai.google.dev/gemini-api/docs/live-api/live-translate
