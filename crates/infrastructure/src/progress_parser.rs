//! Reads the `key=value` blocks ffmpeg writes with `-progress pipe:1`.
//!
//! Each block ends with `progress=continue` or `progress=end`. Values can be
//! `N/A` early on, `out_time_us` can be negative before the first frame, and
//! `out_time_ms` is (despite the name) also in microseconds.

use upscale4k_application::ports::TranscodeProgress;

#[derive(Debug, Default)]
pub struct ProgressParser {
    out_time_us: Option<i64>,
    out_time_ms: Option<i64>,
    speed: Option<f64>,
}

impl ProgressParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one line. Returns a report when the line closes a block.
    pub fn feed(&mut self, line: &str) -> Option<TranscodeProgress> {
        let (key, value) = line.trim().split_once('=')?;
        let value = value.trim();
        match key.trim() {
            "out_time_us" => self.out_time_us = value.parse().ok(),
            "out_time_ms" => self.out_time_ms = value.parse().ok(),
            "speed" => {
                let number = value.strip_suffix('x').unwrap_or(value).trim();
                self.speed = number.parse::<f64>().ok().filter(|s| s.is_finite());
            }
            "progress" => {
                let report =
                    TranscodeProgress { out_time_us: self.out_time_us.or(self.out_time_ms), speed: self.speed };
                *self = Self::default();
                return Some(report);
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block captured from ffmpeg 9.0.2.
    const BLOCK: &str = "frame=60\nfps=0.00\nstream_0_0_q=-1.0\nbitrate=N/A\ntotal_size=N/A\nout_time_us=2000000\n\
                         out_time_ms=2000000\nout_time=00:00:02.000000\ndup_frames=0\ndrop_frames=0\nspeed=35.8x\n\
                         progress=end\n";

    fn feed_all(parser: &mut ProgressParser, text: &str) -> Vec<TranscodeProgress> {
        text.lines().filter_map(|line| parser.feed(line)).collect()
    }

    #[test]
    fn a_full_block_reports_position_and_speed_once() {
        let got = feed_all(&mut ProgressParser::new(), BLOCK);
        assert_eq!(got, vec![TranscodeProgress { out_time_us: Some(2_000_000), speed: Some(35.8) }]);
    }

    #[test]
    fn nothing_is_reported_before_the_block_ends() {
        let mut parser = ProgressParser::new();
        assert_eq!(parser.feed("frame=10"), None);
        assert_eq!(parser.feed("out_time_us=500000"), None);
        assert_eq!(parser.feed("speed=1.5x"), None);
    }

    #[test]
    fn not_available_values_are_reported_as_unknown() {
        let got =
            feed_all(&mut ProgressParser::new(), "out_time_us=N/A\nout_time_ms=N/A\nspeed=N/A\nprogress=continue\n");
        assert_eq!(got, vec![TranscodeProgress { out_time_us: None, speed: None }]);
    }

    #[test]
    fn negative_start_positions_are_passed_through() {
        let got = feed_all(&mut ProgressParser::new(), "out_time_us=-23220\nspeed=N/A\nprogress=continue\n");
        assert_eq!(got[0].out_time_us, Some(-23_220));
    }

    #[test]
    fn padded_speed_values_and_windows_line_endings_are_read() {
        let got = feed_all(&mut ProgressParser::new(), "out_time_us=1000000\r\nspeed=   2x\r\nprogress=continue\r\n");
        assert_eq!(got, vec![TranscodeProgress { out_time_us: Some(1_000_000), speed: Some(2.0) }]);
    }

    #[test]
    fn out_time_ms_is_used_when_out_time_us_is_missing() {
        let got = feed_all(&mut ProgressParser::new(), "out_time_ms=750000\nspeed=1x\nprogress=continue\n");
        assert_eq!(got[0].out_time_us, Some(750_000));
    }

    #[test]
    fn consecutive_blocks_each_report_their_own_values() {
        let text = "out_time_us=1000000\nspeed=1x\nprogress=continue\nout_time_us=3000000\nspeed=1.5x\nprogress=end\n";
        let got = feed_all(&mut ProgressParser::new(), text);
        assert_eq!(
            got,
            vec![
                TranscodeProgress { out_time_us: Some(1_000_000), speed: Some(1.0) },
                TranscodeProgress { out_time_us: Some(3_000_000), speed: Some(1.5) },
            ]
        );
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let got = feed_all(&mut ProgressParser::new(), "hello\n=\nout_time_us=abc\nprogress=continue\n");
        assert_eq!(got, vec![TranscodeProgress { out_time_us: None, speed: None }]);
    }
}
