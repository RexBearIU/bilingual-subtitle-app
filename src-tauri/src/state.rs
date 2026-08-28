//! Application state, shared across commands via `tauri::State<Mutex<AppState>>`.

use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize}};

use tauri::{AppHandle, Emitter, Manager};

use crate::translate::ProviderInfo;
use crate::types::{AudioProcess, ClickThrough, EngineStatus, SourceHint, SubtitleMode};

/// Lock AppState, apply `f`, then broadcast the resulting `engine_status`.
/// No-op if the state is unavailable or the lock is poisoned.
pub fn update_and_emit(app: &AppHandle, f: impl FnOnce(&mut AppState)) {
    if let Some(st) = app.try_state::<Mutex<AppState>>() {
        if let Ok(mut s) = st.lock() {
            f(&mut s);
            let _ = app.emit("engine_status", EngineStatus::from_state(&s));
        }
    }
}

/// Lock AppState and read a value out of it.
/// Returns `None` if the state is unavailable or the lock is poisoned.
pub fn read_state<T>(app: &AppHandle, f: impl FnOnce(&AppState) -> T) -> Option<T> {
    app.try_state::<Mutex<AppState>>()
        .and_then(|st| st.lock().ok().map(|s| f(&s)))
}

/// One clickable rectangle, in CSS pixels relative to the window client area.
/// Converted to physical screen coordinates by the hit-test thread, which is
/// the only place that knows the window's position and DPI scale.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct HitRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug)]
pub struct AppState {
    pub mode: SubtitleMode,
    pub source_hint: SourceHint,
    pub font_size: u32,
    pub click_through: ClickThrough,
    /// Whether the mouse is being passed through right now. Owned by the
    /// hit-test thread; in `Auto` it flips as the cursor crosses a region.
    pub click_through_active: bool,
    /// Rectangles the frontend wants to keep clickable, in CSS pixels relative
    /// to the window's client area. Empty means "nothing is interactive", which
    /// in `Auto` makes the whole window transparent to the mouse.
    pub hit_regions: Vec<HitRect>,
    pub always_on_top: bool,
    pub captioning: bool,
    /// Latest RMS from the capture thread (updated ~every 200 ms).
    pub rms: f32,
    /// Signal to stop the running capture/VAD/ASR threads (None when idle).
    pub audio_stop: Option<Arc<AtomicBool>>,
    /// ASR engine status: "unloaded" | "loading" | "ready" | "error"
    pub asr_status: String,
    /// Translation engine status: "unloaded" | "loading" | "ready" | "error"
    pub translation_status: String,
    /// Subtitle background opacity (0.0–1.0).  Sent in EngineStatus so the
    /// frontend can apply it as a CSS custom property.
    pub subtitle_opacity: f64,
    /// Translation providers in preference order, published by the pipeline
    /// after `RemoteConfig::resolve`. Key-free by construction.
    pub translate_providers: Vec<ProviderInfo>,
    /// Index into `translate_providers` the worker is using.
    ///
    /// Shared with the translate worker rather than copied, so a switch from
    /// the UI takes effect on the next subtitle instead of the next restart —
    /// and so a failover the worker performs is visible to the UI.
    pub translate_active: Arc<AtomicUsize>,
    /// True when `TRANSLATE_PROVIDERS` supplied the list.
    pub translate_env_managed: bool,
    /// OpenRouter model slug in use (empty = built-in default).
    /// The API key is deliberately kept out of `AppState` — this struct derives
    /// `Debug` and is logged on state changes.
    pub openrouter_model: String,
    /// Whether an API key was found (env var or settings file) at the last
    /// check.  Mirrored to the UI so it can show a "key missing" hint.
    pub openrouter_key_set: bool,
    /// VAD speech threshold override. 0 = adaptive auto-mode (recommended).
    /// > 0 = fixed RMS threshold (manual override).
    pub speech_threshold: f32,
    /// Music mode: bypass VAD, use fixed 10 s chunks + song-lyrics prompt.
    pub music_mode: bool,
    /// Shared with the VAD worker so toggling takes effect immediately.
    pub music_mode_flag: Arc<AtomicBool>,
    /// The process currently being captured (None = system-wide loopback).
    /// Changing this requires stopping and restarting the pipeline.
    pub capture_target: Option<AudioProcess>,
    /// ASR backend: "whisper" | "sensevoice". Takes effect on next asr-srv launch.
    pub asr_backend: String,
    /// Last process-loopback error, shown in UI until cleared.
    pub loopback_error: Option<String>,
    /// Whisper model size: "turbo" (large-v3-turbo, default) | "large" (large-v3 int8).
    pub whisper_model: String,
    /// SenseVoice model precision: "int8" (default, ~70 MB) | "fp32" (~220 MB, more accurate).
    pub sensevoice_precision: String,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            mode: SubtitleMode::default(),
            source_hint: SourceHint::default(),
            font_size: 28,
            click_through: ClickThrough::default(),
            click_through_active: false,
            hit_regions: Vec::new(),
            always_on_top: true,
            captioning: false,
            rms: 0.0,
            audio_stop: None,
            asr_status: "unloaded".into(),
            translation_status: "unloaded".into(),
            subtitle_opacity: 0.55,
            translate_providers: Vec::new(),
            translate_active: Arc::new(AtomicUsize::new(0)),
            translate_env_managed: false,
            openrouter_model: String::new(),
            openrouter_key_set: false,
            speech_threshold: 0.0, // 0 = adaptive auto-mode
            music_mode: false,
            music_mode_flag: Arc::new(AtomicBool::new(false)),
            capture_target: None,
            asr_backend: "whisper".into(),
            loopback_error: None,
            whisper_model: "turbo".into(),
            sensevoice_precision: "int8".into(),
        }
    }
}

/// Wrapper around the asr-srv child process.
/// Stored as separate managed state so `AppState` stays `Debug`-derivable.
/// The process is kept alive across Start/Stop cycles and only killed on Drop
/// (i.e. when the app exits), so model weights stay loaded in GPU memory.
pub struct AsrProc(pub Option<std::process::Child>);

impl Drop for AsrProc {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            log::info!("AsrProc: asr-srv killed on exit");
        }
    }
}
