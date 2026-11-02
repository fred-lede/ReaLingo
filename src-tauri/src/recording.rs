//! Optional microphone capture, written independently of the ASR queue.
use anyhow::{anyhow, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

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
    let partial = path.with_extension("wav.part");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)?;
    let (tx, rx) = mpsc::sync_channel(256);
    let overflowed = Arc::new(AtomicBool::new(false));
    let worker = std::thread::spawn(move || write_wav(file, rx, rate, channels));
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
}
