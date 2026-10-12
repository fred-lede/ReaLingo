//! Wire-level probe: push a local audio file at the realtime endpoint and print every
//! server frame verbatim. This is how the event names and field shapes get verified against
//! the live service instead of guessed from the docs.
//!
//!   set DASHSCOPE_API_KEY=sk-...
//!   cargo run --example probe -- path\to\audio.wav [target_lang] [model]
//!   set DASHSCOPE_WORKSPACE_ID=llm-...   (optional dedicated endpoint)
//!   set OPENAI_API_KEY=sk-...            (instead: probe OpenAI gpt-realtime-translate)
//!   set PROBE_PROTOCOL=qwen38            (qwen35 | qwen38 | openai; default follows the key)

use realingo_lib::config::{Protocol, Region, Settings};
use realingo_lib::decode;

use base64::prelude::{Engine, BASE64_STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: probe <audio file> [target lang] [model]");
    let target = std::env::args().nth(2).unwrap_or_else(|| "zh".into());
    let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let protocol = match std::env::var("PROBE_PROTOCOL").as_deref() {
        Ok("qwen38") => Protocol::Qwen38,
        Ok("qwen35") => Protocol::Qwen35,
        _ if openai_key.is_empty() => Protocol::Qwen35,
        _ => Protocol::Openai,
    };
    let api_key = if openai_key.is_empty() {
        std::env::var("DASHSCOPE_API_KEY").expect("set DASHSCOPE_API_KEY or OPENAI_API_KEY")
    } else {
        String::new()
    };

    let settings = Settings {
        protocol,
        api_key,
        openai_key,
        url: std::env::var("PROBE_URL").unwrap_or_default(),
        workspace_id: std::env::var("DASHSCOPE_WORKSPACE_ID").unwrap_or_default(),
        region: Region::Beijing,
        source_lang: "auto".into(),
        target_lang: target,
        // Empty falls back to the protocol's default model.
        model: std::env::args().nth(3).unwrap_or_default(),
        hotwords: Default::default(),
        // set PROBE_SPEAK=1 to see the audio-modality frames too.
        speak: std::env::var_os("PROBE_SPEAK").is_some(),
        voice: std::env::var("PROBE_VOICE").unwrap_or_else(|_| realingo_lib::config::VOICE.into()),
    };

    println!("→ {}", settings.ws_url());
    let mut request = settings.ws_url().into_client_request()?;
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", settings.key()))?,
    );

    let (ws, response) = tokio_tungstenite::connect_async(request).await?;
    println!("← handshake {}", response.status());

    let (mut write, mut read) = ws.split();
    let update = settings.session_update();
    println!("→ {update}");
    write.send(Message::Text(update.to_string().into())).await?;

    let rate = settings.sample_rate();
    let (append, finish, _) = settings.wire();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<i16>>(64);
    let stop = Arc::new(AtomicBool::new(false));
    let file = std::path::PathBuf::from(path);
    std::thread::spawn(move || {
        if let Err(e) = decode::stream_file(&file, rate, tx, stop, |_, _| {}) {
            eprintln!("decode: {e}");
        }
    });

    let uplink = tokio::spawn(async move {
        let mut chunks = 0u32;
        while let Some(chunk) = rx.recv().await {
            let mut bytes = Vec::with_capacity(chunk.len() * 2);
            for s in &chunk {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            let frame = json!({
                "type": append,
                "audio": BASE64_STANDARD.encode(&bytes),
            });
            if write.send(Message::Text(frame.to_string().into())).await.is_err() {
                return;
            }
            chunks += 1;
        }
        eprintln!("[uplink] sent {chunks} chunks ({}s of audio)", chunks / 10);
        let _ = write.send(Message::Text(json!({ "type": finish }).to_string().into())).await;
    });

    let idle = std::time::Duration::from_secs(20);
    while let Ok(Some(Ok(msg))) = tokio::time::timeout(idle, read.next()).await {
        match msg {
            Message::Text(t) => println!("← {t}"),
            Message::Close(c) => {
                println!("← close {c:?}");
                break;
            }
            _ => {}
        }
    }
    let _ = uplink.await;
    Ok(())
}
