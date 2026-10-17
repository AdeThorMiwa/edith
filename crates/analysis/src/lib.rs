//! Edith analysis (fork roadmap F10): pure functions that measure footage and audio so that later
//! features (the editing coach, reference analysis, the consistency checker) can talk in numbers.
//!
//! - [`color`]: sRGB to CIE Lab, colour distance and correlated colour temperature.
//! - [`image`]: [`FrameStats`] for one picture: luma, contrast, saturation, a five-colour palette in
//!   Lab, colour temperature, sharpness, grain and letterbox/aspect.
//!
//! No I/O and no decoding: callers hand in pixels or samples they decoded. Everything handed in is
//! untrusted: sizes are checked before anything is allocated and no input makes these functions
//! panic. Each family of measures has a version number that goes up whenever a number could change,
//! so a cached result can be recognised as stale.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod color;
pub mod image;

pub use image::{Frame, FrameStats, Letterbox, PaletteColor, analyze_rgba};

/// Why a measurement could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnalysisError {
    /// Nothing to measure: a picture with no pixels.
    Empty,
    /// Larger than the limits ([`image::MAX_DIMENSION`], [`image::MAX_PIXELS`]).
    TooLarge { width: u32, height: u32 },
    /// The buffer holds fewer bytes than the stated size needs.
    ShortBuffer { need: usize, have: usize },
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalysisError::Empty => write!(f, "there is nothing to measure"),
            AnalysisError::TooLarge { width, height } => write!(f, "a {width}x{height} picture is larger than can be analysed"),
            AnalysisError::ShortBuffer { need, have } => write!(f, "the picture needs {need} bytes but has {have}"),
        }
    }
}

impl std::error::Error for AnalysisError {}

pub type Result<T> = std::result::Result<T, AnalysisError>;

#[cfg(test)]
mod tests;
