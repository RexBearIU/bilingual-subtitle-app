//! Shared IPC types. Keep in sync with `docs/IPC-CONTRACT.md` and `src/lib/types.ts`.

use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// The two-language display mode. Serializes to `"zh-ko"` / `"zh-en"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubtitleMode {
    ZhKo,
    ZhEn,
}

impl Default for SubtitleMode {
    fn default() -> Self {
        SubtitleMode::ZhKo
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
    pub font_size: u32,
    pub click_through: bool,
    pub always_on_top: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rms: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl EngineStatus {
    pub fn from_state(s: &AppState) -> Self {
        EngineStatus {
            capture: if s.captioning { "running" } else { "stopped" }.into(),
            // Engines are wired in M4/M5; unloaded until then.
            asr: "unloaded".into(),
            translation: "unloaded".into(),
            mode: s.mode,
            font_size: s.font_size,
            click_through: s.click_through,
            always_on_top: s.always_on_top,
            rms: None,
            message: None,
        }
    }
}
