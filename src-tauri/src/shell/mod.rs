//! Aaru-OS — Host shell execution layer.
//!
//! The Almanac *command grammar* lives in [`crate::almanac`]. This module is
//! only the controlled bridge to the underlying host runtime that Almanac
//! falls back to for commands it does not implement natively (`npm`, `git`,
//! `python`, …).

pub mod host;
pub mod tokenize;

pub use host::{HostCommand, HostError, ProcessRunner, StreamEvent, SystemProcessRunner};
pub use tokenize::tokenize_host_line;
