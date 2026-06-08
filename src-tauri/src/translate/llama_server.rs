//! llama-server HTTP client (OpenAI-compatible API).
//!
//! Receives `TranslationRequest`s from the ASR worker, calls the
//! /v1/chat/completions endpoint with a subtitle-style prompt, and emits
//! `subtitle_update` events with the translated Chinese text.
//!
//! Uses Qwen3's `/no_think` directive to suppress chain-of-thought reasoning
//! and get direct translation output.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;
use crate::translate::TranslationRequest;
use crate::types::{EngineStatus, SubtitleMode, SubtitleTexts, SubtitleUpdate};

const WAIT_TIMEOUT_SECS: u64 = 30;

// ── public API ──────────────────────────────────────────────────────────────

/// Spawn the translation worker thread (detached).
/// Exits when `stop` is set or the sender side of `rx` is dropped.
pub fn start_translate_worker(
    rx: std::sync::mpsc::Receiver<TranslationRequest>,
    app: AppHandle,
    port: u16,
    stop: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("translate-worker".into())
        .spawn(move || translate_loop(rx, &app, port, &stop))
        .expect("spawn translate-worker thread");
}

// ── internal ────────────────────────────────────────────────────────────────

fn translate_loop(
    rx: std::sync::mpsc::Receiver<TranslationRequest>,
    app: &AppHandle,
    port: u16,
    stop: &Arc<AtomicBool>,
) {
    let base = format!("http://127.0.0.1:{port}");
    log::info!("TL: waiting for llama-server at {base}");
    set_tl_status(app, "loading");

    if !wait_for_server(&base, WAIT_TIMEOUT_SECS) {
        log::error!(
            "TL: llama-server did not respond within {WAIT_TIMEOUT_SECS}s \
             — check LLAMA_SERVER_BIN / LLAMA_MODEL env vars"
        );
        set_tl_status(app, "error");
        return;
    }
    log::info!("TL: llama-server ready");
    set_tl_status(app, "ready");

    let url = format!("{base}/v1/chat/completions");

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let req = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(r) => r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        // "No translation" mode — just promote source text to final subtitle.
        if req.mode == SubtitleMode::NoTranslate {
            let mut subtitles = crate::types::SubtitleTexts::default();
            match req.source_lang.as_str() {
                "ko" => subtitles.ko = Some(req.source_text.clone()),
                "en" => subtitles.en = Some(req.source_text.clone()),
                _    => subtitles.zh = Some(req.source_text.clone()),
            }
            let update = SubtitleUpdate {
                id: req.id.clone(),
                source_lang: req.source_lang.clone(),
                source_text: req.source_text.clone(),
                mode: req.mode,
                subtitles,
                is_final: true,
                started_at_ms: Some(req.started_at_ms),
                ended_at_ms: Some(req.ended_at_ms),
            };
            let _ = app.emit("subtitle_update", update);
            continue;
        }

        let target = req.mode.target_lang();
        log::info!("TL [{}→{}]: {:?}", req.source_lang, target, req.source_text);

        // Source is already in the target language — nothing to translate.
        if req.source_lang == target {
            emit_translated(app, &req, req.source_text.clone());
            continue;
        }

        let t_tl = std::time::Instant::now();
        match call_translate(&url, &req.source_lang, &req.source_text, req.mode) {
            Ok(translated) => {
                let tl_ms = t_tl.elapsed().as_millis();
                log::info!("TL [{} → {}] {tl_ms}ms → {:?}", req.source_lang, req.mode.target_lang(), translated);
                emit_translated(app, &req, translated);
            }
            Err(e) => {
                let tl_ms = t_tl.elapsed().as_millis();
                log::warn!("TL [{} {tl_ms}ms] error: {e}", req.source_lang);
                // Don't emit — the source-only subtitle (emitted by ASR worker)
                // stays on screen.
            }
        }
    }

    set_tl_status(app, "unloaded");
    log::info!("translate worker exited");
}

/// Call llama-server and return the translation in the target language.
fn call_translate(
    url: &str,
    source_lang: &str,
    text: &str,
    mode: crate::types::SubtitleMode,
) -> Result<String, String> {
    let source_name = match source_lang {
        "ko" => "Korean",
        "en" => "English",
        "zh" => "Chinese",
        other => other,
    };
    let target_name = mode.target_name();

    let system = format!(
        "You are a real-time subtitle translator. \
         Output ONLY the {target_name} translation — no explanations, no additions. \
         Keep the natural spoken tone. Translate incomplete sentences as-is."
    );

    // /no_think disables Qwen3's chain-of-thought so the first token is the answer.
    let user = format!("/no_think [{source_name}→{target_name}] {text}");

    let body = serde_json::json!({
        "model": "local",
        "messages": [
            { "role": "system", "content": &system },
            { "role": "user",   "content": &user   }
        ],
        "max_tokens": 200,
        "temperature": 0
    });

    let resp = ureq::post(url)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;

    let raw = json
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Safety-net: strip any residual <think>…</think> tags.
    // With /no_think these should never appear, but guard anyway.
    let content = strip_think_tags(&raw);

    if content.is_empty() {
        // Dump full response to help diagnose empty-translation bugs.
        log::warn!(
            "TL empty content for [{source_lang}] {:?} — full response:\n{json}",
            text
        );
        Err(format!("empty translation for: {text}"))
    } else {
        log::debug!("TL raw={raw:?}  stripped={content:?}");
        Ok(content)
    }
}

/// Remove `<think>…</think>` blocks; take everything after the last `</think>`.
fn strip_think_tags(s: &str) -> String {
    if let Some(pos) = s.rfind("</think>") {
        return s[pos + "</think>".len()..].trim().to_string();
    }
    s.trim().to_string()
}

/// Emit a `subtitle_update` event with the Chinese translation filled in.
fn emit_translated(app: &AppHandle, req: &TranslationRequest, zh: String) {
    let mode = req.mode;
    let mut subtitles = SubtitleTexts::default();

    let target = req.mode.target_lang();

    // Put translation in the target language slot.
    match target {
        "zh" => subtitles.zh = Some(zh),
        "ko" => subtitles.ko = Some(zh),
        "en" => subtitles.en = Some(zh),
        _    => subtitles.zh = Some(zh),
    }

    // Preserve source text in its own slot (so viewer sees both original + translation).
    // Skip if source == target (pass-through case, already handled above).
    if req.source_lang != target {
        match req.source_lang.as_str() {
            "ko" => subtitles.ko = Some(req.source_text.clone()),
            "en" => subtitles.en = Some(req.source_text.clone()),
            "zh" => subtitles.zh = subtitles.zh.clone().or(Some(req.source_text.clone())),
            other => log::debug!("TL emit: unhandled source_lang {other:?}"),
        }
    }


    log::debug!(
        "TL emit [mode={mode:?} src={src}]: zh={zh_ok} ko={ko_ok} en={en_ok}",
        mode = req.mode,
        src = req.source_lang,
        zh_ok = subtitles.zh.is_some(),
        ko_ok = subtitles.ko.is_some(),
        en_ok = subtitles.en.is_some(),
    );

    let update = SubtitleUpdate {
        id: req.id.clone(),
        source_lang: req.source_lang.clone(),
        source_text: req.source_text.clone(),
        mode,
        subtitles,
        is_final: true,
        started_at_ms: Some(req.started_at_ms),
        ended_at_ms: Some(req.ended_at_ms),
    };

    let _ = app.emit("subtitle_update", update);
}

/// Poll `GET /health` until the server responds 200 or the timeout expires.
fn wait_for_server(base: &str, timeout_secs: u64) -> bool {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let url = format!("{base}/health");
    while std::time::Instant::now() < deadline {
        if ureq::get(&url).call().is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    false
}

/// Update `AppState.translation_status` and re-broadcast `engine_status`.
fn set_tl_status(app: &AppHandle, status: &str) {
    if let Some(st) = app.try_state::<Mutex<AppState>>() {
        if let Ok(mut s) = st.lock() {
            s.translation_status = status.to_string();
            let eng = EngineStatus::from_state(&s);
            let _ = app.emit("engine_status", eng);
        }
    }
}
