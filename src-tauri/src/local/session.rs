use super::LocalModels;
use crate::{
    config::Settings,
    realtime::{self, Event},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::AppHandle;
use tokio::sync::mpsc::Receiver;

const RATE: usize = 16_000;
const HOP: usize = RATE * 2;
const MAX_PENDING: usize = RATE * 16;

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
    pub fn push(&mut self, samples: &[i16]) -> Vec<TranscriptionWindow> {
        let mut windows = Vec::new();
        for sample in samples {
            self.pending.push(*sample);
            self.since_window += 1;
            if self.pending.len() >= MAX_PENDING {
                windows.push(self.window());
                self.advance();
            } else if self.since_window >= HOP {
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
    fn advance(&mut self) {
        let drain = HOP.min(self.pending.len());
        self.pending.drain(..drain);
        self.consumed += drain;
        self.since_window = 0;
    }
}

pub struct LocalSession;

impl LocalSession {
    pub fn run(
        app: &AppHandle,
        settings: &Settings,
        target_language_name: &str,
        mut audio_rx: Receiver<Vec<i16>>,
        stop: &Arc<AtomicBool>,
        models: &mut LocalModels,
    ) -> Result<(), String> {
        realtime::note(app, "status", "connecting");
        realtime::note(app, "status", "connected");
        let mut segmenter = SpeechSegmenter::default();
        let mut committed_until_ms = -1i64;
        let mut turn = 0u64;
        loop {
            let chunk = audio_rx.blocking_recv();
            if let Some(samples) = chunk {
                for window in segmenter.push(&samples) {
                    Self::process_window(
                        app,
                        settings,
                        target_language_name,
                        models,
                        window,
                        false,
                        &mut committed_until_ms,
                        &mut turn,
                    )?;
                }
            } else {
                break;
            }
            if stop.load(Ordering::Relaxed) && audio_rx.is_closed() {
                break;
            }
        }
        if let Some(window) = segmenter.flush() {
            Self::process_window(
                app,
                settings,
                target_language_name,
                models,
                window,
                true,
                &mut committed_until_ms,
                &mut turn,
            )?;
        }
        realtime::note(app, "status", "closed");
        Ok(())
    }

    fn process_window(
        app: &AppHandle,
        settings: &Settings,
        target: &str,
        models: &mut LocalModels,
        window: TranscriptionWindow,
        force: bool,
        committed_until_ms: &mut i64,
        turn: &mut u64,
    ) -> Result<(), String> {
        let lang = (settings.source_lang != "auto").then_some(settings.source_lang.as_str());
        let segments = models
            .asr
            .transcribe_samples(&window.samples, lang, window.offset_ms)?;
        let window_end_ms = window.offset_ms + (window.samples.len() * 1000 / RATE) as i64;
        let stable_before_ms = if force {
            i64::MAX
        } else {
            window_end_ms - 2_000
        };
        for segment in segments {
            if segment.end_ms <= *committed_until_ms
                || segment.end_ms > stable_before_ms
                || segment.start_ms < *committed_until_ms
            {
                continue;
            }
            if segment.text.trim().is_empty() {
                continue;
            }
            *committed_until_ms = segment.end_ms;
            *turn += 1;
            let id = format!("local-{turn}");
            let input = format!("{id}-input");
            realtime::emit(
                app,
                Event {
                    kind: "link",
                    text: input.clone(),
                    id: id.clone(),
                    done: true,
                    ..Default::default()
                },
            );
            realtime::emit(
                app,
                Event {
                    kind: "source",
                    text: segment.text.clone(),
                    lang: if settings.source_lang == "auto" {
                        String::new()
                    } else {
                        settings.source_lang.clone()
                    },
                    id: input,
                    done: true,
                    ..Default::default()
                },
            );
            let translated = models.translator.translate(segment.text.trim(), target)?;
            realtime::emit(
                app,
                Event {
                    kind: "target",
                    text: translated,
                    id,
                    done: true,
                    ..Default::default()
                },
            );
        }
        Ok(())
    }
}
