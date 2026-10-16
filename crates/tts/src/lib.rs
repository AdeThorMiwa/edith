//! Text to speech for FilmCraft's Text to Speech panel (narrations).
//!
//! - [`Voice`]: the trait every voice implements: a script in, mono samples out.
//! - [`voices`] / [`voice`]: the voice catalogue (id, name, language, gender, engine, licence).
//! - [`script`]: splits a script into speech and pause markers (`[pause 1s]`, `[pause 500ms]`).
//! - [`formant`]: the built-in voices, an original source–filter formant synthesizer. They need
//!   no download and work everywhere (also in the web build); they sound robotic. The neural voices
//!   (Kokoro-82M, downloaded on first use) are added behind a feature in a later milestone.
//!
//! Everything is deterministic: the same script and settings give the same samples.

pub mod catalog;
pub mod formant;
#[cfg(feature = "kokoro")]
pub mod kokoro;
#[cfg(feature = "kokoro")]
pub mod neural;
pub mod script;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Sample rate of every voice's output.
pub const SAMPLE_RATE: u32 = 24_000;
/// Longest narration a voice produces, in seconds.
pub const MAX_SECONDS: f64 = 30.0 * 60.0;
/// Longest script accepted, in bytes (same as `filmcraft_project::narration::MAX_TEXT_BYTES`).
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Pace limits.
pub const MIN_PACE: f64 = 0.5;
pub const MAX_PACE: f64 = 2.0;
/// Pitch shift limit in semitones (either way).
pub const MAX_SEMITONES: f64 = 12.0;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TtsError {
    #[error("unknown voice `{0}`")]
    UnknownVoice(String),
    #[error("the script has nothing to say")]
    Empty,
    #[error("the script is longer than {} KB", MAX_TEXT_BYTES / 1024)]
    TextTooLong,
    #[error("the narration would be longer than {} minutes", MAX_SECONDS / 60.0)]
    AudioTooLong,
    #[error("{0}")]
    Invalid(String),
    #[error("voice model: {0}")]
    Model(String),
    #[error("the natural voices are not downloaded yet")]
    NotInstalled,
    #[error("natural voices are not available in this build (built without the `kokoro` feature)")]
    Unavailable,
}

/// How a voice speaks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Pitch shift in semitones, ±[`MAX_SEMITONES`].
    pub semitones: f64,
    /// Speaking rate, [`MIN_PACE`]–[`MAX_PACE`] (1 = normal).
    pub pace: f64,
    /// Which reading of the script: 0 is the voice's plain reading; 1 to [`MAX_TAKE`] are other
    /// deliveries of the same words, with a little different timing and stress, each repeatable.
    /// Only the natural voices have takes; the built-in voices say every take the same way.
    pub take: u32,
}

/// The highest take number.
pub const MAX_TAKE: u32 = 99;

impl Default for Params {
    fn default() -> Self {
        Params { semitones: 0.0, pace: 1.0, take: 0 }
    }
}

impl Params {
    /// These settings with a direction on top: pace times `pace`, pitch plus `semitones`, both kept
    /// within the voice's limits (a non-finite request leaves the setting as it is).
    pub fn directed(&self, pace: f64, semitones: f64) -> Params {
        let p = if pace.is_finite() { self.pace * pace } else { self.pace };
        let st = if semitones.is_finite() { self.semitones + semitones } else { self.semitones };
        Params { pace: p.clamp(MIN_PACE, MAX_PACE), semitones: st.clamp(-MAX_SEMITONES, MAX_SEMITONES), take: self.take }
    }

    pub fn check(&self) -> Result<(), TtsError> {
        if !(MIN_PACE..=MAX_PACE).contains(&self.pace) {
            return Err(TtsError::Invalid(format!("pace must be between {MIN_PACE} and {MAX_PACE}")));
        }
        if !(-MAX_SEMITONES..=MAX_SEMITONES).contains(&self.semitones) {
            return Err(TtsError::Invalid(format!("pitch must be within ±{MAX_SEMITONES} semitones")));
        }
        if self.take > MAX_TAKE {
            return Err(TtsError::Invalid(format!("take must be 0 to {MAX_TAKE}")));
        }
        Ok(())
    }
}

/// Mono audio at [`SAMPLE_RATE`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Audio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl Audio {
    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / f64::from(self.sample_rate.max(1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Gender {
    Female,
    Male,
}

/// A voice in the catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// BCP 47 tag.
    pub language: &'static str,
    pub gender: Gender,
    /// `basic` (built in) or `neural` (downloaded).
    pub engine: &'static str,
    pub description: &'static str,
    pub license: &'static str,
    /// Usable now (built in, or downloaded).
    pub installed: bool,
}

/// How you want words said: word (any case) → a respelling in plain English (`koo brick`) or
/// phonemes between slashes. Only the natural voices use it; the built-in voices ignore it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pronunciations(pub BTreeMap<String, String>);

impl Pronunciations {
    /// Longest word, longest way of saying it, and most entries kept.
    pub const MAX_WORD: usize = 40;
    pub const MAX_SAY: usize = 200;
    pub const MAX_ENTRIES: usize = 1_000;

    /// A word and how to say it, checked and tidied: the word lowercased (letters, apostrophes and
    /// hyphens only), the way of saying it trimmed, either plain English words or phonemes between
    /// slashes (`/kjuːbɹɪk/` style, using only the symbols the voice knows).
    pub fn checked_entry(word: &str, say: &str) -> Result<(String, String), String> {
        let word = word.trim().to_lowercase();
        let n = word.chars().count();
        if n == 0
            || n > Self::MAX_WORD
            || !word.chars().all(|c| c.is_alphabetic() || matches!(c, '\'' | '\u{2019}' | '-'))
            || !word.chars().any(char::is_alphabetic)
        {
            return Err(format!("a word is 1 to {} letters (apostrophes and hyphens allowed)", Self::MAX_WORD));
        }
        let say = say.trim().to_string();
        let n = say.chars().count();
        if n == 0 || n > Self::MAX_SAY || say.chars().any(char::is_control) {
            return Err(format!("how to say it is 1 to {} characters", Self::MAX_SAY));
        }
        if say.starts_with('/') {
            let inner = say.strip_prefix('/').and_then(|r| r.strip_suffix('/')).filter(|i| !i.is_empty());
            let ok = inner.is_some_and(|i| {
                #[cfg(feature = "kokoro")]
                {
                    i.chars().all(|c| filmcraft_tts_text::SYMBOLS.contains(c))
                }
                #[cfg(not(feature = "kokoro"))]
                {
                    let _ = i;
                    true
                }
            });
            if !ok {
                return Err("phonemes go between two slashes and use only the voice's own symbols; otherwise write the sound in plain words".into());
            }
        }
        Ok((word, say))
    }

    /// A short stable text for the list, to tell two lists apart (a cache key).
    pub fn fingerprint(&self) -> String {
        self.0.iter().map(|(w, s)| format!("{w}\u{1e}{s}")).collect::<Vec<_>>().join("\u{1d}")
    }
}

/// A word the voice would have to guess at, and how often the script says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnknownWord {
    pub word: String,
    pub count: u32,
}

/// A voice that turns a script into audio.
pub trait Voice: Send + Sync {
    fn info(&self) -> VoiceInfo;
    /// Speak `text` (pause markers allowed). Fails on an empty or over-long script, or bad params.
    fn synthesize(&self, text: &str, params: &Params) -> Result<Audio, TtsError>;
    /// [`Self::synthesize`] saying the words in `pronounce` as given. Voices that cannot ignore it.
    fn synthesize_with(&self, text: &str, params: &Params, pronounce: &Pronunciations) -> Result<Audio, TtsError> {
        let _ = pronounce;
        self.synthesize(text, params)
    }
}

/// The words of `text` the natural voices would guess at (not in the dictionary, not built from
/// one, and not in `pronounce`). Needs the downloaded voice package, which carries the dictionary.
pub fn check_words(text: &str, models_dir: Option<&std::path::Path>, pronounce: &Pronunciations) -> Result<Vec<UnknownWord>, TtsError> {
    #[cfg(feature = "kokoro")]
    {
        let dir = models_dir.ok_or(TtsError::NotInstalled)?;
        neural::check_words(&script::strip_directions(text), dir, pronounce)
    }
    #[cfg(not(feature = "kokoro"))]
    {
        let _ = (text, models_dir, pronounce);
        Err(TtsError::Unavailable)
    }
}

/// Languages offered (English only for now).
pub const LANGUAGES: &[(&str, &str)] = &[("en-US", "English (United States)")];

/// Every voice FilmCraft knows: the built-in voices, then the neural voices (installed when their
/// package is downloaded into `models_dir`).
pub fn voices_in(models_dir: Option<&std::path::Path>) -> Vec<VoiceInfo> {
    let installed = models_dir.is_some_and(catalog::installed) && cfg!(feature = "kokoro");
    formant::VOICES.iter().map(|v| v.info).chain(catalog::VOICES.iter().map(|v| VoiceInfo { installed, ..v.info })).collect()
}

/// The built-in voices (always available).
pub fn voices() -> Vec<VoiceInfo> {
    formant::VOICES.iter().map(|v| v.info).collect()
}

/// The voice with this id.
pub fn voice(id: &str) -> Result<Box<dyn Voice>, TtsError> {
    formant::VOICES
        .iter()
        .find(|v| v.info.id == id)
        .map(|v| Box::new(formant::FormantVoice::new(*v)) as Box<dyn Voice>)
        .ok_or_else(|| TtsError::UnknownVoice(id.to_string()))
}

/// The voice `id`: built in, or (feature `kokoro`) a neural voice from the package in `models_dir`.
pub fn voice_in(id: &str, models_dir: Option<&std::path::Path>) -> Result<Box<dyn Voice>, TtsError> {
    if catalog::find(id).is_some() {
        #[cfg(feature = "kokoro")]
        {
            let dir = models_dir.ok_or(TtsError::NotInstalled)?;
            return neural::load(id, dir).map(|v| Box::new(v) as Box<dyn Voice>);
        }
        #[cfg(not(feature = "kokoro"))]
        {
            let _ = models_dir;
            return Err(TtsError::Unavailable);
        }
    }
    voice(id)
}

/// The voice used when none is chosen.
pub fn default_voice_id() -> &'static str {
    formant::VOICES.first().map(|v| v.info.id).unwrap_or("basic-female")
}

/// The sentence "Hear this voice" plays.
pub const SAMPLE_SENTENCE: &str = "Hello! This is how I sound when I read your script.";

/// Validate a script before synthesis: length, and that it says something.
pub fn check_text(text: &str) -> Result<(), TtsError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(TtsError::TextTooLong);
    }
    let says_something = script::parse(text).iter().any(|s| matches!(s, script::Segment::Speech(t) if t.chars().any(char::is_alphanumeric)));
    if !says_something {
        return Err(TtsError::Empty);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_voices_ignore_pronunciations_and_the_list_tells_itself_apart() {
        let v = voice(default_voice_id()).unwrap();
        let p = Params::default();
        let list = Pronunciations([("kubrick".to_string(), "koo brick".to_string())].into_iter().collect());
        assert_eq!(v.synthesize_with("Hello Kubrick.", &p, &list).unwrap(), v.synthesize("Hello Kubrick.", &p).unwrap());
        assert_ne!(list.fingerprint(), Pronunciations::default().fingerprint());
        let other = Pronunciations([("kubrick".to_string(), "kyoo brick".to_string())].into_iter().collect());
        assert_ne!(list.fingerprint(), other.fingerprint());
        assert_eq!(list.fingerprint(), list.clone().fingerprint());
    }

    #[test]
    fn an_entry_is_tidied_and_hostile_ones_are_refused() {
        assert_eq!(Pronunciations::checked_entry("  Kubrick ", " koo brick ").unwrap(), ("kubrick".to_string(), "koo brick".to_string()));
        assert!(Pronunciations::checked_entry("Reykjavík", "rake yah veek").is_ok());
        assert!(Pronunciations::checked_entry("O'Brien", "oh bry en").is_ok() && Pronunciations::checked_entry("Jean-Luc", "zhon luke").is_ok());
        for (w, say) in [
            ("", "x"),
            ("   ", "x"),
            ("two words", "x"),
            ("a1", "x"),
            ("'-'", "x"),
            ("x", ""),
            ("x", "   "),
            ("x", "bad\u{7}control"),
            ("x", "/"),
            ("x", "//"),
            ("x", "/unterminated"),
        ] {
            assert!(Pronunciations::checked_entry(w, say).is_err(), "{w:?} {say:?}");
        }
        assert!(Pronunciations::checked_entry(&"w".repeat(41), "x").is_err() && Pronunciations::checked_entry("w", &"x".repeat(201)).is_err());
        if cfg!(feature = "kokoro") {
            assert!(Pronunciations::checked_entry("x", "/kjuːbɹɪk/").is_err(), "a symbol the voice does not have (the length mark)");
            assert!(Pronunciations::checked_entry("x", "/kjˈubɹɪk/").is_ok());
        }
    }

    #[test]
    fn a_direction_is_laid_over_the_settings_and_kept_within_the_limits() {
        let base = Params { semitones: 3.0, pace: 1.5, take: 4 };
        assert_eq!(base.directed(1.0, 0.0), base);
        let d = base.directed(0.5, -5.0);
        assert_eq!((d.pace, d.semitones, d.take), (0.75, -2.0, 4));
        let d = base.directed(100.0, 100.0);
        assert_eq!((d.pace, d.semitones), (MAX_PACE, MAX_SEMITONES));
        let d = base.directed(0.0001, -100.0);
        assert_eq!((d.pace, d.semitones), (MIN_PACE, -MAX_SEMITONES));
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(base.directed(bad, bad), base, "{bad}");
        }
        assert!(base.directed(0.7, 2.0).check().is_ok());
    }

    #[test]
    fn directions_are_not_something_to_say() {
        assert_eq!(check_text("[pace 0.8] [pitch -2] [normal]").err(), Some(TtsError::Empty));
        assert!(check_text("[pace 0.8] Hello.").is_ok());
    }

    #[test]
    fn checking_words_needs_the_natural_voices() {
        let none = Pronunciations::default();
        let want = if cfg!(feature = "kokoro") { TtsError::NotInstalled } else { TtsError::Unavailable };
        assert_eq!(check_words("Hello Kubrick.", None, &none).err(), Some(want));
        let empty = std::env::temp_dir().join(format!("filmcraft-tts-nomodels-{}", std::process::id()));
        let r = check_words("Hello Kubrick.", Some(&empty), &none);
        assert!(matches!(r, Err(TtsError::NotInstalled | TtsError::Unavailable)), "{r:?}");
        // an empty script is refused before anything is loaded
        if cfg!(feature = "kokoro") {
            assert_eq!(check_words("   ", Some(&empty), &none).err(), Some(TtsError::Empty));
        }
    }

    #[test]
    fn catalogue_ids_are_unique_and_resolvable() {
        let v = voices();
        assert!(v.len() >= 2);
        for (i, a) in v.iter().enumerate() {
            assert!(v.iter().skip(i + 1).all(|b| b.id != a.id), "duplicate {}", a.id);
            assert_eq!(voice(a.id).unwrap().info().id, a.id);
        }
        assert_eq!(voice("nope").err(), Some(TtsError::UnknownVoice("nope".into())));
        assert!(voice(default_voice_id()).is_ok());
    }

    #[test]
    fn params_are_checked() {
        assert!(Params::default().check().is_ok());
        for p in [
            Params { pace: f64::NAN, ..Default::default() },
            Params { pace: 0.1, ..Default::default() },
            Params { semitones: 40.0, ..Default::default() },
            Params { semitones: f64::INFINITY, ..Default::default() },
            Params { take: MAX_TAKE + 1, ..Default::default() },
        ] {
            assert!(p.check().is_err(), "{p:?}");
        }
    }

    #[test]
    fn empty_scripts_are_refused() {
        assert_eq!(check_text(""), Err(TtsError::Empty));
        assert_eq!(check_text("  [pause 1s] ... "), Err(TtsError::Empty));
        assert_eq!(check_text(&"a".repeat(MAX_TEXT_BYTES + 1)), Err(TtsError::TextTooLong));
        assert!(check_text("Hi").is_ok());
    }
}
