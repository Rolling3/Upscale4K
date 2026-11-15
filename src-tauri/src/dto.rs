//! Shapes sent to and from the web interface. They mirror the use-case
//! types with names that read well in TypeScript.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use upscale4k_application::batch::BatchEvent;
use upscale4k_application::environment::Environment;
use upscale4k_application::error::UserMessage;
use upscale4k_domain::encoder::Encoder;
use upscale4k_domain::media::{FrameSize, VideoInfo, upscaled_size};
use upscale4k_domain::naming::ConflictChoice;

pub fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EncoderDto {
    pub label: &'static str,
    pub hardware: bool,
}

impl From<Encoder> for EncoderDto {
    fn from(encoder: Encoder) -> Self {
        Self { label: encoder.label(), hardware: encoder.is_hardware() }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstallDto {
    pub command: String,
    pub can_run_in_app: bool,
    pub note: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub version: &'static str,
    pub platform: &'static str,
    pub ffmpeg_found: bool,
    pub ffmpeg_version: Option<String>,
    pub ffmpeg_path: Option<String>,
    pub encoder: Option<EncoderDto>,
    pub install: InstallDto,
    pub output_folder: String,
}

impl StatusDto {
    pub fn new(env: &Environment, output_folder: &Path) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            platform: if cfg!(windows) {
                "windows"
            } else if cfg!(target_os = "macos") {
                "macos"
            } else {
                "other"
            },
            ffmpeg_found: env.toolkit.is_some(),
            ffmpeg_version: env.toolkit.as_ref().map(|t| t.version.clone()),
            ffmpeg_path: env.toolkit.as_ref().map(|t| path_text(&t.ffmpeg)),
            encoder: env.encoder.map(EncoderDto::from),
            install: InstallDto {
                command: env.install.command.clone(),
                can_run_in_app: env.install.can_run_in_app,
                note: env.install.note.clone(),
            },
            output_folder: path_text(output_folder),
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MessageDto {
    pub text: String,
    pub hint: Option<String>,
}

impl From<UserMessage> for MessageDto {
    fn from(message: UserMessage) -> Self {
        Self { text: message.text, hint: message.hint }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FilteredDto {
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClipDto {
    pub path: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<f64>,
    pub duration_secs: Option<f64>,
    pub out_width: Option<u32>,
    pub out_height: Option<u32>,
    pub error: Option<MessageDto>,
}

impl ClipDto {
    pub fn from_info(path: &Path, info: &VideoInfo) -> Self {
        let FrameSize { width: out_width, height: out_height } = upscaled_size(info.size);
        Self {
            path: path_text(path),
            width: Some(info.size.width),
            height: Some(info.size.height),
            frame_rate: info.frame_rate,
            duration_secs: info.duration_secs,
            out_width: Some(out_width),
            out_height: Some(out_height),
            error: None,
        }
    }

    pub fn from_error(path: &Path, error: MessageDto) -> Self {
        Self {
            path: path_text(path),
            width: None,
            height: None,
            frame_rate: None,
            duration_secs: None,
            out_width: None,
            out_height: None,
            error: Some(error),
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum ConflictChoiceDto {
    KeepBoth,
    Replace,
    Skip,
}

impl From<ConflictChoiceDto> for ConflictChoice {
    fn from(choice: ConflictChoiceDto) -> Self {
        match choice {
            ConflictChoiceDto::KeepBoth => ConflictChoice::KeepBoth,
            ConflictChoiceDto::Replace => ConflictChoice::Replace,
            ConflictChoiceDto::Skip => ConflictChoice::Skip,
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemDto {
    pub path: PathBuf,
    pub on_conflict: ConflictChoiceDto,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "event", content = "data")]
pub enum BatchEventDto {
    FileStarted {
        index: usize,
        output: String,
        encoder: EncoderDto,
        notes: Vec<String>,
    },
    FileProgress {
        index: usize,
        fraction: Option<f64>,
        eta_secs: Option<f64>,
        speed: Option<f64>,
    },
    FileDone {
        index: usize,
        output: String,
        note: Option<String>,
    },
    FileSkipped {
        index: usize,
        existing: String,
    },
    FileFailed {
        index: usize,
        message: MessageDto,
    },
    FileCancelled {
        index: usize,
    },
    FolderFailed {
        message: MessageDto,
    },
    Finished {
        saved: usize,
        failed: usize,
        skipped: usize,
        cancelled: bool,
    },
    /// Something unexpected stopped the batch.
    Aborted {
        message: MessageDto,
    },
}

impl From<BatchEvent> for BatchEventDto {
    fn from(event: BatchEvent) -> Self {
        match event {
            BatchEvent::FileStarted { index, output, encoder, notes } => {
                Self::FileStarted { index, output: path_text(&output), encoder: encoder.into(), notes }
            }
            BatchEvent::FileProgress { index, fraction, eta_secs, speed } => {
                Self::FileProgress { index, fraction, eta_secs, speed }
            }
            BatchEvent::FileDone { index, output, note } => Self::FileDone { index, output: path_text(&output), note },
            BatchEvent::FileSkipped { index, existing } => Self::FileSkipped { index, existing: path_text(&existing) },
            BatchEvent::FileFailed { index, message } => Self::FileFailed { index, message: message.into() },
            BatchEvent::FileCancelled { index } => Self::FileCancelled { index },
            BatchEvent::FolderFailed { message } => Self::FolderFailed { message: message.into() },
            BatchEvent::Finished(s) => {
                Self::Finished { saved: s.saved, failed: s.failed, skipped: s.skipped, cancelled: s.cancelled }
            }
        }
    }
}
