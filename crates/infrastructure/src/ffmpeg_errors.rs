//! Turns ffmpeg's error output into an `UpscaleError` the user can act on.

use std::path::Path;

use upscale4k_application::error::UpscaleError;
use upscale4k_domain::encoder::Encoder;

/// Classify a failed ffmpeg run. `stderr` is what ffmpeg printed with
/// `-loglevel error`; `input` and `output` are the paths it was given.
pub fn classify_failure(
    stderr: &str,
    exit_code: Option<i32>,
    encoder: Encoder,
    input: &Path,
    output: &Path,
) -> UpscaleError {
    let lines: Vec<&str> = stderr.lines().map(strip_context).filter(|l| !l.is_empty()).collect();
    let lower: Vec<String> = lines.iter().map(|l| l.to_lowercase()).collect();
    let folder = output.parent().map(Path::to_path_buf).unwrap_or_default();
    let find = |test: &dyn Fn(&str) -> bool| lower.iter().position(|l| test(l));

    if find(&|l| l.contains("no space left on device") || l.contains("not enough space on the disk")).is_some() {
        return UpscaleError::NotEnoughSpace { folder };
    }

    let output_line =
        |reason: &str| find(&|l: &str| l.starts_with("error opening output") && l.contains(reason)).is_some();
    if ["permission denied", "access is denied", "operation not permitted", "read-only file system"]
        .iter()
        .any(|reason| output_line(reason))
    {
        return UpscaleError::OutputFolderNotWritable {
            folder,
            detail: "ffmpeg was denied access to the folder.".into(),
        };
    }
    if output_line("no such file or directory") {
        return UpscaleError::OutputFolderNotWritable { folder, detail: "The folder doesn't exist any more.".into() };
    }

    if let Some(i) = find(&|l| l.starts_with("error opening input:")) {
        if lower[i].contains("no such file or directory") {
            return UpscaleError::InputNotFound { path: input.to_path_buf() };
        }
        let reason = lines[i].split_once(": ").map(|(_, r)| r).unwrap_or(lines[i]);
        return UpscaleError::InputUnreadable { path: input.to_path_buf(), detail: reason.trim().to_string() };
    }

    if let Some(i) = find(&|l| l.contains("unknown encoder")) {
        return UpscaleError::EncoderFailed { encoder, detail: lines[i].to_string() };
    }

    // "Error opening output <path>: <reason>" for any other reason the file couldn't be created.
    if let Some(i) = find(&|l| l.starts_with("error opening output ") && !l.starts_with("error opening output file")) {
        let reason = lines[i].rsplit_once(": ").map(|(_, r)| r).unwrap_or(lines[i]).trim();
        return UpscaleError::OutputFolderNotWritable {
            folder,
            detail: format!("ffmpeg couldn't create the file: {reason}"),
        };
    }

    let detail = match find(&|l| !is_generic(l)).or(if lines.is_empty() { None } else { Some(0) }) {
        Some(i) => lines[i].to_string(),
        None => match exit_code {
            Some(code) => format!("ffmpeg exited with code {code}."),
            None => "ffmpeg stopped unexpectedly.".to_string(),
        },
    };
    UpscaleError::EncoderFailed { encoder, detail }
}

/// Drop ffmpeg's `[component @ address]` prefixes.
fn strip_context(line: &str) -> &str {
    let mut rest = line.trim();
    while rest.starts_with('[') {
        match rest.find("] ") {
            Some(end) => rest = rest[end + 2..].trim_start(),
            None => break,
        }
    }
    rest.trim()
}

/// Summary lines that say a failure happened but not why.
fn is_generic(lower_line: &str) -> bool {
    [
        "error opening output file",
        "error opening output files",
        "conversion failed",
        "error while opening encoder",
        "error while filtering",
        "nothing was written into output file",
        "error selecting an encoder",
    ]
    .iter()
    .any(|generic| lower_line.starts_with(generic))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // Paths are built with the platform's separator so the tests mean the same on Windows and macOS.
    fn input() -> PathBuf {
        Path::new("clips").join("boss.mkv")
    }

    fn output_dir() -> PathBuf {
        Path::new("out").join("4K")
    }

    fn output() -> PathBuf {
        output_dir().join("boss_4K.mp4.part")
    }

    fn classify(stderr: &str) -> UpscaleError {
        classify_failure(stderr, Some(1), Encoder::Nvenc, &input(), &output())
    }

    // The stderr texts below were captured from ffmpeg 9.0.2.

    #[test]
    fn a_missing_input_is_reported_as_not_found() {
        let stderr = "[in#0 @ 0000021089183540] Error opening input: No such file or directory\n\
                      Error opening input file C:\\clips\\boss.mkv.\n\
                      Error opening input files: No such file or directory\n";
        assert_eq!(classify(stderr), UpscaleError::InputNotFound { path: input() });
    }

    #[test]
    fn an_input_that_is_not_a_video_is_unreadable_with_ffmpegs_reason() {
        let stderr = "[in#0 @ 000001e0a4523980] moov atom not found\n\
                      [in#0 @ 000001e0a4523440] Error opening input: Invalid data found when processing input\n\
                      Error opening input file C:\\clips\\boss.mkv.\n\
                      Error opening input files: Invalid data found when processing input\n";
        assert_eq!(
            classify(stderr),
            UpscaleError::InputUnreadable { path: input(), detail: "Invalid data found when processing input".into() }
        );
    }

    #[test]
    fn permission_denied_on_the_output_is_an_unwritable_folder() {
        let stderr = "[out#0/mp4 @ 000001e2155b4040] Error opening output C:\\OPFile\\4K\\boss_4K.mp4.part: Permission denied\n\
                      Error opening output file C:\\OPFile\\4K\\boss_4K.mp4.part.\n\
                      Error opening output files: Permission denied\n";
        assert_eq!(
            classify(stderr),
            UpscaleError::OutputFolderNotWritable {
                folder: output_dir(),
                detail: "ffmpeg was denied access to the folder.".into()
            }
        );
    }

    #[test]
    fn a_vanished_output_folder_is_an_unwritable_folder() {
        let stderr = "[out#0/mp4 @ 000001f7d67f30c0] Error opening output C:\\OPFile\\4K\\boss_4K.mp4.part: No such file or directory\n\
                      Error opening output file C:\\OPFile\\4K\\boss_4K.mp4.part.\n\
                      Error opening output files: No such file or directory\n";
        assert_eq!(
            classify(stderr),
            UpscaleError::OutputFolderNotWritable {
                folder: output_dir(),
                detail: "The folder doesn't exist any more.".into()
            }
        );
    }

    #[test]
    fn any_other_reason_for_not_opening_the_output_is_an_unwritable_folder_with_that_reason() {
        let stderr = "[out#0/mp4 @ 0000] Error opening output C:\\OPFile\\4K\\boss_4K.mp4.part: File name too long\n\
                      Error opening output file C:\\OPFile\\4K\\boss_4K.mp4.part.\n\
                      Error opening output files: File name too long\n";
        assert_eq!(
            classify(stderr),
            UpscaleError::OutputFolderNotWritable {
                folder: output_dir(),
                detail: "ffmpeg couldn't create the file: File name too long".into()
            }
        );
    }

    #[test]
    fn a_full_disk_is_reported_as_such() {
        let stderr = "[out#0/mp4 @ 0000] Error writing trailer: No space left on device\n";
        assert_eq!(classify(stderr), UpscaleError::NotEnoughSpace { folder: output_dir() });
    }

    #[test]
    fn an_unknown_encoder_is_an_encoder_failure_naming_it() {
        let stderr = "[vost#0:0 @ 000001481a721c00] Unknown encoder 'h264_videotoolbox'\n\
                      [vost#0:0 @ 000001481a721c00] Error selecting an encoder\n\
                      Error opening output file C:\\OPFile\\4K\\boss_4K.mp4.part.\n\
                      Error opening output files: Encoder not found\n";
        assert_eq!(
            classify_failure(stderr, Some(1), Encoder::VideoToolbox, &input(), &output()),
            UpscaleError::EncoderFailed {
                encoder: Encoder::VideoToolbox,
                detail: "Unknown encoder 'h264_videotoolbox'".into()
            }
        );
    }

    #[test]
    fn other_failures_keep_the_first_meaningful_ffmpeg_line() {
        let stderr = "[h264_nvenc @ 0000] OpenEncodeSessionEx failed: unsupported device (2): (no details)\n\
                      [vost#0:0/h264_nvenc @ 0000] Error while opening encoder - maybe incorrect parameters such as bit_rate, rate, width or height.\n";
        assert_eq!(
            classify(stderr),
            UpscaleError::EncoderFailed {
                encoder: Encoder::Nvenc,
                detail: "OpenEncodeSessionEx failed: unsupported device (2): (no details)".into()
            }
        );
    }

    #[test]
    fn silent_failures_report_the_exit_code() {
        assert_eq!(
            classify_failure("", Some(-22), Encoder::X264, &input(), &output()),
            UpscaleError::EncoderFailed { encoder: Encoder::X264, detail: "ffmpeg exited with code -22.".into() }
        );
        assert_eq!(
            classify_failure("  \n", None, Encoder::X264, &input(), &output()),
            UpscaleError::EncoderFailed { encoder: Encoder::X264, detail: "ffmpeg stopped unexpectedly.".into() }
        );
    }
}
