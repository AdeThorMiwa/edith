//! Narrations: audio generated from text (the Text to Speech panel).
//!
//! A [`Narration`] belongs to the project item of the WAV file that was generated for it
//! (`Project::narrations`, keyed by the item id), so every clip of that item can be clicked to
//! reopen the script and voice settings. Editing a narration writes a new WAV (a new item) and
//! swaps the clip over to it; the old item and its narration stay, so undo restores them exactly.

use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};

/// Longest script accepted, in bytes.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Pace limits (speaking rate multiplier).
pub const MIN_PACE: f64 = 0.5;
pub const MAX_PACE: f64 = 2.0;
/// The highest take (0 is the voice's plain reading).
pub const MAX_TAKE: u32 = 99;
/// Most spoken words kept with a narration (a script of [`MAX_TEXT_BYTES`] has fewer).
pub const MAX_WORDS: usize = 40_000;
/// Longest word text kept, in characters.
pub const MAX_WORD_CHARS: usize = 64;

/// Text to Speech ▸ Advanced ▸ Vocal pitch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VocalPitch {
    ExtraLow,
    Low,
    #[default]
    Default,
    High,
    ExtraHigh,
}

impl VocalPitch {
    pub const ALL: [VocalPitch; 5] = [VocalPitch::ExtraLow, VocalPitch::Low, VocalPitch::Default, VocalPitch::High, VocalPitch::ExtraHigh];

    pub fn label(self) -> &'static str {
        match self {
            VocalPitch::ExtraLow => "Extra low",
            VocalPitch::Low => "Low",
            VocalPitch::Default => "Default",
            VocalPitch::High => "High",
            VocalPitch::ExtraHigh => "Extra high",
        }
    }

    /// The command / serde id (`extraLow`, `low`, `default`, `high`, `extraHigh`).
    pub fn id(self) -> &'static str {
        match self {
            VocalPitch::ExtraLow => "extraLow",
            VocalPitch::Low => "low",
            VocalPitch::Default => "default",
            VocalPitch::High => "high",
            VocalPitch::ExtraHigh => "extraHigh",
        }
    }

    pub fn from_id(s: &str) -> Option<VocalPitch> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        Self::ALL.into_iter().find(|p| p.id().to_ascii_lowercase() == n)
    }

    /// Pitch shift in semitones.
    pub fn semitones(self) -> f64 {
        match self {
            VocalPitch::ExtraLow => -6.0,
            VocalPitch::Low => -3.0,
            VocalPitch::Default => 0.0,
            VocalPitch::High => 3.0,
            VocalPitch::ExtraHigh => 6.0,
        }
    }
}

/// A word as the voice spoke it ("nineteen", for a script that says "19"), timed from the start of the
/// generated file (fork roadmap F16.2). Words are in order and do not overlap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpokenWord {
    pub text: String,
    pub start: Tick,
    pub end: Tick,
    /// Which sentence of the script it belongs to, counting from 0.
    pub sentence: u32,
    /// Whether the voice knew the time, or it was worked out from the length of the text.
    pub exact: bool,
}

/// The script and voice settings a narration was generated from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Narration {
    /// The script, with pause markers (`[pause 1s]`) as typed.
    pub text: String,
    /// BCP 47 language tag (`en-US`).
    pub language: String,
    /// Voice id (`tts.voices`).
    pub voice: String,
    #[serde(default)]
    pub pitch: VocalPitch,
    /// Speaking rate, [`MIN_PACE`]–[`MAX_PACE`].
    pub pace: f64,
    /// Which reading of the script: 0 is the plain one, 1 to [`MAX_TAKE`] are other deliveries of
    /// the same words (natural voices only). Files saved before takes existed read as 0.
    #[serde(default)]
    pub take: u32,
    /// Length of the speech in the generated file. The file can be longer: when an edit made the
    /// speech shorter than its clip, the file is padded with silence to the clip's length.
    pub speech_duration: Tick,
    /// When each word is spoken, from the voice, for the assistant and the cut checks. Empty for files saved
    /// before it existed, and for a voice that cannot say.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<SpokenWord>,
}

impl Narration {
    /// Check values read from a project file or a command.
    pub fn check(&self) -> Result<(), String> {
        if self.text.len() > MAX_TEXT_BYTES {
            return Err(format!("the script is longer than {} KB", MAX_TEXT_BYTES / 1024));
        }
        if !(MIN_PACE..=MAX_PACE).contains(&self.pace) {
            return Err(format!("pace must be between {MIN_PACE} and {MAX_PACE}"));
        }
        if self.take > MAX_TAKE {
            return Err(format!("take must be between 0 and {MAX_TAKE}"));
        }
        if self.speech_duration < Tick::ZERO {
            return Err("negative speech duration".into());
        }
        if self.words.len() > MAX_WORDS {
            return Err(format!("more than {MAX_WORDS} spoken words"));
        }
        if self.words.iter().any(|w| w.start < Tick::ZERO || w.end < w.start || w.text.chars().count() > MAX_WORD_CHARS) {
            return Err("a spoken word has a negative or backwards time, or is too long".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Narration {
        Narration {
            text: "Hello there. [pause 1s] Welcome.".into(),
            language: "en-US".into(),
            voice: "basic-female".into(),
            pitch: VocalPitch::High,
            pace: 1.25,
            take: 0,
            speech_duration: Tick::from_seconds_f64(2.5),
            words: Vec::new(),
        }
    }

    #[test]
    fn round_trips_through_json() {
        let n = sample();
        let s = serde_json::to_string(&n).unwrap();
        assert!(s.contains("\"pitch\":\"high\""), "{s}");
        assert_eq!(serde_json::from_str::<Narration>(&s).unwrap(), n);
    }

    #[test]
    fn a_file_from_before_takes_reads_as_the_plain_reading_and_a_bad_take_is_refused() {
        let old = r#"{"text":"Hi.","language":"en-US","voice":"basic-female","pitch":"default","pace":1.0,"speechDuration":0}"#;
        let n: Narration = serde_json::from_str(old).unwrap();
        assert_eq!(n.take, 0);
        assert!(n.check().is_ok());
        let with = Narration { take: 7, ..n.clone() };
        assert_eq!(serde_json::from_str::<Narration>(&serde_json::to_string(&with).unwrap()).unwrap().take, 7);
        assert!(Narration { take: MAX_TAKE + 1, ..n }.check().is_err());
    }

    #[test]
    fn spoken_words_round_trip_stay_out_of_old_files_and_hostile_ones_are_refused() {
        let word = |t: &str, a: i64, b: i64| SpokenWord { text: t.into(), start: Tick(a), end: Tick(b), sentence: 0, exact: true };
        let mut n = sample();
        assert!(!serde_json::to_string(&n).unwrap().contains("words"), "nothing to say, nothing written");
        n.words = vec![word("hello", 0, 10), word("there", 12, 20)];
        let s = serde_json::to_string(&n).unwrap();
        assert_eq!(serde_json::from_str::<Narration>(&s).unwrap(), n);
        assert!(n.check().is_ok());
        n.words.push(word("back", 30, 25));
        assert!(n.check().is_err(), "backwards");
        n.words = vec![word("neg", -1, 5)];
        assert!(n.check().is_err(), "negative");
        n.words = vec![word(&"x".repeat(MAX_WORD_CHARS + 1), 0, 1)];
        assert!(n.check().is_err(), "long");
        n.words = vec![word("w", 0, 1); MAX_WORDS + 1];
        assert!(n.check().is_err(), "too many");
    }

    #[test]
    fn pitch_ids_round_trip() {
        for p in VocalPitch::ALL {
            assert_eq!(VocalPitch::from_id(p.id()), Some(p));
        }
        assert_eq!(VocalPitch::from_id("Extra High"), Some(VocalPitch::ExtraHigh));
        assert_eq!(VocalPitch::from_id("loud"), None);
    }

    #[test]
    fn check_rejects_hostile_values() {
        let mut n = sample();
        n.pace = f64::NAN;
        assert!(n.check().is_err());
        n.pace = 3.0;
        assert!(n.check().is_err());
        let mut n = sample();
        n.text = "a".repeat(MAX_TEXT_BYTES + 1);
        assert!(n.check().is_err());
        let mut n = sample();
        n.speech_duration = Tick(-1);
        assert!(n.check().is_err());
        assert!(sample().check().is_ok());
    }
}
