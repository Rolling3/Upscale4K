//! Commands the web interface calls. Anything that touches the disk, a
//! dialog or ffmpeg runs off the interface thread, so the window never
//! freezes on a slow or disconnected drive.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::thread;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use upscale4k_application::batch::{BatchEvent, BatchItem, BatchRequest, Upscaler};
use upscale4k_application::error::UpscaleError;
use upscale4k_application::ports::MediaProbe;
use upscale4k_application::settings::change_output_folder;
use upscale4k_domain::media::{SUPPORTED_EXTENSIONS, is_supported_video};
use upscale4k_domain::naming::conflicting_inputs;
use upscale4k_infrastructure::ffmpeg_runner::FfmpegTranscoder;
use upscale4k_infrastructure::ffprobe::FfprobeMediaProbe;
use upscale4k_infrastructure::local_fs::LocalFileSystem;
use upscale4k_infrastructure::platform;

use crate::dto::{BatchEventDto, BatchItemDto, ClipDto, FilteredDto, MessageDto, StatusDto, path_text};
use crate::state::AppState;

/// Run blocking work (disk, dialogs, ffmpeg) off the interface thread.
async fn blocking<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&AppHandle, &AppState) -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || work(&app, &app.state::<AppState>())).await.map_err(|e| e.to_string())
}

fn plain(text: impl Into<String>) -> MessageDto {
    MessageDto { text: text.into(), hint: None }
}

/// Dialogs are attached to the main window, so it can't be used while they are open.
pub fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

/// Find ffmpeg and the best working encoder, and report what the interface shows.
#[tauri::command]
pub async fn check_environment(app: AppHandle) -> Result<StatusDto, String> {
    blocking(app, |_, state| {
        let env = state.detect();
        StatusDto::new(&env, &state.output_folder())
    })
    .await
}

#[tauri::command]
pub async fn pick_videos(app: AppHandle) -> Result<Vec<String>, String> {
    blocking(app, |app, _| {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Choose videos to upscale")
            .add_filter("Videos (MP4, MKV, MOV)", &SUPPORTED_EXTENSIONS);
        if let Some(window) = main_window(app) {
            dialog = dialog.set_parent(&window);
        }
        dialog
            .blocking_pick_files()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|file| file.into_path().ok())
            .map(|path| path_text(&path))
            .collect()
    })
    .await
}

/// Keep supported videos from dropped paths. A dropped folder adds the videos
/// directly inside it, skipping hidden files such as macOS `._clip.mp4` stubs.
#[tauri::command]
pub async fn filter_videos(app: AppHandle, paths: Vec<String>) -> Result<FilteredDto, String> {
    blocking(app, move |_, _| {
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        for path in paths.into_iter().map(PathBuf::from) {
            if path.is_dir() {
                let inside = videos_in(&path);
                if inside.is_empty() {
                    rejected.push(path_text(&path));
                }
                accepted.extend(inside.iter().map(|p| path_text(p)));
            } else if is_supported_video(&path) {
                accepted.push(path_text(&path));
            } else {
                rejected.push(path_text(&path));
            }
        }
        FilteredDto { accepted, rejected }
    })
    .await
}

fn videos_in(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| !is_hidden(p) && p.is_file() && is_supported_video(p))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn is_hidden(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

/// Read size, frame rate and length of each clip for the list.
#[tauri::command]
pub async fn inspect_videos(app: AppHandle, paths: Vec<String>) -> Result<Vec<ClipDto>, String> {
    blocking(app, move |_, state| {
        let platform = state.platform();
        let probe = state.environment().and_then(|e| e.toolkit).map(|t| FfprobeMediaProbe::new(t.ffprobe));
        paths
            .iter()
            .map(PathBuf::from)
            .map(|path| match &probe {
                None => ClipDto::from_error(&path, UpscaleError::FfmpegNotFound.to_user_message(&platform).into()),
                Some(probe) => match probe.probe(&path) {
                    Ok(info) => ClipDto::from_info(&path, &info),
                    Err(error) => ClipDto::from_error(&path, error.to_user_message(&platform).into()),
                },
            })
            .collect()
    })
    .await
}

/// Indices of clips whose `_4K.mp4` already exists in the output folder.
#[tauri::command]
pub async fn find_conflicts(app: AppHandle, paths: Vec<String>) -> Result<Vec<usize>, String> {
    blocking(app, move |_, state| {
        let folder = state.output_folder();
        let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        conflicting_inputs(&folder, &inputs, &|p| p.exists())
    })
    .await
}

/// Let the user choose the output folder; it is kept only if it can be written.
#[tauri::command]
pub async fn pick_output_folder(app: AppHandle) -> Result<Option<String>, MessageDto> {
    let picked = blocking(app, |app, state| {
        if state.running_batch().is_some() {
            return Some(Err(plain("The output folder can't be changed while videos are being upscaled.")));
        }
        let mut dialog = app.dialog().file().set_title("Choose the output folder");
        if let Some(start) = existing_ancestor(&state.output_folder()) {
            dialog = dialog.set_directory(start);
        }
        if let Some(window) = main_window(app) {
            dialog = dialog.set_parent(&window);
        }
        let folder = dialog.blocking_pick_folder().and_then(|f| f.into_path().ok())?;
        if state.running_batch().is_some() {
            return Some(Err(plain("The output folder can't be changed while videos are being upscaled.")));
        }
        Some(
            change_output_folder(&folder, &state.settings_store(), &LocalFileSystem, &state.platform())
                .map(|saved| path_text(&saved))
                .map_err(MessageDto::from),
        )
    })
    .await
    .map_err(plain)?;
    picked.transpose()
}

fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf)
}

/// Upscale the clips one by one, streaming progress to the interface.
#[tauri::command]
pub async fn start_batch(
    app: AppHandle,
    items: Vec<BatchItemDto>,
    on_event: Channel<BatchEventDto>,
) -> Result<(), MessageDto> {
    blocking(app, move |app, state| {
        let platform = state.platform();
        let Some((toolkit, encoder)) = state.environment().and_then(|e| Some((e.toolkit?, e.encoder?))) else {
            return Err(UpscaleError::FfmpegNotFound.to_user_message(&platform).into());
        };
        let Some(token) = state.begin_batch() else {
            return Err(plain("Upscaling is already running."));
        };
        let request = BatchRequest {
            output_dir: state.output_folder(),
            preferred_encoder: encoder,
            items: items.into_iter().map(|i| BatchItem { input: i.path, on_conflict: i.on_conflict.into() }).collect(),
        };

        let worker_app = app.clone();
        thread::spawn(move || {
            let probe = FfprobeMediaProbe::new(toolkit.ffprobe);
            let transcoder = FfmpegTranscoder::new(toolkit.ffmpeg);
            let upscaler =
                Upscaler { probe: &probe, transcoder: &transcoder, fs: &LocalFileSystem, platform: &platform };
            let state = worker_app.state::<AppState>();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                upscaler.run(&request, &token, &mut |event| {
                    // Free the slot before the interface hears "finished", so it can start again at once.
                    if matches!(event, BatchEvent::Finished(_)) {
                        state.end_batch(&token);
                    }
                    let _ = on_event.send(event.into());
                })
            }));
            state.end_batch(&token);
            if outcome.is_err() {
                let _ = on_event.send(BatchEventDto::Aborted {
                    message: plain("Something went wrong and upscaling stopped. Files that were finished are saved."),
                });
            }
        });
        Ok(())
    })
    .await
    .map_err(plain)?
}

#[tauri::command]
pub fn cancel_batch(app: AppHandle) {
    if let Some(token) = app.state::<AppState>().running_batch() {
        token.cancel();
    }
}

/// Show the output folder, creating it first if it doesn't exist yet.
#[tauri::command]
pub async fn open_output_folder(app: AppHandle) -> Result<(), MessageDto> {
    blocking(app, |_, state| {
        let folder = state.output_folder();
        std::fs::create_dir_all(&folder)
            .and_then(|()| platform::open_folder(&folder))
            .map_err(|e| plain(format!("Couldn't open {}: {e}", folder.display())))
    })
    .await
    .map_err(plain)?
}

/// Windows: open a terminal that runs the winget command.
#[tauri::command]
pub async fn install_ffmpeg(app: AppHandle) -> Result<(), MessageDto> {
    blocking(app, |_, _| {
        platform::start_ffmpeg_install().map_err(|e| plain(format!("Couldn't start the installer: {e}")))
    })
    .await
    .map_err(plain)?
}
