//! Rules for turning a gameplay clip into a 2160p MP4 that YouTube encodes
//! with its better codec. Everything here is pure: no files, no processes.

pub mod audio;
pub mod encoder;
pub mod media;
pub mod naming;
pub mod progress;
