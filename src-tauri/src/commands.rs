//! Tauri command handlers (frontend → Rust). See `docs/IPC-CONTRACT.md`.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::audio;
use crate::settings::{PersistSettings, SettingsPath};
use crate::state::{AppState, AsrProc, LlamaProc};
use crate::types::{AudioProcess, EngineStatus, SourceHint, SubtitleMode, SubtitleUpdate};

type Db<'a>      = State<'a, Mutex<AppState>>;
type ProcDb<'a>  = State<'a, Mutex<AsrProc>>;
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
    let (stop, ngl, asr_backend, whisper_model, sv_precision) = {
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
        (stop, s.llama_gpu_layers, s.asr_backend.clone(),
         s.whisper_model.clone(), s.sensevoice_precision.clone())
    }; // AppState lock released

    // Launch asr-srv only if it is not already running.
    // On subsequent Start calls the process stays alive — no model reload needed.
    {
        let mut proc = proc_db.lock().map_err(|e| e.to_string())?;
        let alive = proc.0.as_mut()
            .map_or(false, |c| c.try_wait().ok().flatten().is_none());
        if alive {
            log::info!("asr-srv already running — reusing");
        } else {
            proc.0 = None; // clear any zombie handle
            match launch_asr_server(&asr_backend, &whisper_model, &sv_precision) {
                Ok(child) => {
                    log::info!("asr-srv started (pid {})", child.id());
                    proc.0 = Some(child);
                }
                Err(e) => {
                    log::warn!("could not start asr-srv: {e}");
                    log::warn!("  → set PYTHON_BIN / ASR_BACKEND / WHISPER_MODEL env vars");
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

    // NOTE: asr-srv and llama-server are intentionally kept alive here.
    // Killing and restarting them on every Stop/Start cycle reloads the models
    // from disk (~10-30 s).  Instead they stay resident until the app exits,
    // at which point AsrProc / LlamaProc Drop impls kill them automatically.

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

/// Kill any process currently listening on `port` (Windows: netstat + taskkill).
///
/// Called before spawning a new sidecar to evict zombie processes left behind
/// when the app was force-killed without running its Drop impls.
fn kill_port(port: u16) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x0800_0000;

        let output = match std::process::Command::new("netstat")
            .args(["-ano"])
            .creation_flags(NO_WINDOW)
            .output()
        {
            Ok(o) => o,
            Err(e) => { log::debug!("kill_port: netstat failed: {e}"); return; }
        };

        let port_tag = format!(":{port}");
        let mut found = false;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if !line.contains(&port_tag) || !line.contains("LISTENING") {
                continue;
            }
            if let Some(pid_str) = line.split_whitespace().last() {
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if pid > 1 {
                        log::info!("kill_port {port}: evicting zombie PID {pid}");
                        let _ = std::process::Command::new("taskkill")
                            .args(["/F", "/PID", &pid.to_string()])
                            .creation_flags(NO_WINDOW)
                            .output();
                        found = true;
                    }
                }
            }
        }
        if !found {
            log::info!("kill_port {port}: port is clear");
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = port; // no-op on non-Windows
}

/// Spawn the ASR server as a child process (Python script).
///
/// Python bin:  env `PYTHON_BIN`          → `python` (system Python).
/// Script path: env `ASR_SERVER_SCRIPT`   → exe-dir/asr_srv.py → cwd.
///              (also accepts legacy `WHISPER_SERVER_SCRIPT`)
/// Backend:     `backend_override` (from AppState/settings) → env `ASR_BACKEND` → `whisper`.
/// Model:       env `WHISPER_MODEL`       → `deepdml/faster-whisper-large-v3-turbo-ct2`
///              env `SENSEVOICE_MODEL`    → sherpa-onnx SenseVoice HF repo id
/// Port:        env `ASR_PORT`            → 9001  (also accepts legacy `WHISPER_ASR_PORT`).
fn launch_asr_server(backend_override: &str, whisper_model_size: &str, sv_precision: &str) -> Result<std::process::Child, String> {
    // Prefer ASR_SERVER_SCRIPT; fall back to legacy WHISPER_SERVER_SCRIPT env var,
    // then exe-dir/asr_srv.py, then cwd.
    let script = match std::env::var("ASR_SERVER_SCRIPT") {
        Ok(v) if !v.is_empty() => v,
        _ => resolve_resource("WHISPER_SERVER_SCRIPT", "asr_srv.py"),
    };
    let port = std::env::var("ASR_PORT")
        .or_else(|_| std::env::var("WHISPER_ASR_PORT")) // legacy compat
        .unwrap_or_else(|_| "9001".to_string());

    let python = std::env::var("PYTHON_BIN")
        .unwrap_or_else(|_| "python".to_string());

    // Resolve backend: in-app setting takes priority over ASR_BACKEND env var.
    let backend = if !backend_override.is_empty() {
        backend_override.to_string()
    } else {
        std::env::var("ASR_BACKEND").unwrap_or_else(|_| "whisper".to_string())
    };
    // Resolve model and optional extra CLI args (compute-type, sv-precision).
    // WHISPER_MODEL / SENSEVOICE_MODEL env vars override the UI setting.
    let env_model = match backend.as_str() {
        "sensevoice" => std::env::var("SENSEVOICE_MODEL").unwrap_or_default(),
        "zipformer-ko" => std::env::var("ZIPFORMER_MODEL").unwrap_or_default(),
        _ => std::env::var("WHISPER_MODEL").unwrap_or_default(),
    };

    let (model, apply_size_setting) = if backend == "sensevoice" {
        let m = if env_model.is_empty() {
            "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17".to_string()
        } else {
            env_model.clone()
        };
        (m, env_model.is_empty())
    } else if backend == "zipformer-ko" {
        // Korean Zipformer transducer (sherpa-onnx). Empty model => the Python
        // server auto-downloads the default model; ZIPFORMER_MODEL may point at a
        // local model directory to override.
        (env_model.clone(), false)
    } else {
        if env_model.is_empty() || env_model.ends_with(".bin") {
            if !env_model.is_empty() {
                log::warn!("WHISPER_MODEL={env_model:?} looks like a whisper.cpp model — \
                            falling back to HuggingFace repo.");
            }
            let repo = if whisper_model_size == "large" {
                "Systran/faster-whisper-large-v3"
            } else {
                // Public ct2 mirror of large-v3-turbo. The original
                // Systran/faster-whisper-large-v3-turbo repo is now HF-gated
                // (returns 401 on fresh download); this mirror is the same model.
                "deepdml/faster-whisper-large-v3-turbo-ct2"
            };
            (repo.to_string(), true)
        } else {
            (env_model.clone(), false)
        }
    };

    // Evict any zombie from a previous session before binding.
    kill_port(port.parse().unwrap_or(9001));

    log::info!("launching asr-srv: python={python}  script={script}  backend={backend}  model={model}  port={port}");

    let mut cmd = std::process::Command::new(&python);
    cmd.args([script.as_str(), "--backend", &backend]);
    // Pass --model only when we have one. zipformer-ko with an empty model lets
    // the Python server auto-download its default Korean model.
    if !model.is_empty() {
        cmd.args(["--model", &model]);
    }
    cmd.args(["--host", "127.0.0.1", "--port", &port]);

    // Quantize large-v3 to int8_float16 on GPU to keep VRAM ~1.5 GB instead of ~3 GB.
    if backend != "sensevoice" && apply_size_setting && whisper_model_size == "large" {
        cmd.args(["--compute-type", "int8_float16"]);
    }
    // SenseVoice precision (fp32 = full-precision model.onnx, better accuracy).
    if backend == "sensevoice" && apply_size_setting && sv_precision != "int8" {
        cmd.args(["--sv-precision", sv_precision]);
    }

    // Prepend the DLL directory to PATH so ctranslate2 finds cublas64_12.dll.
    // The Python script no longer needs to search for it.
    if let Some(dll_dir) = find_dll_dir() {
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{};{}", dll_dir.display(), path));
        log::info!("asr-srv: DLL dir → {}", dll_dir.display());
    } else {
        log::warn!("asr-srv: cublas64_12.dll not found — GPU inference may fail");
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

    // Evict any zombie from a previous session before binding.
    kill_port(port.parse().unwrap_or(9002));

    log::info!("launching llama-server: bin={bin}  model={model}  port={port}  ngl={ngl}");

    let mut cmd = std::process::Command::new(&bin);
    cmd.args([
        "-m", &model,
        "--host", "127.0.0.1",
        "--port", &port,
        "-ngl", &ngl,
        // 2048: system prompt + one-shot example + input + 200-token output can
        // exceed 512, and llama-server silently truncates the prompt when it
        // does — which degrades translation quality mid-session.
        "-c", "2048",
        "--no-webui",
    ]);
    // CUDA graphs cause 10× decode regression for batch=1 small contexts on some
    // driver/build combinations (measured: 1.28 tok/s with graphs vs 87 tok/s without).
    cmd.env("GGML_CUDA_NO_GRAPHS", "1");

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
        asr_backend: s.asr_backend.clone(),
        whisper_model: s.whisper_model.clone(),
        sensevoice_precision: s.sensevoice_precision.clone(),
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
    proc_db: ProcDb,
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
        if let Some(ref backend) = patch.asr_backend {
            let backend = backend.trim().to_lowercase();
            let backend = match backend.as_str() {
                "sensevoice" => "sensevoice",
                "zipformer-ko" => "zipformer-ko",
                _ => "whisper",
            };
            if s.asr_backend != backend {
                log::info!("settings: asr_backend → {backend}");
                s.asr_backend = backend.into();
                saved.asr_backend = backend.into();
                if !s.captioning {
                    if let Ok(mut proc) = proc_db.lock() {
                        if let Some(mut child) = proc.0.take() {
                            let _ = child.kill();
                            log::info!("settings: asr-srv killed for backend switch");
                        }
                    }
                }
            }
        }
        if let Some(ref wm) = patch.whisper_model {
            let wm = if wm.trim() == "large" { "large" } else { "turbo" };
            if s.whisper_model != wm {
                log::info!("settings: whisper_model → {wm}");
                s.whisper_model = wm.into();
                saved.whisper_model = wm.into();
                if !s.captioning {
                    if let Ok(mut proc) = proc_db.lock() {
                        if let Some(mut child) = proc.0.take() {
                            let _ = child.kill();
                            log::info!("settings: asr-srv killed for model switch");
                        }
                    }
                }
            }
        }
        if let Some(ref sp) = patch.sensevoice_precision {
            let sp = if sp.trim() == "fp32" { "fp32" } else { "int8" };
            if s.sensevoice_precision != sp {
                log::info!("settings: sensevoice_precision → {sp}");
                s.sensevoice_precision = sp.into();
                saved.sensevoice_precision = sp.into();
                if !s.captioning {
                    if let Ok(mut proc) = proc_db.lock() {
                        if let Some(mut child) = proc.0.take() {
                            let _ = child.kill();
                            log::info!("settings: asr-srv killed for precision switch");
                        }
                    }
                }
            }
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
    pub asr_backend: Option<String>,
    pub whisper_model: Option<String>,
    pub sensevoice_precision: Option<String>,
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
    cfg.asr_backend = s.asr_backend.clone();
    cfg.whisper_model = s.whisper_model.clone();
    cfg.sensevoice_precision = s.sensevoice_precision.clone();
    if let Err(e) = cfg.save(&sp.0) {
        log::warn!("settings save failed: {e}");
    }
}
