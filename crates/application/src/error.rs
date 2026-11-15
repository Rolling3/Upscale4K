//! Failures and the words the user sees for them.
//!
//! A missing or unwritable file is described as exactly that. Nothing here
//! calls a file "corrupted": the app can't know that, and the usual causes
//! (moved file, blocked folder, file open elsewhere) are fixable.

use std::path::{Path, PathBuf};

use upscale4k_domain::encoder::Encoder;

use crate::ports::Platform;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpscaleError {
    FfmpegNotFound,
    InputNotFound {
        path: PathBuf,
    },
    /// ffmpeg couldn't read the file as a video.
    InputUnreadable {
        path: PathBuf,
        detail: String,
    },
    NoVideoStream {
        path: PathBuf,
    },
    OutputFolderNotWritable {
        folder: PathBuf,
        detail: String,
    },
    /// The finished file couldn't be moved into place.
    OutputFileBusy {
        path: PathBuf,
        detail: String,
    },
    NotEnoughSpace {
        folder: PathBuf,
    },
    EncoderFailed {
        encoder: Encoder,
        detail: String,
    },
    Cancelled,
}

/// Text for the interface: what happened, and what to try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessage {
    pub text: String,
    pub hint: Option<String>,
}

impl UpscaleError {
    pub fn to_user_message(&self, platform: &dyn Platform) -> UserMessage {
        let (text, hint) = match self {
            UpscaleError::FfmpegNotFound => (
                "ffmpeg isn't installed, or Upscale4K couldn't find it.".to_string(),
                Some(format!("Install it with: {}", platform.install_hint().command)),
            ),
            UpscaleError::InputNotFound { path } => {
                (format!("{} was not found. It may have been moved, renamed or deleted.", file_name(path)), None)
            }
            UpscaleError::InputUnreadable { path, detail } => {
                (format!("ffmpeg could not read {} as a video. ffmpeg said: {detail}", file_name(path)), None)
            }
            UpscaleError::NoVideoStream { path } => (format!("{} has no video track.", file_name(path)), None),
            UpscaleError::OutputFolderNotWritable { folder, detail } => (
                format!("Can't write to the output folder {}. {detail}", folder.display()),
                platform.write_blocked_hint(),
            ),
            UpscaleError::OutputFileBusy { path, detail } => {
                (format!("Couldn't save {}. It may be open in another app. {detail}", path.display()), None)
            }
            UpscaleError::NotEnoughSpace { folder } => {
                (format!("There isn't enough free space in {} to save the video.", folder.display()), None)
            }
            UpscaleError::EncoderFailed { encoder, detail } => {
                (format!("The {} encoder stopped with an error. ffmpeg said: {detail}", encoder.label()), None)
            }
            UpscaleError::Cancelled => ("Cancelled.".to_string(), None),
        };
        UserMessage { text, hint }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::FakePlatform;

    fn all_errors() -> Vec<UpscaleError> {
        let path = PathBuf::from("clips").join("boss.mkv");
        vec![
            UpscaleError::FfmpegNotFound,
            UpscaleError::InputNotFound { path: path.clone() },
            UpscaleError::InputUnreadable {
                path: path.clone(),
                detail: "Invalid data found when processing input".into(),
            },
            UpscaleError::NoVideoStream { path: path.clone() },
            UpscaleError::OutputFolderNotWritable { folder: PathBuf::from("out"), detail: "Access is denied.".into() },
            UpscaleError::OutputFileBusy { path: PathBuf::from("out").join("boss_4K.mp4"), detail: "in use".into() },
            UpscaleError::NotEnoughSpace { folder: PathBuf::from("out") },
            UpscaleError::EncoderFailed { encoder: Encoder::Nvenc, detail: "OpenEncodeSessionEx failed".into() },
            UpscaleError::Cancelled,
        ]
    }

    #[test]
    fn no_message_ever_calls_a_file_corrupted() {
        let platform = FakePlatform::windows_like();
        for error in all_errors() {
            let msg = error.to_user_message(&platform);
            let all = format!("{} {}", msg.text, msg.hint.unwrap_or_default()).to_lowercase();
            assert!(!all.contains("corrupt"), "{error:?} -> {all}");
            assert!(!all.contains("damaged"), "{error:?} -> {all}");
        }
    }

    #[test]
    fn a_missing_input_says_it_was_not_found_and_names_the_file() {
        let msg = UpscaleError::InputNotFound { path: PathBuf::from("clips").join("boss.mkv") }
            .to_user_message(&FakePlatform::windows_like());
        assert!(msg.text.contains("not found"), "{}", msg.text);
        assert!(msg.text.contains("boss.mkv"), "{}", msg.text);
    }

    #[test]
    fn an_unwritable_folder_says_so_and_carries_the_platform_hint() {
        let msg =
            UpscaleError::OutputFolderNotWritable { folder: PathBuf::from("out"), detail: "Access is denied.".into() }
                .to_user_message(&FakePlatform::windows_like());
        assert!(msg.text.contains("Can't write"), "{}", msg.text);
        assert!(msg.text.contains("out"), "{}", msg.text);
        assert_eq!(msg.hint.as_deref(), Some(FakePlatform::BLOCKED_HINT));
    }

    #[test]
    fn an_unwritable_folder_has_no_hint_where_the_platform_gives_none() {
        let msg =
            UpscaleError::OutputFolderNotWritable { folder: PathBuf::from("out"), detail: "Permission denied".into() }
                .to_user_message(&FakePlatform::mac_like());
        assert_eq!(msg.hint, None);
    }

    #[test]
    fn a_full_disk_says_there_is_not_enough_space_without_the_folder_access_hint() {
        let msg = UpscaleError::NotEnoughSpace { folder: PathBuf::from("out") }
            .to_user_message(&FakePlatform::windows_like());
        assert!(msg.text.contains("enough free space"), "{}", msg.text);
        assert_eq!(msg.hint, None);
    }

    #[test]
    fn missing_ffmpeg_points_at_the_install_command() {
        let msg = UpscaleError::FfmpegNotFound.to_user_message(&FakePlatform::windows_like());
        assert!(msg.hint.unwrap_or_default().contains("winget install --id Gyan.FFmpeg -e"));
    }

    #[test]
    fn an_encoder_failure_names_the_encoder_and_keeps_ffmpegs_words() {
        let msg = UpscaleError::EncoderFailed { encoder: Encoder::Nvenc, detail: "OpenEncodeSessionEx failed".into() }
            .to_user_message(&FakePlatform::windows_like());
        assert!(msg.text.contains("NVIDIA NVENC"), "{}", msg.text);
        assert!(msg.text.contains("OpenEncodeSessionEx failed"), "{}", msg.text);
    }
}
