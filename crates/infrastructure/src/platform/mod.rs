//! Defaults and advice that differ between Windows and macOS: default
//! output folder, encoder order, how to install ffmpeg, where to look for it.

use std::path::{Path, PathBuf};

use upscale4k_application::ports::{InstallHint, Platform};
use upscale4k_domain::encoder::Encoder;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod other;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(not(any(windows, target_os = "macos")))]
use other as imp;
#[cfg(windows)]
use windows as imp;

/// The `Platform` port for the machine the app runs on.
#[derive(Debug, Clone, Default)]
pub struct DesktopPlatform {
    ffmpeg: Option<PathBuf>,
}

impl DesktopPlatform {
    /// `ffmpeg` is named in advice about blocked folders.
    pub fn new(ffmpeg: Option<PathBuf>) -> Self {
        Self { ffmpeg }
    }
}

impl Platform for DesktopPlatform {
    fn default_output_folder(&self) -> PathBuf {
        imp::default_output_folder()
    }

    fn preferred_encoders(&self) -> Vec<Encoder> {
        imp::preferred_encoders()
    }

    fn install_hint(&self) -> InstallHint {
        imp::install_hint()
    }

    fn write_blocked_hint(&self) -> Option<String> {
        imp::write_blocked_hint(self.ffmpeg.as_deref())
    }
}

/// Folders to search for ffmpeg, best first.
pub fn ffmpeg_search_dirs() -> Vec<PathBuf> {
    imp::ffmpeg_search_dirs()
}

/// Open a terminal that installs ffmpeg, where the platform allows it.
pub fn start_ffmpeg_install() -> std::io::Result<()> {
    imp::start_ffmpeg_install()
}

/// Show a folder in the file manager.
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    imp::open_folder(dir)
}

/// Folders listed in the PATH environment variable of this process.
fn process_path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

#[cfg_attr(windows, allow(dead_code))]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}
