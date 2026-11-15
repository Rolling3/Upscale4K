//! Upscale a list of clips, one after another.

use std::path::{Path, PathBuf};

use upscale4k_domain::audio::{AudioPlan, plan_audio};
use upscale4k_domain::encoder::{EncodeJob, Encoder, FallbackReason, encoder_for, ffmpeg_args};
use upscale4k_domain::media::{VideoInfo, upscaled_size};
use upscale4k_domain::naming::{ConflictChoice, OutputDecision, OutputPlanner, temp_path};
use upscale4k_domain::progress::progress_update;

use crate::cancel::CancelToken;
use crate::error::{UpscaleError, UserMessage};
use crate::ports::{FileSystem, MediaProbe, Platform, TranscodeProgress, Transcoder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchItem {
    pub input: PathBuf,
    /// Used only when the output name already exists on disk.
    pub on_conflict: ConflictChoice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchRequest {
    pub output_dir: PathBuf,
    /// The encoder found at startup; a clip may still fall back to libx264.
    pub preferred_encoder: Encoder,
    pub items: Vec<BatchItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchSummary {
    pub saved: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cancelled: bool,
}

/// What the interface hears while a batch runs. `index` is the position in
/// `BatchRequest::items`.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchEvent {
    FileStarted {
        index: usize,
        output: PathBuf,
        encoder: Encoder,
        notes: Vec<String>,
    },
    FileProgress {
        index: usize,
        fraction: Option<f64>,
        eta_secs: Option<f64>,
        speed: Option<f64>,
    },
    FileDone {
        index: usize,
        output: PathBuf,
        note: Option<String>,
    },
    FileSkipped {
        index: usize,
        existing: PathBuf,
    },
    FileFailed {
        index: usize,
        message: UserMessage,
    },
    FileCancelled {
        index: usize,
    },
    /// The output folder can't be used; nothing was processed.
    FolderFailed {
        message: UserMessage,
    },
    Finished(BatchSummary),
}

pub struct Upscaler<'a> {
    pub probe: &'a dyn MediaProbe,
    pub transcoder: &'a dyn Transcoder,
    pub fs: &'a dyn FileSystem,
    pub platform: &'a dyn Platform,
}

enum Outcome {
    Saved,
    Skipped,
    Failed,
    Cancelled,
}

impl Upscaler<'_> {
    /// Process every item in order and report through `emit`. The last event
    /// is always `Finished`.
    pub fn run(&self, request: &BatchRequest, cancel: &CancelToken, emit: &mut dyn FnMut(BatchEvent)) -> BatchSummary {
        let mut summary = BatchSummary::default();

        if let Err(error) = self.fs.ensure_writable_dir(&request.output_dir) {
            emit(BatchEvent::FolderFailed { message: error.to_user_message(self.platform) });
            summary.failed = request.items.len();
            emit(BatchEvent::Finished(summary));
            return summary;
        }

        let inputs: Vec<PathBuf> = request.items.iter().map(|item| item.input.clone()).collect();
        let mut planner = OutputPlanner::new(&request.output_dir, &inputs);

        for (index, item) in request.items.iter().enumerate() {
            if cancel.is_cancelled() {
                emit(BatchEvent::FileCancelled { index });
                summary.cancelled = true;
                continue;
            }
            match self.process(index, item, request.preferred_encoder, &mut planner, cancel, emit) {
                Outcome::Saved => summary.saved += 1,
                Outcome::Skipped => summary.skipped += 1,
                Outcome::Failed => summary.failed += 1,
                Outcome::Cancelled => summary.cancelled = true,
            }
        }

        emit(BatchEvent::Finished(summary));
        summary
    }

    fn process(
        &self,
        index: usize,
        item: &BatchItem,
        preferred: Encoder,
        planner: &mut OutputPlanner,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(BatchEvent),
    ) -> Outcome {
        // Check the input before reserving an output name, so a clip that
        // can't be read doesn't push the next one to a numbered name.
        if !self.fs.exists(&item.input) {
            return self.failed(index, UpscaleError::InputNotFound { path: item.input.clone() }, emit);
        }
        let info = match self.probe.probe(&item.input) {
            Ok(info) => info,
            Err(error) => return self.failed(index, error, emit),
        };

        let (output, replace) = match planner.decide(&item.input, item.on_conflict, &|p| self.fs.exists(p)) {
            OutputDecision::Skip { existing } => {
                emit(BatchEvent::FileSkipped { index, existing });
                return Outcome::Skipped;
            }
            OutputDecision::New(path) => (path, false),
            OutputDecision::Replace(path) => (path, true),
        };

        let choice = encoder_for(preferred, upscaled_size(info.size));
        let audio = plan_audio(&info.audio_codecs);
        let mut notes = Vec::new();
        if let Some(reason) = &choice.fallback {
            notes.push(describe_fallback(reason));
        }
        if let AudioPlan::ConvertToAac { codec } = &audio {
            notes.push(format!("The {codec} audio can't be copied into MP4, so it is converted to AAC."));
        }

        let job = Job { index, input: &item.input, output: &output, audio: &audio, info: &info };
        let mut result = self.encode(&job, choice.encoder, notes.clone(), cancel, emit);
        if let Err(UpscaleError::EncoderFailed { encoder, .. }) = &result
            && encoder.is_hardware()
            && !cancel.is_cancelled()
        {
            notes.push(format!(
                "{} stopped with an error, so this clip is encoded again with libx264 (CPU).",
                encoder.label()
            ));
            result = self.encode(&job, Encoder::X264, notes, cancel, emit);
        }

        match result {
            Ok(temp) => self.finish(&job, &temp, replace, planner, emit),
            Err(UpscaleError::Cancelled) => {
                emit(BatchEvent::FileCancelled { index });
                Outcome::Cancelled
            }
            Err(error) => self.failed(index, error, emit),
        }
    }

    /// Run one encode into a fresh temp file. On any error the temp file is removed.
    fn encode(
        &self,
        job: &Job<'_>,
        encoder: Encoder,
        notes: Vec<String>,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(BatchEvent),
    ) -> Result<PathBuf, UpscaleError> {
        let temp = temp_path(job.output, &|p| self.fs.exists(p));
        let args = ffmpeg_args(&EncodeJob {
            input: job.input,
            temp_output: &temp,
            encoder,
            audio: job.audio,
            frame_rate: job.info.frame_rate,
        });
        emit(BatchEvent::FileStarted { index: job.index, output: job.output.to_path_buf(), encoder, notes });

        let duration = job.info.duration_secs;
        let index = job.index;
        // ffmpeg's position can dip while it flushes the last frames; the bar only moves forward.
        let mut furthest = 0.0_f64;
        let mut report = |p: TranscodeProgress| {
            let update = progress_update(p.out_time_us, duration, p.speed);
            let fraction = update.fraction.map(|f| {
                furthest = furthest.max(f);
                furthest
            });
            emit(BatchEvent::FileProgress { index, fraction, eta_secs: update.eta_secs, speed: update.speed });
        };
        match self.transcoder.transcode(&args, encoder, &mut report, cancel) {
            Ok(()) => Ok(temp),
            Err(error) => {
                self.fs.remove_file(&temp);
                Err(error)
            }
        }
    }

    /// Move the finished temp file to its final name without overwriting
    /// anything the user didn't agree to replace.
    fn finish(
        &self,
        job: &Job<'_>,
        temp: &Path,
        replace: bool,
        planner: &mut OutputPlanner,
        emit: &mut dyn FnMut(BatchEvent),
    ) -> Outcome {
        let exists = |p: &Path| self.fs.exists(p);
        let mut note = None;
        let target = if replace {
            match self.fs.move_file(temp, job.output, true) {
                Ok(()) => return self.done(job.index, job.output.to_path_buf(), None, emit),
                Err(_) => {
                    let numbered = planner.next_free(job.input, &exists);
                    note = Some(format!(
                        "Couldn't replace {} (it may be open in another app), so the new file was saved as {}.",
                        file_name(job.output),
                        file_name(&numbered)
                    ));
                    numbered
                }
            }
        } else if exists(job.output) {
            planner.next_free(job.input, &exists)
        } else {
            job.output.to_path_buf()
        };

        match self.fs.move_file(temp, &target, false) {
            Ok(()) => self.done(job.index, target, note, emit),
            Err(error) => {
                self.fs.remove_file(temp);
                self.failed(job.index, error, emit)
            }
        }
    }

    fn done(&self, index: usize, output: PathBuf, note: Option<String>, emit: &mut dyn FnMut(BatchEvent)) -> Outcome {
        emit(BatchEvent::FileDone { index, output, note });
        Outcome::Saved
    }

    fn failed(&self, index: usize, error: UpscaleError, emit: &mut dyn FnMut(BatchEvent)) -> Outcome {
        emit(BatchEvent::FileFailed { index, message: error.to_user_message(self.platform) });
        Outcome::Failed
    }
}

/// The parts of one clip's work that stay the same across a retry.
struct Job<'a> {
    index: usize,
    input: &'a Path,
    output: &'a Path,
    audio: &'a AudioPlan,
    info: &'a VideoInfo,
}

fn describe_fallback(reason: &FallbackReason) -> String {
    match reason {
        FallbackReason::FrameTooLarge { size, max_side } => format!(
            "After upscaling this clip is {}×{}, larger than the hardware encoder allows ({max_side} px), \
             so it uses libx264 (CPU). This takes longer.",
            size.width, size.height
        ),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::UpscaleError;
    use crate::fakes::{FakeFs, FakePlatform, FakeProbe, FakeTranscoder, Step, standard_clip};
    use crate::ports::TranscodeProgress;
    use upscale4k_domain::media::FrameSize;

    fn out() -> PathBuf {
        PathBuf::from("out")
    }

    fn clip(name: &str) -> PathBuf {
        PathBuf::from("clips").join(name)
    }

    fn item(name: &str, on_conflict: ConflictChoice) -> BatchItem {
        BatchItem { input: clip(name), on_conflict }
    }

    fn request(encoder: Encoder, items: Vec<BatchItem>) -> BatchRequest {
        BatchRequest { output_dir: out(), preferred_encoder: encoder, items }
    }

    struct Run {
        events: Vec<BatchEvent>,
        summary: BatchSummary,
    }

    fn run_with(probe: &FakeProbe, transcoder: &FakeTranscoder, fs: &FakeFs, req: &BatchRequest) -> Run {
        run_with_token(probe, transcoder, fs, req, &CancelToken::new())
    }

    fn run_with_token(
        probe: &FakeProbe,
        transcoder: &FakeTranscoder,
        fs: &FakeFs,
        req: &BatchRequest,
        cancel: &CancelToken,
    ) -> Run {
        let platform = FakePlatform::windows_like();
        let upscaler = Upscaler { probe, transcoder, fs, platform: &platform };
        let mut events = Vec::new();
        let summary = upscaler.run(req, cancel, &mut |e| events.push(e));
        Run { events, summary }
    }

    fn started(events: &[BatchEvent]) -> Vec<(usize, PathBuf, Encoder, Vec<String>)> {
        events
            .iter()
            .filter_map(|e| match e {
                BatchEvent::FileStarted { index, output, encoder, notes } => {
                    Some((*index, output.clone(), *encoder, notes.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn with_inputs(names: &[&str]) -> FakeFs {
        FakeFs::with_files(&names.iter().map(|n| clip(n)).collect::<Vec<_>>())
    }

    #[test]
    fn each_clip_is_encoded_to_a_temp_file_and_moved_to_its_4k_name() {
        let fs = with_inputs(&["a.mp4", "b.mkv"]);
        let tx = FakeTranscoder::new(&fs, vec![]);
        let req = request(
            Encoder::Nvenc,
            vec![item("a.mp4", ConflictChoice::KeepBoth), item("b.mkv", ConflictChoice::KeepBoth)],
        );

        let run = run_with(&FakeProbe::default(), &tx, &fs, &req);

        assert_eq!(tx.call_count(), 2);
        assert_eq!(tx.args_of(0).last().map(String::as_str), Some(out().join("a_4K.mp4.part").to_str().unwrap()));
        assert_eq!(
            *fs.moves.borrow(),
            vec![
                (out().join("a_4K.mp4.part"), out().join("a_4K.mp4"), false),
                (out().join("b_4K.mp4.part"), out().join("b_4K.mp4"), false),
            ]
        );
        assert!(run.events.contains(&BatchEvent::FileDone { index: 0, output: out().join("a_4K.mp4"), note: None }));
        assert!(run.events.contains(&BatchEvent::FileDone { index: 1, output: out().join("b_4K.mp4"), note: None }));
        assert_eq!(run.summary, BatchSummary { saved: 2, failed: 0, skipped: 0, cancelled: false });
        assert_eq!(run.events.last(), Some(&BatchEvent::Finished(run.summary)));
    }

    #[test]
    fn progress_is_reported_as_a_fraction_of_the_clip_length() {
        let fs = with_inputs(&["a.mp4"]);
        let samples = vec![TranscodeProgress { out_time_us: Some(2_500_000), speed: Some(2.0) }];
        let tx = FakeTranscoder::new(&fs, vec![Step::Succeed(samples)]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::KeepBoth)]),
        );

        assert!(run.events.contains(&BatchEvent::FileProgress {
            index: 0,
            fraction: Some(0.5),
            eta_secs: Some(1.25),
            speed: Some(2.0)
        }));
    }

    #[test]
    fn progress_never_moves_backwards_even_when_ffmpeg_reports_an_earlier_time() {
        // Seen with NVENC on a real clip: ffmpeg's position dips while it flushes the last frames.
        let fs = with_inputs(&["a.mp4"]);
        let at = |us: i64| TranscodeProgress { out_time_us: Some(us), speed: Some(1.0) };
        let tx = FakeTranscoder::new(&fs, vec![Step::Succeed(vec![at(4_500_000), at(4_000_000), at(5_000_000)])]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::KeepBoth)]),
        );

        let fractions: Vec<f64> = run
            .events
            .iter()
            .filter_map(|e| match e {
                BatchEvent::FileProgress { fraction, .. } => *fraction,
                _ => None,
            })
            .collect();
        assert_eq!(fractions, vec![0.9, 0.9, 1.0]);
    }

    #[test]
    fn keep_both_never_touches_the_existing_file() {
        let fs = with_inputs(&["a.mp4"]);
        fs.files.borrow_mut().insert(out().join("a_4K.mp4"));
        let tx = FakeTranscoder::new(&fs, vec![]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::KeepBoth)]),
        );

        assert_eq!(*fs.moves.borrow(), vec![(out().join("a_4K (2).mp4.part"), out().join("a_4K (2).mp4"), false)]);
        assert!(fs.has(&out().join("a_4K.mp4")));
        assert_eq!(run.summary.saved, 1);
    }

    #[test]
    fn replace_moves_over_the_existing_file_only_when_chosen() {
        let fs = with_inputs(&["a.mp4"]);
        fs.files.borrow_mut().insert(out().join("a_4K.mp4"));
        let tx = FakeTranscoder::new(&fs, vec![]);
        run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::Replace)]),
        );

        assert_eq!(*fs.moves.borrow(), vec![(out().join("a_4K.mp4.part"), out().join("a_4K.mp4"), true)]);
    }

    #[test]
    fn skip_leaves_the_existing_file_and_runs_no_encode() {
        let fs = with_inputs(&["a.mp4"]);
        fs.files.borrow_mut().insert(out().join("a_4K.mp4"));
        let tx = FakeTranscoder::new(&fs, vec![]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::Skip)]),
        );

        assert_eq!(tx.call_count(), 0);
        assert!(run.events.contains(&BatchEvent::FileSkipped { index: 0, existing: out().join("a_4K.mp4") }));
        assert_eq!(run.summary, BatchSummary { saved: 0, failed: 0, skipped: 1, cancelled: false });
    }

    #[test]
    fn a_missing_input_fails_alone_and_the_batch_goes_on() {
        let fs = with_inputs(&["b.mp4"]);
        let tx = FakeTranscoder::new(&fs, vec![]);
        let req = request(
            Encoder::Nvenc,
            vec![item("gone.mp4", ConflictChoice::KeepBoth), item("b.mp4", ConflictChoice::KeepBoth)],
        );
        let run = run_with(&FakeProbe::default(), &tx, &fs, &req);

        let failure = run.events.iter().find_map(|e| match e {
            BatchEvent::FileFailed { index: 0, message } => Some(message.clone()),
            _ => None,
        });
        let failure = failure.expect("first file must fail");
        assert!(failure.text.contains("not found"), "{}", failure.text);
        assert_eq!(tx.call_count(), 1);
        assert_eq!(run.summary, BatchSummary { saved: 1, failed: 1, skipped: 0, cancelled: false });
    }

    #[test]
    fn an_unwritable_output_folder_stops_before_any_encode() {
        let fs = FakeFs::read_only();
        fs.files.borrow_mut().insert(clip("a.mp4"));
        let tx = FakeTranscoder::new(&fs, vec![]);
        let req = request(
            Encoder::Nvenc,
            vec![item("a.mp4", ConflictChoice::KeepBoth), item("b.mp4", ConflictChoice::KeepBoth)],
        );
        let run = run_with(&FakeProbe::default(), &tx, &fs, &req);

        assert_eq!(tx.call_count(), 0);
        match &run.events[0] {
            BatchEvent::FolderFailed { message } => {
                assert!(message.text.contains("Can't write"), "{}", message.text);
                assert_eq!(message.hint.as_deref(), Some(FakePlatform::BLOCKED_HINT));
            }
            other => panic!("expected FolderFailed, got {other:?}"),
        }
        assert_eq!(run.summary, BatchSummary { saved: 0, failed: 2, skipped: 0, cancelled: false });
    }

    #[test]
    fn a_failed_encode_removes_its_temp_file_and_the_next_clip_still_runs() {
        let fs = with_inputs(&["a.mp4", "b.mp4"]);
        let unreadable = UpscaleError::InputUnreadable { path: clip("a.mp4"), detail: "Invalid data".into() };
        let tx = FakeTranscoder::new(&fs, vec![Step::Fail(unreadable)]);
        let req = request(
            Encoder::X264,
            vec![item("a.mp4", ConflictChoice::KeepBoth), item("b.mp4", ConflictChoice::KeepBoth)],
        );
        let run = run_with(&FakeProbe::default(), &tx, &fs, &req);

        assert!(!fs.has(&out().join("a_4K.mp4.part")));
        assert!(fs.removed.borrow().contains(&out().join("a_4K.mp4.part")));
        assert!(fs.has(&out().join("b_4K.mp4")));
        assert_eq!(run.summary, BatchSummary { saved: 1, failed: 1, skipped: 0, cancelled: false });
    }

    #[test]
    fn cancel_stops_the_current_clip_removes_its_temp_file_and_skips_the_rest() {
        let fs = with_inputs(&["a.mp4", "b.mp4"]);
        let tx = FakeTranscoder::new(&fs, vec![Step::CancelHalfway]);
        let req = request(
            Encoder::Nvenc,
            vec![item("a.mp4", ConflictChoice::KeepBoth), item("b.mp4", ConflictChoice::KeepBoth)],
        );
        let run = run_with(&FakeProbe::default(), &tx, &fs, &req);

        assert_eq!(tx.call_count(), 1);
        assert!(!fs.has(&out().join("a_4K.mp4.part")));
        assert!(!fs.has(&out().join("a_4K.mp4")));
        assert!(run.events.contains(&BatchEvent::FileCancelled { index: 0 }));
        assert!(run.events.contains(&BatchEvent::FileCancelled { index: 1 }));
        assert_eq!(run.summary, BatchSummary { saved: 0, failed: 0, skipped: 0, cancelled: true });
    }

    #[test]
    fn a_token_cancelled_before_start_runs_nothing() {
        let fs = with_inputs(&["a.mp4"]);
        let tx = FakeTranscoder::new(&fs, vec![]);
        let token = CancelToken::new();
        token.cancel();
        let req = request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::KeepBoth)]);
        let run = run_with_token(&FakeProbe::default(), &tx, &fs, &req, &token);

        assert_eq!(tx.call_count(), 0);
        assert!(run.summary.cancelled);
    }

    #[test]
    fn a_gpu_failure_retries_the_clip_once_with_libx264() {
        let fs = with_inputs(&["a.mp4"]);
        let gpu_error = UpscaleError::EncoderFailed { encoder: Encoder::Nvenc, detail: "out of memory".into() };
        let tx = FakeTranscoder::new(&fs, vec![Step::Fail(gpu_error)]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::KeepBoth)]),
        );

        assert_eq!(tx.call_count(), 2);
        assert_eq!(tx.encoder_of(0), Encoder::Nvenc);
        assert_eq!(tx.encoder_of(1), Encoder::X264);
        assert!(tx.args_of(1).contains(&"libx264".to_string()));
        let starts = started(&run.events);
        assert_eq!(starts.len(), 2);
        assert!(starts[1].3.iter().any(|n| n.contains("libx264")), "{:?}", starts[1].3);
        assert!(fs.has(&out().join("a_4K.mp4")));
        assert_eq!(run.summary.saved, 1);
    }

    #[test]
    fn a_libx264_failure_is_not_retried() {
        let fs = with_inputs(&["a.mp4"]);
        let error = UpscaleError::EncoderFailed { encoder: Encoder::X264, detail: "x264 error".into() };
        let tx = FakeTranscoder::new(&fs, vec![Step::Fail(error)]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::X264, vec![item("a.mp4", ConflictChoice::KeepBoth)]),
        );

        assert_eq!(tx.call_count(), 1);
        assert_eq!(run.summary.failed, 1);
    }

    #[test]
    fn an_ultrawide_clip_uses_libx264_and_says_why() {
        let fs = with_inputs(&["wide.mp4"]);
        let mut info = standard_clip();
        info.size = FrameSize { width: 2560, height: 1080 };
        let probe = FakeProbe::with(&clip("wide.mp4"), Ok(info));
        let tx = FakeTranscoder::new(&fs, vec![]);
        let run =
            run_with(&probe, &tx, &fs, &request(Encoder::Nvenc, vec![item("wide.mp4", ConflictChoice::KeepBoth)]));

        assert_eq!(tx.encoder_of(0), Encoder::X264);
        let starts = started(&run.events);
        assert!(starts[0].3.iter().any(|n| n.contains("5120")), "{:?}", starts[0].3);
    }

    #[test]
    fn audio_that_mp4_cannot_hold_is_converted_with_a_note() {
        let fs = with_inputs(&["cam.mov"]);
        let mut info = standard_clip();
        info.audio_codecs = vec!["pcm_s16le".into()];
        let probe = FakeProbe::with(&clip("cam.mov"), Ok(info));
        let tx = FakeTranscoder::new(&fs, vec![]);
        let run = run_with(&probe, &tx, &fs, &request(Encoder::Nvenc, vec![item("cam.mov", ConflictChoice::KeepBoth)]));

        let args = tx.args_of(0);
        assert!(args.windows(2).any(|w| w[0] == "-c:a" && w[1] == "aac"), "{args:?}");
        let starts = started(&run.events);
        assert!(starts[0].3.iter().any(|n| n.contains("pcm_s16le")), "{:?}", starts[0].3);
    }

    #[test]
    fn an_unreadable_input_is_reported_without_encoding() {
        let fs = with_inputs(&["bad.mp4"]);
        let probe = FakeProbe::with(
            &clip("bad.mp4"),
            Err(UpscaleError::InputUnreadable { path: clip("bad.mp4"), detail: "Invalid data".into() }),
        );
        let tx = FakeTranscoder::new(&fs, vec![]);
        let run = run_with(&probe, &tx, &fs, &request(Encoder::Nvenc, vec![item("bad.mp4", ConflictChoice::KeepBoth)]));

        assert_eq!(tx.call_count(), 0);
        assert_eq!(run.summary.failed, 1);
    }

    #[test]
    fn a_name_taken_during_the_encode_gets_the_next_number_instead_of_being_overwritten() {
        let fs = with_inputs(&["a.mp4"]);
        let tx = FakeTranscoder::new(&fs, vec![Step::SucceedWhileCreating(out().join("a_4K.mp4"))]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::KeepBoth)]),
        );

        assert_eq!(*fs.moves.borrow(), vec![(out().join("a_4K.mp4.part"), out().join("a_4K (2).mp4"), false)]);
        assert!(run.events.contains(&BatchEvent::FileDone {
            index: 0,
            output: out().join("a_4K (2).mp4"),
            note: None
        }));
    }

    #[test]
    fn a_file_that_cannot_be_replaced_is_kept_and_the_new_one_saved_under_a_number() {
        let fs = with_inputs(&["a.mp4"]);
        fs.files.borrow_mut().insert(out().join("a_4K.mp4"));
        fs.busy.borrow_mut().insert(out().join("a_4K.mp4"));
        let tx = FakeTranscoder::new(&fs, vec![]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::Replace)]),
        );

        let done = run.events.iter().find_map(|e| match e {
            BatchEvent::FileDone { output, note, .. } => Some((output.clone(), note.clone())),
            _ => None,
        });
        let (output, note) = done.expect("file must still be saved");
        assert_eq!(output, out().join("a_4K (2).mp4"));
        assert!(note.unwrap_or_default().contains("a_4K.mp4"));
        assert!(fs.has(&out().join("a_4K.mp4")));
    }

    #[test]
    fn a_failed_input_does_not_use_up_the_plain_output_name() {
        let fs = with_inputs(&["gone.mkv"]);
        let tx = FakeTranscoder::new(&fs, vec![]);
        let req = request(
            Encoder::Nvenc,
            vec![item("gone.mp4", ConflictChoice::KeepBoth), item("gone.mkv", ConflictChoice::KeepBoth)],
        );
        let run = run_with(&FakeProbe::default(), &tx, &fs, &req);

        assert!(run.events.contains(&BatchEvent::FileDone { index: 1, output: out().join("gone_4K.mp4"), note: None }));
    }

    #[test]
    fn cancel_pressed_as_the_gpu_fails_does_not_start_the_libx264_retry() {
        let fs = with_inputs(&["a.mp4"]);
        let gpu_error = UpscaleError::EncoderFailed { encoder: Encoder::Nvenc, detail: "lost device".into() };
        let tx = FakeTranscoder::new(&fs, vec![Step::FailAfterCancel(gpu_error)]);
        run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::KeepBoth)]),
        );

        assert_eq!(tx.call_count(), 1);
        assert!(!fs.has(&out().join("a_4K.mp4.part")));
    }

    #[test]
    fn the_clips_frame_rate_reaches_the_encoder_settings() {
        let fs = with_inputs(&["a.mov"]);
        let mut info = standard_clip();
        info.frame_rate = Some(30.0);
        let probe = FakeProbe::with(&clip("a.mov"), Ok(info));
        let tx = FakeTranscoder::new(&fs, vec![]);
        run_with(&probe, &tx, &fs, &request(Encoder::VideoToolbox, vec![item("a.mov", ConflictChoice::KeepBoth)]));

        let args = tx.args_of(0);
        assert!(args.windows(2).any(|w| w[0] == "-b:v" && w[1] == "45M"), "{args:?}");
    }

    #[test]
    fn a_failed_final_move_removes_the_temp_file_and_reports_it() {
        let fs = with_inputs(&["a.mp4"]);
        fs.busy.borrow_mut().insert(out().join("a_4K.mp4"));
        let tx = FakeTranscoder::new(&fs, vec![]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::KeepBoth)]),
        );

        assert!(!fs.has(&out().join("a_4K.mp4.part")));
        assert_eq!(run.summary, BatchSummary { saved: 0, failed: 1, skipped: 0, cancelled: false });
    }

    #[test]
    fn started_event_names_the_final_output_and_encoder() {
        let fs = with_inputs(&["a.mp4"]);
        let tx = FakeTranscoder::new(&fs, vec![]);
        let run = run_with(
            &FakeProbe::default(),
            &tx,
            &fs,
            &request(Encoder::Nvenc, vec![item("a.mp4", ConflictChoice::KeepBoth)]),
        );

        assert_eq!(started(&run.events), vec![(0, out().join("a_4K.mp4"), Encoder::Nvenc, vec![])]);
    }
}
