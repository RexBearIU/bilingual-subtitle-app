//! Tauri command handlers (frontend → Rust). See `docs/IPC-CONTRACT.md`.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, State, WebviewWindow};

use crate::audio;
use crate::state::{AppState, WhisperProc};
use crate::types::{EngineStatus, SubtitleMode, SubtitleUpdate};

type Db<'a> = State<'a, Mutex<AppState>>;
type ProcDb<'a> = State<'a, Mutex<WhisperProc>>;

fn emit_status(app: &AppHandle, s: &AppState) {
    let _ = app.emit("engine_status", EngineStatus::from_state(s));
}

// ── captioning lifecycle ─────────────────────────────────────────────────────

#[tauri::command]
pub fn start_captioning(
    state: Db,
    proc_db: ProcDb,
    app: AppHandle,
) -> Result<(), String> {
    // Set state and build stop flag while holding the AppState lock.
    let stop = {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        if s.captioning {
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        s.audio_stop = Some(stop.clone());
        s.captioning = true;
        s.rms = 0.0;
        s.asr_status = "loading".into();
        emit_status(&app, &s);
        stop
    }; // AppState lock released

    // Launch whisper-server; non-fatal if binary is absent (ASR status → error
    // is set by the ASR worker after the 30 s poll timeout).
    {
        let mut proc = proc_db.lock().map_err(|e| e.to_string())?;
        match launch_whisper_server() {
            Ok(child) => {
                log::info!("whisper-server started (pid {})", child.id());
                proc.0 = Some(child);
            }
            Err(e) => {
                log::warn!("could not start whisper-server: {e}");
                log::warn!("  → set WHISPER_SERVER_BIN / WHISPER_MODEL env vars or place binary on PATH");
                // ASR worker will time-out and set status to "error" itself.
            }
        }
    }

    audio::capture::start_loopback_capture(app, stop);
    log::info!("start_captioning: pipeline started");
    Ok(())
}

#[tauri::command]
pub fn stop_captioning(
    state: Db,
    proc_db: ProcDb,
    app: AppHandle,
) -> Result<(), String> {
    // Signal all worker threads to stop.
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        if let Some(stop) = s.audio_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        s.captioning = false;
        s.rms = 0.0;
        s.asr_status = "unloaded".into();
        emit_status(&app, &s);
    }

    // Kill the whisper-server process.
    {
        let mut proc = proc_db.lock().map_err(|e| e.to_string())?;
        if let Some(mut child) = proc.0.take() {
            let _ = child.kill();
            log::info!("whisper-server stopped");
        }
    }

    log::info!("stop_captioning");
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Spawn whisper-server as a child process.
///
/// Binary path: env `WHISPER_SERVER_BIN` → `whisper-server` (PATH).
/// Model path:  env `WHISPER_MODEL`      → `models/ggml-medium.bin`.
/// Port:        env `WHISPER_ASR_PORT`   → 9001.
fn launch_whisper_server() -> Result<std::process::Child, String> {
    let bin = std::env::var("WHISPER_SERVER_BIN")
        .unwrap_or_else(|_| "whisper-server".to_string());
    let model = std::env::var("WHISPER_MODEL")
        .unwrap_or_else(|_| "models/ggml-medium.bin".to_string());
    let port = std::env::var("WHISPER_ASR_PORT")
        .unwrap_or_else(|_| "9001".to_string());

    log::info!("launching whisper-server: bin={bin}  model={model}  port={port}");

    let mut cmd = std::process::Command::new(&bin);
    cmd.args(["-m", &model, "--host", "127.0.0.1", "--port", &port, "--language", "auto"]);

    // Suppress the console window that would flash up on Windows.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    cmd.spawn().map_err(|e| format!("spawn {bin}: {e}"))
}

// ── other commands ───────────────────────────────────────────────────────────

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
