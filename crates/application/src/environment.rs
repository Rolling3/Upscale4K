//! What the app finds at startup: ffmpeg, and the best encoder that works.

use upscale4k_domain::encoder::Encoder;

use crate::ports::{EncoderTester, InstallHint, Platform, Toolkit, ToolkitLocator};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub toolkit: Option<Toolkit>,
    /// The first encoder from the platform's list that passed a test encode.
    pub encoder: Option<Encoder>,
    pub install: InstallHint,
}

pub fn detect_environment(
    locator: &dyn ToolkitLocator,
    tester: &dyn EncoderTester,
    platform: &dyn Platform,
) -> Environment {
    let toolkit = locator.locate();
    let encoder =
        toolkit.as_ref().and_then(|kit| platform.preferred_encoders().into_iter().find(|enc| tester.works(kit, *enc)));
    Environment { toolkit, encoder, install: platform.install_hint() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{FakeLocator, FakePlatform, FakeTester};

    #[test]
    fn a_working_gpu_encoder_is_preferred() {
        let tester = FakeTester::working(&[Encoder::Nvenc, Encoder::X264]);
        let env = detect_environment(&FakeLocator::found(), &tester, &FakePlatform::windows_like());
        assert_eq!(env.encoder, Some(Encoder::Nvenc));
        assert!(env.toolkit.is_some());
    }

    #[test]
    fn libx264_is_used_when_the_gpu_test_encode_fails() {
        let tester = FakeTester::working(&[Encoder::X264]);
        let env = detect_environment(&FakeLocator::found(), &tester, &FakePlatform::windows_like());
        assert_eq!(env.encoder, Some(Encoder::X264));
        assert_eq!(tester.tried(), vec![Encoder::Nvenc, Encoder::X264]);
    }

    #[test]
    fn no_encoder_is_reported_when_none_works() {
        let env = detect_environment(&FakeLocator::found(), &FakeTester::working(&[]), &FakePlatform::windows_like());
        assert_eq!(env.encoder, None);
        assert!(env.toolkit.is_some());
    }

    #[test]
    fn missing_ffmpeg_gives_the_install_hint_and_tests_nothing() {
        let tester = FakeTester::working(&[Encoder::Nvenc, Encoder::X264]);
        let env = detect_environment(&FakeLocator::missing(), &tester, &FakePlatform::windows_like());
        assert_eq!(env.toolkit, None);
        assert_eq!(env.encoder, None);
        assert_eq!(env.install.command, "winget install --id Gyan.FFmpeg -e");
        assert!(tester.tried().is_empty());
    }
}
