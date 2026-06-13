//! ASR server HTTP client.
//!
//! Receives `AudioChunk`s from the VAD pipeline, encodes them as 16-bit PCM
//! WAV, POSTs them to the ASR server's `/inference` endpoint, and emits
//! `subtitle_update` events with the resulting transcription.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::asr::AudioChunk;
use crate::state;
use crate::translate::TranslationRequest;
use crate::types::{SubtitleTexts, SubtitleUpdate};

const SAMPLE_RATE: u32 = 16_000;
/// Multipart boundary (arbitrary fixed string).
const BOUNDARY: &str = "----AsrBoundary8f3a2e1d";

// ── public API ──────────────────────────────────────────────────────────────

/// Spawn the ASR worker thread (detached).
/// Exits when `stop` is set or the sender side of `rx` is dropped.
/// `lang_hint` is an optional ISO-639-1 code passed to the ASR server per request.
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
    log::info!("ASR: waiting for asr-srv at {base}");
    set_asr_status(app, "loading");

    // Longer timeout: first run downloads the model from HuggingFace.
    if !crate::util::wait_for_http_ok(&format!("{base}/"), 300) {
        log::error!("ASR: asr-srv did not respond within 5 min — check Python path and model");
        set_asr_status(app, "error");
        return;
    }
    log::info!("ASR: asr-srv ready");
    set_asr_status(app, "ready");

    let infer_url = format!("{base}/inference");
    let mut chunk_seq: u64 = 0; // for log messages only
    // Rolling prompt: last transcribed sentence passed back to whisper as
    // initial_prompt so it can maintain continuity across chunks (names,
    // punctuation, etc.). Keep the last ~200 chars to stay inside the token budget.
    let mut last_prompt: Option<String> = None;
    // Last valid (non-hallucinated) text, used for consecutive-repetition detection.
    let mut last_valid_text: Option<String> = None;
    // How many consecutive chunks produced exactly the same text.
    let mut repeat_count: u32 = 0;
    // Track which utterance_id had a partial sent, so the following final chunk
    // is not incorrectly flagged as a consecutive repeat of the partial's text.
    let mut last_partial_utterance_id: Option<u64> = None;

    // Backlog of chunks pulled off the channel but not yet transcribed.
    let mut pending: std::collections::VecDeque<AudioChunk> = std::collections::VecDeque::new();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if pending.is_empty() {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(c) => pending.push_back(c),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // Drain whatever else is already queued, then coalesce: a partial is a
        // throw-away preview, so any partial with a newer chunk behind it is
        // stale — skip it instead of burning an inference on it.  Finals are
        // always kept.
        while let Ok(c) = rx.try_recv() {
            pending.push_back(c);
        }
        if pending.len() > 1 {
            let last_idx = pending.len() - 1;
            let before = pending.len();
            let mut kept: std::collections::VecDeque<AudioChunk> =
                std::collections::VecDeque::with_capacity(pending.len());
            for (i, c) in pending.drain(..).enumerate() {
                if c.is_partial && i < last_idx {
                    continue;
                }
                kept.push_back(c);
            }
            pending = kept;
            if pending.len() < before {
                log::debug!("ASR: skipped {} stale partial(s) in backlog", before - pending.len());
            }
        }
        let chunk = match pending.pop_front() {
            Some(c) => c,
            None => continue,
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
        let (lang_hint, music_mode) = state::read_state(app, |s| (
            s.source_hint.lang_code().map(str::to_string),
            s.music_mode,
        ))
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

        // Partial chunks are throw-away previews — greedy (1) is fast enough.
        // Final chunks get beam=5 for accuracy; Korean especially benefits from this.
        // Music mode always uses beam=3 for lyric accuracy.
        let beam_size = if music_mode {
            Some(3u32)
        } else if chunk.is_partial {
            Some(1u32)
        } else {
            Some(5u32)
        };

        let duration_s = chunk.samples.len() as f64 / SAMPLE_RATE as f64;
        log::info!(
            "ASR [u{} {}] {:.2}s → asr-srv (prompt={})...",
            chunk.utterance_id,
            if chunk.is_partial { "partial" } else { "final" },
            duration_s,
            effective_prompt.map_or(0, |p| p.len()),
        );
        let t_infer = std::time::Instant::now();
        match transcribe(&infer_url, &chunk, effective_prompt, lang_hint.as_deref(), beam_size) {
            Ok((text, lang, no_speech_prob)) => {
                let infer_ms = t_infer.elapsed().as_millis();
                let text = text.trim().to_string();
                if text.is_empty() {
                    log::info!("ASR [u{}]: empty result ({infer_ms}ms) — skipped", chunk.utterance_id);
                    continue;
                }

                // Primary filter: no_speech_prob from faster-whisper/whisper.cpp.
                // Values ≥ 0.7 mean the model itself thinks there was no real speech
                // in this chunk — silence, music bed, or background noise.
                if no_speech_prob >= 0.7 {
                    log::info!(
                        "ASR [u{}]: no_speech={no_speech_prob:.2} ≥ 0.7, suppressed ({infer_ms}ms): {text:?}",
                        chunk.utterance_id,
                    );
                    continue;
                }

                // Secondary filter: keyword blocklist.
                if is_hallucination(&text) {
                    log::info!(
                        "ASR [u{}]: hallucination suppressed ({infer_ms}ms): {text:?}",
                        chunk.utterance_id,
                    );
                    continue;
                }

                // Consecutive-repeat detection (initial_prompt feedback loop).
                // ONE exact repeat is allowed — quick echoed replies between
                // speakers ("네." / "네.") are real speech.  A second consecutive
                // repeat is a feedback loop and gets suppressed.
                // Skip the check for a final chunk that completes a partial of
                // the same utterance — the partial already set last_valid_text
                // to the same sentence.
                let is_completing_partial =
                    !chunk.is_partial && last_partial_utterance_id == Some(chunk.utterance_id);
                let is_repeat = !is_completing_partial
                    && text.chars().count() < 60
                    && last_valid_text.as_deref().is_some_and(|prev| prev.trim() == text);
                repeat_count = if is_repeat { repeat_count + 1 } else { 0 };
                if repeat_count >= 2 {
                    log::info!(
                        "ASR [u{}]: repeated {}× consecutively — suppressed ({infer_ms}ms): {text:?}",
                        chunk.utterance_id, repeat_count + 1,
                    );
                    continue;
                }

                log::info!(
                    "ASR [u{} {}]: {infer_ms}ms  no_speech={no_speech_prob:.2}  lang={lang:?}  {text:?}",
                    chunk.utterance_id,
                    if chunk.is_partial { "partial" } else { "final" },
                );

                // Update rolling prompt and valid-text tracker.
                // Truncate on a char boundary — a raw byte slice panics when
                // it lands inside a multi-byte char (Korean is 3 bytes/char).
                last_prompt = Some(if text.len() > 200 {
                    let mut start = text.len() - 200;
                    while !text.is_char_boundary(start) {
                        start += 1;
                    }
                    text[start..].to_string()
                } else {
                    text.clone()
                });
                last_valid_text = Some(text.clone());

                // All chunks from the same utterance share the same subtitle slot ID so
                // that partial updates replace the previous partial in-place on screen.
                let subtitle_id = chunk.utterance_id;

                // 1. Emit source-language subtitle immediately.
                emit_subtitle(app, subtitle_id, &text, &lang, chunk.started_at_ms, chunk.ended_at_ms);

                // 2. For PARTIAL chunks: record utterance id so the following final
                //    skips the consecutive-repeat check, then skip translation.
                if chunk.is_partial {
                    last_partial_utterance_id = Some(chunk.utterance_id);
                    log::debug!("ASR u{subtitle_id}: partial — translation skipped");
                    continue;
                }
                // Clear the partial tracker now that the final has arrived.
                if is_completing_partial {
                    last_partial_utterance_id = None;
                }

                let mode = state::read_state(app, |s| s.mode).unwrap_or_default();

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

/// Send a chunk to asr-srv and return `(text, lang, no_speech_prob)`.
///
/// `no_speech_prob` is the mean probability across all returned segments that
/// the audio contains no real speech.  Values ≥ 0.7 typically indicate silence,
/// noise, or music without lyrics — the caller should discard such results.
/// Returns 0.0 when the field is absent.
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

    // Whisper's per-chunk language detection is unreliable on short audio —
    // it regularly claims "en" while emitting Hangul text.  The downstream
    // translation prompt then says [English→…] with Korean input, which
    // confuses the LLM.  The text's actual script is ground truth: override
    // the claimed language whenever they clearly disagree.
    let lang = match detect_script_lang(&text) {
        Some(script_lang) if script_lang != lang => {
            log::debug!("ASR lang: whisper said {lang:?} but text script is {script_lang:?} — overriding");
            script_lang.to_string()
        }
        _ => lang,
    };

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
/// during music or silence.  Consecutive-repeat detection lives in the caller
/// (it needs cross-chunk state).
fn is_hallucination(text: &str) -> bool {
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
    DENY.iter().any(|&h| lower.contains(h))
}

/// Detect the dominant script of `text` and map it to a language code.
/// Returns `None` when no script reaches a clear majority (mixed/ambiguous
/// text keeps whisper's own detection).
fn detect_script_lang(text: &str) -> Option<&'static str> {
    let mut hangul = 0usize;
    let mut cjk = 0usize;    // Han ideographs (zh; also ja kanji)
    let mut kana = 0usize;   // hiragana / katakana → ja
    let mut latin = 0usize;
    for c in text.chars() {
        match c as u32 {
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul += 1,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF => cjk += 1,
            0x3040..=0x30FF => kana += 1,
            _ if c.is_ascii_alphabetic() => latin += 1,
            _ => {}
        }
    }
    let total = hangul + cjk + kana + latin;
    if total < 2 {
        return None; // too short to judge
    }
    // Any kana at all marks Japanese (ja text mixes kanji + kana freely).
    if kana * 4 >= total {
        return Some("ja");
    }
    let majority = |n: usize| n * 2 > total;
    if majority(hangul) {
        Some("ko")
    } else if majority(cjk + kana) && kana > 0 {
        Some("ja")
    } else if majority(cjk) {
        Some("zh")
    } else if majority(latin) {
        Some("en")
    } else {
        None
    }
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
    let mode = state::read_state(app, |s| s.mode).unwrap_or_default();

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
    state::update_and_emit(app, |s| s.asr_status = status.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lang_maps_names_codes_and_unknowns() {
        assert_eq!(normalize_lang("korean"), "ko");
        assert_eq!(normalize_lang("kor"), "ko");
        assert_eq!(normalize_lang("ko"), "ko");
        assert_eq!(normalize_lang("cantonese"), "zh");
        assert_eq!(normalize_lang("mandarin"), "zh");
        assert_eq!(normalize_lang("japanese"), "ja");
        // Unknown 2-letter code passes through; anything else falls back to en.
        assert_eq!(normalize_lang("de"), "de");
        assert_eq!(normalize_lang("klingon"), "en");
    }

    #[test]
    fn detect_script_lang_picks_the_dominant_script() {
        assert_eq!(detect_script_lang("안녕하세요 여러분"), Some("ko"));
        assert_eq!(detect_script_lang("你好世界"), Some("zh"));
        // Any kana marks Japanese even mixed with kanji.
        assert_eq!(detect_script_lang("こんにちは"), Some("ja"));
        assert_eq!(detect_script_lang("日本語のテスト"), Some("ja"));
        assert_eq!(detect_script_lang("hello world"), Some("en"));
        // Too short / no clear majority → keep whisper's own guess.
        assert_eq!(detect_script_lang("a"), None);
    }

    #[test]
    fn is_hallucination_catches_event_tags_and_credits() {
        assert!(is_hallucination("[Music]"));
        assert!(is_hallucination("[음악]"));
        assert!(is_hallucination("請訂閱我們的頻道"));
        assert!(is_hallucination("Thanks for watching!"));
        // Real speech must survive.
        assert!(!is_hallucination("안녕하세요 여러분"));
        assert!(!is_hallucination("Hello everyone, welcome back"));
    }

    #[test]
    fn encode_wav_16bit_writes_a_valid_44_byte_header() {
        let wav = encode_wav_16bit(&[0.0, 1.0, -1.0]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 3 * 2); // header + 3 i16 samples
        // Full-scale samples clamp to the i16 range.
        let last = i16::from_le_bytes([wav[44 + 4], wav[44 + 5]]);
        assert_eq!(last, -32_767);
    }
}
