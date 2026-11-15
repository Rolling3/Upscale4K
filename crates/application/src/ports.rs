//! What the use cases need from the outside world.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use upscale4k_domain::encoder::Encoder;
use upscale4k_domain::media::VideoInfo;

use crate::cancel::CancelToken;
use crate::error::UpscaleError;
use crate::settings::Settings;

/// Reads facts about an input clip.
pub trait MediaProbe {
    fn probe(&self, input: &Path) -> Result<VideoInfo, UpscaleError>;
}

/// One progress report from a running encode.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TranscodeProgress {
    /// Position in the output, in microseconds.
    pub out_time_us: Option<i64>,
    /// Speed relative to real time.
    pub speed: Option<f64>,
}

/// Runs one encode.
pub trait Transcoder {
    /// Run ffmpeg with `args` and return when it exits. Once `cancel` is set
    /// the encode must stop promptly and return `Err(UpscaleError::Cancelled)`.
    fn transcode(
        &self,
        args: &[OsString],
        encoder: Encoder,
        on_progress: &mut dyn FnMut(TranscodeProgress),
        cancel: &CancelToken,
    ) -> Result<(), UpscaleError>;
}

/// The few file operations the batch needs.
pub trait FileSystem {
    fn exists(&self, path: &Path) -> bool;
    /// Create `dir` if needed and prove that a file can be written in it.
    fn ensure_writable_dir(&self, dir: &Path) -> Result<(), UpscaleError>;
    /// Move `from` to `to`. Without `replace`, an existing `to` is an error.
    fn move_file(&self, from: &Path, to: &Path, replace: bool) -> Result<(), UpscaleError>;
    /// Best-effort removal of a temporary file.
    fn remove_file(&self, path: &Path);
}

/// How to install ffmpeg on this platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallHint {
    pub command: String,
    /// The app can open a terminal that runs `command`.
    pub can_run_in_app: bool,
    pub note: Option<String>,
}

/// Defaults and advice that differ between Windows and macOS.
pub trait Platform {
    fn default_output_folder(&self) -> PathBuf;
    /// Encoders to try, best first.
    fn preferred_encoders(&self) -> Vec<Encoder>;
    fn install_hint(&self) -> InstallHint;
    /// Extra advice when a folder can't be written, e.g. Controlled Folder Access.
    fn write_blocked_hint(&self) -> Option<String>;
}

/// Saved preferences.
pub trait SettingsStore {
    fn load(&self) -> Settings;
    fn save(&self, settings: &Settings) -> Result<(), String>;
}

/// The ffmpeg programs found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolkit {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// First line of `ffmpeg -version`.
    pub version: String,
}

pub trait ToolkitLocator {
    fn locate(&self) -> Option<Toolkit>;
}

/// Runs a tiny test encode to see whether an encoder works here.
pub trait EncoderTester {
    fn works(&self, toolkit: &Toolkit, encoder: Encoder) -> bool;
}
