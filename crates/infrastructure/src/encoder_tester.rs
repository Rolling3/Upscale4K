//! Tells whether an encoder really works here by running a tiny test encode.

use std::time::Duration;

use upscale4k_application::ports::{EncoderTester, Toolkit};
use upscale4k_domain::encoder::{Encoder, probe_args};

use crate::process::{hidden_command, run_with_timeout};

#[derive(Debug, Default, Clone, Copy)]
pub struct FfmpegEncoderTester;

impl EncoderTester for FfmpegEncoderTester {
    fn works(&self, toolkit: &Toolkit, encoder: Encoder) -> bool {
        let mut command = hidden_command(&toolkit.ffmpeg);
        command.args(probe_args(encoder));
        run_with_timeout(command, Duration::from_secs(20)).is_some_and(|run| run.status.success())
    }
}
