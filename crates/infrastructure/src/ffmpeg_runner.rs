//! Runs one ffmpeg encode, reports progress, and stops it on cancel.

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use upscale4k_application::cancel::CancelToken;
use upscale4k_application::error::UpscaleError;
use upscale4k_application::ports::{TranscodeProgress, Transcoder};
use upscale4k_domain::encoder::Encoder;

use crate::ffmpeg_errors::classify_failure;
use crate::process::{hidden_command, read_tail};
use crate::progress_parser::ProgressParser;

/// Keep at most this much of ffmpeg's error output.
const STDERR_LIMIT: usize = 64 * 1024;
const POLL: Duration = Duration::from_millis(100);

pub struct FfmpegTranscoder {
    ffmpeg: PathBuf,
}

impl FfmpegTranscoder {
    pub fn new(ffmpeg: PathBuf) -> Self {
        Self { ffmpeg }
    }
}

impl Transcoder for FfmpegTranscoder {
    fn transcode(
        &self,
        args: &[OsString],
        encoder: Encoder,
        on_progress: &mut dyn FnMut(TranscodeProgress),
        cancel: &CancelToken,
    ) -> Result<(), UpscaleError> {
        if cancel.is_cancelled() {
            return Err(UpscaleError::Cancelled);
        }
        let start_error = |detail: String| UpscaleError::EncoderFailed { encoder, detail };

        let mut command = hidden_command(&self.ffmpeg);
        command.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| start_error(format!("ffmpeg couldn't be started: {e}")))?;
        let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
            stop(&mut child);
            return Err(start_error("ffmpeg's output couldn't be read.".into()));
        };

        // Both pipes are drained on their own threads so ffmpeg never blocks on a full pipe.
        // Only the tail is kept while draining, so a clip that makes ffmpeg print
        // errors for hours cannot grow memory without limit.
        let stderr_reader =
            thread::spawn(move || String::from_utf8_lossy(&read_tail(&mut stderr, STDERR_LIMIT)).into_owned());
        let (lines_tx, lines_rx) = mpsc::channel::<String>();
        let stdout_reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines_tx.send(line).is_err() {
                    break;
                }
            }
        });

        let mut parser = ProgressParser::new();
        let mut killed = false;
        loop {
            match lines_rx.recv_timeout(POLL) {
                Ok(line) => {
                    if let Some(progress) = parser.feed(&line) {
                        on_progress(progress);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if cancel.is_cancelled() && !killed {
                let _ = child.kill();
                killed = true;
            }
        }
        let status = wait(&mut child, cancel, &mut killed);
        let _ = stdout_reader.join();
        let stderr_text = stderr_reader.join().unwrap_or_default();

        if cancel.is_cancelled() {
            return Err(UpscaleError::Cancelled);
        }
        match status {
            Some(status) if status.success() => Ok(()),
            Some(status) => {
                let input = args.iter().position(|a| a == "-i").and_then(|i| args.get(i + 1));
                let input = PathBuf::from(input.cloned().unwrap_or_default());
                let output = PathBuf::from(args.last().cloned().unwrap_or_default());
                Err(classify_failure(&stderr_text, status.code(), encoder, &input, &output))
            }
            None => Err(start_error("ffmpeg stopped unexpectedly.".into())),
        }
    }
}

/// Wait for ffmpeg to exit, still honouring cancel while it shuts down.
fn wait(child: &mut Child, cancel: &CancelToken, killed: &mut bool) -> Option<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if cancel.is_cancelled() && !*killed {
                    let _ = child.kill();
                    *killed = true;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                stop(child);
                return None;
            }
        }
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
