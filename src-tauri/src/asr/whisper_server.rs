//! whisper-server HTTP client (ADR-0001).
//!
//! Receives `AudioChunk`s from the VAD pipeline, encodes them as 16-bit PCM
//! WAV, POSTs them to whisper-server's `/inference` endpoint, and emits
//! `subtitle_update` events with the resulting transcription.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager};

use crate::asr::AudioChunk;
use crate::state::AppState;
use crate::types::{EngineStatus, SubtitleTexts, SubtitleUpdate};

const SAMPLE_RATE: u32 = 16_000;
/// Multipart boundary (arbitrary fixed string).
const BOUNDARY: &str = "----WhisperBoundary8f3a2e1d";

// ── public API ──────────────────────────────────────────────────────────────

/// Spawn the ASR worker thread (detached).
/// Exits when `stop` is set or the sender side of `rx` is dropped.
pub fn start_asr_worker(
    rx: std::sync::mpsc::Receiver<AudioChunk>,
    app: AppHandle,
    port: u16,
    stop: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("asr-worker".into())
        .spawn(move || asr_loop(rx, &app, port, &stop))
        .expect("spawn asr-worker thread");
}

// ── internal ────────────────────────────────────────────────────────────────

fn asr_loop(
    rx: std::sync::mpsc::Receiver<AudioChunk>,
    app: &AppHandle,
    port: u16,
    stop: &Arc<AtomicBool>,
) {
    let base = format!("http://127.0.0.1:{port}");
    log::info!("ASR: waiting for whisper-server at {base}");
    set_asr_status(app, "loading");

    if !wait_for_server(&base, 30) {
        log::error!("ASR: whisper-server did not respond within 30 s — check binary/model path");
        set_asr_status(app, "error");
        return;
    }
    log::info!("ASR: whisper-server ready");
    set_asr_status(app, "ready");

    let infer_url = format!("{base}/inference");
    let mut chunk_id: u64 = 0;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let chunk = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(c) => c,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        chunk_id += 1;
        log::info!(
            "ASR chunk {}: {} samples ({:.2}s)",
            chunk_id,
            chunk.samples.len(),
            chunk.samples.len() as f64 / SAMPLE_RATE as f64
        );

        match transcribe(&infer_url, &chunk) {
            Ok((text, lang)) => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    log::debug!("ASR chunk {chunk_id}: empty transcription, skipping");
                    continue;
                }
                log::info!("ASR [{lang}]: {text}");
                emit_subtitle(app, chunk_id, text, lang, chunk.started_at_ms, chunk.ended_at_ms);
            }
            Err(e) => log::warn!("ASR chunk {chunk_id} error: {e}"),
        }
    }

    set_asr_status(app, "unloaded");
    log::info!("ASR worker exited");
}

/// Poll `GET /` until the server responds 200 or the timeout expires.
fn wait_for_server(base: &str, timeout_secs: u64) -> bool {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let url = format!("{base}/");
    while std::time::Instant::now() < deadline {
        if ureq::get(&url).call().is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    false
}

/// Send a chunk to whisper-server and return `(text, lang)`.
fn transcribe(url: &str, chunk: &AudioChunk) -> Result<(String, String), String> {
    let wav = encode_wav_16bit(&chunk.samples);
    let body = build_multipart(&wav);
    let ct = format!("multipart/form-data; boundary={BOUNDARY}");

    let response = ureq::post(url)
        .set("Content-Type", &ct)
        .send_bytes(&body)
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value = response.into_json().map_err(|e| e.to_string())?;

    let text = json
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // `language` is an ISO-639-1 code in verbose_json mode (e.g. "en", "ko", "zh").
    // Some builds return the full English name; normalize_lang handles both.
    let lang_raw = json
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("en")
        .to_lowercase();
    let lang = normalize_lang(&lang_raw);

    Ok((text, lang))
}

/// Build a `multipart/form-data` body with the WAV bytes and `verbose_json` format.
fn build_multipart(wav: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();

    // Part 1: audio file
    let file_hdr = format!(
        "--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\n"
    );
    body.extend_from_slice(file_hdr.as_bytes());
    body.extend_from_slice(wav);

    // Part 2: response_format = verbose_json (includes `language` field)
    let rf = format!(
        "\r\n--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"response_format\"\r\n\r\n\
         verbose_json"
    );
    body.extend_from_slice(rf.as_bytes());

    // Closing boundary
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

/// Map full language names and ISO codes to our canonical 2-letter codes.
fn normalize_lang(raw: &str) -> String {
    match raw {
        "en" | "english" => "en",
        "ko" | "korean" => "ko",
        "zh" | "chinese" | "mandarin" | "cantonese" => "zh",
        // Accept any 2-letter code we don't recognise rather than defaulting
        other if other.len() == 2 => return other.to_string(),
        _ => "en",
    }
    .to_string()
}

/// Emit a `subtitle_update` event for the transcribed chunk.
fn emit_subtitle(
    app: &AppHandle,
    id: u64,
    text: String,
    lang: String,
    started_at_ms: u64,
    ended_at_ms: u64,
) {
    let mode = app
        .try_state::<Mutex<AppState>>()
        .and_then(|st| st.lock().ok().map(|s| s.mode))
        .unwrap_or_default();

    // Populate only the source-language slot for now; M5 translation fills the rest.
    let mut subtitles = SubtitleTexts::default();
    match lang.as_str() {
        "ko" => subtitles.ko = Some(text.clone()),
        "zh" => subtitles.zh = Some(text.clone()),
        _ => subtitles.en = Some(text.clone()),
    }

    let update = SubtitleUpdate {
        id: format!("asr_{id}"),
        source_lang: lang,
        source_text: text,
        mode,
        subtitles,
        is_final: true,
        started_at_ms: Some(started_at_ms),
        ended_at_ms: Some(ended_at_ms),
    };

    let _ = app.emit("subtitle_update", update);
}

/// Update `AppState.asr_status` and re-broadcast `engine_status`.
fn set_asr_status(app: &AppHandle, status: &str) {
    if let Some(st) = app.try_state::<Mutex<AppState>>() {
        if let Ok(mut s) = st.lock() {
            s.asr_status = status.to_string();
            let eng = EngineStatus::from_state(&s);
            let _ = app.emit("engine_status", eng);
        }
    }
}

/// Encode f32 samples as 16-bit PCM WAV at 16 kHz mono.
fn encode_wav_16bit(samples: &[f32]) -> Vec<u8> {
    let data_size = samples.len() * 2; // 2 bytes per i16 sample
    let mut wav = Vec::with_capacity(44 + data_size);

    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + data_size) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // fmt chunk — PCM, mono, 16 kHz, 16-bit
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());           // chunk size
    wav.extend_from_slice(&1u16.to_le_bytes());            // PCM
    wav.extend_from_slice(&1u16.to_le_bytes());            // 1 channel
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());     // 16 000 Hz
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes());            // block align
    wav.extend_from_slice(&16u16.to_le_bytes());           // bits per sample

    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_size as u32).to_le_bytes());
    for &s in samples {
        let v = (s * 32_767.0).clamp(-32_768.0, 32_767.0) as i16;
        wav.extend_from_slice(&v.to_le_bytes());
    }

    wav
}
