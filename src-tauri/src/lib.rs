mod asr;
// Public so `src/bin/record_loopback.rs` can reuse the WASAPI capture and
// resampling primitives instead of reimplementing them.
pub mod audio;
mod commands;
mod hittest;
mod pipeline;
mod settings;
mod state;
mod translate;
mod types;
mod util;

use std::sync::Mutex;

use state::AppState;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Listener, Manager};
use types::ClickThrough;

/// Record a mouse-policy change, apply it, persist it, and broadcast status.
///
/// The window call itself lives in `hittest`, which is the only place allowed
/// to touch `set_ignore_cursor_events` — otherwise the poll thread and this
/// path would fight over the flag.
fn apply_click_through(app: &AppHandle, mode: ClickThrough) {
    state::update_and_emit(app, |s| s.click_through = mode);
    hittest::apply_now(app);
    if mode == ClickThrough::Off {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.set_focus();
        }
    }
    commands::save_current_settings(app);
}

fn apply_always_on_top(app: &AppHandle, enabled: bool) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_always_on_top(enabled);
    }
    state::update_and_emit(app, |s| s.always_on_top = enabled);
}

fn toggle_always_on_top(app: &AppHandle) {
    let cur = state::read_state(app, |s| s.always_on_top).unwrap_or(true);
    apply_always_on_top(app, !cur);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything reads the environment: a gitignored `.env` next to the
    // project (or the exe) can supply OPENROUTER_API_KEY and friends. Real
    // environment variables still take priority.
    let dotenv_path = util::load_dotenv();

    tauri::Builder::default()
        .manage(Mutex::new(AppState::default()))
        .manage(Mutex::new(state::AsrProc(None)))
        .manage(Mutex::new(settings::SettingsPath(std::path::PathBuf::new())))
        .manage(translate::Registry::default())
        .invoke_handler(tauri::generate_handler![
            commands::start_captioning,
            commands::stop_captioning,
            commands::set_subtitle_mode,
            commands::set_source_hint,
            commands::set_click_through,
            commands::set_hit_regions,
            commands::set_translate_provider,
            commands::set_translate_providers,
            commands::translate_preset_names,
            commands::set_always_on_top,
            commands::set_font_size,
            commands::get_status,
            commands::dev_inject_subtitle,
            commands::get_settings,
            commands::update_settings,
            commands::list_audio_processes,
            commands::set_capture_process,
        ])
        .setup(move |app| {
            // First, so the rest of setup can log. Everything below — the
            // settings path, the .env location, the resolved provider list —
            // used to run before the logger existed and vanished silently,
            // which made "did it load my config?" unanswerable from the log.
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        // Our own code: full debug
                        .level(log::LevelFilter::Debug)
                        // External crates: warn-only (suppress ureq/wasapi spam)
                        .level_for("ureq",   log::LevelFilter::Warn)
                        .level_for("wasapi", log::LevelFilter::Warn)
                        .level_for("tauri",  log::LevelFilter::Warn)
                        .level_for("tao",    log::LevelFilter::Warn)
                        .level_for("wry",    log::LevelFilter::Warn)
                        .build(),
                )?;
            }

            match &dotenv_path {
                // Path only — never the contents, which hold the API key.
                Some(p) => log::info!("loaded .env from {p:?}"),
                None => log::debug!("no .env file found"),
            }

            // ── Load persistent settings ────────────────────────────────────
            let settings_path = app
                .path()
                .app_data_dir()
                .map(|d| d.join("settings.json"))
                .unwrap_or_else(|_| std::path::PathBuf::from("settings.json"));

            let cfg = settings::PersistSettings::load(&settings_path);
            log::info!("settings loaded from {:?}", settings_path);

            // Store the path so commands can read/write it.
            if let Some(sp) = app.try_state::<Mutex<settings::SettingsPath>>() {
                if let Ok(mut sp) = sp.lock() {
                    sp.0 = settings_path;
                }
            }

            // Apply saved settings to AppState.
            if let Some(st) = app.try_state::<Mutex<AppState>>() {
                if let Ok(mut s) = st.lock() {
                    s.mode = cfg.mode;
                    s.source_hint = cfg.source_hint;
                    s.font_size = cfg.font_size;
                    s.subtitle_opacity = cfg.subtitle_opacity;
                    s.click_through = cfg.click_through;
                    s.speech_threshold = cfg.speech_threshold;
                    s.asr_backend = cfg.asr_backend.clone();
                    s.whisper_model = cfg.whisper_model.clone();
                    s.sensevoice_precision = cfg.sensevoice_precision.clone();
                }
            }

            // ── Restore window position / size ──────────────────────────────
            if let Some(w) = app.get_webview_window("main") {
                use tauri::PhysicalPosition;
                use tauri::PhysicalSize;
                let _ = w.set_position(PhysicalPosition::new(cfg.overlay.x, cfg.overlay.y));
                let _ = w.set_size(PhysicalSize::new(cfg.overlay.w, cfg.overlay.h));
                // Start interactive regardless of the saved mode: the frontend
                // has not reported its clickable regions yet, so `Auto` would
                // otherwise make the whole window transparent to the mouse for
                // the first moments after launch.
                let _ = w.set_ignore_cursor_events(false);
                let _ = w.set_always_on_top(true);
            }

            // Build the provider list before the first Start, so Settings can
            // show, edit and reorder it on a freshly launched, idle app.
            // Cheap: it only reads env vars and settings.json.
            let infos = translate::refresh(&app.handle().clone());
            log::info!("TL: providers {}", translate::describe(&infos));

            hittest::spawn(app.handle().clone());

            // System tray — always clickable, even when the overlay is passing
            // mouse events through. Checkable items mirror the live state.
            // Three mutually exclusive items rather than one checkbox: the
            // policy has three states and the tray is the escape hatch, so it
            // has to be able to reach all of them.
            let ct_off = CheckMenuItem::with_id(
                app, "ct_off", "互動 / Interactive", true, false, None::<&str>,
            )?;
            let ct_auto = CheckMenuItem::with_id(
                app, "ct_auto", "自動穿透 / Auto", true, true, None::<&str>,
            )?;
            let ct_on = CheckMenuItem::with_id(
                app, "ct_on", "全穿透 / Click-through", true, false, None::<&str>,
            )?;
            let top_item = CheckMenuItem::with_id(
                app,
                "top",
                "置頂 / Always-on-top",
                true,
                true,
                None::<&str>,
            )?;
            let quit_item =
                MenuItem::with_id(app, "quit", "結束 / Quit", true, None::<&str>)?;
            let tray_menu = Menu::with_items(
                app,
                &[
                    &ct_off,
                    &ct_auto,
                    &ct_on,
                    &PredefinedMenuItem::separator(app)?,
                    &top_item,
                    &PredefinedMenuItem::separator(app)?,
                    &quit_item,
                ],
            )?;

            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Bilingual Subtitles")
                .menu(&tray_menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "ct_off" => apply_click_through(app, ClickThrough::Off),
                    "ct_auto" => apply_click_through(app, ClickThrough::Auto),
                    "ct_on" => apply_click_through(app, ClickThrough::On),
                    "top" => toggle_always_on_top(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            // Keep the tray checkmarks in sync with every status broadcast,
            // no matter the source (overlay UI, tray, or escape hotkey).
            let (off_sync, auto_sync, on_sync) =
                (ct_off.clone(), ct_auto.clone(), ct_on.clone());
            let top_sync = top_item.clone();
            app.listen("engine_status", move |event| {
                if let Ok(s) = serde_json::from_str::<types::EngineStatus>(event.payload()) {
                    let _ = off_sync.set_checked(s.click_through == ClickThrough::Off);
                    let _ = auto_sync.set_checked(s.click_through == ClickThrough::Auto);
                    let _ = on_sync.set_checked(s.click_through == ClickThrough::On);
                    let _ = top_sync.set_checked(s.always_on_top);
                }
            });

            // Global escape hatch hotkey: Ctrl+Alt+P (backup for the tray button).
            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::{
                    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
                };

                let escape = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyP);
                let escape_for_handler = escape;
                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_handler(move |app, shortcut, event| {
                            if shortcut == &escape_for_handler
                                && event.state() == ShortcutState::Pressed
                            {
                                apply_click_through(app, ClickThrough::Off);
                                apply_always_on_top(app, true);
                                log::info!("escape hotkey: click-through forced off");
                            }
                        })
                        .build(),
                )?;
                if let Err(e) = app.global_shortcut().register(escape) {
                    log::warn!("failed to register Ctrl+Alt+P escape hotkey: {e}");
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
