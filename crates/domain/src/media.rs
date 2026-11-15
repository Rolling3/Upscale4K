//! Which files are accepted and what size the upscaled frame will be.

use std::path::Path;

/// Output frame height in pixels.
pub const TARGET_HEIGHT: u32 = 2160;

/// File extensions the app accepts, lower case.
pub const SUPPORTED_EXTENSIONS: [&str; 3] = ["mp4", "mkv", "mov"];

/// True when the path ends in one of the supported video extensions (any case).
pub fn is_supported_video(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| SUPPORTED_EXTENSIONS.iter().any(|ok| ext.eq_ignore_ascii_case(ok)))
}

/// Width and height of a frame in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

/// Frame size as shown to the viewer: a clip stored landscape with a 90 or
/// 270 degree rotation flag plays as portrait.
pub fn displayed_size(coded: FrameSize, rotation_degrees: i32) -> FrameSize {
    let quarter_turns = (rotation_degrees.rem_euclid(360) + 45) / 90 % 4;
    if quarter_turns % 2 == 1 { FrameSize { width: coded.height, height: coded.width } } else { coded }
}

/// Size produced by `scale=-2:2160`: height 2160 and the width that keeps the
/// aspect ratio, rounded to the nearest even number the way ffmpeg does
/// (`av_rescale(2160, w, h * 2) * 2`, rounding half away from zero).
pub fn upscaled_size(displayed: FrameSize) -> FrameSize {
    let width = if displayed.height == 0 {
        0
    } else {
        let num = u64::from(TARGET_HEIGHT) * u64::from(displayed.width);
        let den = u64::from(displayed.height) * 2;
        let half_width = (num + den / 2) / den;
        u32::try_from(half_width * 2).unwrap_or(u32::MAX)
    };
    FrameSize { width, height: TARGET_HEIGHT }
}

/// What ffprobe told us about an input clip.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    /// Size as displayed (rotation already applied).
    pub size: FrameSize,
    pub duration_secs: Option<f64>,
    pub frame_rate: Option<f64>,
    /// Codec names of the audio tracks, in stream order.
    pub audio_codecs: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u32, height: u32) -> FrameSize {
        FrameSize { width, height }
    }

    #[test]
    fn accepts_the_three_supported_extensions_in_any_case() {
        for name in ["a.mp4", "b.mkv", "c.mov", "D.MP4", "e.MkV", "clip.final.Mov"] {
            assert!(is_supported_video(Path::new(name)), "{name} should be accepted");
        }
    }

    #[test]
    fn rejects_other_or_missing_extensions() {
        for name in ["a.avi", "b.mp4.txt", "mp4", "c.", "notes", "d.webm", "e.m4v"] {
            assert!(!is_supported_video(Path::new(name)), "{name} should be rejected");
        }
    }

    #[test]
    fn rotation_of_90_or_270_swaps_width_and_height() {
        assert_eq!(displayed_size(size(1920, 1080), 90), size(1080, 1920));
        assert_eq!(displayed_size(size(1920, 1080), -90), size(1080, 1920));
        assert_eq!(displayed_size(size(1920, 1080), 270), size(1080, 1920));
        assert_eq!(displayed_size(size(1920, 1080), 450), size(1080, 1920));
    }

    #[test]
    fn rotation_of_0_or_180_keeps_the_size() {
        assert_eq!(displayed_size(size(1920, 1080), 0), size(1920, 1080));
        assert_eq!(displayed_size(size(1920, 1080), 180), size(1920, 1080));
        assert_eq!(displayed_size(size(1920, 1080), -180), size(1920, 1080));
    }

    #[test]
    fn sixteen_by_nine_sources_become_3840_by_2160() {
        assert_eq!(upscaled_size(size(1920, 1080)), size(3840, 2160));
        assert_eq!(upscaled_size(size(1280, 720)), size(3840, 2160));
        assert_eq!(upscaled_size(size(2560, 1440)), size(3840, 2160));
    }

    #[test]
    fn ultrawide_sources_keep_their_aspect_ratio() {
        assert_eq!(upscaled_size(size(2560, 1080)), size(5120, 2160));
        assert_eq!(upscaled_size(size(3440, 1440)), size(5160, 2160));
    }

    #[test]
    fn odd_widths_round_to_the_nearest_even_number_like_ffmpeg() {
        // 2160 * 1080 / 1920 = 1215 exactly -> ffmpeg computes round(1215 / 2) * 2 = 1216.
        assert_eq!(upscaled_size(size(1080, 1920)), size(1216, 2160));
        // 2160 * 1366 / 768 = 3841.875 -> round(1920.94) * 2 = 3842.
        assert_eq!(upscaled_size(size(1366, 768)), size(3842, 2160));
        // 2160 * 1000 / 1080 = 2000 -> stays 2000.
        assert_eq!(upscaled_size(size(1000, 1080)), size(2000, 2160));
    }

    #[test]
    fn a_zero_height_never_divides_by_zero() {
        assert_eq!(upscaled_size(size(1920, 0)), size(0, 2160));
    }
}
