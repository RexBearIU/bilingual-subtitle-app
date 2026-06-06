//! Tauri command handlers (frontend → Rust). See `docs/IPC-CONTRACT.md`.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, State, WebviewWindow};

use crate::audio;
use crate::state::AppState;
use crate::types::{EngineStatus, SubtitleMode, SubtitleUpdate};

type Db<'a> = State<'a, Mutex<AppState>>;

fn emit_status(app: &AppHandle, s: &AppState) {
    let _ = app.emit("engine_status", EngineStatus::from_state(s));
}

#[tauri::command]
pub fn start_captioning(state: Db, app: AppHandle) -> Result<(), String> {
    // Build the stop flag and flip state while holding the lock.
    let stop = {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        if s.captioning {
            return Ok(()); // already running
        }
        let stop = Arc::new(AtomicBool::new(false));
        s.audio_stop = Some(stop.clone());
        s.captioning = true;
        s.rms = 0.0;
        emit_status(&app, &s);
        stop
    }; // lock released before spawning

    audio::capture::start_loopback_capture(app, stop);
    log::info!("start_captioning: WASAPI loopback requested");
    Ok(())
}

#[tauri::command]
pub fn stop_captioning(state: Db, app: AppHandle) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if let Some(stop) = s.audio_stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    s.captioning = false;
    s.rms = 0.0;
    emit_status(&app, &s);
    log::info!("stop_captioning");
    Ok(())
}

#[tauri::command]
pub fn set_subtitle_mode(mode: SubtitleMode, state: Db, app: AppHandle) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    s.mode = mode;
    log::info!("subtitle mode → {:?}", mode);
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
    log::info!("click-through → {}", enabled);
    emit_status(&app, &s);
    Ok(())
}

/// Re-assert (or clear) topmost. Calling with `true` re-stacks the overlay
/// above other always-on-top windows.
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
    log::info!("always-on-top → {}", enabled);
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
#[tauri::command]
pub fn dev_inject_subtitle(payload: SubtitleUpdate, app: AppHandle) -> Result<(), String> {
    app.emit("subtitle_update", payload)
        .map_err(|e| e.to_string())
}
