use super::{asr::SubtitleSegment, glossary::GlossaryMatcher, LocalModels, LocalTranslator, WhisperEngine};
use crate::{
    config::Settings,
    realtime::{self, Event},
};
use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::Receiver;

const RATE: usize = 16_000;
const HOP: usize = RATE * 2;
const MAX_PENDING: usize = RATE * 16;
// Suppress only a long, nearly silent tail after capture stops (about -50 dBFS).
const QUIET_FLUSH_RMS_PCM: f64 = 104.0;
const AUDIBLE_BLOCK_RMS_PCM: f64 = 184.0;
const CATCH_UP_START_CHUNKS: usize = 20;
const CATCH_UP_END_CHUNKS: usize = 5;
// Whisper can move a segment's start backward between overlapping windows.
const TIMESTAMP_JITTER_MS: i64 = 1_000;
const MAX_TRANSLATION_WAIT: Duration = Duration::from_secs(2);
const MAX_GROUP_PARTS: usize = 3;

fn hop_for_queue(queue_chunks: usize, live_capture: bool, catching_up: &mut bool) -> usize {
    if !live_capture {
        return HOP;
    }
    if *catching_up {
        if queue_chunks <= CATCH_UP_END_CHUNKS {
            *catching_up = false;
        }
    } else if queue_chunks >= CATCH_UP_START_CHUNKS {
        *catching_up = true;
    }
    if *catching_up { HOP * 2 } else { HOP }
}

fn collapse_repeated_asr_segments(segments: Vec<SubtitleSegment>) -> Vec<SubtitleSegment> {
    // Whisper can turn one window into several copies of the same long phrase.
    let normalized = |text: &str| -> Vec<char> {
        let text = text.trim();
        let text = if text.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("and ")) {
            &text[4..]
        } else {
            text
        };
        text.chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let mut kept = Vec::with_capacity(segments.len());
    let mut start = 0;
    while start < segments.len() {
        let first = normalized(&segments[start].text);
        let mut end = start + 1;
        while first.len() >= 12
            && end < segments.len()
            && normalized(&segments[end].text) == first
        {
            end += 1;
        }
        let keep_end = if end - start >= 4 { start + 1 } else { end };
        kept.extend_from_slice(&segments[start..keep_end]);
        start = end;
    }
    kept
}

fn committable(segment: &SubtitleSegment, committed_until_ms: i64, stable_before_ms: i64) -> bool {
    segment.end_ms > committed_until_ms
        && segment.end_ms <= stable_before_ms
        && segment.start_ms >= committed_until_ms.saturating_sub(TIMESTAMP_JITTER_MS)
}

fn letters(text: &str) -> Vec<(char, usize)> {
    text.char_indices()
        .filter_map(|(index, character)| {
            character.is_alphanumeric().then(|| {
                (character.to_lowercase().next().unwrap_or(character), index + character.len_utf8())
            })
        })
        .collect()
}

fn trim_repeated_prefix(previous: &str, current: &str) -> String {
    let old = letters(previous);
    let new = letters(current);
    let minimum = if new
        .first()
        .is_some_and(|(character, _)| character.is_ascii_alphanumeric())
    {
        1
    } else {
        4
    };
    for count in (minimum..=old.len().min(new.len())).rev() {
        if old[old.len() - count..].iter().zip(&new[..count]).all(|(a, b)| a.0 == b.0) {
            let remaining = &current[new[count - 1].1..];
            if new[count - 1].0.is_ascii_alphabetic()
                && remaining.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                continue;
            }
            return remaining.trim_start_matches(|c: char| !c.is_alphanumeric()).to_owned();
        }
    }
    current.to_owned()
}

pub struct TranscriptionWindow {
    pub samples: Vec<f32>,
    pub offset_ms: i64,
}

#[derive(Default)]
pub struct SpeechSegmenter {
    pending: Vec<i16>,
    since_window: usize,
    consumed: usize,
}

impl SpeechSegmenter {
    pub fn push(&mut self, samples: &[i16], hop: usize) -> Vec<TranscriptionWindow> {
        let mut windows = Vec::new();
        for sample in samples {
            self.pending.push(*sample);
            self.since_window += 1;
            if self.pending.len() >= MAX_PENDING {
                windows.push(self.window());
                self.advance(hop);
            } else if self.since_window >= hop {
                windows.push(self.window());
                self.since_window = 0;
            }
        }
        windows
    }
    pub fn flush(&mut self) -> Option<TranscriptionWindow> {
        if self.pending.is_empty() {
            return None;
        }
        if self.pending.len() >= RATE * 8 {
            let mean_square = self.pending.iter().map(|sample| (*sample as f64).powi(2)).sum::<f64>()
                / self.pending.len() as f64;
            let audible_block = self.pending.chunks(RATE / 10).any(|block| {
                block.iter().map(|sample| (*sample as f64).powi(2)).sum::<f64>()
                    / block.len() as f64 >= AUDIBLE_BLOCK_RMS_PCM.powi(2)
            });
            if mean_square < QUIET_FLUSH_RMS_PCM.powi(2) && !audible_block {
                self.pending.clear();
                self.since_window = 0;
                return None;
            }
        }
        let window = self.window();
        self.pending.clear();
        self.since_window = 0;
        Some(window)
    }
    fn window(&self) -> TranscriptionWindow {
        TranscriptionWindow {
            samples: self.pending.iter().map(|s| *s as f32 / 32768.0).collect(),
            offset_ms: (self.consumed * 1000 / RATE) as i64,
        }
    }
    fn advance(&mut self, hop: usize) {
        let drain = hop.min(self.pending.len());
        self.pending.drain(..drain);
        self.consumed += drain;
        self.since_window = 0;
    }
}

pub struct LocalSession;

struct TranslationJob {
    id: String,
    text: String,
    hints: Vec<(String, String)>,
    queued_at: Instant,
}

struct PendingGroup {
    id: String,
    input: String,
    parts: Vec<SubtitleSegment>,
    hints: Vec<(String, String)>,
    started: Instant,
}

impl PendingGroup {
    fn new(id: String, started: Instant) -> Self {
        let input = format!("{id}-input");
        Self { id, input, parts: Vec::new(), hints: Vec::new(), started }
    }

    fn push(&mut self, part: SubtitleSegment, hints: Vec<(String, String)>) {
        self.parts.push(part);
        for hint in hints {
            if self.hints.len() < 8 && !self.hints.contains(&hint) {
                self.hints.push(hint);
            }
        }
    }

    fn display_text(&self) -> String {
        self.parts.iter().map(|part| part.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    fn translation_text(&self) -> String {
        self.parts.iter().map(|part| part.text.as_str()).collect::<Vec<_>>().join(" ")
    }

    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.started) >= MAX_TRANSLATION_WAIT
    }

    fn ready(&self, now: Instant) -> bool {
        self.parts.len() >= MAX_GROUP_PARTS
            || self.parts.last().is_some_and(|part| {
                part.text.trim_end().ends_with(['.', '?', '!', '。', '？', '！'])
            })
            || self.expired(now)
    }
}

impl LocalSession {
    pub fn run(
        app: &AppHandle,
        settings: &Settings,
        target_language_name: &str,
        live_capture: bool,
        mut audio_rx: Receiver<Vec<i16>>,
        stop: &Arc<AtomicBool>,
        models: &mut LocalModels,
    ) -> Result<(), String> {
        realtime::note(app, "status", "connecting");
        realtime::note(app, "status", "connected");
        let matcher = GlossaryMatcher::new(&settings.glossary, &settings.target_lang)?;
        let (translation_tx, translation_rx) = std::sync::mpsc::channel();
        let (asr, translator) = (&models.asr, &mut models.translator);
        std::thread::scope(|scope| {
            let translation_matcher = &matcher;
            let translation = scope.spawn(move || {
                Self::translate_loop(app, settings, target_language_name, translator, translation_matcher, translation_rx)
            });
            let asr_result = Self::run_asr(app, settings, live_capture, &mut audio_rx, stop, asr, &matcher, &translation_tx);
            drop(translation_tx);
            let translation_result = translation
                .join()
                .map_err(|_| "Translation worker panicked".to_string())?;
            translation_result?;
            asr_result
        })?;
        realtime::note(app, "status", "closed");
        Ok(())
    }

    fn run_asr(
        app: &AppHandle,
        settings: &Settings,
        live_capture: bool,
        audio_rx: &mut Receiver<Vec<i16>>,
        stop: &Arc<AtomicBool>,
        asr: &WhisperEngine,
        matcher: &GlossaryMatcher,
        translation_tx: &Sender<TranslationJob>,
    ) -> Result<(), String> {
        let mut segmenter = SpeechSegmenter::default();
        let mut committed_until_ms = -1i64;
        let mut last_source = String::new();
        let mut turn = 0u64;
        let mut pending_group: Option<PendingGroup> = None;
        let mut catching_up = false;
        let mut hop = HOP;
        loop {
            let chunk = if pending_group.is_some() {
                match audio_rx.try_recv() {
                    Ok(samples) => Some(samples),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                        if pending_group.as_ref().is_some_and(|group| group.expired(Instant::now())) {
                            Self::flush_group(app, translation_tx, &mut pending_group)?;
                        } else {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        continue;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => None,
                }
            } else {
                audio_rx.blocking_recv()
            };
            if let Some(samples) = chunk {
                hop = hop_for_queue(audio_rx.len(), live_capture, &mut catching_up);
                for window in segmenter.push(&samples, hop) {
                    if pending_group.is_some() {
                        Self::flush_group(app, translation_tx, &mut pending_group)?;
                    }
                    Self::process_window(
                        app,
                        settings,
                        asr,
                        matcher,
                        translation_tx,
                        window,
                        hop,
                        false,
                        &mut committed_until_ms,
                        &mut last_source,
                        &mut turn,
                        &mut pending_group,
                        audio_rx.len(),
                    )?;
                }
                if pending_group.as_ref().is_some_and(|group| group.expired(Instant::now())) {
                    Self::flush_group(app, translation_tx, &mut pending_group)?;
                }
            } else {
                break;
            }
            if stop.load(Ordering::Relaxed) && audio_rx.is_closed() {
                break;
            }
        }
        if let Some(window) = segmenter.flush() {
            if pending_group.is_some() {
                Self::flush_group(app, translation_tx, &mut pending_group)?;
            }
            Self::process_window(
                app,
                settings,
                asr,
                matcher,
                translation_tx,
                window,
                hop,
                true,
                &mut committed_until_ms,
                &mut last_source,
                &mut turn,
                &mut pending_group,
                audio_rx.len(),
            )?;
        }
        Self::flush_group(app, translation_tx, &mut pending_group)?;
        Ok(())
    }

    fn flush_group(
        app: &AppHandle,
        translation_tx: &Sender<TranslationJob>,
        pending: &mut Option<PendingGroup>,
    ) -> Result<(), String> {
        let Some(group) = pending.take() else { return Ok(()) };
        realtime::emit(app, Event {
            kind: "source",
            text: group.display_text(),
            id: group.input.clone(),
            done: true,
            ..Default::default()
        });
        let _ = app.emit(realtime::RAW_EVENT, serde_json::json!({
            "type": "local.translation_group",
            "id": &group.id,
            "parts": group.parts.len(),
            "startMs": group.parts.first().map(|part| part.start_ms),
            "endMs": group.parts.last().map(|part| part.end_ms),
            "waitMs": group.started.elapsed().as_millis(),
        }));
        let text = group.translation_text();
        translation_tx.send(TranslationJob {
            id: group.id,
            text,
            hints: group.hints,
            queued_at: Instant::now(),
        }).map_err(|_| "Translation worker stopped".to_string())
    }

    fn process_window(
        app: &AppHandle,
        settings: &Settings,
        asr: &WhisperEngine,
        matcher: &GlossaryMatcher,
        translation_tx: &Sender<TranslationJob>,
        window: TranscriptionWindow,
        hop: usize,
        force: bool,
        committed_until_ms: &mut i64,
        last_source: &mut String,
        turn: &mut u64,
        pending_group: &mut Option<PendingGroup>,
        audio_queue_len: usize,
    ) -> Result<(), String> {
        let lang = (settings.source_lang != "auto").then_some(settings.source_lang.as_str());
        let started = Instant::now();
        let clip_after_ms = (*committed_until_ms >= 0).then_some(*committed_until_ms);
        let segments = asr.transcribe_samples(&window.samples, lang, window.offset_ms, clip_after_ms)?;
        let asr_elapsed_ms = started.elapsed().as_millis();
        let raw_segments = segments.len();
        let tail = &window.samples[window.samples.len().saturating_sub(HOP)..];
        let tail_rms = (tail.iter().map(|sample| sample * sample).sum::<f32>()
            / tail.len().max(1) as f32)
            .sqrt();
        let tail_rms_dbfs = (20.0 * tail_rms.max(1e-6).log10()).round() as i32;
        let window_end_ms = window.offset_ms + (window.samples.len() * 1000 / RATE) as i64;
        let stable_before_ms = if force {
            i64::MAX
        } else {
            window_end_ms - 2_000
        };
        let deferred_unstable = segments.iter()
            .filter(|segment| segment.end_ms > stable_before_ms)
            .count();
        let deferred_overlap = segments.iter()
            .filter(|segment| segment.end_ms > *committed_until_ms
                && segment.start_ms < committed_until_ms.saturating_sub(TIMESTAMP_JITTER_MS))
            .count();
        let eligible: Vec<_> = segments.into_iter()
            .filter(|segment| committable(segment, *committed_until_ms, stable_before_ms))
            .collect();
        let eligible_count = eligible.len();
        let candidates = collapse_repeated_asr_segments(eligible);
        let repeat_suppressed = eligible_count - candidates.len();
        let mut committed_segments = 0;
        for segment in candidates {
            if !committable(&segment, *committed_until_ms, stable_before_ms) {
                continue;
            }
            let raw_text = if segment.start_ms < *committed_until_ms {
                trim_repeated_prefix(last_source, &segment.text)
            } else {
                segment.text.clone()
            };
            if raw_text.trim().is_empty() {
                continue;
            }
            *committed_until_ms = segment.end_ms;
            *last_source = raw_text.clone();
            committed_segments += 1;
            let matched = matcher.apply(raw_text.trim());
            if pending_group.as_ref().is_some_and(|group| group.expired(Instant::now())) {
                Self::flush_group(app, translation_tx, pending_group)?;
            }
            if pending_group.is_none() {
                *turn += 1;
                let id = format!("local-{turn}");
                let group = PendingGroup::new(id.clone(), Instant::now());
                realtime::emit(app, Event {
                    kind: "link",
                    text: group.input.clone(),
                    id,
                    done: true,
                    ..Default::default()
                });
                *pending_group = Some(group);
            }
            let group = pending_group.as_mut().unwrap();
            let start_ms = segment.start_ms;
            let end_ms = segment.end_ms;
            group.push(
                SubtitleSegment { start_ms, end_ms, text: matched.text },
                matched.hints,
            );
            realtime::emit(app, Event {
                kind: "source",
                text: group.display_text(),
                lang: if settings.source_lang == "auto" {
                    String::new()
                } else {
                    settings.source_lang.clone()
                },
                id: group.input.clone(),
                start_ms: Some(start_ms),
                end_ms: Some(end_ms),
                done: false,
                ..Default::default()
            });
            if group.ready(Instant::now()) {
                Self::flush_group(app, translation_tx, pending_group)?;
            }
        }
        let _ = app.emit(realtime::RAW_EVENT, serde_json::json!({
            "type": "local.asr",
            "windowStartMs": window.offset_ms,
            "windowMs": window.samples.len() * 1000 / RATE,
            "tailRmsDbfs": tail_rms_dbfs,
            "targetHopMs": hop * 1000 / RATE,
            "elapsedMs": asr_elapsed_ms,
            "segments": raw_segments,
            "eligibleSegments": eligible_count,
            "committedSegments": committed_segments,
            "committedUntilMs": *committed_until_ms,
            "deferredUnstable": deferred_unstable,
            "deferredOverlap": deferred_overlap,
            "repeatSuppressed": repeat_suppressed,
            "audioQueueChunks": audio_queue_len,
        }));
        Ok(())
    }

    fn translate_loop(
        app: &AppHandle,
        settings: &Settings,
        target: &str,
        translator: &mut LocalTranslator,
        matcher: &GlossaryMatcher,
        jobs: std::sync::mpsc::Receiver<TranslationJob>,
    ) -> Result<(), String> {
        let mut context = VecDeque::with_capacity(2);
        for job in jobs {
            let queue_wait_ms = job.queued_at.elapsed().as_millis();
            let context_text = context.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
            let possible = matcher.possible_terms(&job.text, &context_text).into_iter()
                .filter(|term| !job.hints.contains(term)).collect::<Vec<_>>();
            let started = Instant::now();
            let result = translator.translate(&job.text, target, &context_text, settings.target_lang == "zh", &job.hints, &possible);
            let _ = app.emit(realtime::RAW_EVENT, serde_json::json!({
                "type": "local.translation",
                "id": &job.id,
                "queueWaitMs": queue_wait_ms,
                "elapsedMs": started.elapsed().as_millis(),
                "ok": result.is_ok(),
            }));
            realtime::emit(app, Event {
                kind: "target",
                text: result?,
                id: job.id,
                done: true,
                ..Default::default()
            });
            if context.len() == 2 {
                context.pop_front();
            }
            context.push_back(job.text);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{collapse_repeated_asr_segments, committable, hop_for_queue, trim_repeated_prefix, PendingGroup, SpeechSegmenter, SubtitleSegment, HOP, RATE};
    use crate::{config::GlossaryEntry, local::glossary::GlossaryMatcher};
    use std::time::{Duration, Instant};

    #[test]
    fn translation_group_keeps_source_fragments_and_translates_one_phrase() {
        let now = Instant::now();
        let mut group = PendingGroup::new("local-1".into(), now);
        for (index, text) in ["so that's where I went to school", "for MIT", "for bachelors, masters and PhD"]
            .into_iter().enumerate()
        {
            let hints = if index == 1 {
                vec![("MIT".into(), "麻省理工學院".into())]
            } else {
                Vec::new()
            };
            group.push(SubtitleSegment {
                start_ms: index as i64 * 1_000,
                end_ms: (index as i64 + 1) * 1_000,
                text: text.into(),
            }, hints);
        }
        assert_eq!(group.display_text(), "so that's where I went to school\nfor MIT\nfor bachelors, masters and PhD");
        assert_eq!(group.translation_text(), "so that's where I went to school for MIT for bachelors, masters and PhD");
        assert_eq!(group.parts[1].start_ms, 1_000);
        assert_eq!(group.hints, vec![("MIT".into(), "麻省理工學院".into())]);
        assert!(group.ready(now));
    }

    #[test]
    fn translation_group_flushes_on_sentence_or_two_second_timeout() {
        let now = Instant::now();
        let mut group = PendingGroup::new("local-1".into(), now);
        group.push(SubtitleSegment { start_ms: 0, end_ms: 1_000, text: "for MIT".into() }, Vec::new());
        assert!(!group.ready(now + Duration::from_millis(1_999)));
        assert!(group.ready(now + Duration::from_secs(2)));
        group.push(SubtitleSegment { start_ms: 1_000, end_ms: 2_000, text: "for a PhD.".into() }, Vec::new());
        assert!(group.ready(now));
    }

    #[test]
    fn raw_overlap_dedup_precedes_glossary_correction() {
        let matcher = GlossaryMatcher::new(&[GlossaryEntry {
            source: "LoRA".into(), aliases: vec!["Loari".into()],
            target: "低秩适配".into(), target_lang: "zh".into(),
        }], "zh").unwrap();
        let raw = trim_repeated_prefix("about Loari", "about Loari and GGUF");
        assert_eq!(raw, "and GGUF");
        let corrected = matcher.apply("Loari and GGUF");
        assert_eq!(corrected.text, "LoRA and GGUF");
        assert_eq!(corrected.hints.len(), 1);
    }

    #[test]
    fn repeated_prefix_is_trimmed_without_losing_new_words() {
        assert_eq!(trim_repeated_prefix("optimizations.", "optimizations. Tested CUDA presets"), "Tested CUDA presets");
        assert_eq!(trim_repeated_prefix("into GGUF packages.", "GGUF packages."), "");
        assert_eq!(trim_repeated_prefix("前面的人工智能", "人工智能技術正在發展"), "技術正在發展");
        assert_eq!(trim_repeated_prefix("previous sentence", "new sentence"), "new sentence");
    }

    #[test]
    fn overlap_does_not_reemit_short_committed_words() {
        assert_eq!(
            trim_repeated_prefix("for bachelors, masters and PhD", "PhD."),
            ""
        );
        assert_eq!(
            trim_repeated_prefix("what do you want to be when you grow up?", "up."),
            ""
        );
        assert_eq!(trim_repeated_prefix("And", "And we made a big bet"), "we made a big bet");
        assert_eq!(trim_repeated_prefix("人工", "工業"), "工業");
    }

    #[test]
    fn timestamp_jitter_does_not_discard_a_new_sentence() {
        let sentence = SubtitleSegment { start_ms: 16_000, end_ms: 19_000, text: "Performance modes".into() };
        assert!(committable(&sentence, 16_400, 24_000));
        let mostly_repeated = SubtitleSegment { start_ms: 4_000, end_ms: 9_600, text: "old text".into() };
        assert!(!committable(&mostly_repeated, 9_260, 18_000));
        let unfinished = SubtitleSegment { start_ms: 20_000, end_ms: 24_000, text: "unfinished".into() };
        assert!(!committable(&unfinished, 19_000, 22_000));
    }

    #[test]
    fn silent_tail_does_not_create_a_final_asr_window() {
        let mut segmenter = SpeechSegmenter::default();
        segmenter.push(&vec![80; RATE * 10], HOP);
        assert!(segmenter.flush().is_none());
        let mut audible = SpeechSegmenter::default();
        audible.push(&vec![300; RATE * 10], HOP);
        assert!(audible.flush().is_some());
        let mut brief_voice = vec![80; RATE * 10];
        brief_voice[RATE * 4..RATE * 4 + RATE / 5].fill(300);
        let mut quiet_with_voice = SpeechSegmenter::default();
        quiet_with_voice.push(&brief_voice, HOP);
        assert!(quiet_with_voice.flush().is_some());
    }

    #[test]
    fn live_capture_uses_a_longer_step_until_the_queue_drains() {
        let mut catching_up = false;
        assert_eq!(hop_for_queue(0, true, &mut catching_up), HOP);
        assert_eq!(hop_for_queue(20, true, &mut catching_up), HOP * 2);
        assert_eq!(hop_for_queue(8, true, &mut catching_up), HOP * 2);
        assert_eq!(hop_for_queue(5, true, &mut catching_up), HOP);
        assert_eq!(hop_for_queue(200, false, &mut catching_up), HOP);
    }

    #[test]
    fn changing_steps_reduces_repeated_windows_without_gaps() {
        let mut segmenter = SpeechSegmenter::default();
        let mut windows = Vec::new();
        for second in 0..40 {
            let hop = if second < 20 { HOP } else { HOP * 2 };
            windows.extend(segmenter.push(&vec![1_000; RATE], hop));
        }
        windows.push(segmenter.flush().unwrap());
        assert!(windows.len() < 21);
        assert!(windows.windows(2).all(|pair| {
            pair[1].offset_ms <= pair[0].offset_ms + (pair[0].samples.len() * 1000 / RATE) as i64
        }));
        let last = windows.last().unwrap();
        assert_eq!(last.offset_ms + (last.samples.len() * 1000 / RATE) as i64, 40_000);
    }

    #[test]
    fn one_asr_window_does_not_emit_a_long_repetition_loop() {
        let texts = [
            "I was a student in the U.S.",
            "I was a student in the U.S.",
            "and I was a student in the U.S.",
            "and I was a student in the U.S.",
            "So when I was a young student, that is what I studied.",
        ];
        let segments = texts.iter().enumerate().map(|(i, text)| SubtitleSegment {
            start_ms: (i * 2_000) as i64,
            end_ms: ((i + 1) * 2_000) as i64,
            text: (*text).into(),
        }).collect();
        let kept = collapse_repeated_asr_segments(segments);
        assert_eq!(kept.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), vec![texts[0], texts[4]]);
    }

    #[test]
    fn three_repeats_and_short_callouts_remain_visible() {
        let texts = ["I was a student in the U.S.", "I was a student in the U.S.", "I was a student in the U.S.", "AMD", "AMD", "AMD"];
        let segments = texts.iter().enumerate().map(|(i, text)| SubtitleSegment {
            start_ms: (i * 1_000) as i64,
            end_ms: ((i + 1) * 1_000) as i64,
            text: (*text).into(),
        }).collect();
        let kept = collapse_repeated_asr_segments(segments);
        assert_eq!(kept.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), texts);
    }

    #[test]
    fn a_different_leading_word_does_not_turn_a_phrase_into_a_duplicate() {
        let texts = [
            "high performance computing",
            "very high performance computing",
            "high performance computing",
        ];
        let segments = texts.iter().enumerate().map(|(i, text)| SubtitleSegment {
            start_ms: (i * 1_000) as i64,
            end_ms: ((i + 1) * 1_000) as i64,
            text: (*text).into(),
        }).collect();
        let kept = collapse_repeated_asr_segments(segments);
        assert_eq!(kept.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), texts);
    }
}
