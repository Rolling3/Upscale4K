//! The batch's file operations on the real disk.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use upscale4k_application::error::UpscaleError;
use upscale4k_application::ports::FileSystem;

#[derive(Debug, Default, Clone, Copy)]
pub struct LocalFileSystem;

impl FileSystem for LocalFileSystem {
    /// Anything at the path counts, even a broken link, so it is never written over.
    fn exists(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    fn ensure_writable_dir(&self, dir: &Path) -> Result<(), UpscaleError> {
        if let Ok(meta) = fs::metadata(dir)
            && !meta.is_dir()
        {
            return Err(UpscaleError::OutputFolderNotWritable {
                folder: dir.to_path_buf(),
                detail: "That path is a file, not a folder.".into(),
            });
        }
        fs::create_dir_all(dir).map_err(|e| folder_error(dir, &e))?;

        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        let probe = dir.join(format!(".upscale4k-write-test-{}-{stamp}.tmp", std::process::id()));
        let written = OpenOptions::new().write(true).create_new(true).open(&probe).and_then(|mut f| f.write_all(b"ok"));
        let _ = fs::remove_file(&probe);
        written.map_err(|e| folder_error(dir, &e))
    }

    fn move_file(&self, from: &Path, to: &Path, replace: bool) -> Result<(), UpscaleError> {
        if !replace && self.exists(to) {
            return Err(UpscaleError::OutputFileBusy {
                path: to.to_path_buf(),
                detail: "A file with this name appeared while saving.".into(),
            });
        }
        // Antivirus or indexing tools can hold a fresh file open for a moment.
        let mut last = None;
        for _ in 0..5 {
            match fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(e) if is_transient(&e) => {
                    last = Some(e);
                    thread::sleep(Duration::from_millis(200));
                }
                Err(e) => {
                    last = Some(e);
                    break;
                }
            }
        }
        let error = last.unwrap_or_else(|| io::Error::other("unknown error"));
        if error.kind() == ErrorKind::StorageFull {
            let folder = to.parent().unwrap_or(to).to_path_buf();
            return Err(UpscaleError::NotEnoughSpace { folder });
        }
        Err(UpscaleError::OutputFileBusy { path: to.to_path_buf(), detail: describe(&error) })
    }

    fn remove_file(&self, path: &Path) {
        for _ in 0..10 {
            match fs::remove_file(path) {
                Ok(()) => return,
                Err(e) if e.kind() == ErrorKind::NotFound => return,
                Err(_) => thread::sleep(Duration::from_millis(150)),
            }
        }
    }
}

fn is_transient(error: &io::Error) -> bool {
    // ERROR_ACCESS_DENIED (5), ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33) on Windows.
    cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

fn folder_error(dir: &Path, error: &io::Error) -> UpscaleError {
    if error.kind() == ErrorKind::StorageFull {
        return UpscaleError::NotEnoughSpace { folder: dir.to_path_buf() };
    }
    UpscaleError::OutputFolderNotWritable { folder: dir.to_path_buf(), detail: describe(error) }
}

/// Plain words for the common cases, the system's text otherwise.
fn describe(error: &io::Error) -> String {
    match error.kind() {
        ErrorKind::PermissionDenied => "Access was denied.".into(),
        ErrorKind::ReadOnlyFilesystem => "The drive is read-only.".into(),
        ErrorKind::NotFound => "The folder doesn't exist and couldn't be created.".into(),
        ErrorKind::NotADirectory => "Part of the path is a file, not a folder.".into(),
        ErrorKind::StorageFull => "The disk is full.".into(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_folder_is_created_and_left_clean() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("new").join("deeper");
        LocalFileSystem.ensure_writable_dir(&dir).unwrap();
        assert!(dir.is_dir());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "the write test file must be removed");
    }

    #[test]
    fn a_file_in_place_of_the_folder_is_reported_as_not_writable() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("taken");
        fs::write(&file, b"x").unwrap();
        let got = LocalFileSystem.ensure_writable_dir(&file);
        assert!(matches!(got, Err(UpscaleError::OutputFolderNotWritable { .. })), "{got:?}");
    }

    #[test]
    fn a_folder_below_a_file_is_reported_as_not_writable() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("taken");
        fs::write(&file, b"x").unwrap();
        let got = LocalFileSystem.ensure_writable_dir(&file.join("sub"));
        assert!(matches!(got, Err(UpscaleError::OutputFolderNotWritable { .. })), "{got:?}");
    }

    #[test]
    fn move_without_replace_refuses_an_existing_target_and_keeps_both_files() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("a.part"), root.path().join("a.mp4"));
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"old").unwrap();
        let got = LocalFileSystem.move_file(&from, &to, false);
        assert!(matches!(got, Err(UpscaleError::OutputFileBusy { .. })), "{got:?}");
        assert_eq!(fs::read(&to).unwrap(), b"old");
        assert!(from.exists());
    }

    #[test]
    fn move_with_replace_overwrites_the_target() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("a.part"), root.path().join("a.mp4"));
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"old").unwrap();
        LocalFileSystem.move_file(&from, &to, true).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"new");
        assert!(!from.exists());
    }

    #[test]
    fn removing_a_missing_file_is_quietly_fine() {
        let root = tempfile::tempdir().unwrap();
        LocalFileSystem.remove_file(&root.path().join("never-there.part"));
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_is_reported_as_not_writable() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("locked");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let got = LocalFileSystem.ensure_writable_dir(&dir);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(got, Err(UpscaleError::OutputFolderNotWritable { .. })), "{got:?}");
    }
}
