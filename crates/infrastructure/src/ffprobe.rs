//! Reads clip facts with ffprobe.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use upscale4k_application::error::UpscaleError;
use upscale4k_application::ports::MediaProbe;
use upscale4k_domain::encoder::INPUT_RESTRICTIONS;
use upscale4k_domain::media::{FrameSize, VideoInfo, displayed_size};

use crate::process::{hidden_command, run_with_timeout};

pub struct FfprobeMediaProbe {
    ffprobe: PathBuf,
}

impl FfprobeMediaProbe {
    pub fn new(ffprobe: PathBuf) -> Self {
        Self { ffprobe }
    }
}

impl MediaProbe for FfprobeMediaProbe {
    fn probe(&self, input: &Path) -> Result<VideoInfo, UpscaleError> {
        if !input.exists() {
            return Err(UpscaleError::InputNotFound { path: input.to_path_buf() });
        }
        let mut command = hidden_command(&self.ffprobe);
        command
            .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
            .args(INPUT_RESTRICTIONS)
            .arg(input);
        let unreadable = |detail: String| UpscaleError::InputUnreadable { path: input.to_path_buf(), detail };

        let Some(run) = run_with_timeout(command, Duration::from_secs(60)) else {
            return Err(unreadable("ffprobe didn't start or didn't finish in time.".into()));
        };
        if !run.status.success() {
            let stderr = String::from_utf8_lossy(&run.stderr);
            let reason = stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("ffprobe failed.");
            let reason = reason.rsplit_once(": ").map(|(_, r)| r).unwrap_or(reason);
            return Err(unreadable(reason.to_string()));
        }
        parse_probe_json(&String::from_utf8_lossy(&run.stdout), input)
    }
}

#[derive(Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    duration: Option<String>,
    disposition: Option<Disposition>,
    tags: Option<Tags>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct Disposition {
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Deserialize)]
struct Tags {
    rotate: Option<String>,
}

#[derive(Deserialize)]
struct SideData {
    rotation: Option<f64>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
}

/// Turn ffprobe's `-print_format json -show_format -show_streams` output into `VideoInfo`.
pub fn parse_probe_json(json: &str, input: &Path) -> Result<VideoInfo, UpscaleError> {
    let path = input.to_path_buf();
    let parsed: ProbeOutput = serde_json::from_str(json).map_err(|e| UpscaleError::InputUnreadable {
        path: path.clone(),
        detail: format!("ffprobe's answer couldn't be read ({e})."),
    })?;

    let video = parsed
        .streams
        .iter()
        .find(|s| {
            s.codec_type.as_deref() == Some("video") && s.disposition.as_ref().is_none_or(|d| d.attached_pic == 0)
        })
        .ok_or_else(|| UpscaleError::NoVideoStream { path: path.clone() })?;
    let (Some(width), Some(height)) = (video.width, video.height) else {
        return Err(UpscaleError::InputUnreadable { path, detail: "ffprobe didn't report the video size.".into() });
    };

    let rotation = video
        .side_data_list
        .iter()
        .find_map(|d| d.rotation)
        .or_else(|| video.tags.as_ref()?.rotate.as_ref()?.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    let size = displayed_size(FrameSize { width, height }, rotation.round() as i32);

    let frame_rate = parse_rate(video.avg_frame_rate.as_deref()).or_else(|| parse_rate(video.r_frame_rate.as_deref()));
    let duration_secs = parsed
        .format
        .as_ref()
        .and_then(|f| parse_seconds(f.duration.as_deref()))
        .or_else(|| parse_seconds(video.duration.as_deref()));
    let audio_codecs = parsed
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .map(|s| s.codec_name.clone().unwrap_or_else(|| "unknown".into()))
        .collect();

    Ok(VideoInfo { size, duration_secs, frame_rate, audio_codecs })
}

/// `"60000/1001"` -> 59.94; `"0/0"`, `"N/A"` and silly values -> None.
fn parse_rate(rate: Option<&str>) -> Option<f64> {
    let (num, den) = rate?.split_once('/')?;
    let (num, den): (f64, f64) = (num.trim().parse().ok()?, den.trim().parse().ok()?);
    let fps = num / den;
    (den > 0.0 && fps.is_finite() && fps > 0.0 && fps <= 1000.0).then_some(fps)
}

fn parse_seconds(value: Option<&str>) -> Option<f64> {
    value?.trim().parse::<f64>().ok().filter(|s| s.is_finite() && *s > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> PathBuf {
        PathBuf::from("clip.mp4")
    }

    /// Trimmed from ffprobe 9.0.2 output for a 2 s clip.
    fn sample(video_extra: &str, audio: &str) -> String {
        format!(
            r#"{{
  "streams": [
    {{ "index": 0, "codec_name": "h264", "codec_type": "video", "width": 1920, "height": 1080,
       "r_frame_rate": "60/1", "avg_frame_rate": "60/1", "duration": "5.000000",
       "disposition": {{ "default": 1, "attached_pic": 0 }} {video_extra} }}
    {audio}
  ],
  "format": {{ "filename": "clip.mp4", "duration": "5.021000", "format_name": "mov,mp4,m4a,3gp,3g2,mj2" }}
}}"#
        )
    }

    const AAC: &str = r#", { "index": 1, "codec_name": "aac", "codec_type": "audio", "r_frame_rate": "0/0",
                             "avg_frame_rate": "0/0", "duration": "5.000000", "disposition": { "attached_pic": 0 } }"#;

    #[test]
    fn a_plain_clip_reports_size_rate_duration_and_audio() {
        let info = parse_probe_json(&sample("", AAC), &input()).unwrap();
        assert_eq!(
            info,
            VideoInfo {
                size: FrameSize { width: 1920, height: 1080 },
                duration_secs: Some(5.021),
                frame_rate: Some(60.0),
                audio_codecs: vec!["aac".into()],
            }
        );
    }

    #[test]
    fn a_rotation_in_the_display_matrix_swaps_the_size() {
        let extra =
            r#", "side_data_list": [ { "side_data_type": "Display Matrix", "displaymatrix": "x", "rotation": -90 } ]"#;
        let info = parse_probe_json(&sample(extra, AAC), &input()).unwrap();
        assert_eq!(info.size, FrameSize { width: 1080, height: 1920 });
    }

    #[test]
    fn an_old_style_rotate_tag_also_swaps_the_size() {
        let extra = r#", "tags": { "rotate": "90" }"#;
        let info = parse_probe_json(&sample(extra, AAC), &input()).unwrap();
        assert_eq!(info.size, FrameSize { width: 1080, height: 1920 });
    }

    #[test]
    fn a_clip_without_audio_has_no_audio_codecs() {
        let info = parse_probe_json(&sample("", ""), &input()).unwrap();
        assert!(info.audio_codecs.is_empty());
    }

    #[test]
    fn cover_art_is_not_mistaken_for_the_video() {
        let json = r#"{ "streams": [
            { "codec_name": "mjpeg", "codec_type": "video", "width": 600, "height": 600,
              "r_frame_rate": "90000/1", "avg_frame_rate": "0/0", "disposition": { "attached_pic": 1 } },
            { "codec_name": "aac", "codec_type": "audio", "disposition": { "attached_pic": 0 } } ],
            "format": { "duration": "3.0" } }"#;
        assert_eq!(parse_probe_json(json, &input()), Err(UpscaleError::NoVideoStream { path: input() }));
    }

    #[test]
    fn ntsc_rates_are_read_as_fractions() {
        let json = sample("", AAC)
            .replace("\"60/1\", \"avg_frame_rate\": \"60/1\"", "\"60000/1001\", \"avg_frame_rate\": \"60000/1001\"");
        let rate = parse_probe_json(&json, &input()).unwrap().frame_rate.unwrap();
        assert!((rate - 59.94).abs() < 0.01, "{rate}");
    }

    #[test]
    fn a_bogus_average_rate_falls_back_to_the_stream_rate() {
        let json = sample("", AAC).replace("\"avg_frame_rate\": \"60/1\"", "\"avg_frame_rate\": \"0/0\"");
        assert_eq!(parse_probe_json(&json, &input()).unwrap().frame_rate, Some(60.0));
    }

    #[test]
    fn missing_format_duration_uses_the_video_stream_duration() {
        let json = sample("", AAC).replace("\"duration\": \"5.021000\", ", "");
        assert_eq!(parse_probe_json(&json, &input()).unwrap().duration_secs, Some(5.0));
    }

    #[test]
    fn output_that_is_not_json_is_unreadable() {
        assert!(matches!(parse_probe_json("not json", &input()), Err(UpscaleError::InputUnreadable { .. })));
    }
}
