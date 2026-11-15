//! End-to-end checks against the ffmpeg installed on this machine.
//!
//! These need ffmpeg and ffprobe (Windows: winget install --id Gyan.FFmpeg -e,
//! macOS: brew install ffmpeg). Each test works in its own temp folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;
use upscale4k_application::batch::{BatchEvent, BatchItem, BatchRequest, BatchSummary, Upscaler};
use upscale4k_application::cancel::CancelToken;
use upscale4k_application::environment::detect_environment;
use upscale4k_application::ports::{EncoderTester, InstallHint, Platform, Toolkit, ToolkitLocator};
use upscale4k_domain::encoder::Encoder;
use upscale4k_domain::naming::ConflictChoice;
use upscale4k_infrastructure::encoder_tester::FfmpegEncoderTester;
use upscale4k_infrastructure::ffmpeg_runner::FfmpegTranscoder;
use upscale4k_infrastructure::ffprobe::FfprobeMediaProbe;
use upscale4k_infrastructure::local_fs::LocalFileSystem;
use upscale4k_infrastructure::locator::FfmpegLocator;
use upscale4k_infrastructure::platform::DesktopPlatform;

fn toolkit() -> Toolkit {
    FfmpegLocator::system().locate().expect("ffmpeg and ffprobe must be installed to run the integration tests")
}

/// Make a test clip with ffmpeg's built-in sources.
fn generate(kit: &Toolkit, out: &Path, args: &[&str]) {
    let status = Command::new(&kit.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(args)
        .arg(out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "generating {out:?} failed");
}

/// A 1080p clip at `fps` with a 440 Hz tone, `secs` long.
fn gameplay_clip(kit: &Toolkit, out: &Path, fps: u32, secs: u32, audio_codec: &str) {
    let video = format!("testsrc2=size=1920x1080:rate={fps}");
    let secs = secs.to_string();
    generate(
        kit,
        out,
        &[
            "-f",
            "lavfi",
            "-i",
            &video,
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            &secs,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            audio_codec,
        ],
    );
}

fn ffprobe(kit: &Toolkit, file: &Path) -> Value {
    let out = Command::new(&kit.ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(file)
        .output()
        .expect("ffprobe runs");
    assert!(out.status.success(), "ffprobe failed on {file:?}");
    serde_json::from_slice(&out.stdout).expect("ffprobe json")
}

fn streams<'a>(info: &'a Value, kind: &str) -> Vec<&'a Value> {
    info["streams"].as_array().unwrap().iter().filter(|s| s["codec_type"] == kind).collect()
}

struct Outcome {
    events: Vec<BatchEvent>,
    summary: BatchSummary,
}

fn run_batch(
    kit: &Toolkit,
    platform: &dyn Platform,
    encoder: Encoder,
    output_dir: &Path,
    inputs: &[PathBuf],
    mut on_event: impl FnMut(&BatchEvent),
    cancel: &CancelToken,
) -> Outcome {
    let probe = FfprobeMediaProbe::new(kit.ffprobe.clone());
    let transcoder = FfmpegTranscoder::new(kit.ffmpeg.clone());
    let upscaler = Upscaler { probe: &probe, transcoder: &transcoder, fs: &LocalFileSystem, platform };
    let request = BatchRequest {
        output_dir: output_dir.to_path_buf(),
        preferred_encoder: encoder,
        items: inputs.iter().map(|p| BatchItem { input: p.clone(), on_conflict: ConflictChoice::KeepBoth }).collect(),
    };
    let mut events = Vec::new();
    let summary = upscaler.run(&request, cancel, &mut |e| {
        on_event(&e);
        events.push(e);
    });
    Outcome { events, summary }
}

fn leftovers(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".part") || n.starts_with(".upscale4k-write-test"))
                .collect()
        })
        .unwrap_or_default()
}

fn assert_upscaled(kit: &Toolkit, file: &Path, fps: &str, audio_codec: &str) {
    assert!(file.is_file(), "{file:?} was not written");
    let info = ffprobe(kit, file);
    let video = streams(&info, "video");
    assert_eq!(video.len(), 1, "{info}");
    assert_eq!(video[0]["codec_name"], "h264");
    assert_eq!(video[0]["height"], 2160);
    assert_eq!(video[0]["width"], 3840);
    assert_eq!(video[0]["r_frame_rate"], fps, "frame rate must not change");
    let audio = streams(&info, "audio");
    assert_eq!(audio.len(), 1, "audio must be present: {info}");
    assert_eq!(audio[0]["codec_name"], audio_codec);
}

/// The encoder the app would choose on this machine.
fn detected_encoder(kit: &Toolkit) -> Encoder {
    struct Found(Toolkit);
    impl ToolkitLocator for Found {
        fn locate(&self) -> Option<Toolkit> {
            Some(self.0.clone())
        }
    }
    let env = detect_environment(&Found(kit.clone()), &FfmpegEncoderTester, &DesktopPlatform::default());
    env.encoder.expect("at least libx264 must work")
}

#[test]
fn a_five_second_1080p60_clip_becomes_2160p60_with_its_audio() {
    let kit = toolkit();
    let work = tempfile::tempdir().unwrap();
    let input = work.path().join("gameplay.mp4");
    gameplay_clip(&kit, &input, 60, 5, "aac");
    let out_dir = work.path().join("4K");

    let encoder = detected_encoder(&kit);
    let platform = DesktopPlatform::new(Some(kit.ffmpeg.clone()));
    let run = run_batch(&kit, &platform, encoder, &out_dir, &[input], |_| {}, &CancelToken::new());

    assert_eq!(run.summary, BatchSummary { saved: 1, failed: 0, skipped: 0, cancelled: false }, "{:?}", run.events);
    let output = out_dir.join("gameplay_4K.mp4");
    assert_upscaled(&kit, &output, "60/1", "aac");
    let duration: f64 = ffprobe(&kit, &output)["format"]["duration"].as_str().unwrap().parse().unwrap();
    assert!((duration - 5.0).abs() < 0.2, "duration {duration}");

    let fractions: Vec<f64> = run
        .events
        .iter()
        .filter_map(|e| match e {
            BatchEvent::FileProgress { fraction, .. } => *fraction,
            _ => None,
        })
        .collect();
    assert!(!fractions.is_empty(), "no progress was reported");
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "progress went backwards: {fractions:?}");
    assert!(fractions.iter().all(|f| (0.0..=1.0).contains(f)));
    assert!(leftovers(&out_dir).is_empty(), "{:?}", leftovers(&out_dir));
}

#[test]
fn an_obs_style_mkv_and_a_mov_with_pcm_audio_keep_their_frame_rate_and_get_numbered_names() {
    let kit = toolkit();
    let work = tempfile::tempdir().unwrap();
    let mkv = work.path().join("session.mkv");
    let mov = work.path().join("session.mov");
    gameplay_clip(&kit, &mkv, 60, 2, "aac");
    gameplay_clip(&kit, &mov, 30, 1, "pcm_s16le");
    let out_dir = work.path().join("out");

    let encoder = detected_encoder(&kit);
    let platform = DesktopPlatform::new(Some(kit.ffmpeg.clone()));
    let run = run_batch(&kit, &platform, encoder, &out_dir, &[mkv, mov], |_| {}, &CancelToken::new());

    assert_eq!(run.summary.saved, 2, "{:?}", run.events);
    assert_upscaled(&kit, &out_dir.join("session_4K.mp4"), "60/1", "aac");
    assert_upscaled(&kit, &out_dir.join("session_4K (2).mp4"), "30/1", "aac");
}

#[test]
fn a_missing_gpu_encoder_falls_back_to_libx264() {
    let kit = toolkit();
    // An encoder that can't work on this OS: NVENC isn't in Homebrew's ffmpeg,
    // and VideoToolbox doesn't exist on Windows.
    let unavailable = if cfg!(target_os = "macos") { Encoder::Nvenc } else { Encoder::VideoToolbox };
    assert!(!FfmpegEncoderTester.works(&kit, unavailable), "{unavailable:?} unexpectedly works here");

    struct PrefersUnavailable(Encoder);
    impl Platform for PrefersUnavailable {
        fn default_output_folder(&self) -> PathBuf {
            DesktopPlatform::default().default_output_folder()
        }
        fn preferred_encoders(&self) -> Vec<Encoder> {
            vec![self.0, Encoder::X264]
        }
        fn install_hint(&self) -> InstallHint {
            DesktopPlatform::default().install_hint()
        }
        fn write_blocked_hint(&self) -> Option<String> {
            None
        }
    }
    struct Found(Toolkit);
    impl ToolkitLocator for Found {
        fn locate(&self) -> Option<Toolkit> {
            Some(self.0.clone())
        }
    }
    let platform = PrefersUnavailable(unavailable);
    let env = detect_environment(&Found(kit.clone()), &FfmpegEncoderTester, &platform);
    assert_eq!(env.encoder, Some(Encoder::X264), "startup detection must fall back to libx264");

    // A clip that hits a failing hardware encoder mid-batch is redone with libx264.
    let work = tempfile::tempdir().unwrap();
    let input = work.path().join("clip.mp4");
    gameplay_clip(&kit, &input, 30, 1, "aac");
    let out_dir = work.path().join("out");
    let run = run_batch(&kit, &platform, unavailable, &out_dir, &[input], |_| {}, &CancelToken::new());

    assert_eq!(run.summary.saved, 1, "{:?}", run.events);
    let encoders: Vec<Encoder> = run
        .events
        .iter()
        .filter_map(|e| match e {
            BatchEvent::FileStarted { encoder, .. } => Some(*encoder),
            _ => None,
        })
        .collect();
    assert_eq!(encoders, vec![unavailable, Encoder::X264]);
    assert_upscaled(&kit, &out_dir.join("clip_4K.mp4"), "30/1", "aac");
    assert!(leftovers(&out_dir).is_empty(), "{:?}", leftovers(&out_dir));
}

#[test]
fn an_output_folder_that_cannot_be_written_is_reported_and_nothing_is_encoded() {
    let kit = toolkit();
    let work = tempfile::tempdir().unwrap();
    let input = work.path().join("clip.mp4");
    gameplay_clip(&kit, &input, 30, 1, "aac");
    // A file sits where the output folder should be created.
    let blocker = work.path().join("not-a-folder");
    fs::write(&blocker, b"x").unwrap();
    let out_dir = blocker.join("4K");

    let platform = DesktopPlatform::new(Some(kit.ffmpeg.clone()));
    let run = run_batch(&kit, &platform, Encoder::X264, &out_dir, &[input], |_| {}, &CancelToken::new());

    let message = match &run.events[0] {
        BatchEvent::FolderFailed { message } => message.clone(),
        other => panic!("expected FolderFailed, got {other:?}"),
    };
    assert!(message.text.contains("Can't write to the output folder"), "{}", message.text);
    assert!(!message.text.to_lowercase().contains("corrupt"));
    if cfg!(windows) {
        assert!(message.hint.unwrap_or_default().contains("Controlled Folder Access"));
    }
    assert_eq!(run.summary.saved, 0);
}

#[cfg(unix)]
#[test]
fn a_read_only_output_folder_is_reported_and_nothing_is_encoded() {
    use std::os::unix::fs::PermissionsExt;
    let kit = toolkit();
    let work = tempfile::tempdir().unwrap();
    let input = work.path().join("clip.mp4");
    gameplay_clip(&kit, &input, 30, 1, "aac");
    let out_dir = work.path().join("locked");
    fs::create_dir(&out_dir).unwrap();
    fs::set_permissions(&out_dir, fs::Permissions::from_mode(0o555)).unwrap();

    let platform = DesktopPlatform::new(Some(kit.ffmpeg.clone()));
    let run = run_batch(&kit, &platform, Encoder::X264, &out_dir, &[input], |_| {}, &CancelToken::new());
    fs::set_permissions(&out_dir, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(matches!(run.events[0], BatchEvent::FolderFailed { .. }), "{:?}", run.events);
    assert_eq!(fs::read_dir(&out_dir).unwrap().count(), 0);
}

#[test]
fn cancel_stops_ffmpeg_quickly_and_leaves_nothing_behind() {
    let kit = toolkit();
    let work = tempfile::tempdir().unwrap();
    let first = work.path().join("long.mp4");
    let second = work.path().join("next.mp4");
    gameplay_clip(&kit, &first, 60, 10, "aac");
    gameplay_clip(&kit, &second, 30, 1, "aac");
    let out_dir = work.path().join("out");

    let token = CancelToken::new();
    let mut cancelled_at = None;
    let platform = DesktopPlatform::new(Some(kit.ffmpeg.clone()));
    let run = run_batch(
        &kit,
        &platform,
        Encoder::X264,
        &out_dir,
        &[first, second],
        |event| {
            if matches!(event, BatchEvent::FileProgress { .. }) && cancelled_at.is_none() {
                token.cancel();
                cancelled_at = Some(Instant::now());
            }
        },
        &token,
    );

    let waited = cancelled_at.expect("progress was reported before cancel").elapsed();
    assert!(waited < Duration::from_secs(5), "cancel took {waited:?}");
    assert_eq!(run.summary, BatchSummary { saved: 0, failed: 0, skipped: 0, cancelled: true });
    assert!(run.events.contains(&BatchEvent::FileCancelled { index: 0 }));
    assert!(run.events.contains(&BatchEvent::FileCancelled { index: 1 }));
    let files: Vec<_> = fs::read_dir(&out_dir).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(files.is_empty(), "left behind: {files:?}");
}

#[test]
fn a_clip_in_another_container_renamed_to_mp4_is_refused_before_encoding() {
    // An MPEG-TS stream renamed to .mp4 would be decoded through content probing;
    // the app only lets ffprobe and ffmpeg use the MP4/MOV and Matroska demuxers.
    let kit = toolkit();
    let work = tempfile::tempdir().unwrap();
    let disguised = work.path().join("renamed.mp4");
    generate(
        &kit,
        &disguised,
        &["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30", "-t", "1", "-c:v", "libx264", "-f", "mpegts"],
    );

    let probe = FfprobeMediaProbe::new(kit.ffprobe.clone());
    let got = upscale4k_application::ports::MediaProbe::probe(&probe, &disguised);
    assert!(
        matches!(got, Err(upscale4k_application::error::UpscaleError::InputUnreadable { .. })),
        "expected the disguised clip to be refused, got {got:?}"
    );
}

/// What happens to ffmpeg when the app's process is ended from outside
/// (End task in Task Manager, or a crash), so its own cancel never runs.
#[cfg(windows)]
mod abrupt_exit {
    use super::*;
    use std::process::Stdio;
    use std::thread;
    use upscale4k_infrastructure::process::{hidden_command, run_with_timeout};

    const MODE: &str = "UPSCALE4K_STAND_IN";
    const DIR: &str = "UPSCALE4K_STAND_IN_DIR";

    /// Stands in for the app in the tests below: they start it as a separate
    /// process and end it while ffmpeg runs. Without the variables it does nothing.
    #[test]
    fn app_stand_in() {
        let (Ok(mode), Ok(dir)) = (std::env::var(MODE), std::env::var(DIR)) else {
            return;
        };
        let kit = toolkit();
        let dir = PathBuf::from(dir);
        if mode == "encode" {
            let platform = DesktopPlatform::new(Some(kit.ffmpeg.clone()));
            // libx264 at 4K is slow enough that the encode is still running when the app is ended.
            let input = dir.join("long.mp4");
            run_batch(&kit, &platform, Encoder::X264, &dir.join("out"), &[input], |_| {}, &CancelToken::new());
        } else {
            // A short helper run (ffprobe, the encoder test) that hangs, for example on a slow drive.
            let mut command = hidden_command(&kit.ffmpeg);
            command
                .args(["-hide_banner", "-loglevel", "error", "-y", "-re", "-f", "lavfi", "-i"])
                .args(["testsrc2=size=320x240:rate=30", "-t", "120", "-f", "nut"])
                .arg(dir.join("helper.nut"));
            let _ = run_with_timeout(command, Duration::from_secs(120));
        }
    }

    /// Start the stand-in, wait until ffmpeg writes `busy_file`, end the
    /// stand-in the way Task Manager does, and tell whether `busy_file` was
    /// released. ffmpeg keeps the file open while it runs, and Windows
    /// refuses to delete an open file.
    fn released_after_abrupt_exit(mode: &str, dir: &Path, busy_file: &Path) -> bool {
        let mut app = Command::new(std::env::current_exe().unwrap())
            .args(["abrupt_exit::app_stand_in", "--exact", "--test-threads=1"])
            .env(MODE, mode)
            .env(DIR, dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while fs::metadata(busy_file).map(|m| m.len()).unwrap_or(0) == 0 {
            assert!(started.elapsed() < Duration::from_secs(60), "ffmpeg never started writing {busy_file:?}");
            thread::sleep(Duration::from_millis(100));
        }
        app.kill().unwrap();
        app.wait().unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if fs::remove_file(busy_file).is_ok() {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    #[test]
    fn ffmpeg_stops_when_the_app_is_ended_in_the_middle_of_an_encode() {
        let kit = toolkit();
        let work = tempfile::tempdir().unwrap();
        gameplay_clip(&kit, &work.path().join("long.mp4"), 30, 20, "aac");
        let part = work.path().join("out").join("long_4K.mp4.part");
        assert!(
            released_after_abrupt_exit("encode", work.path(), &part),
            "ffmpeg kept encoding after the app was gone"
        );
    }

    #[test]
    fn a_helper_run_stops_when_the_app_is_ended_while_waiting_for_it() {
        let work = tempfile::tempdir().unwrap();
        let file = work.path().join("helper.nut");
        assert!(
            released_after_abrupt_exit("helper", work.path(), &file),
            "the helper kept running after the app was gone"
        );
    }
}

#[test]
fn missing_and_unreadable_inputs_are_described_plainly() {
    let kit = toolkit();
    let work = tempfile::tempdir().unwrap();
    let missing = work.path().join("moved-away.mp4");
    let not_video = work.path().join("notes.mp4");
    fs::write(&not_video, b"this is a text file with a video extension").unwrap();
    let out_dir = work.path().join("out");

    let platform = DesktopPlatform::new(Some(kit.ffmpeg.clone()));
    let run = run_batch(&kit, &platform, Encoder::X264, &out_dir, &[missing, not_video], |_| {}, &CancelToken::new());

    let messages: Vec<String> = run
        .events
        .iter()
        .filter_map(|e| match e {
            BatchEvent::FileFailed { message, .. } => Some(message.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(messages.len(), 2, "{:?}", run.events);
    assert!(messages[0].contains("moved-away.mp4 was not found"), "{}", messages[0]);
    assert!(messages[1].contains("could not read notes.mp4"), "{}", messages[1]);
    for message in &messages {
        assert!(!message.to_lowercase().contains("corrupt"), "{message}");
    }
}
