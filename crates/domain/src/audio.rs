//! Whether the audio tracks can be copied into MP4 as they are.

/// Codecs that MP4 holds and that players and YouTube read when copied.
pub const MP4_COPY_SAFE_CODECS: [&str; 6] = ["aac", "mp3", "ac3", "eac3", "opus", "alac"];

/// Bitrate used when audio has to be converted (YouTube guidance for stereo).
pub const AAC_BITRATE: &str = "384k";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioPlan {
    /// Copy every audio track unchanged (also used when there is no audio).
    Copy,
    /// Convert every audio track to AAC because `codec` can't go into MP4 as is.
    ConvertToAac { codec: String },
}

/// Copy the audio unless one of the tracks uses a codec MP4 can't hold.
pub fn plan_audio(audio_codecs: &[String]) -> AudioPlan {
    let blocked =
        audio_codecs.iter().find(|codec| !MP4_COPY_SAFE_CODECS.iter().any(|ok| codec.eq_ignore_ascii_case(ok)));
    match blocked {
        Some(codec) => AudioPlan::ConvertToAac { codec: codec.clone() },
        None => AudioPlan::Copy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codecs(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn aac_audio_is_copied() {
        assert_eq!(plan_audio(&codecs(&["aac"])), AudioPlan::Copy);
    }

    #[test]
    fn several_mp4_safe_tracks_are_copied() {
        assert_eq!(plan_audio(&codecs(&["aac", "opus", "eac3"])), AudioPlan::Copy);
    }

    #[test]
    fn a_clip_without_audio_needs_no_conversion() {
        assert_eq!(plan_audio(&[]), AudioPlan::Copy);
    }

    #[test]
    fn pcm_audio_from_a_mov_is_converted_to_aac() {
        assert_eq!(plan_audio(&codecs(&["pcm_s16le"])), AudioPlan::ConvertToAac { codec: "pcm_s16le".into() });
    }

    #[test]
    fn one_unsupported_track_converts_all_and_names_that_codec() {
        assert_eq!(plan_audio(&codecs(&["aac", "vorbis"])), AudioPlan::ConvertToAac { codec: "vorbis".into() });
    }

    #[test]
    fn codec_names_are_matched_without_case() {
        assert_eq!(plan_audio(&codecs(&["AAC"])), AudioPlan::Copy);
    }
}
