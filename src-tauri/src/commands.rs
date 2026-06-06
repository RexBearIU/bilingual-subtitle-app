//! Tauri command handlers (frontend → Rust). See `docs/IPC-CONTRACT.md`.
//!
//! For Milestone 1 the capture/ASR/translation pipeline does not exist yet, so
//! `start_captioning` / `stop_captioning` only flip state and report status. Real
//! subtitles arrive once M2–M5 land; until then the overlay is exercised through
//! the dev-only `dev_inject_subtitle` command, which emits a **real**
//! `subtitle_update` over the **real** event path (see ADR-0005).

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, State, WebviewWindow};

use crate::state::AppState;
use crate::types::{EngineStatus, SubtitleMode, SubtitleUpdate};

type Db<'a> = State<'a, Mutex<AppState>>;

/// Emit the current engine/UI status so all windows stay in sync.
fn emit_status(app: &AppHandle, s: &AppState) {
    let _ = app.emit("engine_status", EngineStatus::from_state(s));
}

#[tauri::command]
pub fn start_captioning(state: Db, app: AppHandle) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.captioning = true;
    log::info!("start_captioning requested (pipeline lands in M2+)");
    emit_status(&app, &s);
    Ok(())
}

#[tauri::command]
pub fn stop_captioning(state: Db, app: AppHandle) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.captioning = false;
    log::info!("stop_captioning requested");
    emit_status(&app, &s);
    Ok(())
}

#[tauri::command]
pub fn set_subtitle_mode(mode: SubtitleMode, state: Db, app: AppHandle) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.mode = mode;
    log::info!("subtitle mode set to {:?}", mode);
    emit_status(&app, &s);
    Ok(())
}

#[tauri::command]
pub fn set_click_through(
    enabled: bool,
    window: WebviewWindow,
    state: Db,
    app: AppHandle,
) -> Result<(), String> {
    window
        .set_ignore_cursor_events(enabled)
        .map_err(|e| e.to_string())?;
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.click_through = enabled;
    log::info!("click-through set to {}", enabled);
    emit_status(&app, &s);
    Ok(())
}

/// Re-assert (or clear) topmost. Calling this with `true` re-stacks the overlay
/// to the top of the OS "always-on-top" band, above other topmost windows.
#[tauri::command]
pub fn set_always_on_top(
    enabled: bool,
    window: WebviewWindow,
    state: Db,
    app: AppHandle,
) -> Result<(), String> {
    window
        .set_always_on_top(enabled)
        .map_err(|e| e.to_string())?;
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.always_on_top = enabled;
    log::info!("always-on-top set to {}", enabled);
    emit_status(&app, &s);
    Ok(())
}

#[tauri::command]
pub fn set_font_size(size: u32, state: Db, app: AppHandle) -> Result<(), String> {
    let size = size.clamp(10, 120);
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.font_size = size;
    emit_status(&app, &s);
    Ok(())
}

#[tauri::command]
pub fn get_status(state: Db) -> Result<EngineStatus, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    Ok(EngineStatus::from_state(&s))
}

/// Dev-only: emit a real `subtitle_update` from a manually supplied payload.
/// Replaces the notion of a "mock" — identical event path, manual data source.
#[tauri::command]
pub fn dev_inject_subtitle(payload: SubtitleUpdate, app: AppHandle) -> Result<(), String> {
    app.emit("subtitle_update", payload)
        .map_err(|e| e.to_string())
}
