//! The output folder preference.

use std::path::{Path, PathBuf};

use crate::error::UserMessage;
use crate::ports::{FileSystem, Platform, SettingsStore};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    pub output_folder: Option<PathBuf>,
}

/// The saved output folder, or the platform default on first run.
pub fn output_folder(store: &dyn SettingsStore, platform: &dyn Platform) -> PathBuf {
    store.load().output_folder.unwrap_or_else(|| platform.default_output_folder())
}

/// Switch to `folder` if a file can be written there, and remember it.
pub fn change_output_folder(
    folder: &Path,
    store: &dyn SettingsStore,
    fs: &dyn FileSystem,
    platform: &dyn Platform,
) -> Result<PathBuf, UserMessage> {
    fs.ensure_writable_dir(folder).map_err(|e| e.to_user_message(platform))?;
    let mut settings = store.load();
    settings.output_folder = Some(folder.to_path_buf());
    store.save(&settings).map_err(|detail| UserMessage {
        text: format!("Couldn't save the output folder setting: {detail}"),
        hint: None,
    })?;
    Ok(folder.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{FakeFs, FakePlatform, FakeStore};

    #[test]
    fn first_run_uses_the_platform_default() {
        let store = FakeStore::default();
        assert_eq!(output_folder(&store, &FakePlatform::windows_like()), PathBuf::from(FakePlatform::DEFAULT_FOLDER));
    }

    #[test]
    fn a_saved_folder_wins_over_the_default() {
        let store = FakeStore::with_folder("D:/Exports");
        assert_eq!(output_folder(&store, &FakePlatform::windows_like()), PathBuf::from("D:/Exports"));
    }

    #[test]
    fn a_writable_folder_is_saved_and_returned() {
        let store = FakeStore::default();
        let fs = FakeFs::writable();
        let got = change_output_folder(Path::new("E:/Renders"), &store, &fs, &FakePlatform::windows_like());
        assert_eq!(got, Ok(PathBuf::from("E:/Renders")));
        assert_eq!(store.saved().output_folder, Some(PathBuf::from("E:/Renders")));
    }

    #[test]
    fn an_unwritable_folder_is_refused_with_the_hint_and_not_saved() {
        let store = FakeStore::with_folder("D:/Exports");
        let fs = FakeFs::read_only();
        let got = change_output_folder(Path::new("C:/Users/me/Videos"), &store, &fs, &FakePlatform::windows_like());
        let message = got.expect_err("folder must be refused");
        assert!(message.text.contains("Can't write"), "{}", message.text);
        assert_eq!(message.hint.as_deref(), Some(FakePlatform::BLOCKED_HINT));
        assert_eq!(store.saved().output_folder, Some(PathBuf::from("D:/Exports")));
    }

    #[test]
    fn a_failed_save_is_reported() {
        let store = FakeStore::failing();
        let fs = FakeFs::writable();
        let got = change_output_folder(Path::new("E:/Renders"), &store, &fs, &FakePlatform::windows_like());
        let message = got.expect_err("a failed save must be reported");
        assert!(message.text.contains("disk full"), "{}", message.text);
    }
}
