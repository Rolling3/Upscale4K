//! Upscale4K desktop app: wires the use cases to Tauri commands.

mod commands;
mod dto;
mod state;

use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

use crate::state::AppState;

/// Sent to the window when it is closed during a batch; the window asks the user.
const CLOSE_REQUESTED: &str = "close-requested";

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            app.manage(AppState::new(config_dir.join("settings.json")));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.app_handle().state::<AppState>();
                if state.running_batch().is_some() && !state.is_quitting() {
                    api.prevent_close();
                    let _ = window.emit(CLOSE_REQUESTED, ());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_environment,
            commands::pick_videos,
            commands::filter_videos,
            commands::inspect_videos,
            commands::find_conflicts,
            commands::pick_output_folder,
            commands::start_batch,
            commands::cancel_batch,
            commands::open_output_folder,
            commands::install_ffmpeg,
            quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("the app window could not be created");

    app.run(|app, event| {
        // Quitting another way (Cmd+Q, logging off) while a batch runs: stop ffmpeg first.
        if let RunEvent::ExitRequested { api, .. } = event
            && !app.state::<AppState>().is_quitting()
            && let Some(token) = app.state::<AppState>().running_batch()
        {
            api.prevent_exit();
            token.cancel();
            exit_when_idle(app.clone());
        }
    });
}

/// The user confirmed quitting: stop the batch, then exit.
#[tauri::command]
fn quit_app(app: AppHandle) {
    if let Some(token) = app.state::<AppState>().running_batch() {
        token.cancel();
    }
    exit_when_idle(app);
}

/// Exit once the batch thread has stopped ffmpeg and cleaned up (at most 10 s).
fn exit_when_idle(app: AppHandle) {
    app.state::<AppState>().mark_quitting();
    thread::spawn(move || {
        let started = Instant::now();
        while app.state::<AppState>().running_batch().is_some() && started.elapsed() < Duration::from_secs(10) {
            thread::sleep(Duration::from_millis(100));
        }
        app.exit(0);
    });
}
