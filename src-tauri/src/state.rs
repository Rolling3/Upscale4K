//! What the app keeps between commands.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use upscale4k_application::cancel::CancelToken;
use upscale4k_application::environment::{Environment, detect_environment};
use upscale4k_application::settings;
use upscale4k_infrastructure::encoder_tester::FfmpegEncoderTester;
use upscale4k_infrastructure::locator::FfmpegLocator;
use upscale4k_infrastructure::platform::DesktopPlatform;
use upscale4k_infrastructure::settings_store::JsonSettingsStore;

pub struct AppState {
    env: Mutex<Option<Environment>>,
    /// Set while a batch runs; holds the token that stops it.
    batch: Mutex<Option<CancelToken>>,
    quitting: AtomicBool,
    settings_file: PathBuf,
}

/// A poisoned lock only means another thread panicked; the data is still usable.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AppState {
    pub fn new(settings_file: PathBuf) -> Self {
        Self { env: Mutex::new(None), batch: Mutex::new(None), quitting: AtomicBool::new(false), settings_file }
    }

    pub fn settings_store(&self) -> JsonSettingsStore {
        JsonSettingsStore::new(self.settings_file.clone())
    }

    pub fn platform(&self) -> DesktopPlatform {
        let ffmpeg = lock(&self.env).as_ref().and_then(|e| e.toolkit.as_ref()).map(|t| t.ffmpeg.clone());
        DesktopPlatform::new(ffmpeg)
    }

    pub fn output_folder(&self) -> PathBuf {
        settings::output_folder(&self.settings_store(), &self.platform())
    }

    /// Look for ffmpeg and test the encoders again. Takes up to a few seconds.
    pub fn detect(&self) -> Environment {
        let env = detect_environment(&FfmpegLocator::system(), &FfmpegEncoderTester, &DesktopPlatform::default());
        *lock(&self.env) = Some(env.clone());
        env
    }

    pub fn environment(&self) -> Option<Environment> {
        lock(&self.env).clone()
    }

    /// Mark a batch as running. Fails when one already is.
    pub fn begin_batch(&self) -> Option<CancelToken> {
        let mut running = lock(&self.batch);
        if running.is_some() {
            return None;
        }
        let token = CancelToken::new();
        *running = Some(token.clone());
        Some(token)
    }

    /// Free the slot, but only if it still belongs to the batch that `token` stops.
    pub fn end_batch(&self, token: &CancelToken) {
        let mut running = lock(&self.batch);
        if running.as_ref().is_some_and(|current| current.same_as(token)) {
            *running = None;
        }
    }

    pub fn running_batch(&self) -> Option<CancelToken> {
        lock(&self.batch).clone()
    }

    /// The app is shutting down; close and exit requests are no longer held back.
    pub fn mark_quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }

    pub fn is_quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }
}
