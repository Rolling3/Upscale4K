//! Settings kept in a small JSON file in the app's config folder.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use upscale4k_application::ports::SettingsStore;
use upscale4k_application::settings::Settings;

pub struct JsonSettingsStore {
    path: PathBuf,
}

impl JsonSettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    output_folder: Option<PathBuf>,
}

impl SettingsStore for JsonSettingsStore {
    /// A missing or unreadable file means first run: defaults are used.
    fn load(&self) -> Settings {
        let file: SettingsFile =
            fs::read_to_string(&self.path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        Settings { output_folder: file.output_folder.filter(|p| !p.as_os_str().is_empty()) }
    }

    fn save(&self, settings: &Settings) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&SettingsFile { output_folder: settings.output_folder.clone() })
            .map_err(|e| e.to_string())?;
        let temp = self.path.with_extension("json.tmp");
        fs::write(&temp, text).map_err(|e| e.to_string())?;
        fs::rename(&temp, &self.path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_folder_is_loaded_back() {
        let root = tempfile::tempdir().unwrap();
        let store = JsonSettingsStore::new(root.path().join("cfg").join("settings.json"));
        let settings = Settings { output_folder: Some(PathBuf::from("D:/Exports/4K")) };
        store.save(&settings).unwrap();
        assert_eq!(store.load(), settings);
    }

    #[test]
    fn no_file_means_defaults() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(JsonSettingsStore::new(root.path().join("settings.json")).load(), Settings::default());
    }

    #[test]
    fn an_unreadable_file_means_defaults() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(JsonSettingsStore::new(path).load(), Settings::default());
    }

    #[test]
    fn saving_twice_keeps_only_the_latest_value_and_no_temp_file() {
        let root = tempfile::tempdir().unwrap();
        let store = JsonSettingsStore::new(root.path().join("settings.json"));
        store.save(&Settings { output_folder: Some("A".into()) }).unwrap();
        store.save(&Settings { output_folder: Some("B".into()) }).unwrap();
        assert_eq!(store.load().output_folder, Some(PathBuf::from("B")));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
