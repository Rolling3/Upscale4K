//! Adapters that connect the Upscale4K use cases to the real machine:
//! ffmpeg and ffprobe processes, the local file system, the settings file,
//! and the defaults that differ between Windows and macOS.

pub mod encoder_tester;
pub mod ffmpeg_errors;
pub mod ffmpeg_runner;
pub mod ffprobe;
pub mod local_fs;
pub mod locator;
pub mod platform;
pub mod process;
pub mod progress_parser;
pub mod settings_store;
