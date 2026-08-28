//! Shared IPC types. Keep in sync with `docs/IPC-CONTRACT.md` and `src/lib/types.ts`.

use serde::{Deserialize, Serialize};

use crate::state::AppState;
use crate::translate::ProviderInfo;

/// How the overlay window treats the mouse.
///
/// Was a bool. It grew a third state because neither of the two was what the
/// window actually wants: fully interactive means a mostly-empty transparent
/// window swallows clicks meant for whatever is behind it, and fully
/// click-through means the controls are unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClickThrough {
    /// The whole window takes the mouse, empty areas included.
    Off,
    /// The mouse passes through except over the control bar and the settings
    /// panel — the regions the frontend reports via `set_hit_regions`. Default.
    #[default]
    Auto,
    /// Nothing in the window is clickable; the mouse always goes behind it.
    On,
}

/// Source-language hint passed to Whisper.
/// `Auto` lets Whisper detect per-chunk (best for multilingual streams).
/// A specific code locks detection and slightly improves accuracy.
/// Serialises as `"auto"` / `"zh"` / `"ko"` / `"en"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceHint { #[default] Auto, Zh, Ko, En }

impl SourceHint {
    /// Returns the ISO-639-1 code to pass to Whisper, or `None` for auto-detect.
    pub fn lang_code(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Zh => Some("zh"),
            Self::Ko => Some("ko"),
            Self::En => Some("en"),
        }
    }
}

/// Target translation language, or `None` to show source text only.
/// Serialises as `"none"` / `"zh"` / `"ko"` / `"en"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleMode {
    /// Show source text only — no translation.
    #[serde(rename = "none")]
    NoTranslate,
    /// Translate everything to Traditional Chinese (繁體中文).
    #[default]
    Zh,
    /// Translate everything to Korean (한국어).
    Ko,
    /// Translate everything to English.
    En,
}

impl SubtitleMode {
    /// ISO-639-1 code of the target language (empty string for NoTranslate).
    pub fn target_lang(self) -> &'static str {
        match self {
            Self::NoTranslate => "",
            Self::Zh => "zh",
            Self::Ko => "ko",
            Self::En => "en",
        }
    }
    /// Human-readable name for translation prompts.
    pub fn target_name(self) -> &'static str {
        match self {
            Self::NoTranslate => "",
            Self::Zh => "Traditional Chinese (繁體中文)",
            Self::Ko => "Korean (한국어)",
            Self::En => "English",
        }
    }
}

/// Populated subtitle strings; only the two languages for the active mode are set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubtitleTexts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zh: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ko: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub en: Option<String>,
}

/// Payload of the `subtitle_update` event (Rust → frontend).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleUpdate {
    pub id: String,
    /// Detected source language: `"ko" | "en" | "zh"`.
    pub source_lang: String,
    pub source_text: String,
    pub mode: SubtitleMode,
    pub subtitles: SubtitleTexts,
    pub is_final: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
}

/// A Windows process that is currently outputting audio.
/// Returned by the `list_audio_processes` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioProcess {
    pub pid: u32,
    /// Basename of the executable (e.g. `"chrome.exe"`).
    pub name: String,
}

/// Payload of the `engine_status` event and the `get_status` command return.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    /// `"stopped" | "running" | "error"`
    pub capture: String,
    /// `"unloaded" | "loading" | "ready" | "error"`
    pub asr: String,
    /// `"unloaded" | "loading" | "ready" | "error"`
    pub translation: String,
    pub mode: SubtitleMode,
    pub source_hint: SourceHint,
    pub font_size: u32,
    pub click_through: ClickThrough,
    /// Whether the mouse is passing through *right now*. In `Auto` this flips
    /// as the cursor moves; the UI uses it only for a live indicator.
    pub click_through_active: bool,
    pub always_on_top: bool,
    /// Subtitle background opacity (0.0–1.0).
    pub subtitle_opacity: f64,
    /// Translation providers in preference order, without their API keys.
    pub translate_providers: Vec<ProviderInfo>,
    /// Index into `translate_providers` the worker is currently using.
    pub translate_active: usize,
    /// True when `TRANSLATE_PROVIDERS` supplied the list, which makes the
    /// Settings panel's key and model fields inert.
    pub translate_env_managed: bool,
    /// OpenRouter model slug in use for translation.
    ///
    /// Only meaningful when `translate_env_managed` is false — otherwise the
    /// per-provider models in `translate_providers` are what actually run.
    pub openrouter_model: String,
    /// Whether an API key is available (env var or settings file).
    /// The key itself is never sent to the frontend.
    pub openrouter_key_set: bool,
    /// VAD speech threshold (linear RMS, 0.0–1.0).
    pub speech_threshold: f32,
    pub music_mode: bool,
    /// Active ASR backend: "whisper" | "sensevoice".
    pub asr_backend: String,
    /// Whisper model size: "turbo" | "large".
    pub whisper_model: String,
    /// SenseVoice model precision: "int8" | "fp32".
    pub sensevoice_precision: String,
    /// Currently targeted audio process; `null` = system-wide loopback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_target: Option<AudioProcess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rms: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl EngineStatus {
    pub fn from_state(s: &AppState) -> Self {
        EngineStatus {
            capture: if s.captioning { "running" } else { "stopped" }.into(),
            asr: s.asr_status.clone(),
            translation: s.translation_status.clone(),
            mode: s.mode,
            source_hint: s.source_hint,
            font_size: s.font_size,
            click_through: s.click_through,
            click_through_active: s.click_through_active,
            always_on_top: s.always_on_top,
            subtitle_opacity: s.subtitle_opacity,
            translate_providers: s.translate_providers.clone(),
            translate_active: s.translate_active.load(std::sync::atomic::Ordering::Relaxed),
            translate_env_managed: s.translate_env_managed,
            openrouter_model: if s.openrouter_model.is_empty() {
                crate::translate::default_model().to_string()
            } else {
                s.openrouter_model.clone()
            },
            openrouter_key_set: s.openrouter_key_set,
            speech_threshold: s.speech_threshold,
            music_mode: s.music_mode,
            asr_backend: s.asr_backend.clone(),
            whisper_model: s.whisper_model.clone(),
            sensevoice_precision: s.sensevoice_precision.clone(),
            capture_target: s.capture_target.clone(),
            rms: if s.captioning { Some(s.rms) } else { None },
            message: s.loopback_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_old_hand_written_impls() {
        // Guards the derive(Default) refactor: SubtitleMode's default is Zh even
        // though NoTranslate is the first variant, and SourceHint's is Auto.
        assert_eq!(SubtitleMode::default(), SubtitleMode::Zh);
        assert_eq!(SourceHint::default(), SourceHint::Auto);
    }

    #[test]
    fn source_hint_lang_code() {
        assert_eq!(SourceHint::Auto.lang_code(), None);
        assert_eq!(SourceHint::Zh.lang_code(), Some("zh"));
        assert_eq!(SourceHint::Ko.lang_code(), Some("ko"));
        assert_eq!(SourceHint::En.lang_code(), Some("en"));
    }

    #[test]
    fn subtitle_mode_target_lang_and_name() {
        assert_eq!(SubtitleMode::NoTranslate.target_lang(), "");
        assert_eq!(SubtitleMode::Zh.target_lang(), "zh");
        assert_eq!(SubtitleMode::Ko.target_lang(), "ko");
        assert_eq!(SubtitleMode::En.target_lang(), "en");
        assert_eq!(SubtitleMode::NoTranslate.target_name(), "");
        assert!(SubtitleMode::Zh.target_name().contains("繁體中文"));
    }

    #[test]
    fn serde_uses_the_wire_names() {
        // The frontend/IPC contract depends on these exact strings.
        assert_eq!(serde_json::to_string(&SubtitleMode::NoTranslate).unwrap(), "\"none\"");
        assert_eq!(serde_json::to_string(&SubtitleMode::Zh).unwrap(), "\"zh\"");
        assert_eq!(serde_json::to_string(&SourceHint::Auto).unwrap(), "\"auto\"");
        assert_eq!(
            serde_json::from_str::<SubtitleMode>("\"none\"").unwrap(),
            SubtitleMode::NoTranslate
        );
        assert_eq!(
            serde_json::from_str::<SourceHint>("\"ko\"").unwrap(),
            SourceHint::Ko
        );
    }
}
