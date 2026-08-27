//! Translation pipeline: receives `TranslationRequest`s from the ASR worker
//! and calls OpenRouter (OpenAI-compatible API) to produce subtitles in the
//! selected target language.

pub mod openrouter;

use tauri::{AppHandle, Manager};

use crate::settings::{PersistSettings, SettingsPath};
use crate::types::SubtitleMode;

/// Default OpenRouter API root.  Override with `OPENROUTER_BASE_URL` to point
/// at a proxy or a self-hosted OpenAI-compatible gateway.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Default translation model.  A small, fast, cheap instruct model beats a
/// large one here: subtitles are one or two sentences and latency is the
/// binding constraint.  Any OpenRouter slug works — see openrouter.ai/models.
pub const DEFAULT_MODEL: &str = "google/gemini-2.5-flash-lite";

/// Everything the translate worker needs to reach the hosted model.
///
/// Deliberately NOT stored in `AppState`: that derives `Debug` and is logged
/// on state changes, which would print the API key.
#[derive(Clone)]
pub struct RemoteConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    /// Optional upstream provider preference (OpenRouter `provider.order`).
    pub provider_order: Option<Vec<String>>,
    /// Optional attribution headers.
    pub referer: Option<String>,
    pub title: Option<String>,
}

impl RemoteConfig {
    /// Resolve config from env vars first, then persisted settings.
    ///
    /// Env wins so a dev can point one run at a different model or key without
    /// touching the saved settings file.
    ///
    /// Returns `Err` only when no API key can be found — every other field has
    /// a working default.
    pub fn resolve(app: &AppHandle) -> Result<Self, String> {
        let saved = app
            .try_state::<SettingsPath>()
            .map(|p| PersistSettings::load(&p.0))
            .unwrap_or_default();

        let api_key = non_empty_env("OPENROUTER_API_KEY")
            .or_else(|| non_empty(saved.openrouter_api_key.clone()))
            .ok_or_else(|| {
                "no OpenRouter API key — set the OPENROUTER_API_KEY environment \
                 variable or `openrouterApiKey` in settings.json"
                    .to_string()
            })?;

        let model = non_empty_env("OPENROUTER_MODEL")
            .or_else(|| non_empty(saved.openrouter_model.clone()))
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let base_url = non_empty_env("OPENROUTER_BASE_URL")
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        let provider_order = non_empty_env("OPENROUTER_PROVIDER_ORDER").map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        });

        Ok(RemoteConfig {
            api_key,
            model,
            base_url,
            provider_order,
            referer: Some("https://github.com/RexBearIU/bilingual-subtitle-app".into()),
            title: Some("Bilingual Subtitles".into()),
        })
    }
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(non_empty)
}

/// Sent from the ASR worker to the translation worker for each transcribed chunk.
pub struct TranslationRequest {
    /// Stable segment identifier — same as the source `subtitle_update` id.
    pub id: String,
    /// ISO-639-1 source language (`"ko"` / `"en"` / `"zh"`).
    pub source_lang: String,
    /// Source text as returned by asr-srv.
    pub source_text: String,
    /// Active subtitle display mode (drives which target language we need).
    pub mode: SubtitleMode,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
}
