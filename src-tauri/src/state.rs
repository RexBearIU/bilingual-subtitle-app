//! Application state, shared across commands via `tauri::State<Mutex<AppState>>`.

use crate::types::SubtitleMode;

#[derive(Debug, Clone)]
pub struct AppState {
    /// Active two-language display mode (source of truth for payload building).
    pub mode: SubtitleMode,
    /// Overlay font size in px.
    pub font_size: u32,
    /// Whether the overlay passes mouse events through to apps beneath it.
    pub click_through: bool,
    /// Whether the overlay is pinned above other windows.
    pub always_on_top: bool,
    /// Whether the capture→ASR→translate pipeline is running.
    pub captioning: bool,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            mode: SubtitleMode::default(),
            font_size: 28,
            click_through: false,
            always_on_top: true,
            captioning: false,
        }
    }
}
