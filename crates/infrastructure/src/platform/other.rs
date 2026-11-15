//! Other systems: not a release target, but the code builds and runs there.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use upscale4k_application::ports::InstallHint;
use upscale4k_domain::encoder::Encoder;

use super::{home_dir, process_path_dirs};

pub fn default_output_folder() -> PathBuf {
    home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("Videos").join("Upscale4K")
}

pub fn preferred_encoders() -> Vec<Encoder> {
    vec![Encoder::X264]
}

pub fn install_hint() -> InstallHint {
    InstallHint { command: "Install ffmpeg with your package manager".into(), can_run_in_app: false, note: None }
}

pub fn write_blocked_hint(_ffmpeg: Option<&Path>) -> Option<String> {
    None
}

pub fn ffmpeg_search_dirs() -> Vec<PathBuf> {
    let mut dirs = process_path_dirs();
    dirs.extend([PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")]);
    dirs
}

pub fn start_ffmpeg_install() -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "install ffmpeg with your package manager"))
}

pub fn open_folder(dir: &Path) -> io::Result<()> {
    Command::new("xdg-open").arg(dir).spawn().map(|_| ())
}
