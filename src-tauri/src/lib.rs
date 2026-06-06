mod audio;
mod commands;
mod state;
mod types;

use std::sync::Mutex;

use state::AppState;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Listener, Manager};

/// Apply + persist a click-through change and broadcast status.
fn apply_click_through(app: &AppHandle, enabled: bool) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_ignore_cursor_events(enabled);
        if !enabled {
            let _ = w.set_focus();
        }
    }
    if let Some(st) = app.try_state::<Mutex<AppState>>() {
        if let Ok(mut s) = st.lock() {
            s.click_through = enabled;
            let _ = app.emit("engine_status", types::EngineStatus::from_state(&s));
        }
    }
}

fn toggle_click_through(app: &AppHandle) {
    let cur = app
        .try_state::<Mutex<AppState>>()
        .and_then(|st| st.lock().ok().map(|s| s.click_through))
        .unwrap_or(false);
    apply_click_through(app, !cur);
}

fn apply_always_on_top(app: &AppHandle, enabled: bool) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_always_on_top(enabled);
    }
    if let Some(st) = app.try_state::<Mutex<AppState>>() {
        if let Ok(mut s) = st.lock() {
            s.always_on_top = enabled;
            let _ = app.emit("engine_status", types::EngineStatus::from_state(&s));
        }
    }
}

fn toggle_always_on_top(app: &AppHandle) {
    let cur = app
        .try_state::<Mutex<AppState>>()
        .and_then(|st| st.lock().ok().map(|s| s.always_on_top))
        .unwrap_or(true);
    apply_always_on_top(app, !cur);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::new(AppState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::start_captioning,
            commands::stop_captioning,
            commands::set_subtitle_mode,
            commands::set_click_through,
            commands::set_always_on_top,
            commands::set_font_size,
            commands::get_status,
            commands::dev_inject_subtitle,
        ])
        .setup(|app| {
            // Start in a known-good interactive, topmost state every launch.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_ignore_cursor_events(false);
                let _ = w.set_always_on_top(true);
            }

            // System tray — always clickable, even when the overlay is passing
            // mouse events through. Checkable items mirror the live state.
            let ct_item = CheckMenuItem::with_id(
                app,
                "ct",
                "穿透 / Click-through",
                true,
                false,
                None::<&str>,
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
                    &ct_item,
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
                    "ct" => toggle_click_through(app),
                    "top" => toggle_always_on_top(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            // Keep the tray checkmarks in sync with every status broadcast,
            // no matter the source (overlay UI, tray, or escape hotkey).
            let ct_sync = ct_item.clone();
            let top_sync = top_item.clone();
            app.listen("engine_status", move |event| {
                if let Ok(s) = serde_json::from_str::<types::EngineStatus>(event.payload()) {
                    let _ = ct_sync.set_checked(s.click_through);
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
                let escape_for_handler = escape.clone();
                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_handler(move |app, shortcut, event| {
                            if shortcut == &escape_for_handler
                                && event.state() == ShortcutState::Pressed
                            {
                                apply_click_through(app, false);
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

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
