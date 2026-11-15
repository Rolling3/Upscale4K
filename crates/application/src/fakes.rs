//! In-memory stand-ins for the ports, used by the use-case tests.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use upscale4k_domain::encoder::Encoder;
use upscale4k_domain::media::{FrameSize, VideoInfo};

use crate::cancel::CancelToken;
use crate::error::UpscaleError;
use crate::ports::{
    EncoderTester, FileSystem, InstallHint, MediaProbe, Platform, SettingsStore, Toolkit, ToolkitLocator,
    TranscodeProgress, Transcoder,
};
use crate::settings::Settings;

pub struct FakePlatform {
    hint: Option<String>,
    install: InstallHint,
}

impl FakePlatform {
    pub const DEFAULT_FOLDER: &'static str = "C:/OPFile/4K";
    pub const BLOCKED_HINT: &'static str = "Controlled Folder Access may block this folder.";

    pub fn windows_like() -> Self {
        Self {
            hint: Some(Self::BLOCKED_HINT.into()),
            install: InstallHint {
                command: "winget install --id Gyan.FFmpeg -e".into(),
                can_run_in_app: true,
                note: None,
            },
        }
    }

    pub fn mac_like() -> Self {
        Self {
            hint: None,
            install: InstallHint { command: "brew install ffmpeg".into(), can_run_in_app: false, note: None },
        }
    }
}

impl Platform for FakePlatform {
    fn default_output_folder(&self) -> PathBuf {
        PathBuf::from(Self::DEFAULT_FOLDER)
    }
    fn preferred_encoders(&self) -> Vec<Encoder> {
        vec![Encoder::Nvenc, Encoder::X264]
    }
    fn install_hint(&self) -> InstallHint {
        self.install.clone()
    }
    fn write_blocked_hint(&self) -> Option<String> {
        self.hint.clone()
    }
}

#[derive(Default)]
pub struct FakeFs {
    pub files: RefCell<HashSet<PathBuf>>,
    read_only: Cell<bool>,
    /// Destinations that refuse to be replaced (open in another app).
    pub busy: RefCell<HashSet<PathBuf>>,
    pub moves: RefCell<Vec<(PathBuf, PathBuf, bool)>>,
    pub removed: RefCell<Vec<PathBuf>>,
}

impl FakeFs {
    pub fn writable() -> Self {
        Self::default()
    }

    pub fn read_only() -> Self {
        let fs = Self::default();
        fs.read_only.set(true);
        fs
    }

    pub fn with_files(paths: &[PathBuf]) -> Self {
        let fs = Self::default();
        fs.files.borrow_mut().extend(paths.iter().cloned());
        fs
    }

    pub fn has(&self, path: &Path) -> bool {
        self.files.borrow().contains(path)
    }
}

impl FileSystem for FakeFs {
    fn exists(&self, path: &Path) -> bool {
        self.has(path)
    }

    fn ensure_writable_dir(&self, dir: &Path) -> Result<(), UpscaleError> {
        if self.read_only.get() {
            Err(UpscaleError::OutputFolderNotWritable { folder: dir.to_path_buf(), detail: "Access is denied.".into() })
        } else {
            Ok(())
        }
    }

    fn move_file(&self, from: &Path, to: &Path, replace: bool) -> Result<(), UpscaleError> {
        let busy = UpscaleError::OutputFileBusy { path: to.to_path_buf(), detail: "in use".into() };
        if self.busy.borrow().contains(to) || (!replace && self.has(to)) {
            return Err(busy);
        }
        assert!(self.has(from), "moving a file that doesn't exist: {from:?}");
        let mut files = self.files.borrow_mut();
        files.remove(from);
        files.insert(to.to_path_buf());
        self.moves.borrow_mut().push((from.to_path_buf(), to.to_path_buf(), replace));
        Ok(())
    }

    fn remove_file(&self, path: &Path) {
        self.files.borrow_mut().remove(path);
        self.removed.borrow_mut().push(path.to_path_buf());
    }
}

/// A 5 second 1080p60 clip with AAC audio.
pub fn standard_clip() -> VideoInfo {
    VideoInfo {
        size: FrameSize { width: 1920, height: 1080 },
        duration_secs: Some(5.0),
        frame_rate: Some(60.0),
        audio_codecs: vec!["aac".into()],
    }
}

#[derive(Default)]
pub struct FakeProbe {
    pub special: HashMap<PathBuf, Result<VideoInfo, UpscaleError>>,
}

impl FakeProbe {
    pub fn with(path: &Path, result: Result<VideoInfo, UpscaleError>) -> Self {
        let mut probe = Self::default();
        probe.special.insert(path.to_path_buf(), result);
        probe
    }
}

impl MediaProbe for FakeProbe {
    fn probe(&self, input: &Path) -> Result<VideoInfo, UpscaleError> {
        self.special.get(input).cloned().unwrap_or_else(|| Ok(standard_clip()))
    }
}

/// What the fake encoder does on its next call.
pub enum Step {
    Succeed(Vec<TranscodeProgress>),
    Fail(UpscaleError),
    /// The user presses Cancel just as the encoder fails on its own.
    FailAfterCancel(UpscaleError),
    /// The user presses Cancel halfway through.
    CancelHalfway,
    /// Succeed, but another program creates `path` meanwhile.
    SucceedWhileCreating(PathBuf),
}

pub struct FakeTranscoder<'a> {
    fs: &'a FakeFs,
    steps: RefCell<VecDeque<Step>>,
    pub calls: RefCell<Vec<(Vec<String>, Encoder)>>,
}

impl<'a> FakeTranscoder<'a> {
    pub fn new(fs: &'a FakeFs, steps: Vec<Step>) -> Self {
        Self { fs, steps: RefCell::new(steps.into()), calls: RefCell::default() }
    }

    pub fn call_count(&self) -> usize {
        self.calls.borrow().len()
    }

    pub fn args_of(&self, call: usize) -> Vec<String> {
        self.calls.borrow()[call].0.clone()
    }

    pub fn encoder_of(&self, call: usize) -> Encoder {
        self.calls.borrow()[call].1
    }
}

impl Transcoder for FakeTranscoder<'_> {
    fn transcode(
        &self,
        args: &[OsString],
        encoder: Encoder,
        on_progress: &mut dyn FnMut(TranscodeProgress),
        cancel: &CancelToken,
    ) -> Result<(), UpscaleError> {
        let strings: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let temp = PathBuf::from(args.last().expect("output path is the last argument"));
        self.calls.borrow_mut().push((strings, encoder));
        // ffmpeg creates its output file as soon as it starts.
        self.fs.files.borrow_mut().insert(temp);

        match self.steps.borrow_mut().pop_front().unwrap_or(Step::Succeed(vec![])) {
            Step::Succeed(samples) => {
                samples.into_iter().for_each(&mut *on_progress);
                Ok(())
            }
            Step::Fail(error) => Err(error),
            Step::FailAfterCancel(error) => {
                cancel.cancel();
                Err(error)
            }
            Step::CancelHalfway => {
                on_progress(TranscodeProgress { out_time_us: Some(2_500_000), speed: Some(1.0) });
                cancel.cancel();
                Err(UpscaleError::Cancelled)
            }
            Step::SucceedWhileCreating(path) => {
                self.fs.files.borrow_mut().insert(path);
                Ok(())
            }
        }
    }
}

#[derive(Default)]
pub struct FakeStore {
    settings: RefCell<Settings>,
    fail: bool,
}

impl FakeStore {
    pub fn with_folder(folder: &str) -> Self {
        Self { settings: RefCell::new(Settings { output_folder: Some(folder.into()) }), fail: false }
    }

    pub fn failing() -> Self {
        Self { settings: RefCell::default(), fail: true }
    }

    pub fn saved(&self) -> Settings {
        self.settings.borrow().clone()
    }
}

impl SettingsStore for FakeStore {
    fn load(&self) -> Settings {
        self.settings.borrow().clone()
    }

    fn save(&self, settings: &Settings) -> Result<(), String> {
        if self.fail {
            return Err("disk full".into());
        }
        *self.settings.borrow_mut() = settings.clone();
        Ok(())
    }
}

pub struct FakeLocator(Option<Toolkit>);

impl FakeLocator {
    pub fn found() -> Self {
        Self(Some(Toolkit {
            ffmpeg: PathBuf::from("bin/ffmpeg"),
            ffprobe: PathBuf::from("bin/ffprobe"),
            version: "ffmpeg version 9.0.2".into(),
        }))
    }

    pub fn missing() -> Self {
        Self(None)
    }
}

impl ToolkitLocator for FakeLocator {
    fn locate(&self) -> Option<Toolkit> {
        self.0.clone()
    }
}

pub struct FakeTester {
    working: Vec<Encoder>,
    tried: RefCell<Vec<Encoder>>,
}

impl FakeTester {
    pub fn working(encoders: &[Encoder]) -> Self {
        Self { working: encoders.to_vec(), tried: RefCell::default() }
    }

    pub fn tried(&self) -> Vec<Encoder> {
        self.tried.borrow().clone()
    }
}

impl EncoderTester for FakeTester {
    fn works(&self, _toolkit: &Toolkit, encoder: Encoder) -> bool {
        self.tried.borrow_mut().push(encoder);
        self.working.contains(&encoder)
    }
}
