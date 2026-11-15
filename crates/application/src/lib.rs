//! Upscale4K use cases and the ports (traits) they need from the outside
//! world. Implementations of the ports live in the infrastructure crate.

pub mod batch;
pub mod cancel;
pub mod environment;
pub mod error;
pub mod ports;
pub mod settings;

#[cfg(test)]
mod fakes;
