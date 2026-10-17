//! Edith analysis (fork roadmap F10): pure functions that measure footage and audio so that later
//! features (the editing coach, reference analysis, the consistency checker) can talk in numbers.
//!
//! - [`color`]: sRGB to CIE Lab, colour distance and correlated colour temperature.
//! - [`audio`]: [`AudioStats`] for a stretch of audio: EBU R128 loudness (from `filmcraft-audio-dsp`),
//!   silence share, spectral balance, a rough speech/music/ambience guess and a beat grid.
//! - [`motion`]: [`MotionPair`] between two pictures (how much changed, how the camera moved, how much
//!   moves on its own) and a rough camera-move class for a run of them.
//! - [`sequence`]: [`SequenceStats`] from the layout of the timeline alone (shot lengths, cuts, J and L
//!   cuts, transitions, narration pace, track occupancy): no decoding.
//! - [`image`]: [`FrameStats`] for one picture: luma, contrast, saturation, a five-colour palette in
//!   Lab, colour temperature, sharpness, grain and letterbox/aspect.
//!
//! No I/O and no decoding: callers hand in pixels or samples they decoded. Everything handed in is
//! untrusted: sizes are checked before anything is allocated and no input makes these functions
//! panic. Each family of measures has a version number that goes up whenever a number could change,
//! so a cached result can be recognised as stale.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod audio;
pub mod color;
pub mod image;
pub mod motion;
pub mod sequence;

pub use audio::{AudioBuffer, AudioKind, AudioStats, analyze_audio};
pub use image::{Frame, FrameStats, Letterbox, PaletteColor, analyze_rgba};
pub use motion::{CameraMove, MotionPair, MotionSummary, analyze_motion, summarize};
pub use sequence::{SequenceInput, SequenceStats, ShotSpan, analyze_sequence, shot_spans};

/// Why a measurement could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnalysisError {
    /// Nothing to measure: a picture with no pixels.
    Empty,
    /// Larger than the limits ([`image::MAX_DIMENSION`], [`image::MAX_PIXELS`]).
    TooLarge { width: u32, height: u32 },
    /// The buffer holds fewer bytes than the stated size needs.
    ShortBuffer { need: usize, have: usize },
    /// A sample rate outside 8 kHz to 384 kHz, or no channels, or more than 16.
    BadAudioFormat { sample_rate: u32, channels: u16 },
    /// The two pictures of a motion measurement differ in size.
    MismatchedFrames,
    /// The time between two pictures is not a number from 0 (exclusive) to 60 seconds.
    BadInterval,
    /// More clips than [`sequence::MAX_CLIPS`].
    TooManyClips { clips: usize },
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalysisError::Empty => write!(f, "there is nothing to measure"),
            AnalysisError::TooLarge { width, height } => write!(f, "a {width}x{height} picture is larger than can be analysed"),
            AnalysisError::ShortBuffer { need, have } => write!(f, "the picture needs {need} bytes but has {have}"),
            AnalysisError::TooManyClips { clips } => write!(f, "{clips} clips are more than can be analysed"),
            AnalysisError::MismatchedFrames => write!(f, "the two pictures differ in size"),
            AnalysisError::BadInterval => write!(f, "the time between the two pictures must be more than 0 and at most 60 seconds"),
            AnalysisError::BadAudioFormat { sample_rate, channels } => write!(f, "audio of {channels} channels at {sample_rate} Hz cannot be analysed"),
        }
    }
}

impl std::error::Error for AnalysisError {}

pub type Result<T> = std::result::Result<T, AnalysisError>;

#[cfg(test)]
mod audio_tests;
#[cfg(test)]
mod motion_tests;
#[cfg(test)]
mod sequence_tests;
#[cfg(test)]
mod tests;
