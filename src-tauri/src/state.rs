//! Application state, shared across commands via `tauri::State<Mutex<AppState>>`.

use std::sync::{Arc, atomic::AtomicBool};

use crate::types::{AudioProcess, SourceHint, SubtitleMode};

#[derive(Debug)]
pub struct AppState {
    pub mode: SubtitleMode,
    pub source_hint: SourceHint,
    pub font_size: u32,
    pub click_through: bool,
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
    /// GPU layers for llama-server (0 = CPU, 36 = full GPU).
    pub llama_gpu_layers: u32,
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
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            mode: SubtitleMode::default(),
            source_hint: SourceHint::default(),
            font_size: 28,
            click_through: false,
            always_on_top: true,
            captioning: false,
            rms: 0.0,
            audio_stop: None,
            asr_status: "unloaded".into(),
            translation_status: "unloaded".into(),
            subtitle_opacity: 0.55,
            llama_gpu_layers: 36,
            speech_threshold: 0.0, // 0 = adaptive auto-mode
            music_mode: false,
            music_mode_flag: Arc::new(AtomicBool::new(false)),
            capture_target: None,
        }
    }
}

/// Wrapper around the whisper-server child process.
/// Stored as separate managed state so `AppState` stays `Debug`-derivable.
/// The process is kept alive across Start/Stop cycles and only killed on Drop
/// (i.e. when the app exits), so model weights stay loaded in GPU memory.
pub struct WhisperProc(pub Option<std::process::Child>);

impl Drop for WhisperProc {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            log::info!("WhisperProc: server killed on exit");
        }
    }
}

/// Wrapper around the llama-server child process.
/// Stored as separate managed state so `AppState` stays `Debug`-derivable.
/// Same keep-alive policy as `WhisperProc`.
pub struct LlamaProc(pub Option<std::process::Child>);

impl Drop for LlamaProc {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            log::info!("LlamaProc: server killed on exit");
        }
    }
}
