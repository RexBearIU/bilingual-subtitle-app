//! OpenRouter chat-completions client.
//!
//! Receives `TranslationRequest`s from the ASR worker, calls OpenRouter's
//! /v1/chat/completions endpoint with a subtitle-style prompt, and emits
//! `subtitle_update` events with the translated text.
//!
//! Replaces the former local llama-server sidecar (ADR-0011): no model weights,
//! no GPU offload, no child process — just an HTTP call to a hosted model.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::state;
use crate::translate::{RemoteConfig, TranslationRequest};
use crate::types::{SubtitleMode, SubtitleTexts, SubtitleUpdate};

/// Per-request ceiling.  Subtitles are short; anything slower than this has
/// already scrolled off screen, so failing fast beats waiting.
const REQUEST_TIMEOUT_SECS: u64 = 12;
const CONNECT_TIMEOUT_SECS: u64 = 5;

// ── public API ──────────────────────────────────────────────────────────────

/// Spawn the translation worker thread (detached).
/// Exits when `stop` is set or the sender side of `rx` is dropped.
pub fn start_translate_worker(
    rx: std::sync::mpsc::Receiver<TranslationRequest>,
    app: AppHandle,
    cfg: RemoteConfig,
    stop: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("translate-worker".into())
        .spawn(move || translate_loop(rx, &app, &cfg, &stop))
        .expect("spawn translate-worker thread");
}

// ── internal ────────────────────────────────────────────────────────────────

fn translate_loop(
    rx: std::sync::mpsc::Receiver<TranslationRequest>,
    app: &AppHandle,
    cfg: &RemoteConfig,
    stop: &Arc<AtomicBool>,
) {
    log::info!("TL: OpenRouter model={} base={}", cfg.model, cfg.base_url);
    set_tl_status(app, "loading");

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout_read(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build();

    // Validate the key up front so a bad/missing key surfaces as an engine
    // error immediately, instead of every subtitle silently failing later.
    match check_credentials(&agent, cfg) {
        Ok(()) => log::info!("TL: OpenRouter key OK"),
        Err(CredError::Unauthorized) => {
            log::error!(
                "TL: OpenRouter rejected the API key (401) \
                 — set OPENROUTER_API_KEY or openrouterApiKey in settings.json"
            );
            set_tl_status(app, "error");
            return;
        }
        // A transient network failure at startup should not disable translation
        // for the whole session — carry on and let per-request retries handle it.
        Err(CredError::Other(e)) => {
            log::warn!("TL: key check inconclusive ({e}) — continuing anyway");
        }
    }
    set_tl_status(app, "ready");

    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    // Rolling context: the last few successful (source, translated) pairs,
    // injected as prior chat turns.  Gives the model cross-subtitle context —
    // pronouns, names, and topic continuity — which matters a lot for Korean,
    // where subjects are routinely omitted and must be inferred.
    const CTX_PAIRS: usize = 3;
    let mut history: std::collections::VecDeque<(String, String)> =
        std::collections::VecDeque::with_capacity(CTX_PAIRS);

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut req = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(r) => r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // If we fell behind, translate only the NEWEST request — the older
        // subtitles are already scrolling away, and the visible line going
        // untranslated is worse than an old line keeping its source text.
        // This matters more with a hosted model than it did locally: network
        // latency is spikier than a warm GPU, so backlogs form more often.
        let mut skipped = 0u32;
        while let Ok(newer) = rx.try_recv() {
            skipped += 1;
            req = newer;
        }
        if skipped > 0 {
            log::info!("TL: backlog — skipped {skipped} stale request(s), translating newest");
        }

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

        // Nothing translatable (punctuation / numbers only) — pass through
        // without burning a paid round-trip on "." → "。".  Cheap locally,
        // but every skipped call is real money against a hosted model.
        if !req.source_text.chars().any(|c| c.is_alphabetic()) {
            emit_translated(app, &req, req.source_text.clone());
            continue;
        }

        let music_mode = state::read_state(app, |s| s.music_mode).unwrap_or(false);

        let prev: Vec<(&str, &str)> = history
            .iter()
            .map(|(s, t)| (s.as_str(), t.as_str()))
            .collect();
        let t_tl = std::time::Instant::now();
        match call_translate(
            &agent, &url, cfg, &req.source_lang, &req.source_text, req.mode, &prev, music_mode,
        ) {
            Ok(translated) => {
                let tl_ms = t_tl.elapsed().as_millis();
                log::info!("TL [{} → {}] {tl_ms}ms → {:?}", req.source_lang, req.mode.target_lang(), translated);
                if history.len() == CTX_PAIRS {
                    history.pop_front();
                }
                history.push_back((req.source_text.clone(), translated.clone()));
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

// ── credential check ────────────────────────────────────────────────────────

enum CredError {
    Unauthorized,
    Other(String),
}

/// GET /key — confirms the key is valid before the first subtitle arrives.
fn check_credentials(agent: &ureq::Agent, cfg: &RemoteConfig) -> Result<(), CredError> {
    let url = format!("{}/key", cfg.base_url.trim_end_matches('/'));
    match agent
        .get(&url)
        .set("Authorization", &format!("Bearer {}", cfg.api_key))
        .call()
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(401, _)) => Err(CredError::Unauthorized),
        Err(e) => Err(CredError::Other(e.to_string())),
    }
}

// ── prompting ───────────────────────────────────────────────────────────────

fn build_system_prompt(source_lang: &str, target_name: &str, music_mode: bool) -> String {
    let mut p = format!(
        "You are a real-time subtitle translator. \
         Output ONLY the {target_name} translation — no explanations, no additions. \
         Keep the natural spoken tone. \
         The input comes from live speech recognition: it may contain transcription \
         errors, odd spacing, or be a fragment cut mid-sentence. Infer the intended \
         words from context, translate only what is present, and never invent \
         content to complete a fragment. \
         The audio may contain several speakers taking turns — if the text switches \
         speaker mid-line (often marked with a dash), translate each utterance and \
         keep them separated with a dash; do not merge different speakers into one \
         sentence."
    );

    if source_lang == "ko" {
        p.push_str(
            " For Korean: keep English loanwords in English, \
             transliterate proper names phonetically, \
             match the speaker's formal or casual register.",
        );
    }

    if music_mode {
        p.push_str(
            " The text is song lyrics — translate lyrically and concisely, \
             preserving imagery and emotion over literal word order.",
        );
    }

    p
}

/// Call OpenRouter and return the translation in the target language.
/// `prev` holds the last few (source, translated) pairs, injected as prior
/// chat turns to keep vocabulary, names, and topic continuity consistent.
#[allow(clippy::too_many_arguments)]
fn call_translate(
    agent: &ureq::Agent,
    url: &str,
    cfg: &RemoteConfig,
    source_lang: &str,
    text: &str,
    mode: crate::types::SubtitleMode,
    prev: &[(&str, &str)],
    music_mode: bool,
) -> Result<String, String> {
    let source_name = match source_lang {
        "ko" => "Korean",
        "en" => "English",
        "zh" => "Chinese",
        "ja" => "Japanese",
        other => other,
    };
    let target_name = mode.target_name();

    let system = build_system_prompt(source_lang, target_name, music_mode);
    let user = format!("[{source_name}→{target_name}] {text}");

    let mut messages = vec![serde_json::json!({ "role": "system", "content": &system })];

    // Inject recent subtitles as prior turns so the model can maintain
    // consistent names, loanwords, and topic context across subtitles.
    for (prev_src, prev_tl) in prev {
        let prev_user = format!("[{source_name}→{target_name}] {prev_src}");
        messages.push(serde_json::json!({ "role": "user",      "content": prev_user }));
        messages.push(serde_json::json!({ "role": "assistant", "content": prev_tl  }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": &user }));

    let mut body = serde_json::json!({
        "model": cfg.model,
        "messages": messages,
        "max_tokens": 200,
        "temperature": 0,
        // Subtitles must not wait on a reasoning preamble.  Models that ignore
        // this are handled by strip_think_tags below; models that REFUSE it
        // (400 "Reasoning is mandatory") are retried without the field by
        // post_with_retry.
        "reasoning": { "enabled": false },
    });
    // Let the user pin specific upstream providers (e.g. to force a low-latency
    // one) without a code change.
    if let Some(order) = &cfg.provider_order {
        body["provider"] = serde_json::json!({ "order": order, "allow_fallbacks": true });
    }

    let json = post_with_retry(agent, url, cfg, &body)?;

    let raw = json
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Safety-net: strip any residual <think>…</think> tags.  Hosted reasoning
    // models sometimes emit them even with reasoning disabled.
    let content = strip_think_tags(&raw);

    if content.is_empty() {
        // A reasoning model can burn the whole max_tokens budget before it
        // emits any content, which looks identical to a broken model unless
        // the cause is named.
        let reasoning_len = json
            .pointer("/choices/0/message/reasoning")
            .and_then(|v| v.as_str())
            .map(str::len)
            .unwrap_or(0);
        if reasoning_len > 0 {
            log::warn!(
                "TL empty content for [{source_lang}] {text:?}: {} spent the token budget on \
                 {reasoning_len} chars of reasoning. Switch to a non-reasoning model — \
                 subtitles cannot afford it.",
                cfg.model
            );
        } else {
            // Dump full response to help diagnose empty-translation bugs.
            log::warn!(
                "TL empty content for [{source_lang}] {:?} — full response:\n{json}",
                text
            );
        }
        Err(format!("empty translation for: {text}"))
    } else {
        log::info!("TL raw={raw:?}  stripped={content:?}");
        Ok(content)
    }
}

/// POST the request, retrying once on a transient failure.
///
/// Rate limits and upstream 5xx are common on a shared hosted endpoint and
/// usually clear immediately; one fast retry recovers the subtitle without
/// stalling the pipeline.  4xx other than 429 are permanent — fail straight
/// through so the error reaches the log instead of being retried pointlessly.
fn post_with_retry(
    agent: &ureq::Agent,
    url: &str,
    cfg: &RemoteConfig,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    const RETRY_DELAY_MS: u64 = 250;
    let mut body = body.clone();
    let mut dropped_reasoning = false;
    let mut attempt = 0;
    loop {
        let mut req = agent
            .post(url)
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {}", cfg.api_key));
        // Optional attribution headers — they place the app on OpenRouter's
        // leaderboards and are ignored when absent.
        if let Some(r) = &cfg.referer {
            req = req.set("HTTP-Referer", r);
        }
        if let Some(t) = &cfg.title {
            req = req.set("X-Title", t);
        }

        let result = req.send_string(&body.to_string());

        match result {
            Ok(resp) => return resp.into_json().map_err(|e| e.to_string()),
            Err(ureq::Error::Status(code, resp)) => {
                let detail = resp.into_string().unwrap_or_default();

                // Some endpoints make reasoning mandatory and reject the
                // disable flag outright ("Reasoning is mandatory for this
                // endpoint and cannot be disabled"). Retry once without it so
                // picking such a model fails soft instead of on every line.
                if code == 400
                    && !dropped_reasoning
                    && detail.to_lowercase().contains("reasoning")
                {
                    dropped_reasoning = true;
                    if let Some(obj) = body.as_object_mut() {
                        obj.remove("reasoning");
                    }
                    log::warn!(
                        "TL: {} rejects reasoning.enabled=false — retrying without it. \
                         Note it will spend max_tokens on reasoning, which can leave the \
                         translation empty; prefer a non-reasoning model for subtitles.",
                        cfg.model
                    );
                    continue;
                }

                let transient = code == 429 || (500..600).contains(&code);
                if transient && attempt == 0 {
                    attempt += 1;
                    log::warn!("TL: HTTP {code} — retrying once");
                    std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
                    continue;
                }
                return Err(format!("HTTP {code}: {}", detail.trim()));
            }
            Err(e) => {
                if attempt == 0 {
                    attempt += 1;
                    log::warn!("TL: transport error ({e}) — retrying once");
                    std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
                    continue;
                }
                return Err(e.to_string());
            }
        }
    }
}

/// Remove `<think>…</think>` blocks; take everything after the last `</think>`.
fn strip_think_tags(s: &str) -> String {
    if let Some(pos) = s.rfind("</think>") {
        return s[pos + "</think>".len()..].trim().to_string();
    }
    s.trim().to_string()
}

/// Emit a `subtitle_update` event with the translation filled in.
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

/// Update `AppState.translation_status` and re-broadcast `engine_status`.
fn set_tl_status(app: &AppHandle, status: &str) {
    state::update_and_emit(app, |s| s.translation_status = status.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_think_tags_removes_reasoning_preamble() {
        assert_eq!(strip_think_tags("<think>hmm</think>你好"), "你好");
        assert_eq!(strip_think_tags("  你好  "), "你好");
        // Only the text after the LAST closing tag survives.
        assert_eq!(strip_think_tags("<think>a</think>x<think>b</think>y"), "y");
    }
}
