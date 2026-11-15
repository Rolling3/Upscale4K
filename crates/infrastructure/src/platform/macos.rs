//! macOS: VideoToolbox first, ~/Movies/Upscale4K by default, ffmpeg from Homebrew.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use upscale4k_application::ports::InstallHint;
use upscale4k_domain::encoder::Encoder;

use super::{home_dir, process_path_dirs};

pub fn default_output_folder() -> PathBuf {
    home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("Movies").join("Upscale4K")
}

pub fn preferred_encoders() -> Vec<Encoder> {
    vec![Encoder::VideoToolbox, Encoder::X264]
}

pub fn install_hint() -> InstallHint {
    InstallHint {
        command: "brew install ffmpeg".into(),
        can_run_in_app: false,
        note: Some("Run it in Terminal. If you don't have Homebrew yet, install it from brew.sh first. Then choose Check again.".into()),
    }
}

pub fn write_blocked_hint(_ffmpeg: Option<&Path>) -> Option<String> {
    Some(
        "macOS may be blocking access. Check System Settings > Privacy & Security > Files and Folders, \
         or choose another output folder."
            .into(),
    )
}

/// Apps started from Finder don't get the shell's PATH, so the usual
/// Homebrew and MacPorts folders are searched first.
pub fn ffmpeg_search_dirs() -> Vec<PathBuf> {
    let mut dirs =
        vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin"), PathBuf::from("/opt/local/bin")];
    dirs.extend(process_path_dirs());
    dirs
}

pub fn start_ffmpeg_install() -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "run brew install ffmpeg in Terminal"))
}

pub fn open_folder(dir: &Path) -> io::Result<()> {
    Command::new("open").arg(dir).spawn().map(|_| ())
}
