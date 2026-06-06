//! Application state, shared across commands via `tauri::State<Mutex<AppState>>`.

use std::sync::{Arc, atomic::AtomicBool};

use crate::types::SubtitleMode;

#[derive(Debug)]
pub struct AppState {
    pub mode: SubtitleMode,
    pub font_size: u32,
    pub click_through: bool,
    pub always_on_top: bool,
    pub captioning: bool,
    /// Latest RMS from the capture thread (updated ~every 200 ms).
    pub rms: f32,
    /// Signal to stop the running capture thread (None when not capturing).
    pub audio_stop: Option<Arc<AtomicBool>>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            mode: SubtitleMode::default(),
            font_size: 28,
            click_through: false,
            always_on_top: true,
            captioning: false,
            rms: 0.0,
            audio_stop: None,
        }
    }
}
