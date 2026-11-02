//! Optional microphone capture, written independently of the ASR queue.
use anyhow::{anyhow, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use rusty_mp3::{Error as Mp3Error, Mp3Encoder, Mp3EncoderConfig};
use crate::resample::Resampler;

pub struct Tap {
    tx: mpsc::SyncSender<Vec<i16>>,
    overflowed: Arc<AtomicBool>,
}

impl Tap {
    pub fn feed(&mut self, frames: &[f32]) {
        if self.overflowed.load(Ordering::Relaxed) {
            return;
        }
        let samples = frames
            .iter()
            .map(|sample| {
                (sample.clamp(-1.0, 1.0) * 32768.0)
                    .round()
                    .clamp(i16::MIN as f32, i16::MAX as f32) as i16
            })
            .collect();
        if self.tx.try_send(samples).is_err() {
            self.overflowed.store(true, Ordering::Relaxed);
        }
    }
}

pub struct Finish {
    worker: JoinHandle<Result<()>>,
    overflowed: Arc<AtomicBool>,
    path: PathBuf,
    partial: PathBuf,
}

impl Finish {
    pub fn complete(self) -> Result<PathBuf> {
        let result = self
            .worker
            .join()
            .unwrap_or_else(|_| Err(anyhow!("recording writer stopped unexpectedly")));
        if let Err(error) = result {
            let _ = fs::remove_file(&self.partial);
            return Err(error);
        }
        if self.overflowed.load(Ordering::Relaxed) {
            let _ = fs::remove_file(&self.partial);
            return Err(anyhow!("recording could not keep up with microphone input"));
        }
        if let Err(error) = fs::rename(&self.partial, &self.path) {
            let _ = fs::remove_file(&self.partial);
            return Err(error.into());
        }
        Ok(self.path)
    }

    pub fn abort(self) {
        let _ = self.worker.join();
        let _ = fs::remove_file(self.partial);
    }
}

pub fn start(path: PathBuf, rate: u32, channels: u16) -> Result<(Tap, Finish)> {
    if rate == 0 || channels == 0 {
        return Err(anyhow!("invalid microphone audio format"));
    }
    let mp3 = path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("mp3"));
    let partial = path.with_extension(if mp3 { "mp3.part" } else { "wav.part" });
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)?;
    let (tx, rx) = mpsc::sync_channel(256);
    let overflowed = Arc::new(AtomicBool::new(false));
    let worker = std::thread::spawn(move || {
        if mp3 { write_mp3(file, rx, rate, channels) } else { write_wav(file, rx, rate, channels) }
    });
    Ok((
        Tap {
            tx,
            overflowed: overflowed.clone(),
        },
        Finish {
            worker,
            overflowed,
            path,
            partial,
        },
    ))
}

fn write_mp3(mut file: File, rx: mpsc::Receiver<Vec<i16>>, rate: u32, channels: u16) -> Result<()> {
    let output_channels = channels.min(2);
    let output_rate = if [8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000].contains(&rate) {
        rate
    } else {
        48_000
    };
    let mut resamplers: Vec<_> = if output_rate == rate { Vec::new() } else {
        (0..output_channels).map(|_| Resampler::new(rate, output_rate)).collect()
    };
    let mut input_frames = 0usize;
    let mut output_frames = 0usize;
    let bitrate_kbps = if output_channels == 1 { 128 } else { 192 };
    // VBR writes frames as they arrive; this encoder's CBR reservoir waits until finish.
    let mut encoder = Mp3Encoder::new(Mp3EncoderConfig {
        bitrate_kbps: 0,
        vbr_quality: Some(bitrate_kbps as f32),
    });
    for samples in rx {
        let pcm = if channels <= 2 {
            samples
        } else {
            samples.chunks_exact(channels as usize)
                .map(|frame| (frame.iter().map(|sample| *sample as i32).sum::<i32>() / channels as i32) as i16)
                .collect()
        };
        input_frames += pcm.len() / output_channels as usize;
        if resamplers.is_empty() {
            encoder.push_pcm_s16(&pcm, output_channels, output_rate)
                .map_err(|error| anyhow!("MP3 encoding failed: {error:?}"))?;
        } else {
            let pcm = resample_mp3(&pcm, output_channels, &mut resamplers);
            output_frames += pcm.len() / output_channels as usize;
            encoder.push_pcm_s16(&pcm, output_channels, output_rate)
                .map_err(|error| anyhow!("MP3 encoding failed: {error:?}"))?;
        }
        drain_mp3(&mut encoder, &mut file)?;
    }
    if !resamplers.is_empty() {
        let expected = ((input_frames as u64 * output_rate as u64 + rate as u64 / 2) / rate as u64) as usize;
        let remaining = expected.saturating_sub(output_frames);
        let tails: Vec<Vec<i16>> = resamplers.iter_mut().map(|resampler| {
            let mut tail = Vec::new();
            resampler.push(&vec![0.0; 64], &mut tail);
            tail
        }).collect();
        let frames = remaining.min(tails.iter().map(Vec::len).min().unwrap_or(0));
        let mut tail = Vec::with_capacity(frames * output_channels as usize);
        for frame in 0..frames {
            for channel in &tails { tail.push(channel[frame]); }
        }
        if !tail.is_empty() {
            encoder.push_pcm_s16(&tail, output_channels, output_rate)
                .map_err(|error| anyhow!("MP3 encoding failed: {error:?}"))?;
        }
    }
    encoder.finish();
    drain_mp3(&mut encoder, &mut file)?;
    if let Some(tag) = encoder.lame_tag_frame() {
        file.seek(SeekFrom::Start(0))?;
        file.write_all(tag)?;
    }
    file.flush()?;
    Ok(())
}

fn resample_mp3(pcm: &[i16], channels: u16, resamplers: &mut [Resampler]) -> Vec<i16> {
    let mut output: Vec<Vec<i16>> = Vec::with_capacity(channels as usize);
    for channel in 0..channels as usize {
        let input: Vec<f32> = pcm.chunks_exact(channels as usize)
            .map(|frame| frame[channel] as f32 / 32768.0).collect();
        let mut converted = Vec::new();
        resamplers[channel].push(&input, &mut converted);
        output.push(converted);
    }
    let frames = output.iter().map(Vec::len).min().unwrap_or(0);
    let mut interleaved = Vec::with_capacity(frames * channels as usize);
    for frame in 0..frames {
        for channel in &output { interleaved.push(channel[frame]); }
    }
    interleaved
}

fn drain_mp3(encoder: &mut Mp3Encoder, file: &mut File) -> Result<()> {
    loop {
        match encoder.next_packet() {
            Ok(packet) => file.write_all(&packet)?,
            Err(Mp3Error::Again | Mp3Error::Eof) => return Ok(()),
            Err(error) => return Err(anyhow!("MP3 encoding failed: {error:?}")),
        }
    }
}

fn write_wav(mut file: File, rx: mpsc::Receiver<Vec<i16>>, rate: u32, channels: u16) -> Result<()> {
    file.write_all(&[0; 44])?;
    let mut data_bytes = 0u64;
    for samples in rx {
        data_bytes += (samples.len() * 2) as u64;
        if data_bytes > u32::MAX as u64 - 36 {
            return Err(anyhow!("recording exceeded WAV size limit"));
        }
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        file.write_all(&bytes)?;
    }
    let size = data_bytes as u32;
    let byte_rate = rate
        .checked_mul(channels as u32)
        .and_then(|n| n.checked_mul(2))
        .ok_or_else(|| anyhow!("invalid microphone audio format"))?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(b"RIFF")?;
    file.write_all(&(size + 36).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&(channels * 2).to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&size.to_le_bytes())?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_all_interleaved_samples_as_a_valid_wav() {
        let path = std::env::temp_dir().join(format!(
            "realingo-recording-test-{}-{}.wav",
            std::process::id(),
            std::thread::current().name().unwrap_or("wav")
        ));
        let (mut tap, finish) = start(path.clone(), 48_000, 2).unwrap();
        tap.feed(&[0.0, 1.0, -1.0, 0.5]);
        tap.feed(&[0.25, -0.25]);
        drop(tap);
        assert_eq!(finish.complete().unwrap(), path);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 48);
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            48_000
        );
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 12);
        assert_eq!(bytes.len(), 56);
        assert_eq!(
            i16::from_le_bytes(bytes[46..48].try_into().unwrap()),
            i16::MAX
        );
        assert_eq!(
            i16::from_le_bytes(bytes[48..50].try_into().unwrap()),
            i16::MIN
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn saves_complete_microphone_audio_as_playable_mp3() {
        let path = std::env::temp_dir().join(format!("realingo-recording-test-{}.mp3", std::process::id()));
        let (mut tap, finish) = start(path.clone(), 48_000, 1).unwrap();
        for block in 0..11 {
            let samples: Vec<f32> = (0..4_800).map(|sample| {
                let position = block * 4_800 + sample;
                (position as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5
            }).collect();
            tap.feed(&samples);
        }
        drop(tap);
        assert_eq!(finish.complete().unwrap(), path);
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        crate::decode::stream_file(&path, 16_000, tx, Arc::new(AtomicBool::new(false)), |_, _| {}).unwrap();
        let mut decoded = Vec::new();
        while let Ok(chunk) = rx.try_recv() { decoded.extend(chunk); }
        assert!((16_000..=19_000).contains(&decoded.len()), "decoded {} samples", decoded.len());
        assert!(decoded.iter().any(|sample| sample.abs() > 1_000));
        assert!(std::fs::metadata(&path).unwrap().len() < 40_000);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mp3_accepts_a_96khz_stereo_microphone() {
        let path = std::env::temp_dir().join(format!("realingo-recording-96k-test-{}.mp3", std::process::id()));
        let (mut tap, finish) = start(path.clone(), 96_000, 2).unwrap();
        let frames: Vec<f32> = (0..48_000).flat_map(|sample| {
            let phase = sample as f32 * std::f32::consts::TAU / 96_000.0;
            [(phase * 440.0).sin() * 0.5, (phase * 660.0).sin() * 0.5]
        }).collect();
        tap.feed(&frames);
        drop(tap);
        finish.complete().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        crate::decode::stream_file(&path, 16_000, tx, Arc::new(AtomicBool::new(false)), |_, _| {}).unwrap();
        let mut decoded = Vec::new();
        while let Ok(chunk) = rx.try_recv() { decoded.extend(chunk); }
        assert!((7_500..=9_000).contains(&decoded.len()), "decoded {} samples", decoded.len());
        assert!(decoded.iter().any(|sample| sample.abs() > 1_000));
        std::fs::remove_file(path).unwrap();
    }
}
