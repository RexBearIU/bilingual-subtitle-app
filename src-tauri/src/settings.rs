//! Persistent settings — loaded at startup, written on change.
//!
//! Stored as JSON in `{app_data_dir}/settings.json`.
//! All fields have serde defaults so a missing/partial file still works.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{ClickThrough, SourceHint, SubtitleMode};

// ── types ────────────────────────────────────────────────────────────────────

/// Window geometry (physical pixels).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Default for OverlayRect {
    fn default() -> Self {
        OverlayRect { x: 220, y: 760, w: 900, h: 220 }
    }
}

/// A translation endpoint the user added from the Settings panel.
///
/// Deliberately a settings type and not a state type: it carries the API key,
/// and `AppState` derives `Debug` and is logged on every change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedProvider {
    /// The identity: keys the stored API key, `TRANSLATE_<NAME>_API_KEY` and
    /// the removal. Unique within the list. Not what the UI shows — see
    /// `label`, which exists so a cosmetic rename cannot orphan a key.
    pub name: String,
    /// What the UI shows. Empty = the built-in preset's label, else `name`.
    #[serde(default)]
    pub label: String,
    /// OpenAI-compatible root, e.g. `https://api.groq.com/openai/v1`.
    /// Empty means "use the built-in preset for this name".
    #[serde(default)]
    pub base_url: String,
    /// Stored in plaintext next to the other settings; never sent to the
    /// webview, never logged.
    #[serde(default)]
    pub api_key: String,
    /// Empty means "use the built-in preset's model for this name".
    #[serde(default)]
    pub model: String,
}

/// All user-configurable settings that survive app restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PersistSettings {
    /// Subtitle display mode (translate target).
    pub mode: SubtitleMode,
    /// Source language hint for Whisper (auto = per-chunk detection).
    #[serde(default)]
    pub source_hint: SourceHint,
    /// What this audio is about, in the user's own words: show, teams, the
    /// names likely to come up. Fed to both ASR and translation — see
    /// `AppState::context`.
    #[serde(default)]
    pub context: String,
    /// Subtitle font size (px).
    pub font_size: u32,
    /// Opacity of the subtitle background box, 0.0–1.0.
    /// Applied as a CSS custom property; does not affect the whole window.
    pub subtitle_opacity: f64,
    /// Overlay window geometry (physical pixels).
    pub overlay: OverlayRect,
    /// Translation endpoints added from the Settings panel, in preference
    /// order. Appended after anything `TRANSLATE_PROVIDERS` supplies, so
    /// adding one from the UI always takes effect rather than being silently
    /// outranked by the environment.
    #[serde(default)]
    pub providers: Vec<SavedProvider>,
    /// How the window treats the mouse: "off" | "auto" | "on".
    #[serde(default)]
    pub click_through: ClickThrough,
    /// OpenRouter API key.  Empty = fall back to the `OPENROUTER_API_KEY`
    /// environment variable; translation is disabled if neither is set.
    ///
    /// Stored in plaintext in the app data dir, same as any other setting —
    /// treat it as a scoped key and revoke it at openrouter.ai if the machine
    /// is shared.
    #[serde(default)]
    pub openrouter_api_key: String,
    /// OpenRouter model slug (e.g. `google/gemini-2.5-flash-lite`).
    /// Empty = use the built-in default.
    #[serde(default)]
    pub openrouter_model: String,
    /// VAD speech threshold override.
    /// `0.0` (default) = fully automatic (adaptive noise-floor EMA).
    /// Set to a linear RMS value (e.g. 0.032 = −30 dBFS) to hard-override.
    pub speech_threshold: f32,
    /// ASR backend: "whisper" (default) | "sensevoice".
    #[serde(default = "default_asr_backend")]
    pub asr_backend: String,
    /// Whisper model size: "turbo" (default) | "large" (large-v3 int8-quantized).
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    /// SenseVoice model precision: "int8" (default) | "fp32".
    #[serde(default = "default_sv_precision")]
    pub sensevoice_precision: String,
}

fn default_asr_backend() -> String { "whisper".into() }
fn default_whisper_model() -> String { "turbo".into() }
fn default_sv_precision() -> String { "int8".into() }

impl Default for PersistSettings {
    fn default() -> Self {
        PersistSettings {
            mode: SubtitleMode::default(),
            source_hint: SourceHint::default(),
            context: String::new(),
            font_size: 28,
            subtitle_opacity: 0.55,
            overlay: OverlayRect::default(),
            providers: Vec::new(),
            click_through: ClickThrough::default(),
            openrouter_api_key: String::new(),
            openrouter_model: String::new(),
            speech_threshold: 0.0,
            asr_backend: default_asr_backend(),
            whisper_model: default_whisper_model(),
            sensevoice_precision: default_sv_precision(),
        }
    }
}

// ── file I/O ─────────────────────────────────────────────────────────────────

impl PersistSettings {
    /// Load settings from `path`; on any error, return defaults silently.
    pub fn load(path: &Path) -> Self {
        let Ok(data) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    /// Write settings to `path`, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }
}

// ── managed state wrapper ─────────────────────────────────────────────────────

/// Holds the settings file path so commands can read/write without knowing the
/// app data directory.  Stored as Tauri managed state.
pub struct SettingsPath(pub PathBuf);
