//! H.264 encoders and the ffmpeg arguments for one upscale.

use std::ffi::OsString;
use std::path::Path;

use crate::audio::{AAC_BITRATE, AudioPlan};
use crate::media::{FrameSize, TARGET_HEIGHT};

/// Largest width or height the hardware H.264 encoders accept.
pub const HARDWARE_MAX_SIDE: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoder {
    /// NVIDIA GPU encoder (Windows).
    Nvenc,
    /// Apple hardware encoder (macOS).
    VideoToolbox,
    /// Software encoder, works everywhere but is slower.
    X264,
}

impl Encoder {
    /// Name ffmpeg uses for `-c:v`.
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Encoder::Nvenc => "h264_nvenc",
            Encoder::VideoToolbox => "h264_videotoolbox",
            Encoder::X264 => "libx264",
        }
    }

    /// Short name for the interface.
    pub fn label(self) -> &'static str {
        match self {
            Encoder::Nvenc => "NVIDIA NVENC",
            Encoder::VideoToolbox => "Apple VideoToolbox",
            Encoder::X264 => "libx264 (CPU)",
        }
    }

    pub fn is_hardware(self) -> bool {
        !matches!(self, Encoder::X264)
    }

    /// Video encoder options. VideoToolbox has no quality mode on every Mac,
    /// so it gets YouTube's recommended 2160p bitrate for the frame rate.
    pub fn video_args(self, frame_rate: Option<f64>) -> Vec<&'static str> {
        match self {
            Encoder::Nvenc => vec!["-c:v", "h264_nvenc", "-preset", "p7", "-rc", "vbr", "-cq", "19", "-b:v", "0"],
            Encoder::VideoToolbox => {
                let standard_rate = frame_rate.is_some_and(|fps| fps > 0.0 && fps <= 31.0);
                let bitrate = if standard_rate { "45M" } else { "68M" };
                vec!["-c:v", "h264_videotoolbox", "-b:v", bitrate, "-profile:v", "high"]
            }
            Encoder::X264 => vec!["-c:v", "libx264", "-preset", "medium", "-crf", "19", "-profile:v", "high"],
        }
    }
}

/// Why a clip doesn't use the preferred encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    /// The upscaled frame is larger than the hardware encoder allows.
    FrameTooLarge { size: FrameSize, max_side: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderChoice {
    pub encoder: Encoder,
    pub fallback: Option<FallbackReason>,
}

/// The encoder for a clip whose upscaled frame is `output`.
pub fn encoder_for(preferred: Encoder, output: FrameSize) -> EncoderChoice {
    let too_large = output.width > HARDWARE_MAX_SIDE || output.height > HARDWARE_MAX_SIDE;
    if preferred.is_hardware() && too_large {
        EncoderChoice {
            encoder: Encoder::X264,
            fallback: Some(FallbackReason::FrameTooLarge { size: output, max_side: HARDWARE_MAX_SIDE }),
        }
    } else {
        EncoderChoice { encoder: preferred, fallback: None }
    }
}

/// One upscale job, ready to turn into ffmpeg arguments.
#[derive(Debug, Clone)]
pub struct EncodeJob<'a> {
    pub input: &'a Path,
    /// ffmpeg writes here; the file is renamed when the encode succeeds.
    pub temp_output: &'a Path,
    pub encoder: Encoder,
    pub audio: &'a AudioPlan,
    pub frame_rate: Option<f64>,
}

/// Options that restrict how ffmpeg and ffprobe open an untrusted clip: only
/// the local file itself, and only the MP4/MOV or Matroska demuxers. Without
/// them, a renamed playlist or other container could make ffmpeg open further
/// files or URLs named inside the clip.
pub const INPUT_RESTRICTIONS: [&str; 4] = ["-protocol_whitelist", "file", "-format_whitelist", "mov,matroska"];

/// Full ffmpeg argument list: scale to 2160 lines with lanczos, keep the
/// frame rate (no `-r`), keep or convert the audio, write MP4 with the index
/// at the front, and report progress on stdout.
pub fn ffmpeg_args(job: &EncodeJob<'_>) -> Vec<OsString> {
    let scale = format!("scale=-2:{TARGET_HEIGHT}:flags=lanczos");
    let audio: &[&str] = match job.audio {
        AudioPlan::Copy => &["-c:a", "copy"],
        AudioPlan::ConvertToAac { .. } => &["-c:a", "aac", "-b:a", AAC_BITRATE],
    };

    let mut args = os(&["-hide_banner", "-nostdin", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-y"]);
    args.extend(os(&INPUT_RESTRICTIONS));
    args.push("-i".into());
    args.push(job.input.as_os_str().to_os_string());
    args.extend(os(&["-map", "0:V:0", "-map", "0:a?", "-vf", &scale]));
    args.extend(os(&job.encoder.video_args(job.frame_rate)));
    args.extend(os(&["-pix_fmt", "yuv420p"]));
    args.extend(os(audio));
    args.extend(os(&["-movflags", "+faststart", "-f", "mp4"]));
    args.push(job.temp_output.as_os_str().to_os_string());
    args
}

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// A tiny test encode that tells whether `encoder` really works on this
/// machine (an encoder can be listed by ffmpeg without a GPU behind it).
pub fn probe_args(encoder: Encoder) -> Vec<OsString> {
    let mut args: Vec<&str> = vec![
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1280x720:rate=30",
        "-frames:v",
        "3",
    ];
    args.extend(encoder.video_args(Some(30.0)));
    args.extend(["-pix_fmt", "yuv420p", "-f", "null", "-"]);
    os(&args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn job_args(encoder: Encoder, audio: AudioPlan, frame_rate: Option<f64>) -> Vec<String> {
        let input = Path::new("in").join("clip.mkv");
        let temp = Path::new("out").join("clip_4K.mp4.part");
        strings(ffmpeg_args(&EncodeJob { input: &input, temp_output: &temp, encoder, audio: &audio, frame_rate }))
    }

    fn expected(video: &[&str], audio: &[&str]) -> Vec<String> {
        let input = Path::new("in").join("clip.mkv").to_string_lossy().into_owned();
        let temp = Path::new("out").join("clip_4K.mp4.part").to_string_lossy().into_owned();
        let mut v: Vec<String> = [
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-nostats",
            "-progress",
            "pipe:1",
            "-y",
            "-protocol_whitelist",
            "file",
            "-format_whitelist",
            "mov,matroska",
            "-i",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        v.push(input);
        v.extend(["-map", "0:V:0", "-map", "0:a?", "-vf", "scale=-2:2160:flags=lanczos"].map(String::from));
        v.extend(video.iter().map(|s| s.to_string()));
        v.extend(["-pix_fmt", "yuv420p"].map(String::from));
        v.extend(audio.iter().map(|s| s.to_string()));
        v.extend(["-movflags", "+faststart", "-f", "mp4"].map(String::from));
        v.push(temp);
        v
    }

    const COPY: [&str; 2] = ["-c:a", "copy"];

    #[test]
    fn nvenc_job_uses_the_scripts_quality_settings() {
        assert_eq!(
            job_args(Encoder::Nvenc, AudioPlan::Copy, Some(60.0)),
            expected(&["-c:v", "h264_nvenc", "-preset", "p7", "-rc", "vbr", "-cq", "19", "-b:v", "0"], &COPY)
        );
    }

    #[test]
    fn libx264_job_uses_crf_19() {
        assert_eq!(
            job_args(Encoder::X264, AudioPlan::Copy, Some(60.0)),
            expected(&["-c:v", "libx264", "-preset", "medium", "-crf", "19", "-profile:v", "high"], &COPY)
        );
    }

    #[test]
    fn videotoolbox_at_60_fps_uses_the_high_frame_rate_bitrate() {
        assert_eq!(
            job_args(Encoder::VideoToolbox, AudioPlan::Copy, Some(60.0)),
            expected(&["-c:v", "h264_videotoolbox", "-b:v", "68M", "-profile:v", "high"], &COPY)
        );
    }

    #[test]
    fn videotoolbox_at_30_fps_or_less_uses_the_standard_bitrate() {
        for fps in [24.0, 29.97, 30.0] {
            assert_eq!(
                job_args(Encoder::VideoToolbox, AudioPlan::Copy, Some(fps)),
                expected(&["-c:v", "h264_videotoolbox", "-b:v", "45M", "-profile:v", "high"], &COPY),
                "fps {fps}"
            );
        }
    }

    #[test]
    fn videotoolbox_at_50_fps_uses_the_high_frame_rate_bitrate() {
        assert_eq!(
            job_args(Encoder::VideoToolbox, AudioPlan::Copy, Some(50.0)),
            expected(&["-c:v", "h264_videotoolbox", "-b:v", "68M", "-profile:v", "high"], &COPY)
        );
    }

    #[test]
    fn videotoolbox_with_unknown_frame_rate_uses_the_higher_bitrate() {
        assert_eq!(
            job_args(Encoder::VideoToolbox, AudioPlan::Copy, None),
            expected(&["-c:v", "h264_videotoolbox", "-b:v", "68M", "-profile:v", "high"], &COPY)
        );
    }

    #[test]
    fn audio_that_mp4_cannot_hold_is_converted_to_aac() {
        assert_eq!(
            job_args(Encoder::X264, AudioPlan::ConvertToAac { codec: "pcm_s16le".into() }, Some(30.0)),
            expected(
                &["-c:v", "libx264", "-preset", "medium", "-crf", "19", "-profile:v", "high"],
                &["-c:a", "aac", "-b:a", "384k"]
            )
        );
    }

    #[test]
    fn the_input_may_only_be_a_local_mp4_mov_or_matroska_file() {
        // Without these, a renamed playlist or other container could make ffmpeg
        // open further files or URLs chosen by the file's content.
        let args = job_args(Encoder::X264, AudioPlan::Copy, None);
        let input_at = args.iter().position(|a| a == "-i").unwrap();
        let before_input = &args[..input_at];
        assert!(before_input.windows(2).any(|w| w[0] == "-protocol_whitelist" && w[1] == "file"), "{args:?}");
        assert!(before_input.windows(2).any(|w| w[0] == "-format_whitelist" && w[1] == "mov,matroska"), "{args:?}");
    }

    #[test]
    fn arguments_never_force_a_frame_rate() {
        let args = job_args(Encoder::Nvenc, AudioPlan::Copy, Some(60.0));
        assert!(!args.iter().any(|a| a == "-r" || a == "-fps_mode" || a == "-vsync"));
    }

    #[test]
    fn non_ascii_paths_reach_ffmpeg_unchanged() {
        let input = Path::new("مقاطع").join("لقطة.mp4");
        let temp = Path::new("out").join("لقطة_4K.mp4.part");
        let audio = AudioPlan::Copy;
        let args = ffmpeg_args(&EncodeJob {
            input: &input,
            temp_output: &temp,
            encoder: Encoder::X264,
            audio: &audio,
            frame_rate: None,
        });
        assert!(args.contains(&input.clone().into_os_string()));
        assert_eq!(args.last(), Some(&temp.into_os_string()));
    }

    #[test]
    fn probe_runs_the_encoder_with_its_real_options_and_discards_output() {
        assert_eq!(
            strings(probe_args(Encoder::Nvenc)),
            [
                "-hide_banner",
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1280x720:rate=30",
                "-frames:v",
                "3",
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p7",
                "-rc",
                "vbr",
                "-cq",
                "19",
                "-b:v",
                "0",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "null",
                "-",
            ]
        );
    }

    #[test]
    fn hardware_encoder_is_kept_for_a_3840_wide_frame() {
        let size = FrameSize { width: 3840, height: TARGET_HEIGHT };
        assert_eq!(encoder_for(Encoder::Nvenc, size), EncoderChoice { encoder: Encoder::Nvenc, fallback: None });
    }

    #[test]
    fn hardware_encoder_is_kept_at_exactly_the_limit() {
        let size = FrameSize { width: 4096, height: TARGET_HEIGHT };
        assert_eq!(encoder_for(Encoder::VideoToolbox, size).encoder, Encoder::VideoToolbox);
    }

    #[test]
    fn ultrawide_frames_fall_back_to_libx264() {
        let size = FrameSize { width: 5120, height: TARGET_HEIGHT };
        for hw in [Encoder::Nvenc, Encoder::VideoToolbox] {
            assert_eq!(
                encoder_for(hw, size),
                EncoderChoice {
                    encoder: Encoder::X264,
                    fallback: Some(FallbackReason::FrameTooLarge { size, max_side: HARDWARE_MAX_SIDE }),
                }
            );
        }
    }

    #[test]
    fn libx264_has_no_size_limit() {
        let size = FrameSize { width: 7680, height: TARGET_HEIGHT };
        assert_eq!(encoder_for(Encoder::X264, size), EncoderChoice { encoder: Encoder::X264, fallback: None });
    }

    #[test]
    fn only_nvenc_and_videotoolbox_are_hardware() {
        assert!(Encoder::Nvenc.is_hardware());
        assert!(Encoder::VideoToolbox.is_hardware());
        assert!(!Encoder::X264.is_hardware());
    }
}
