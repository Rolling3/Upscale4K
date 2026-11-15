//! Finds ffmpeg and ffprobe on this machine.

use std::path::{Path, PathBuf};
use std::time::Duration;

use upscale4k_application::ports::{Toolkit, ToolkitLocator};

use crate::platform;
use crate::process::{hidden_command, run_with_timeout};

pub struct FfmpegLocator {
    dirs: Vec<PathBuf>,
}

impl FfmpegLocator {
    /// Search PATH plus the places installers put ffmpeg on this platform.
    pub fn system() -> Self {
        Self { dirs: platform::ffmpeg_search_dirs() }
    }

    /// Search only `dirs`, in order.
    pub fn in_dirs(dirs: Vec<PathBuf>) -> Self {
        Self { dirs }
    }
}

impl ToolkitLocator for FfmpegLocator {
    fn locate(&self) -> Option<Toolkit> {
        unique(&self.dirs).into_iter().find_map(|dir| toolkit_in(&dir))
    }
}

/// ffmpeg and ffprobe side by side in `dir`, with ffmpeg answering `-version`.
fn toolkit_in(dir: &Path) -> Option<Toolkit> {
    let ffmpeg = dir.join(executable("ffmpeg"));
    let ffprobe = dir.join(executable("ffprobe"));
    if !ffmpeg.is_file() || !ffprobe.is_file() {
        return None;
    }
    let mut command = hidden_command(&ffmpeg);
    command.args(["-hide_banner", "-version"]);
    let run = run_with_timeout(command, Duration::from_secs(10))?;
    if !run.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&run.stdout).lines().next().unwrap_or("ffmpeg").trim().to_string();
    Some(Toolkit { ffmpeg, ffprobe, version })
}

fn executable(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Drop repeats, comparing without case on Windows.
fn unique(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    dirs.iter()
        .filter(|d| !d.as_os_str().is_empty())
        .filter(|d| {
            let key = d.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
            seen.insert(if cfg!(windows) { key.to_lowercase() } else { key })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_found_in_empty_folders() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(FfmpegLocator::in_dirs(vec![root.path().to_path_buf()]).locate(), None);
    }

    #[test]
    fn ffmpeg_without_ffprobe_next_to_it_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(executable("ffmpeg")), b"").unwrap();
        assert_eq!(FfmpegLocator::in_dirs(vec![root.path().to_path_buf()]).locate(), None);
    }

    #[test]
    fn a_file_that_is_not_a_working_ffmpeg_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(executable("ffmpeg")), b"not a program").unwrap();
        std::fs::write(root.path().join(executable("ffprobe")), b"not a program").unwrap();
        assert_eq!(FfmpegLocator::in_dirs(vec![root.path().to_path_buf()]).locate(), None);
    }

    #[test]
    fn repeated_folders_are_searched_once() {
        let dirs = vec![PathBuf::from("/a/bin"), PathBuf::from("/a/bin/"), PathBuf::from("/b"), PathBuf::new()];
        assert_eq!(unique(&dirs), vec![PathBuf::from("/a/bin"), PathBuf::from("/b")]);
    }
}
