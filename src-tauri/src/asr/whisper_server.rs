//! whisper-server HTTP client (ADR-0001).
//!
//! Receives `AudioChunk`s from the VAD pipeline, encodes them as 16-bit PCM
//! WAV, POSTs them to whisper-server's `/inference` endpoint, and emits
//! `subtitle_update` events with the resulting transcription.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager};

use crate::asr::AudioChunk;
use crate::state::AppState;
use crate::translate::TranslationRequest;
use crate::types::{EngineStatus, SubtitleTexts, SubtitleUpdate};

const SAMPLE_RATE: u32 = 16_000;
/// Multipart boundary (arbitrary fixed string).
const BOUNDARY: &str = "----WhisperBoundary8f3a2e1d";

// ── public API ──────────────────────────────────────────────────────────────

/// Spawn the ASR worker thread (detached).
/// Exits when `stop` is set or the sender side of `rx` is dropped.
/// `lang_hint` is an optional ISO-639-1 code to pass to whisper per request
/// (e.g. `Some("ko")` for Korean-only streams for better accuracy).
/// `None` = auto-detect (multilingual).
pub fn start_asr_worker(
    rx: std::sync::mpsc::Receiver<AudioChunk>,
    app: AppHandle,
    port: u16,
    stop: Arc<AtomicBool>,
    translate_tx: SyncSender<TranslationRequest>,
) {
    std::thread::Builder::new()
        .name("asr-worker".into())
        .spawn(move || asr_loop(rx, &app, port, &stop, translate_tx))
        .expect("spawn asr-worker thread");
}

// ── internal ────────────────────────────────────────────────────────────────

fn asr_loop(
    rx: std::sync::mpsc::Receiver<AudioChunk>,
    app: &AppHandle,
    port: u16,
    stop: &Arc<AtomicBool>,
    translate_tx: SyncSender<TranslationRequest>,
) {
    let base = format!("http://127.0.0.1:{port}");
    log::info!("ASR: waiting for whisper-server at {base}");
    set_asr_status(app, "loading");

    // Longer timeout: first run of faster-whisper downloads ~1.5 GB model.
    if !wait_for_server(&base, 300) {
        log::error!("ASR: whisper-server did not respond within 5 min — check binary/model path");
        set_asr_status(app, "error");
        return;
    }
    log::info!("ASR: whisper-server ready");
    set_asr_status(app, "ready");

    let infer_url = format!("{base}/inference");
    let mut chunk_seq: u64 = 0; // for log messages only
    // Rolling prompt: last transcribed sentence passed back to whisper as
    // initial_prompt so it can maintain continuity across chunks (names,
    // punctuation, etc.). Keep the last ~200 chars to stay inside the token budget.
    let mut last_prompt: Option<String> = None;
    // Last valid (non-hallucinated) text, used for consecutive-repetition detection.
    let mut last_valid_text: Option<String> = None;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let chunk = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(c) => c,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        chunk_seq += 1;
        log::info!(
            "ASR chunk {} [u{} {}]: {} samples ({:.2}s)",
            chunk_seq,
            chunk.utterance_id,
            if chunk.is_partial { "partial" } else { "final" },
            chunk.samples.len(),
            chunk.samples.len() as f64 / SAMPLE_RATE as f64
        );

        // Read live settings (source hint + music mode) for this chunk.
        let (lang_hint, music_mode) = app
            .try_state::<std::sync::Mutex<crate::state::AppState>>()
            .and_then(|st| st.lock().ok().map(|s| (
                s.source_hint.lang_code().map(str::to_string),
                s.music_mode,
            )))
            .unwrap_or((None, false));

        // In music mode, prepend "Song lyrics:" so Whisper knows the context.
        let music_prompt = if music_mode {
            let base = "Song lyrics:";
            Some(match last_prompt.as_deref() {
                Some(p) if !p.is_empty() => format!("{base} {p}"),
                _ => base.to_string(),
            })
        } else {
            None
        };
        let effective_prompt = music_prompt.as_deref().or(last_prompt.as_deref());

        // beam_size=3 in music mode for better lyric accuracy.
        let beam_size = if music_mode { Some(3u32) } else { None };

        let t_infer = std::time::Instant::now();
        match transcribe(&infer_url, &chunk, effective_prompt, lang_hint.as_deref(), beam_size) {
            Ok((text, lang, no_speech_prob)) => {
                let infer_ms = t_infer.elapsed().as_millis();
                let text = text.trim().to_string();
                if text.is_empty() {
                    log::debug!("ASR chunk {chunk_seq} [u{}]: empty transcription ({infer_ms}ms), skipping", chunk.utterance_id);
                    continue;
                }

                // Primary filter: no_speech_prob from faster-whisper/whisper.cpp.
                // Values ≥ 0.7 mean the model itself thinks there was no real speech
                // in this chunk — silence, music bed, or background noise.
                if no_speech_prob >= 0.7 {
                    log::info!(
                        "ASR chunk {} [u{}]: no_speech_prob={no_speech_prob:.2} ≥ 0.7, suppressed ({infer_ms}ms): {text:?}",
                        chunk_seq, chunk.utterance_id
                    );
                    continue;
                }

                // Secondary filter: keyword blocklist for known hallucination phrases
                // that slip through even when no_speech_prob is low (e.g. music mode).
                if is_hallucination(&text, last_valid_text.as_deref()) {
                    log::info!(
                        "ASR chunk {} [u{}]: hallucination suppressed ({infer_ms}ms): {text:?}",
                        chunk_seq, chunk.utterance_id
                    );
                    continue;
                }

                log::info!(
                    "ASR chunk {} [u{} {}]: infer={infer_ms}ms  lang={lang:?}  text={text:?}",
                    chunk_seq, chunk.utterance_id,
                    if chunk.is_partial { "partial" } else { "final" },
                );

                // Update rolling prompt and valid-text tracker.
                last_prompt = Some(if text.len() > 200 {
                    text[text.len() - 200..].to_string()
                } else {
                    text.clone()
                });
                last_valid_text = Some(text.clone());

                // All chunks from the same utterance share the same subtitle slot ID so
                // that partial updates replace the previous partial in-place on screen.
                let subtitle_id = chunk.utterance_id;

                // 1. Emit source-language subtitle immediately.
                emit_subtitle(app, subtitle_id, &text, &lang, chunk.started_at_ms, chunk.ended_at_ms);

                // 2. For PARTIAL chunks: skip translation — the text will be superseded
                //    shortly and translating every 5 s chunk wastes LLM calls.
                //    For FINAL chunks: enqueue for translation (non-blocking).
                if chunk.is_partial {
                    log::debug!("ASR u{subtitle_id}: partial — translation skipped");
                    continue;
                }

                let mode = app
                    .try_state::<std::sync::Mutex<AppState>>()
                    .and_then(|st| st.lock().ok().map(|s| s.mode))
                    .unwrap_or_default();

                let req = TranslationRequest {
                    id: format!("asr_{subtitle_id}"),
                    source_lang: lang,
                    source_text: text,
                    mode,
                    started_at_ms: chunk.started_at_ms,
                    ended_at_ms: chunk.ended_at_ms,
                };
                match translate_tx.try_send(req) {
                    Ok(_) => {}
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        log::warn!("ASR u{subtitle_id}: translation channel full, skipping");
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                        log::warn!("ASR u{subtitle_id}: translation channel closed");
                    }
                }
            }
            Err(e) => log::warn!("ASR chunk {chunk_seq} [u{}] error: {e}", chunk.utterance_id),
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

/// Send a chunk to whisper-server and return `(text, lang, no_speech_prob)`.
///
/// `no_speech_prob` is the mean probability across all returned segments that
/// the audio contains no real speech.  Values ≥ 0.7 typically indicate silence,
/// noise, or music without lyrics — the caller should discard such results.
/// Returns 0.0 when the field is absent (old whisper.cpp server builds).
fn transcribe(
    url: &str,
    chunk: &AudioChunk,
    prompt: Option<&str>,
    lang_hint: Option<&str>,
    beam_size: Option<u32>,
) -> Result<(String, String, f32), String> {
    let wav = encode_wav_16bit(&chunk.samples);
    let body = build_multipart(&wav, prompt, lang_hint, beam_size);
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

    // Log at debug level; unknown codes are warned inside normalize_lang.
    log::debug!("ASR lang: whisper={lang_raw:?} → {lang:?}");

    // Mean no_speech_prob across all segments.
    // faster-whisper always provides this; whisper.cpp verbose_json also includes it.
    // Absent → 0.0 (treat as speech, don't suppress).
    let no_speech_prob = json
        .get("segments")
        .and_then(|s| s.as_array())
        .filter(|segs| !segs.is_empty())
        .map(|segs| {
            let sum: f64 = segs
                .iter()
                .filter_map(|s| s.get("no_speech_prob").and_then(|v| v.as_f64()))
                .sum();
            (sum / segs.len() as f64) as f32
        })
        .unwrap_or(0.0_f32);

    Ok((text, lang, no_speech_prob))
}

/// Build a `multipart/form-data` body with the WAV bytes, `verbose_json` format,
/// an optional `initial_prompt` for continuity, and an optional `language` hint.
fn build_multipart(wav: &[u8], prompt: Option<&str>, lang_hint: Option<&str>, beam_size: Option<u32>) -> Vec<u8> {
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

    // Part 3 (optional): initial_prompt for cross-chunk continuity
    if let Some(p) = prompt {
        if !p.is_empty() {
            let pr = format!(
                "\r\n--{BOUNDARY}\r\n\
                 Content-Disposition: form-data; name=\"initial_prompt\"\r\n\r\n\
                 {p}"
            );
            body.extend_from_slice(pr.as_bytes());
        }
    }

    // Part 4 (optional): language hint from user setting (overrides whisper auto-detect)
    if let Some(lang) = lang_hint {
        let lh = format!(
            "\r\n--{BOUNDARY}\r\n\
             Content-Disposition: form-data; name=\"language\"\r\n\r\n\
             {lang}"
        );
        body.extend_from_slice(lh.as_bytes());
    }

    // Part 5 (optional): beam_size override (music mode uses 3 for better accuracy)
    if let Some(bs) = beam_size {
        let b = format!(
            "\r\n--{BOUNDARY}\r\n\
             Content-Disposition: form-data; name=\"beam_size\"\r\n\r\n\
             {bs}"
        );
        body.extend_from_slice(b.as_bytes());
    }

    // Closing boundary
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

/// Return `true` if `text` looks like a Whisper hallucination that should be
/// silently dropped.
///
/// Whisper is trained on YouTube data and hallucinates common closing phrases
/// (subscribe, credits, etc.) when the audio has no clear speech — especially
/// during music or silence.  We also suppress consecutive-duplicate outputs
/// which indicate a feedback loop via `initial_prompt`.
fn is_hallucination(text: &str, last_valid: Option<&str>) -> bool {
    let t = text.trim();

    // 1. Pure bracket-enclosed event tags: "[Music]", "[BLANK_AUDIO]", "[訂閱 / 感謝]", …
    //    These are never real speech.
    if t.starts_with('[') && t.ends_with(']') {
        return true;
    }

    // 2. Known hallucination substrings (case-insensitive, Chinese & English).
    //    Only common, unambiguous phrases that should NEVER appear in real speech.
    const DENY: &[&str] = &[
        // Chinese YouTube credits Whisper has memorised
        "字幕由",       // "字幕由愛好字幕組提供"
        "請訂閱",       // "請訂閱我們的頻道"
        "感謝收看",
        "謝謝收看",
        "歡迎訂閱",
        // English equivalents
        "thanks for watching",
        "thank you for watching",
        "like and subscribe",
        "please subscribe",
        // Whisper event markers that leak out of bracket detection
        "[music]",
        "[blank_audio]",
        "[applause]",
        "[laughter]",
        "[silence]",
        "[음악]",   // Korean [Music]
        "[박수]",   // Korean [Applause]
    ];
    let lower = t.to_lowercase();
    if DENY.iter().any(|&h| lower.contains(h)) {
        return true;
    }

    // 3. Consecutive exact repetition → feedback loop via initial_prompt.
    //    Only trigger for shorter texts to avoid suppressing real repeated speech.
    if let Some(prev) = last_valid {
        if prev.trim() == t && t.chars().count() < 60 {
            return true;
        }
    }

    false
}

/// Map full language names and ISO codes to our canonical 2-letter codes.
///
/// whisper-server returns ISO-639-1 ("ko") in most builds, but some return
/// the full English name ("korean") or the ISO-639-2 code ("kor"). We handle
/// all variants here. Truly unknown codes fall back to "en" with a warn.
fn normalize_lang(raw: &str) -> String {
    match raw {
        "en" | "eng" | "english" => "en",
        "ko" | "kor" | "korean" => "ko",
        "zh" | "zho" | "cmn" | "chinese" | "mandarin" | "cantonese" => "zh",
        "ja" | "jpn" | "japanese" => "ja",
        // Accept any 2-letter code we don't explicitly know.
        other if other.len() == 2 => return other.to_string(),
        other => {
            log::warn!("ASR: unknown language code {:?}, defaulting to \"en\"", other);
            "en"
        }
    }
    .to_string()
}

/// Emit a source-only `subtitle_update` event.
/// The `id` is the utterance id — all partial + final chunks share it so
/// the frontend updates the same slot in-place instead of stacking.
fn emit_subtitle(
    app: &AppHandle,
    id: u64,
    text: &str,
    lang: &str,
    started_at_ms: u64,
    ended_at_ms: u64,
) {
    let mode = app
        .try_state::<Mutex<AppState>>()
        .and_then(|st| st.lock().ok().map(|s| s.mode))
        .unwrap_or_default();

    // Populate only the source-language slot; translation worker fills zh.
    let mut subtitles = SubtitleTexts::default();
    match lang {
        "ko" => subtitles.ko = Some(text.to_string()),
        "zh" => subtitles.zh = Some(text.to_string()),
        _ => subtitles.en = Some(text.to_string()),
    }

    let update = SubtitleUpdate {
        id: format!("asr_{id}"),
        source_lang: lang.to_string(),
        source_text: text.to_string(),
        mode,
        subtitles,
        is_final: false, // becomes final when translation worker emits the zh slot
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
