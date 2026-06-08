//! Tauri command handlers (frontend → Rust). See `docs/IPC-CONTRACT.md`.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::audio;
use crate::settings::{PersistSettings, SettingsPath};
use crate::state::{AppState, LlamaProc, WhisperProc};
use crate::types::{AudioProcess, EngineStatus, SourceHint, SubtitleMode, SubtitleUpdate};

type Db<'a>      = State<'a, Mutex<AppState>>;
type ProcDb<'a>  = State<'a, Mutex<WhisperProc>>;
type LlamDb<'a>  = State<'a, Mutex<LlamaProc>>;
type SpDb<'a>    = State<'a, Mutex<SettingsPath>>;

fn emit_status(app: &AppHandle, s: &AppState) {
    let _ = app.emit("engine_status", EngineStatus::from_state(s));
}

// ── captioning lifecycle ─────────────────────────────────────────────────────

#[tauri::command]
pub fn start_captioning(
    state: Db,
    proc_db: ProcDb,
    llam_db: LlamDb,
    app: AppHandle,
) -> Result<(), String> {
    // Set state and build stop flag while holding the AppState lock.
    let (stop, ngl) = {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        if s.captioning {
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        s.audio_stop = Some(stop.clone());
        s.captioning = true;
        s.rms = 0.0;
        s.asr_status = "loading".into();
        s.translation_status = "loading".into();
        emit_status(&app, &s);
        (stop, s.llama_gpu_layers)
    }; // AppState lock released

    // Launch whisper-server only if it is not already running.
    // On subsequent Start calls the process stays alive — no model reload needed.
    {
        let mut proc = proc_db.lock().map_err(|e| e.to_string())?;
        let alive = proc.0.as_mut()
            .map_or(false, |c| c.try_wait().ok().flatten().is_none());
        if alive {
            log::info!("whisper-server already running — reusing");
        } else {
            proc.0 = None; // clear any zombie handle
            match launch_whisper_server() {
                Ok(child) => {
                    log::info!("whisper-server started (pid {})", child.id());
                    proc.0 = Some(child);
                }
                Err(e) => {
                    log::warn!("could not start whisper-server: {e}");
                    log::warn!("  → set WHISPER_SERVER_BIN / WHISPER_MODEL env vars or place binary on PATH");
                }
            }
        }
    }

    // Launch llama-server only if not already running.
    {
        let mut llam = llam_db.lock().map_err(|e| e.to_string())?;
        let alive = llam.0.as_mut()
            .map_or(false, |c| c.try_wait().ok().flatten().is_none());
        if alive {
            log::info!("llama-server already running — reusing");
        } else {
            llam.0 = None;
            match launch_llama_server(ngl) {
                Ok(child) => {
                    log::info!("llama-server started (pid {})", child.id());
                    llam.0 = Some(child);
                }
                Err(e) => {
                    log::warn!("could not start llama-server: {e}");
                    log::warn!("  → set LLAMA_SERVER_BIN / LLAMA_MODEL env vars or place binary on PATH");
                }
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
        s.translation_status = "unloaded".into();
        emit_status(&app, &s);
    }

    // NOTE: whisper-server and llama-server are intentionally kept alive here.
    // Killing and restarting them on every Stop/Start cycle reloads the models
    // from disk (~10-30 s).  Instead they stay resident until the app exits,
    // at which point WhisperProc / LlamaProc Drop impls kill them automatically.

    log::info!("stop_captioning");
    Ok(())
}

// ── path resolution ───────────────────────────────────────────────────────────

/// Returns the directory that contains the running executable.
/// In a bundled release this is the install dir, where sidecars and DLLs live.
fn exe_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
}

/// Resolve a sidecar executable path.
/// Priority: (1) env var override → (2) exe dir → (3) PATH.
fn resolve_bin(env_var: &str, stem: &str) -> String {
    if let Ok(v) = std::env::var(env_var) {
        if !v.is_empty() { return v; }
    }
    if let Some(dir) = exe_dir() {
        // Try with .exe suffix (Windows) then without (PATH fallback handles it).
        #[cfg(target_os = "windows")]
        let name = format!("{stem}.exe");
        #[cfg(not(target_os = "windows"))]
        let name = stem.to_string();
        let candidate = dir.join(&name);
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    stem.to_string()
}

/// Resolve a resource file path (e.g. a Python script bundled alongside the exe).
/// Priority: (1) env var override → (2) exe dir → (3) cwd (dev mode).
fn resolve_resource(env_var: &str, name: &str) -> String {
    if let Ok(v) = std::env::var(env_var) {
        if !v.is_empty() { return v; }
    }
    if let Some(dir) = exe_dir() {
        let candidate = dir.join(name);
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

/// Find the directory that contains the sidecar DLLs (e.g. cublas64_12.dll).
///
/// - Release: DLLs are bundled alongside the exe, so exe_dir() is it.
/// - Dev:     LLAMA_SERVER_BIN points to binaries/llama-server.exe;
///            its parent directory is the binaries/ folder with all DLLs.
fn find_dll_dir() -> Option<std::path::PathBuf> {
    // Release mode: DLLs are in the same directory as the app exe.
    if let Some(dir) = exe_dir() {
        if dir.join("cublas64_12.dll").exists() {
            return Some(dir);
        }
    }
    // Dev mode: derive from LLAMA_SERVER_BIN env var.
    if let Ok(llama_bin) = std::env::var("LLAMA_SERVER_BIN") {
        if let Some(parent) = std::path::Path::new(&llama_bin).parent() {
            if parent.join("cublas64_12.dll").exists() {
                return Some(parent.to_path_buf());
            }
        }
    }
    None
}

// ── sidecar launchers ─────────────────────────────────────────────────────────

/// Spawn faster-whisper-server as a child process (Python script).
///
/// Python bin:  env `PYTHON_BIN`              → `python` (system Python).
/// Script path: env `WHISPER_SERVER_SCRIPT`   → exe-dir/faster_whisper_srv.py → cwd.
/// Model:       env `WHISPER_MODEL`           → `Systran/faster-whisper-medium`
///              (HuggingFace repo id; downloaded on first run ~1.5 GB).
/// Port:        env `WHISPER_ASR_PORT`        → 9001.
fn launch_whisper_server() -> Result<std::process::Child, String> {
    let python = std::env::var("PYTHON_BIN")
        .unwrap_or_else(|_| "python".to_string());
    let script = resolve_resource("WHISPER_SERVER_SCRIPT", "faster_whisper_srv.py");
    // Accept a HuggingFace repo id or a local directory. If WHISPER_MODEL still
    // points to a whisper.cpp .bin file (old env var value), ignore it and use
    // the faster-whisper default so the app works without manual env var cleanup.
    let model = {
        let raw = std::env::var("WHISPER_MODEL")
            .unwrap_or_default();
        if raw.is_empty() || raw.ends_with(".bin") {
            if !raw.is_empty() {
                log::warn!("WHISPER_MODEL={raw:?} looks like a whisper.cpp model file — \
                            faster-whisper needs a HuggingFace repo id or local directory. \
                            Falling back to Systran/faster-whisper-medium.");
            }
            "Systran/faster-whisper-medium".to_string()
        } else {
            raw
        }
    };
    let port = std::env::var("WHISPER_ASR_PORT")
        .unwrap_or_else(|_| "9001".to_string());

    log::info!("launching faster-whisper: python={python}  script={script}  model={model}  port={port}");

    let mut cmd = std::process::Command::new(&python);
    cmd.args([
        script.as_str(),
        "--model", &model,
        "--host", "127.0.0.1",
        "--port", &port,
    ]);

    // Prepend the DLL directory to PATH so ctranslate2 finds cublas64_12.dll.
    // The Python script no longer needs to search for it.
    if let Some(dll_dir) = find_dll_dir() {
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{};{}", dll_dir.display(), path));
        log::info!("faster-whisper: DLL dir → {}", dll_dir.display());
    } else {
        log::warn!("faster-whisper: cublas64_12.dll not found — GPU inference may fail");
    }

    // Suppress the console window that would flash up on Windows.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    cmd.spawn().map_err(|e| format!("spawn {python} {script}: {e}"))
}

/// Spawn llama-server as a child process.
///
/// Binary path:  env `LLAMA_SERVER_BIN`  → exe-dir/llama-server.exe → PATH.
/// Model path:   env `LLAMA_MODEL`       → `models/Qwen3-4B-Q4_K_M.gguf`.
/// Port:         env `LLAMA_PORT`        → 9002.
/// GPU layers:   AppState.llama_gpu_layers (UI toggle) → env `LLAMA_GPU_LAYERS` → 36.
fn launch_llama_server(ngl_override: u32) -> Result<std::process::Child, String> {
    let bin = resolve_bin("LLAMA_SERVER_BIN", "llama-server");
    let model = std::env::var("LLAMA_MODEL")
        .unwrap_or_else(|_| "models/Qwen3-4B-Q4_K_M.gguf".to_string());
    let port = std::env::var("LLAMA_PORT")
        .unwrap_or_else(|_| "9002".to_string());
    let ngl = ngl_override.to_string();

    log::info!("launching llama-server: bin={bin}  model={model}  port={port}  ngl={ngl}");

    let mut cmd = std::process::Command::new(&bin);
    cmd.args([
        "-m", &model,
        "--host", "127.0.0.1",
        "--port", &port,
        "-ngl", &ngl,
        "-c", "512",    // subtitle prompts are short; 512 tokens is plenty
        "--no-webui",
    ]);

    // Suppress the console window on Windows.
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
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        s.mode = mode;
        log::info!("subtitle mode → {:?}", mode);
        emit_status(&app, &s);
    }
    save_current_settings(&app);
    Ok(())
}

#[tauri::command]
pub fn set_music_mode(enabled: bool, state: Db, app: AppHandle) -> Result<(), String> {
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        s.music_mode = enabled;
        s.music_mode_flag.store(enabled, std::sync::atomic::Ordering::Relaxed);
        log::info!("music mode → {enabled}");
        emit_status(&app, &s);
    }
    save_current_settings(&app);
    Ok(())
}

#[tauri::command]
pub fn set_source_hint(hint: SourceHint, state: Db, app: AppHandle) -> Result<(), String> {
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        s.source_hint = hint;
        log::info!("source hint → {:?}", hint);
        emit_status(&app, &s);
    }
    save_current_settings(&app);
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
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        s.font_size = size;
        emit_status(&app, &s);
    }
    save_current_settings(&app);
    Ok(())
}

/// List all processes that currently have an active audio session on the default
/// render device (speakers / headphones). Used to populate the process picker.
#[tauri::command]
pub fn list_audio_processes() -> Result<Vec<AudioProcess>, String> {
    #[cfg(target_os = "windows")]
    {
        // Tauri command threads may already have COM initialised as STA
        // (by the WebView2 / message-loop setup).  Calling initialize_mta()
        // on an STA thread fails with RPC_E_CHANGED_MODE (0x80010106).
        // Fix: spawn a fresh thread — it has no prior COM state, so MTA init
        // succeeds cleanly.
        std::thread::spawn(|| {
            wasapi::initialize_mta().map_err(|e| e.to_string())?;
            audio::session_enum::list_audio_processes()
        })
        .join()
        .map_err(|_| "audio-enum thread panicked".to_string())?
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(vec![])
    }
}

/// Set (or clear) the per-process audio capture target.
/// `pid == 0` means revert to system-wide WASAPI loopback (the default).
/// Change takes effect on the next `start_captioning` call.
#[tauri::command]
pub fn set_capture_process(
    pid: u32,
    name: String,
    state: Db,
    app: AppHandle,
) -> Result<(), String> {
    let mut s = state.lock().map_err(|e| e.to_string())?;
    if pid == 0 {
        s.capture_target = None;
        log::info!("capture target cleared (system-wide loopback)");
    } else {
        s.capture_target = Some(AudioProcess { pid, name: name.clone() });
        log::info!("capture target set → {name} (pid {pid})");
    }
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

// ── settings persistence ─────────────────────────────────────────────────────

/// Return current persistent settings (rebuilt from AppState + file for overlay).
#[tauri::command]
pub fn get_settings(state: Db, sp: SpDb) -> Result<PersistSettings, String> {
    let s = state.lock().map_err(|e| e.to_string())?;
    let sp = sp.lock().map_err(|e| e.to_string())?;
    // Rebuild from live AppState (mode/font/opacity/gpu come from there);
    // overlay rect is read fresh from the file (window position is managed by JS).
    let saved = PersistSettings::load(&sp.0);
    Ok(PersistSettings {
        mode: s.mode,
        source_hint: s.source_hint,
        font_size: s.font_size,
        subtitle_opacity: s.subtitle_opacity,
        overlay: saved.overlay,
        llama_gpu_layers: s.llama_gpu_layers,
        speech_threshold: s.speech_threshold,
        music_mode: s.music_mode,
    })
}

/// Partial settings update — applies each Some() field and persists to disk.
///
/// The frontend calls this:
///  - on window move / resize (to update `overlay`)
///  - when the user changes opacity or GPU layers from the ControlBar
///  - (font_size and mode are handled by their own existing commands)
#[tauri::command]
pub fn update_settings(
    patch: SettingsPatch,
    state: Db,
    sp: SpDb,
    app: AppHandle,
) -> Result<(), String> {
    // Build updated saved settings + updated engine status in one critical section.
    let (saved, eng, settings_path) = {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        let sp = sp.lock().map_err(|e| e.to_string())?;
        let mut saved = PersistSettings::load(&sp.0);

        if let Some(op) = patch.subtitle_opacity {
            let clamped = op.clamp(0.0, 1.0);
            s.subtitle_opacity = clamped;
            saved.subtitle_opacity = clamped;
        }
        if let Some(ngl) = patch.llama_gpu_layers {
            s.llama_gpu_layers = ngl;
            saved.llama_gpu_layers = ngl;
            log::info!("settings: llama_gpu_layers → {ngl}");
        }
        if let Some(thr) = patch.speech_threshold {
            // Allow 0 as "auto mode"; otherwise clamp to [0.001, 0.5].
            let clamped = if thr < 0.001 { 0.0 } else { thr.clamp(0.001, 0.5) };
            s.speech_threshold = clamped;
            saved.speech_threshold = clamped;
            log::info!("settings: speech_threshold → {clamped:.4} ({:.1} dBFS)",
                20.0_f32 * clamped.log10());
        }
        if let Some(ov) = patch.overlay {
            saved.overlay = ov;
        }
        // Keep mode/font in sync with live state.
        saved.mode = s.mode;
        saved.font_size = s.font_size;

        let eng = EngineStatus::from_state(&s);
        let path = sp.0.clone();
        (saved, eng, path)
        // s and sp drop here — locks released before disk I/O
    };

    // Broadcast so the UI re-renders opacity/gpu immediately.
    let _ = app.emit("engine_status", eng);

    // Persist.
    saved.save(&settings_path)
}

/// Patch struct for `update_settings` — all fields optional.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub subtitle_opacity: Option<f64>,
    pub llama_gpu_layers: Option<u32>,
    pub speech_threshold: Option<f32>,
    pub overlay: Option<crate::settings::OverlayRect>,
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Save current AppState fields to the settings file.
/// Call this after any command that changes a persistent field.
pub fn save_current_settings(app: &AppHandle) {
    let sp_state = app.state::<Mutex<SettingsPath>>();
    let Ok(sp) = sp_state.lock() else { return };
    let as_state = app.state::<Mutex<AppState>>();
    let Ok(s) = as_state.lock() else { return };

    // Load overlay from disk (window position is JS-maintained).
    let mut cfg = PersistSettings::load(&sp.0);
    cfg.mode = s.mode;
    cfg.source_hint = s.source_hint;
    cfg.music_mode = s.music_mode;
    cfg.font_size = s.font_size;
    cfg.subtitle_opacity = s.subtitle_opacity;
    cfg.llama_gpu_layers = s.llama_gpu_layers;
    cfg.speech_threshold = s.speech_threshold;
    if let Err(e) = cfg.save(&sp.0) {
        log::warn!("settings save failed: {e}");
    }
}
