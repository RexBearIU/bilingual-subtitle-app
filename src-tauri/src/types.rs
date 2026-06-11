//! Shared IPC types. Keep in sync with `docs/IPC-CONTRACT.md` and `src/lib/types.ts`.

use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Source-language hint passed to Whisper.
/// `Auto` lets Whisper detect per-chunk (best for multilingual streams).
/// A specific code locks detection and slightly improves accuracy.
/// Serialises as `"auto"` / `"zh"` / `"ko"` / `"en"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceHint { Auto, Zh, Ko, En }

impl Default for SourceHint {
    fn default() -> Self { SourceHint::Auto }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleMode {
    /// Show source text only — no translation.
    #[serde(rename = "none")]
    NoTranslate,
    /// Translate everything to Traditional Chinese (繁體中文).
    Zh,
    /// Translate everything to Korean (한국어).
    Ko,
    /// Translate everything to English.
    En,
}

impl Default for SubtitleMode {
    fn default() -> Self { SubtitleMode::Zh }
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
    pub click_through: bool,
    pub always_on_top: bool,
    /// Subtitle background opacity (0.0–1.0).
    pub subtitle_opacity: f64,
    /// GPU layers for llama-server (0 = CPU, 36 = all GPU).
    pub llama_gpu_layers: u32,
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
            always_on_top: s.always_on_top,
            subtitle_opacity: s.subtitle_opacity,
            llama_gpu_layers: s.llama_gpu_layers,
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
