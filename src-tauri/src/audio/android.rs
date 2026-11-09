//! Android's authorized AudioPlaybackCapture feeds PCM directly from its native service.
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc::Sender;
use crate::audio::{InputLevel, Pipe};

struct Capture { pipe: Pipe, stop: Arc<AtomicBool> }
static CAPTURE: Mutex<Option<Capture>> = Mutex::new(None);

pub(crate) fn start(rate: u32, tx: Sender<Vec<i16>>, level: InputLevel, stop: Arc<AtomicBool>) -> anyhow::Result<()> {
    *CAPTURE.lock().map_err(|e| anyhow::anyhow!(e.to_string()))? =
        Some(Capture { pipe: Pipe::new(48_000, 1, rate, tx, level, Arc::new(AtomicU64::new(0))), stop });
    Ok(())
}

fn feed(samples: &[i16]) -> bool {
    let Ok(mut state) = CAPTURE.lock() else { return false; };
    // Permission/service setup may finish a moment before start_stream wires its sender.
    let Some(capture) = state.as_mut() else { return true; };
    if capture.stop.load(Ordering::Relaxed) { *state = None; return false; }
    let frames: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
    capture.pipe.feed(&frames);
    true
}

fn finish() { if let Ok(mut state) = CAPTURE.lock() { *state = None; } }

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_nosugark_realingo_SubtitleService_pushAudio(
    env: jni::JNIEnv, _service: jni::objects::JObject, samples: jni::objects::JShortArray, count: jni::sys::jint,
) -> jni::sys::jboolean {
    // Never unwind through JNI, including on a stopped recorder or invalid array.
    if count <= 0 { finish(); return jni::sys::JNI_FALSE; }
    if count > 48_000 || env.get_array_length(&samples).map_or(true, |len| count > len) {
        return jni::sys::JNI_FALSE;
    }
    let mut pcm = vec![0i16; count as usize];
    if env.get_short_array_region(&samples, 0, &mut pcm).is_err() { return jni::sys::JNI_FALSE; }
    if feed(&pcm) { jni::sys::JNI_TRUE } else { jni::sys::JNI_FALSE }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playback_pcm_is_resampled_and_stopping_releases_the_session() {
        let stop = Arc::new(AtomicBool::new(false));
        let level = InputLevel::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        start(16_000, tx, level.clone(), stop.clone()).unwrap();
        assert!(feed(&vec![8192i16; 9600]));
        let chunk = rx.try_recv().unwrap();
        assert_eq!(chunk.len(), 1600);
        // Level is measured per sent chunk (the audio the service hears), and smoothed:
        // 8192 / 32768 = 0.25, first reading is the smoothed value, not raw.
        assert!(level.rms() > 0.01 && level.rms() < 0.02, "rms was {}", level.rms());
        stop.store(true, Ordering::Relaxed);
        assert!(!feed(&[0; 48]));
        while rx.try_recv().is_ok() {}
        assert!(rx.is_closed());
        finish();
    }
}
