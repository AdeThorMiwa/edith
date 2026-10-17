//! Edith planner (fork roadmap F08): the structure of a video, kept next to the edit, and the
//! Format Packs that give a kind of video its starting structure.
//!
//! - [`model`]: [`Planner`] (video → sections → beats → shots), its limits and its checks.
//! - [`pack`]: [`FormatPack`], read from TOML and turned into a plan with exact integer arithmetic.
//!
//! Pure data and arithmetic, no I/O: the engine stores a planner in the project and reads pack files
//! from disk. Everything read from a file or a project is untrusted and is checked or cut to fit.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod model;
pub mod pack;
pub mod script;

pub use model::{Beat, Cue, CueKind, Link, MusicCue, Planner, Section, Shot, SoundPlan, Status};
pub use pack::{FormatPack, builtin_packs, split};
pub use script::{BeatRange, ranges, sentences, speech_ticks};

#[cfg(test)]
mod tests;
