//! Translation pipeline: receives `TranslationRequest`s from the ASR worker
//! and calls a hosted OpenAI-compatible chat-completions endpoint to produce
//! subtitles in the selected target language.
//!
//! Providers are configuration, not code. Any endpoint speaking the
//! OpenAI-compatible `/chat/completions` shape works — OpenRouter, Google AI
//! Studio, Groq — and several can be listed so one failing does not take
//! translation down with it.

pub mod remote;

use std::sync::atomic::AtomicBool;

use tauri::{AppHandle, Manager};

use crate::settings::{PersistSettings, SettingsPath};
use crate::types::SubtitleMode;

/// Built-in presets so a provider can be named instead of spelled out.
/// `(name, base_url, default_model)`
const PRESETS: &[(&str, &str, &str)] = &[
    (
        "aistudio",
        "https://generativelanguage.googleapis.com/v1beta/openai/",
        "gemini-3.5-flash-lite",
    ),
    (
        "openrouter",
        "https://openrouter.ai/api/v1",
        "google/gemini-3.5-flash-lite",
    ),
    (
        "groq",
        "https://api.groq.com/openai/v1",
        "llama-3.3-70b-versatile",
    ),
];

/// Used when nothing is configured at all.
pub const DEFAULT_PROVIDER: &str = "openrouter";

/// Model shown in the UI when the user has not chosen one.
pub fn default_model() -> &'static str {
    preset(DEFAULT_PROVIDER).map(|(_, m)| m).unwrap_or("")
}

fn preset(name: &str) -> Option<(&'static str, &'static str)> {
    PRESETS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, base, model)| (*base, *model))
}

/// One configured endpoint.
///
/// Deliberately NOT stored in `AppState`: that derives `Debug` and is logged on
/// state changes, which would print the API key.
pub struct Provider {
    /// Config key and log label (`aistudio`, `openrouter`, …).
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Optional upstream preference — OpenRouter's `provider.order`.
    pub provider_order: Option<Vec<String>>,
    /// Learned at runtime: this endpoint rejects `reasoning.enabled=false`.
    ///
    /// Google's OpenAI-compatible endpoint rejects it on EVERY call, so
    /// remembering the first rejection saves a wasted round trip per subtitle.
    /// Per-provider because it is a property of the endpoint, not the app.
    pub reasoning_unsupported: AtomicBool,
}

impl Provider {
    /// `{base_url}/chat/completions`, tolerating a trailing slash.
    pub fn completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    pub fn key_url(&self) -> String {
        format!("{}/models", self.base_url.trim_end_matches('/'))
    }
}

/// Everything the translate worker needs, in preference order.
pub struct RemoteConfig {
    /// At least one; tried in order, later entries are fallbacks.
    pub providers: Vec<Provider>,
    /// Optional attribution headers (OpenRouter leaderboards; ignored elsewhere).
    pub referer: Option<String>,
    pub title: Option<String>,
}

impl RemoteConfig {
    /// Resolve from env vars first, then persisted settings.
    ///
    /// Two shapes are accepted:
    ///
    /// 1. **Multi-provider.** `TRANSLATE_PROVIDERS=aistudio,openrouter` plus,
    ///    for each name, `TRANSLATE_<NAME>_API_KEY` and optionally
    ///    `_BASE_URL` / `_MODEL` / `_PROVIDER_ORDER`. Names with a preset need
    ///    only the key.
    ///
    /// 2. **Single provider (legacy).** `OPENROUTER_API_KEY` / `_MODEL` /
    ///    `_BASE_URL` / `_PROVIDER_ORDER`, falling back to `settings.json`.
    ///    Kept working so existing setups and the Settings panel are unaffected.
    ///
    /// Returns `Err` only when no usable provider has a key.
    pub fn resolve(app: &AppHandle) -> Result<Self, String> {
        let saved = app
            .try_state::<SettingsPath>()
            .map(|p| PersistSettings::load(&p.0))
            .unwrap_or_default();

        let mut providers = Vec::new();
        let mut skipped: Vec<String> = Vec::new();

        if let Some(list) = non_empty_env("TRANSLATE_PROVIDERS") {
            for name in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                match build_named(name) {
                    Some(p) => providers.push(p),
                    None => skipped.push(name.to_string()),
                }
            }
        }

        // Legacy single-provider config. Also the path the Settings panel writes.
        if providers.is_empty() {
            if let Some(p) = build_legacy(&saved) {
                providers.push(p);
            }
        }

        if providers.is_empty() {
            let hint = if skipped.is_empty() {
                String::new()
            } else {
                format!(" (no API key for: {})", skipped.join(", "))
            };
            return Err(format!(
                "no translation provider configured{hint} — set TRANSLATE_<NAME>_API_KEY \
                 (with TRANSLATE_PROVIDERS), or OPENROUTER_API_KEY, or the key in Settings"
            ));
        }
        if !skipped.is_empty() {
            log::warn!("TL: skipping providers with no API key: {}", skipped.join(", "));
        }

        Ok(RemoteConfig {
            providers,
            referer: Some("https://github.com/RexBearIU/bilingual-subtitle-app".into()),
            title: Some("Bilingual Subtitles".into()),
        })
    }

    /// Names in preference order, for logging.
    pub fn names(&self) -> String {
        self.providers
            .iter()
            .map(|p| format!("{}({})", p.name, p.model))
            .collect::<Vec<_>>()
            .join(" → ")
    }
}

/// Build `TRANSLATE_<NAME>_*`. Returns `None` when no API key is set.
fn build_named(name: &str) -> Option<Provider> {
    let upper = name.to_uppercase().replace('-', "_");
    let var = |suffix: &str| non_empty_env(&format!("TRANSLATE_{upper}_{suffix}"));

    let api_key = var("API_KEY")?;
    let (preset_base, preset_model) = preset(name).unwrap_or(("", ""));

    let base_url = var("BASE_URL").or_else(|| non_empty(preset_base.to_string()))?;
    let model = var("MODEL")
        .or_else(|| non_empty(preset_model.to_string()))
        .unwrap_or_default();
    if model.is_empty() {
        log::warn!("TL: provider {name:?} has no model — set TRANSLATE_{upper}_MODEL");
        return None;
    }

    Some(Provider {
        name: name.to_string(),
        base_url,
        api_key,
        model,
        provider_order: var("PROVIDER_ORDER").map(split_csv),
        reasoning_unsupported: AtomicBool::new(false),
    })
}

/// Build from the original single-provider variables, then `settings.json`.
fn build_legacy(saved: &PersistSettings) -> Option<Provider> {
    let api_key = non_empty_env("OPENROUTER_API_KEY")
        .or_else(|| non_empty(saved.openrouter_api_key.clone()))?;

    let base_url = non_empty_env("OPENROUTER_BASE_URL")
        .unwrap_or_else(|| preset(DEFAULT_PROVIDER).map(|(b, _)| b.to_string()).unwrap_or_default());

    // Name the provider after the endpoint actually in use, so the logs do not
    // claim "openrouter" while pointing at Google.
    let name = PRESETS
        .iter()
        .find(|(_, base, _)| base_url.trim_end_matches('/') == base.trim_end_matches('/'))
        .map(|(n, _, _)| (*n).to_string())
        .unwrap_or_else(|| "custom".to_string());

    let model = non_empty_env("OPENROUTER_MODEL")
        .or_else(|| non_empty(saved.openrouter_model.clone()))
        .or_else(|| preset(&name).map(|(_, m)| m.to_string()))
        .unwrap_or_default();

    Some(Provider {
        name,
        base_url,
        api_key,
        model,
        provider_order: non_empty_env("OPENROUTER_PROVIDER_ORDER").map(split_csv),
        reasoning_unsupported: AtomicBool::new(false),
    })
}

fn split_csv(v: String) -> Vec<String> {
    v.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_cover_the_documented_names() {
        for name in ["aistudio", "openrouter", "groq"] {
            let (base, model) = preset(name).expect("preset missing");
            assert!(base.starts_with("https://"), "{name} base_url");
            assert!(!model.is_empty(), "{name} model");
        }
        assert!(preset("nope").is_none());
    }

    #[test]
    fn completions_url_tolerates_a_trailing_slash() {
        // Google's documented base URL ends in '/', OpenRouter's does not.
        let mk = |base: &str| Provider {
            name: "t".into(),
            base_url: base.into(),
            api_key: "k".into(),
            model: "m".into(),
            provider_order: None,
            reasoning_unsupported: AtomicBool::new(false),
        };
        assert_eq!(
            mk("https://x/v1/").completions_url(),
            "https://x/v1/chat/completions"
        );
        assert_eq!(
            mk("https://x/v1").completions_url(),
            "https://x/v1/chat/completions"
        );
    }

    #[test]
    fn split_csv_trims_and_drops_blanks() {
        assert_eq!(split_csv(" a , ,b ".into()), vec!["a".to_string(), "b".into()]);
    }
}
