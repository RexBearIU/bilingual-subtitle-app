//! Hosted chat-completions client for any OpenAI-compatible endpoint.
//!
//! Receives `TranslationRequest`s from the ASR worker, calls
//! `/chat/completions` with a subtitle-style prompt, and emits
//! `subtitle_update` events with the translated text.
//!
//! Replaces the former local llama-server sidecar (ADR-0011): no model weights,
//! no GPU offload, no child process — just an HTTP call to a hosted model.
//!
//! The provider is configuration, not code (see `translate::RemoteConfig`).
//! Several can be listed; the worker falls forward to the next one when the
//! active endpoint keeps failing, so one provider's outage or rate limit does
//! not take subtitles down with it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};

use crate::state;
use crate::translate::{self, Provider, TranslationRequest};
use crate::types::{SubtitleMode, SubtitleTexts, SubtitleUpdate};

/// Sent on every request.  Providers sit behind bots-and-abuse filters that
/// judge the client by its User-Agent — Groq's Cloudflare rules reject the
/// stock `Python-urllib` one with a 403 that looks nothing like an auth error.
/// `ureq`'s default passes today; naming the app makes that independent of the
/// HTTP crate's version.
const USER_AGENT: &str = concat!("BilingualSubtitles/", env!("CARGO_PKG_VERSION"));

/// Per-request ceiling.  Subtitles are short; anything slower than this has
/// already scrolled off screen, so failing fast beats waiting.
/// Optional attribution headers. OpenRouter uses them for its leaderboards;
/// every other endpoint ignores them.
const REFERER: &str = "https://github.com/RexBearIU/bilingual-subtitle-app";
const TITLE: &str = "Bilingual Subtitles";

const REQUEST_TIMEOUT_SECS: u64 = 12;
const CONNECT_TIMEOUT_SECS: u64 = 5;

// ── public API ──────────────────────────────────────────────────────────────

/// Spawn the translation worker thread (detached).
/// Exits when `stop` is set or the sender side of `rx` is dropped.
/// `active` is shared with `AppState` so the UI can switch provider mid-session
/// and can see the failovers the worker performs on its own.
pub fn start_translate_worker(
    rx: std::sync::mpsc::Receiver<TranslationRequest>,
    app: AppHandle,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
) {
    std::thread::Builder::new()
        .name("translate-worker".into())
        .spawn(move || translate_loop(rx, &app, &stop, &active))
        .expect("spawn translate-worker thread");
}

// ── internal ────────────────────────────────────────────────────────────────

fn translate_loop(
    rx: std::sync::mpsc::Receiver<TranslationRequest>,
    app: &AppHandle,
    stop: &Arc<AtomicBool>,
    active: &Arc<AtomicUsize>,
) {
    set_tl_status(app, "loading");

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout_read(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build();

    // Start on the first provider whose key is accepted, so a stale key in the
    // first slot costs one startup check rather than every subtitle.
    //
    // The scan begins at whatever the UI last selected rather than at 0, so a
    // provider the user pinned in an earlier session is not silently undone by
    // a restart.
    // Snapshot the list once for the startup key check. The live list can be
    // edited from Settings at any time, so everything after this reads it fresh
    // per request instead.
    let all: Vec<Arc<Provider>> = match app.try_state::<translate::Registry>() {
        Some(reg) => reg.0.lock().map(|l| l.clone()).unwrap_or_default(),
        None => Vec::new(),
    };
    // Indices into the full list, since `active` addresses that. Entries that
    // are not `Ready` stay in the list for the UI to show, but there is nothing
    // to authenticate against.
    let startup: Vec<usize> = (0..all.len())
        .filter(|&i| all[i].readiness == translate::Readiness::Ready)
        .collect();
    if startup.is_empty() {
        if all.is_empty() {
            log::error!(
                "TL: no provider configured - add one in Settings, or set \
                 TRANSLATE_<NAME>_API_KEY / OPENROUTER_API_KEY"
            );
        } else {
            log::error!(
                "TL: no provider is callable — {}",
                translate::describe(&all.iter().map(|p| p.info()).collect::<Vec<_>>()),
            );
        }
        set_tl_status(app, "error");
        return;
    }

    let n = startup.len();
    // Resume from whatever the UI last selected, rounded to the next callable
    // entry, so a pinned provider survives a restart.
    let first = startup
        .iter()
        .position(|&i| i >= active.load(Ordering::Relaxed))
        .unwrap_or(0);
    let mut rejected = 0usize;
    for step in 0..n {
        let i = startup[(first + step) % n];
        let p = &all[i];
        match check_credentials(&agent, p) {
            Ok(()) => {
                log::info!("TL: {} key OK", p.label);
                active.store(i, Ordering::Relaxed);
                break;
            }
            Err(CredError::Unauthorized) => {
                log::error!("TL: {} rejected the API key (401)", p.label);
                rejected += 1;
            }
            // A transient network failure at startup should not disable
            // translation — carry on and let per-request retries handle it.
            Err(CredError::Other(e)) => {
                log::warn!("TL: {} key check inconclusive ({e}) — using it anyway", p.label);
                active.store(i, Ordering::Relaxed);
                break;
            }
        }
    }
    if rejected == n {
        log::error!(
            "TL: every provider rejected its key — check the keys in Settings, or \
             TRANSLATE_<NAME>_API_KEY / OPENROUTER_API_KEY"
        );
        set_tl_status(app, "error");
        return;
    }
    set_tl_status(app, "ready");
    drop(startup);
    drop(all);

    /// Consecutive failures on the active provider before falling forward.
    /// Two, not one: a single 429 or blip is what the per-request retry is for.
    const FAILOVER_AFTER: u32 = 2;
    let mut consecutive_failures = 0u32;
    // Tracks which provider the failure counter belongs to. A switch from the
    // UI must not inherit the outgoing provider's strikes, or the new one can
    // be failed over on its very first error.
    let mut counting_for = active.load(Ordering::Relaxed);
    // Rolling context: the last few successful (source, translated) pairs,
    // injected as prior chat turns.  Gives the model cross-subtitle context —
    // pronouns, names, and topic continuity — which matters a lot for Korean,
    // where subjects are routinely omitted and must be inferred.
    const CTX_PAIRS: usize = 3;
    let mut history: std::collections::VecDeque<(String, String)> =
        std::collections::VecDeque::with_capacity(CTX_PAIRS);
    // Set when coalescing passed over a request to finish a final first. The
    // channel has no push-back, so it is held here until the next iteration.
    let mut deferred: Option<TranslationRequest> = None;
    // What the history was collected under. The prior turns are re-rendered
    // with the CURRENT `[source→target]` tag, so carrying them across a change
    // tells the model a Korean line was English.
    let mut ctx_for: Option<(String, SubtitleMode)> = None;
    let mut ctx_end_ms: u64 = 0;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut req = match deferred.take() {
            Some(r) => r,
            None => match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(r) => r,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            },
        };
        // If we fell behind, drop stale PREVIEWS and translate the newest
        // request — an old preview is worthless, since the line it previewed
        // has already been spoken to the end.
        //
        // A final is never dropped this way. It is the version that stays on
        // screen and the only one allowed to become context, so skipping it
        // would leave that subtitle in its source language for good. When a
        // final is passed over in favour of something newer, it is translated
        // first and the newer request is put back at the front of the queue.
        let mut skipped = 0u32;
        while let Ok(newer) = rx.try_recv() {
            let (now, later) = coalesce(req, newer);
            req = now;
            if later.is_some() {
                deferred = later;
                break;
            }
            skipped += 1;
        }
        if skipped > 0 {
            log::info!("TL: backlog — skipped {skipped} stale preview(s), translating newest");
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
                is_final: !req.is_partial,
                started_at_ms: Some(req.started_at_ms),
                ended_at_ms: Some(req.ended_at_ms),
            };
            let _ = app.emit("subtitle_update", update);
            continue;
        }

        let target = req.mode.target_lang();
        let kind = if req.is_partial { "preview" } else { "final" };
        log::info!("TL [{}→{} {kind}]: {:?}", req.source_lang, target, req.source_text);

        // Source is already in the target language — nothing to translate.
        if is_noop_translation(&req.source_lang, target) {
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

        // Context is only useful while it is still the same conversation.
        let now = (req.source_lang.clone(), req.mode);
        if !context_survives(ctx_for.as_ref(), &now, req.started_at_ms.saturating_sub(ctx_end_ms)) {
            history.clear();
        }
        ctx_for = Some(now);

        let prev: Vec<(&str, &str)> = history
            .iter()
            .map(|(s, t)| (s.as_str(), t.as_str()))
            .collect();
        let t_tl = std::time::Instant::now();
        // Read the live list every request: Settings can have switched the
        // active provider, or added, removed or reordered entries, since the
        // last subtitle.
        let Some((idx, provider)) = translate::pick_provider(app, active) else {
            log::warn!("TL: no callable provider — check the list in Settings");
            continue;
        };
        if idx != counting_for {
            consecutive_failures = 0;
            counting_for = idx;
        }
        let context = state::read_state(app, |s| s.context.clone()).unwrap_or_default();
        match call_translate(
            &agent, &provider, &req.source_lang, &req.source_text, req.mode, &prev, &context,
        ) {
            Ok(translated) => {
                consecutive_failures = 0;
                let tl_ms = t_tl.elapsed().as_millis();
                log::info!(
                    "TL [{} → {} {kind}] {tl_ms}ms → {:?}",
                    req.source_lang, req.mode.target_lang(), translated,
                );
                // Previews are fragments of a sentence still being spoken.
                // Feeding one back as a completed turn would teach the model
                // that half-sentences are what a finished translation looks
                // like, and every later subtitle would inherit that.
                if !req.is_partial {
                    if history.len() == CTX_PAIRS {
                        history.pop_front();
                    }
                    history.push_back((req.source_text.clone(), translated.clone()));
                    ctx_end_ms = req.ended_at_ms;
                }
                emit_translated(app, &req, translated);
            }
            Err(e) => {
                let tl_ms = t_tl.elapsed().as_millis();
                log::warn!("TL [{} {tl_ms}ms] error: {e}", req.source_lang);
                // Don't emit — the source-only subtitle (emitted by ASR worker)
                // stays on screen.

                // Fall forward once the active provider looks genuinely down
                // rather than briefly unlucky. Cheap and stateless: the next
                // success on the new provider resets the counter, and nothing
                // pins us there, so a later rotation can come back around.
                consecutive_failures += 1;
                if consecutive_failures >= FAILOVER_AFTER {
                    // +1 rather than a length-aware wrap: `pick_provider` takes
                    // the index modulo the live list, so this lands correctly
                    // even if the list changed size since the failure.
                    let next = idx + 1;
                    log::warn!(
                        "TL: {} failed {consecutive_failures}x — falling forward",
                        provider.label,
                    );
                    active.store(next, Ordering::Relaxed);
                    counting_for = next;
                    consecutive_failures = 0;
                    // Let the UI show which provider is live now, not the one
                    // that was picked at startup.
                    state::update_and_emit(app, |_| {});
                }
            }
        }
    }

    set_tl_status(app, "unloaded");
    log::info!("translate worker exited");
}

/// Whether a request needs no translation because it is already in the target.
///
/// Chinese is deliberately included, and this is worth knowing before
/// "fixing" it: "zh" is one code for two scripts, so a Chinese source under
/// the 繁中 target can arrive in Simplified and stay that way. That is a
/// product decision, not an oversight — a reader of Chinese reads both
/// scripts, so paying an API call and its latency on every line to convert
/// between them buys nothing.
///
/// The script targets still do their job where it matters: ko→繁中 and
/// en→简中 are real translations and honour the script they name. Only a
/// source that is already Chinese short-circuits.
fn is_noop_translation(source_lang: &str, target: &str) -> bool {
    source_lang == target
}

/// Choose between the request in hand and a newer one waiting behind it.
///
/// Returns `(translate now, keep for later)`. A preview in hand is dropped for
/// the newer request: the line it previewed has already been spoken to the end,
/// so translating it buys nothing. A final in hand is never dropped — it is the
/// version that stays on screen and the only one allowed to become context, so
/// losing it would leave that subtitle in its source language permanently.
fn coalesce(
    current: TranslationRequest,
    newer: TranslationRequest,
) -> (TranslationRequest, Option<TranslationRequest>) {
    if current.is_partial {
        (newer, None)
    } else {
        (current, Some(newer))
    }
}

/// Whether the rolling context still applies to the request about to be sent.
///
/// Two ways it stops applying. A different source language or subtitle mode
/// means the prior turns would be re-labelled with the wrong
/// `[source→target]` tag, which is worse than no context at all: it states
/// something false. And a long silence usually means a different scene or
/// speaker, where stale names and topic bias the translation rather than
/// steady it.
fn context_survives(
    was: Option<&(String, SubtitleMode)>,
    now: &(String, SubtitleMode),
    gap_ms: u64,
) -> bool {
    /// Silence long enough to assume the topic moved on.
    const MAX_GAP_MS: u64 = 30_000;
    was.is_some_and(|w| w == now) && gap_ms <= MAX_GAP_MS
}

// ── credential check ────────────────────────────────────────────────────────

enum CredError {
    Unauthorized,
    Other(String),
}

/// Authenticated GET /models — catches a bad key before the first subtitle.
///
/// `/models` rather than OpenRouter's `/key`, because it is the one listing
/// endpoint every OpenAI-compatible provider exposes. The trade-off: OpenRouter
/// serves `/models` unauthenticated, so a bad key there passes this check and
/// surfaces on the first real request instead. Google's endpoint does reject it.
fn check_credentials(agent: &ureq::Agent, provider: &Provider) -> Result<(), CredError> {
    match agent
        .get(&provider.key_url())
        .set("Authorization", &format!("Bearer {}", provider.api_key))
        .set("User-Agent", USER_AGENT)
        .call()
    {
        Ok(_) => Ok(()),
        // 403 as well as 401: Google answers an invalid key with PERMISSION_DENIED.
        Err(ureq::Error::Status(401 | 403, _)) => Err(CredError::Unauthorized),
        Err(e) => Err(CredError::Other(e.to_string())),
    }
}

// ── prompting ───────────────────────────────────────────────────────────────

fn build_system_prompt(source_lang: &str, target_name: &str, context: &str) -> String {
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

    // What the user says this audio is about. Byte-identical on every request,
    // so a provider with prompt caching charges for it once; the note is capped
    // at 400 chars where it is stored, which bounds the cost where it is not.
    let context = context.trim();
    if !context.is_empty() {
        p.push_str(
            " The following describes what is being watched. Use it for names, \
             titles and terminology; it is background, never something to \
             translate or mention: ",
        );
        p.push_str(context);
    }

    p
}

/// Call OpenRouter and return the translation in the target language.
/// `prev` holds the last few (source, translated) pairs, injected as prior
/// chat turns to keep vocabulary, names, and topic continuity consistent.
#[allow(clippy::too_many_arguments)]
fn call_translate(
    agent: &ureq::Agent,
    provider: &Provider,
    source_lang: &str,
    text: &str,
    mode: crate::types::SubtitleMode,
    prev: &[(&str, &str)],
    context: &str,
) -> Result<String, String> {
    let source_name = match source_lang {
        "ko" => "Korean",
        "en" => "English",
        "zh" => "Chinese",
        "ja" => "Japanese",
        other => other,
    };
    let target_name = mode.target_name();

    let system = build_system_prompt(source_lang, target_name, context);
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
        "model": provider.model,
        "messages": messages,
        "max_tokens": 200,
        "temperature": 0,
    });
    // Subtitles must not wait on a reasoning preamble.  Models that ignore this
    // are handled by strip_think_tags below.  Endpoints that REFUSE the field
    // (400 "Reasoning is mandatory", and Google's compat layer, which rejects it
    // outright) get it dropped by post_with_retry — and remembered, so only the
    // first subtitle of the session pays for that discovery.
    if !provider.reasoning_unsupported.load(Ordering::Relaxed) {
        body["reasoning"] = serde_json::json!({ "enabled": false });
    }
    // Let the user pin specific upstream providers (e.g. to force a low-latency
    // one) without a code change.  OpenRouter-specific; ignored elsewhere.
    if let Some(order) = &provider.provider_order {
        body["provider"] = serde_json::json!({ "order": order, "allow_fallbacks": true });
    }

    let json = post_with_retry(agent, provider, &body)?;

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
                provider.model
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
    provider: &Provider,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    const RETRY_DELAY_MS: u64 = 250;
    let url = provider.completions_url();
    let reasoning_unsupported = &provider.reasoning_unsupported;
    let mut body = body.clone();
    let mut dropped_reasoning = false;
    let mut attempt = 0;
    loop {
        let req = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .set("User-Agent", USER_AGENT)
            .set("Authorization", &format!("Bearer {}", provider.api_key))
            .set("HTTP-Referer", REFERER)
            .set("X-Title", TITLE);

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
                    // Remember it for the rest of the session. Google's
                    // OpenAI-compatible endpoint rejects the field on EVERY
                    // call, so without this every subtitle would pay two round
                    // trips instead of one.
                    if !reasoning_unsupported.swap(true, Ordering::Relaxed) {
                        log::warn!(
                            "TL: {} ({}) rejects reasoning.enabled=false — dropping it for the \
                             rest of this session. If the model reasons anyway it will spend \
                             max_tokens thinking, which can leave the translation empty.",
                            provider.label,
                            provider.model
                        );
                    }
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
        is_final: !req.is_partial,
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
    #[test]
    fn same_language_needs_no_translation() {
        assert!(is_noop_translation("ko", "ko"));
        assert!(is_noop_translation("en", "en"));
    }

    #[test]
    fn chinese_to_chinese_passes_straight_through() {
        // Deliberate: whisper may hand back either script, and converting
        // between them costs a call and its latency on every single line to
        // produce something the reader could already read.
        assert!(is_noop_translation("zh", "zh"));
    }

    #[test]
    fn different_languages_always_translate() {
        assert!(!is_noop_translation("ko", "zh"));
        assert!(!is_noop_translation("en", "ko"));
    }

    fn req(id: &str, is_partial: bool) -> TranslationRequest {
        TranslationRequest {
            id: id.into(),
            source_lang: "ko".into(),
            source_text: "x".into(),
            mode: SubtitleMode::Zh,
            is_partial,
            started_at_ms: 0,
            ended_at_ms: 0,
        }
    }

    #[test]
    fn a_stale_preview_is_dropped_for_the_newer_request() {
        let (now, later) = coalesce(req("old", true), req("new", true));
        assert_eq!(now.id, "new");
        assert!(later.is_none(), "nothing to come back to");
    }

    #[test]
    fn a_final_is_never_dropped_by_coalescing() {
        // The invariant that matters: skipping a final leaves that subtitle in
        // its source language for good, because nothing sends it again.
        let (now, later) = coalesce(req("final", false), req("newer", true));
        assert_eq!(now.id, "final");
        assert_eq!(later.expect("deferred").id, "newer");
    }

    #[test]
    fn context_is_kept_within_one_conversation() {
        let k = ("ko".to_string(), SubtitleMode::Zh);
        assert!(context_survives(Some(&k), &k, 1_000));
    }

    #[test]
    fn context_is_dropped_when_the_language_or_mode_changes() {
        let ko_zh = ("ko".to_string(), SubtitleMode::Zh);
        let en_zh = ("en".to_string(), SubtitleMode::Zh);
        let ko_en = ("ko".to_string(), SubtitleMode::En);
        // Prior turns get re-tagged with the current pair, so keeping them
        // across either change would label them with a language they are not.
        assert!(!context_survives(Some(&ko_zh), &en_zh, 0));
        assert!(!context_survives(Some(&ko_zh), &ko_en, 0));
    }

    #[test]
    fn context_is_dropped_after_a_long_silence() {
        let k = ("ko".to_string(), SubtitleMode::Zh);
        assert!(context_survives(Some(&k), &k, 30_000), "at the limit");
        assert!(!context_survives(Some(&k), &k, 30_001));
    }

    #[test]
    fn there_is_no_context_on_the_first_subtitle() {
        assert!(!context_survives(None, &("ko".to_string(), SubtitleMode::Zh), 0));
    }

    use super::*;

    #[test]
    fn strip_think_tags_removes_reasoning_preamble() {
        assert_eq!(strip_think_tags("<think>hmm</think>你好"), "你好");
        assert_eq!(strip_think_tags("  你好  "), "你好");
        // Only the text after the LAST closing tag survives.
        assert_eq!(strip_think_tags("<think>a</think>x<think>b</think>y"), "y");
    }
}
