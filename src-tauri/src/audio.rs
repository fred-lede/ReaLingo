//! Live capture: microphones, and system audio where the platform allows it.
//!
//! "Record what the speakers play" is the same cpal call everywhere — `build_input_stream`
//! on an *output* device — but the backends get there differently:
//!
//! - **Windows / WASAPI**: cpal adds `AUDCLNT_STREAMFLAGS_LOOPBACK` when the endpoint is a
//!   render device. Works on any supported Windows version.
//! - **macOS / CoreAudio**: for a device with no input, cpal creates a Core Audio *process
//!   tap* plus a private aggregate device. Needs macOS 14.4+, and the app bundle must carry
//!   `NSAudioCaptureUsageDescription` (see `Info.plist`) — without it TCC denies access
//!   *silently*, handing back perfectly valid buffers full of zeros. cpal 0.17's own tap
//!   never starts (also silence), so we build the tap ourselves: [`crate::tap`].
//! - **Linux / ALSA**: there is no loopback flag, and the monitor sources that would serve
//!   the purpose belong to PulseAudio/PipeWire, which ALSA does not enumerate. cpal
//!   therefore cannot reach them at all, so system audio here does not go through cpal: see
//!   [`crate::pulse`], which asks the sound server directly. Only when no sound server
//!   answers does the old story hold — no system-audio entries, route it in pavucontrol.
//!
//! When we speak the translation ourselves, system audio must leave our own voice out or it
//! gets translated again. `exclude_self` asks for that: Windows switches to process loopback
//! ([`crate::wasapi`]), macOS to a tap that lists our process as excluded ([`crate::tap`]).
//! Linux has no such filter, so the frontend does not offer speaking with system audio there.

use anyhow::{anyhow, Result};
use serde::Serialize;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::path::PathBuf;
use tokio::sync::mpsc::Sender;

use crate::resample::{chunk_samples, downmix, Resampler};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub loopback: bool,
}

const MIC: &str = "mic";
const SYS: &str = "sys";

/// Whether this machine can capture what the speakers are playing. See the module docs.
///
/// A function, not a constant, because on Linux the answer is a runtime one: it depends on
/// a sound server being up, which can change while the app is open. Elsewhere it folds to a
/// compile-time fact.
pub fn loopback_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::pulse::available()
    }
    #[cfg(not(target_os = "linux"))]
    {
        cfg!(any(target_os = "windows", target_os = "macos"))
    }
}
/// `DeviceId`'s own Display already contains ':', so separate our prefix with something else.
const SEP: char = '|';

/// cpal's `DeviceId` is stable across reboots and reconnects; the display name is not
/// (two identical headsets report the same name), so match on the id and show the name.
fn describe(d: &cpal::Device, kind: &str) -> Option<DeviceInfo> {
    let id = d.id().ok()?.to_string();
    let name = d.description().ok()?.name().to_string();
    Some(DeviceInfo { id: format!("{kind}{SEP}{id}"), name, loopback: kind == SYS })
}

pub fn list_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let mut out = Vec::new();

    if let Ok(devices) = host.input_devices() {
        out.extend(devices.filter_map(|d| describe(&d, MIC)));
    }
    // Listing output devices where loopback is impossible would only offer the user streams
    // that fail to build.
    #[cfg(target_os = "linux")]
    out.extend(crate::pulse::monitors().into_iter().map(|m| DeviceInfo {
        id: format!("{SYS}{SEP}{}", m.name),
        name: m.label,
        loopback: true,
    }));
    #[cfg(not(target_os = "linux"))]
    if loopback_supported() {
        if let Ok(devices) = host.output_devices() {
            out.extend(devices.filter_map(|d| describe(&d, SYS)));
        }
    }
    out
}

/// Best guess for the initial UI selection: default mic, else default output (loopback).
pub fn default_device_id(loopback: bool) -> Option<String> {
    let host = cpal::default_host();
    if !loopback {
        return host.default_input_device().and_then(|d| describe(&d, MIC)).map(|d| d.id);
    }

    #[cfg(target_os = "linux")]
    {
        // Fall back to the first monitor: `pactl get-default-sink` can name a sink whose
        // monitor is not actually offered (a just-removed device), and an id that matches
        // nothing leaves the picker looking empty.
        let all = crate::pulse::monitors();
        let preferred = crate::pulse::default_monitor()
            .filter(|n| all.iter().any(|m| &m.name == n))
            .or_else(|| all.first().map(|m| m.name.clone()))?;
        Some(format!("{SYS}{SEP}{preferred}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        if !loopback_supported() {
            return None;
        }
        host.default_output_device().and_then(|d| describe(&d, SYS)).map(|d| d.id)
    }
}

fn find(id: &str) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let host = cpal::default_host();
    let (prefix, target) = id
        .split_once(SEP)
        .ok_or_else(|| anyhow!("bad device id: {id}"))?;
    let loopback = prefix == SYS;

    let mut devices: Box<dyn Iterator<Item = cpal::Device>> = if loopback {
        Box::new(host.output_devices()?)
    } else {
        Box::new(host.input_devices()?)
    };
    let device = devices
        .find(|d| d.id().map(|i| i.to_string() == target).unwrap_or(false))
        .ok_or_else(|| anyhow!("audio device is no longer available"))?;

    // A render endpoint has no *input* config; its mix format is the output config, and that
    // is exactly what the loopback stream delivers.
    let config = if loopback { device.default_output_config()? } else { device.default_input_config()? };
    Ok((device, config))
}

/// Starts capture on its own thread (cpal streams are `!Send` on Windows) and returns once
/// the stream is actually running, so the caller learns about device errors immediately.
pub fn start(
    id: &str,
    rate: u32,
    tx: Sender<Vec<i16>>,
    level: InputLevel,
    dropped: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    exclude_self: bool,
    recording_path: Option<PathBuf>,
    on_recording: impl FnOnce(Result<PathBuf>) + Send + 'static,
) -> Result<Option<std::thread::JoinHandle<()>>> {
    // Linux system audio never reaches cpal — it is a `parec` pipe, which owns its own
    // thread and reports failure synchronously already.
    #[cfg(target_os = "linux")]
    if let Some(source) = id.strip_prefix(SYS).and_then(|r| r.strip_prefix(SEP)) {
        let _ = dropped;
        let _ = (recording_path, on_recording);
        crate::pulse::start(source, rate, tx, level, stop)?;
        return Ok(None);
    }

    #[cfg(windows)]
    if exclude_self && is_default_output(id) {
        let pipe = Pipe::new(crate::wasapi::RATE, crate::wasapi::CHANNELS as usize, rate, tx, level, dropped);
        return crate::wasapi::start(pipe, stop, recording_path, on_recording).map(Some);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = exclude_self;

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let id = id.to_string();

    let handle = std::thread::spawn(move || {
        // macOS: system audio always goes through our own tap — cpal 0.17's loopback leaves
        // tap auto-start off and records silence. The tap has to outlive the stream.
        #[cfg(target_os = "macos")]
        let (_tap, id) = match id
            .strip_prefix(SYS)
            .and_then(|r| r.strip_prefix(SEP))
            .map(|output| crate::tap::Tap::new(output, exclude_self))
        {
            None => (None, id),
            Some(Ok(tap)) => {
                let id = format!("{MIC}{SEP}{}", tap.device_id);
                (Some(tap), id)
            }
            Some(Err(e)) => {
                let _ = ready_tx.send(Err(e.to_string()));
                return;
            }
        };
        let (stream, finish) = match build(&id, rate, tx, level, dropped, recording_path) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(e.to_string()));
                return;
            }
        };
        if let Err(e) = stream.play() {
            drop(stream);
            if let Some(finish) = finish { finish.abort(); }
            let _ = ready_tx.send(Err(e.to_string()));
            return;
        }
        let _ = ready_tx.send(Ok(()));
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(60));
        }
        drop(stream);
        if let Some(finish) = finish { on_recording(finish.complete()); }
    });

    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| anyhow!("audio device did not start within 10s"))?
        .map_err(|e| anyhow!(e))?;
    Ok(Some(handle))
}

/// Whether `id` is the system-audio entry for the default output — the device we play on.
#[cfg(windows)]
fn is_default_output(id: &str) -> bool {
    let Some(target) = id.strip_prefix(SYS).and_then(|r| r.strip_prefix(SEP)) else {
        return false;
    };
    cpal::default_host()
        .default_output_device()
        .and_then(|d| d.id().ok())
        .is_some_and(|d| d.to_string() == target)
}

fn build(id: &str, rate: u32, tx: Sender<Vec<i16>>, level: InputLevel, dropped: Arc<AtomicU64>, recording_path: Option<PathBuf>) -> Result<(cpal::Stream, Option<crate::recording::Finish>)> {
    let (device, supported) = find(id)?;
    let sample_format = supported.sample_format();
    let channels = supported.channels() as usize;
    let in_rate = supported.sample_rate();
    let config: cpal::StreamConfig = supported.into();

    let mut pipe = Pipe::new(in_rate, channels, rate, tx, level, dropped);
    let (mut recording, finish) = if let Some(path) = recording_path {
        let (tap, finish) = crate::recording::start(path, in_rate, channels as u16)?;
        (Some(tap), Some(finish))
    } else { (None, None) };
    let sink = move |frames: &[f32]| {
        if let Some(tap) = &mut recording { tap.feed(frames); }
        pipe.feed(frames);
    };

    let stream = match sample_format {
        SampleFormat::F32 => typed::<f32>(&device, &config, sink),
        SampleFormat::I16 => typed::<i16>(&device, &config, sink),
        SampleFormat::U16 => typed::<u16>(&device, &config, sink),
        SampleFormat::I32 => typed::<i32>(&device, &config, sink),
        SampleFormat::I8 => typed::<i8>(&device, &config, sink),
        other => Err(anyhow!("unsupported sample format: {other}")),
    };
    match stream {
        Ok(stream) => Ok((stream, finish)),
        Err(error) => {
            if let Some(finish) = finish { finish.abort(); }
            Err(error)
        }
    }
}

fn typed<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut sink: impl FnMut(&[f32]) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let mut scratch: Vec<f32> = Vec::new();
    let stream = device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            scratch.clear();
            scratch.extend(data.iter().map(|&s| f32::from_sample(s)));
            sink(&scratch);
        },
        |e| eprintln!("[audio] stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

/// Interleaved device frames -> mono -> model rate -> 100 ms chunks on the channel.
/// The input level in two forms: the peak, which the meter shows, and a smoothed RMS,
/// which says whether the input is loud enough to be heard at all.
///
/// The peak says how loud the loudest moment was; the RMS says how loud the input
/// actually is. A microphone at a low gain has a peak all over the place and an RMS
/// far below what a remote service needs — that is the one worth warning about.
#[derive(Clone, Default)]
pub struct InputLevel {
    peak: Arc<AtomicU32>,
    rms: Arc<AtomicU32>,
}

impl InputLevel {
    pub fn new() -> Self {
        Self::default()
    }

    /// The peak of the latest chunk, in 0..1.
    pub fn store_peak(&self, peak: f32) {
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
    }

    /// The level of the input, smoothed so the meter and the warning do not flicker.
    /// The smoothing is over a couple of seconds, so the silence between words does not
    /// read as a muted microphone.
    pub fn store_rms(&self, rms: f32) {
        let smoothed = self.rms() * 0.95 + rms * 0.05;
        self.rms.store(smoothed.to_bits(), Ordering::Relaxed);
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    pub fn rms(&self) -> f32 {
        f32::from_bits(self.rms.load(Ordering::Relaxed))
    }

    pub fn reset(&self) {
        self.store_peak(0.0);
        self.store_rms(0.0);
    }
}

pub(crate) struct Pipe {
    resampler: Resampler,
    mono: Vec<f32>,
    pcm: Vec<i16>,
    chunk: usize,
    channels: usize,
    tx: Sender<Vec<i16>>,
    level: InputLevel,
    dropped: Arc<AtomicU64>,
}

impl Pipe {
    pub(crate) fn new(in_rate: u32, channels: usize, out_rate: u32, tx: Sender<Vec<i16>>, level: InputLevel, dropped: Arc<AtomicU64>) -> Self {
        Self {
            resampler: Resampler::new(in_rate, out_rate),
            mono: Vec::new(),
            pcm: Vec::new(),
            chunk: chunk_samples(out_rate),
            channels,
            tx,
            level,
            dropped,
        }
    }

    pub(crate) fn feed(&mut self, frames: &[f32]) {
        self.mono.clear();
        downmix(frames, self.channels, &mut self.mono);

        let peak = self.mono.iter().fold(0f32, |m, s| m.max(s.abs()));
        self.level.store_peak(peak);

        self.resampler.push(&self.mono, &mut self.pcm);
        while self.pcm.len() >= self.chunk {
            let chunk: Vec<i16> = self.pcm.drain(..self.chunk).collect();
            // The level of what is actually sent, not of the loudest instant in it: this
            // is the audio the service will hear.
            let mean = chunk.iter().map(|&s| (s as f32 / 32768.0).powi(2)).sum::<f32>()
                / chunk.len().max(1) as f32;
            self.level.store_rms(mean.sqrt());
            // Never block the audio thread: if the uplink is backed up, drop the chunk.
            if let Err(error) = self.tx.try_send(chunk) {
                if matches!(error, tokio::sync::mpsc::error::TrySendError::Full(_)) {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
        }
    }
}

/// The peak and the smoothed RMS of the input, for the meter and the low-level warning.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputLevels {
    pub peak: f32,
    pub rms: f32,
}

pub fn read_levels(level: &InputLevel) -> InputLevels {
    InputLevels { peak: level.peak(), rms: level.rms() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_audio_channel_counts_dropped_chunks() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut pipe = Pipe::new(16_000, 1, 16_000, tx, InputLevel::new(), dropped.clone());
        pipe.feed(&vec![0.25; 3_200]);
        assert_eq!(rx.try_recv().unwrap().len(), 1_600);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn the_smoothed_rms_holds_through_a_pause() {
        // A half second of silence between words is ordinary speech, not a mute: the
        // reading has to survive it, or the warning flickers between every sentence.
        let level = InputLevel::new();
        for _ in 0..100 {
            level.store_rms(0.01);
        }
        for _ in 0..5 {
            level.store_rms(0.0);
        }
        assert!(level.rms() > 0.005, "a pause must not empty the reading: {}", level.rms());
    }

    #[test]
    fn the_rms_is_measured_on_the_audio_that_is_sent() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let level = InputLevel::new();
        let mut pipe = Pipe::new(16_000, 1, 16_000, tx, level.clone(), Arc::new(AtomicU64::new(0)));
        // A quarter of full scale, for long enough for the smoothed reading to settle.
        let amplitude = 0.25_f32;
        for _ in 0..200 {
            pipe.feed(&vec![amplitude; 1_600]);
        }
        let sent = rx.try_recv().expect("a chunk should have been sent");
        let sent_rms = (sent.iter().map(|&s| (s as f32 / 32768.0).powi(2)).sum::<f32>() / sent.len() as f32).sqrt();
        assert!((level.rms() - sent_rms).abs() < 0.01, "level {} vs sent {}", level.rms(), sent_rms);
    }

    #[test]
    fn the_smoothed_rms_settles_on_the_input_level() {
        let level = InputLevel::new();
        assert_eq!(level.rms(), 0.0);
        level.store_rms(0.1);
        assert!(level.rms() > 0.0 && level.rms() < 0.1, "the first reading is smoothed, not taken raw");
        for _ in 0..100 {
            level.store_rms(0.1);
        }
        assert!((level.rms() - 0.1).abs() < 0.01, "the smoothed rms settles on the input level");
    }

    #[test]
    fn the_peak_is_reported_as_it_is() {
        let level = InputLevel::new();
        level.store_peak(0.42);
        assert!((level.peak() - 0.42).abs() < 0.0001);
    }

    #[test]
    fn a_quiet_input_reports_a_quiet_rms() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let level = InputLevel::new();
        let mut pipe = Pipe::new(16_000, 1, 16_000, tx, level.clone(), Arc::new(AtomicU64::new(0)));
        // A microphone at a low gain: about 0.0067 rms, well under what a remote
        // service needs to hear it.
        let amplitude = 0.0067_f32;
        for _ in 0..20 {
            pipe.feed(&vec![amplitude; 1_600]);
        }
        assert!(level.rms() > 0.004 && level.rms() < 0.01, "rms was {}", level.rms());
    }
}
