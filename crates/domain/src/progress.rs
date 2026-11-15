//! Progress math for one encode.

/// How far an encode is, derived from ffmpeg's position in the output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressUpdate {
    /// 0.0 to 1.0, or `None` when the clip length is unknown.
    pub fraction: Option<f64>,
    /// Seconds left at the current speed, when it can be estimated.
    pub eta_secs: Option<f64>,
    /// Encode speed relative to real time (2.0 = twice as fast as playback).
    pub speed: Option<f64>,
}

/// Combine ffmpeg's `out_time_us` and `speed` with the clip duration.
pub fn progress_update(out_time_us: Option<i64>, duration_secs: Option<f64>, speed: Option<f64>) -> ProgressUpdate {
    let duration = duration_secs.filter(|d| d.is_finite() && *d > 0.0);
    let position = out_time_us.map(|us| us.max(0) as f64 / 1_000_000.0);
    let speed = speed.filter(|s| s.is_finite() && *s > 0.0);

    let fraction = match (position, duration) {
        (Some(pos), Some(dur)) => Some((pos / dur).clamp(0.0, 1.0)),
        _ => None,
    };
    let eta_secs = match (position, duration, speed) {
        (Some(pos), Some(dur), Some(s)) => Some((dur - pos).max(0.0) / s),
        _ => None,
    };
    ProgressUpdate { fraction, eta_secs, speed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halfway_through_a_five_second_clip_is_half_done() {
        let got = progress_update(Some(2_500_000), Some(5.0), Some(2.0));
        assert_eq!(got.fraction, Some(0.5));
        // 2.5 s of video left at twice real time.
        assert_eq!(got.eta_secs, Some(1.25));
        assert_eq!(got.speed, Some(2.0));
    }

    #[test]
    fn position_past_the_end_is_capped_at_done() {
        let got = progress_update(Some(6_000_000), Some(5.0), Some(1.0));
        assert_eq!(got.fraction, Some(1.0));
        assert_eq!(got.eta_secs, Some(0.0));
    }

    #[test]
    fn negative_position_at_the_start_counts_as_zero() {
        let got = progress_update(Some(-40_000), Some(5.0), Some(1.0));
        assert_eq!(got.fraction, Some(0.0));
        assert_eq!(got.eta_secs, Some(5.0));
    }

    #[test]
    fn unknown_duration_gives_no_fraction_or_eta() {
        let got = progress_update(Some(1_000_000), None, Some(1.0));
        assert_eq!(got.fraction, None);
        assert_eq!(got.eta_secs, None);
    }

    #[test]
    fn zero_duration_gives_no_fraction() {
        assert_eq!(progress_update(Some(1_000_000), Some(0.0), Some(1.0)).fraction, None);
    }

    #[test]
    fn missing_position_gives_no_fraction() {
        assert_eq!(progress_update(None, Some(5.0), Some(1.0)).fraction, None);
    }

    #[test]
    fn zero_or_missing_speed_gives_no_eta() {
        assert_eq!(progress_update(Some(1_000_000), Some(5.0), Some(0.0)).eta_secs, None);
        assert_eq!(progress_update(Some(1_000_000), Some(5.0), None).eta_secs, None);
    }
}
