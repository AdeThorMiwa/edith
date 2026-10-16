//! The neural voices (feature `kokoro`): a script goes through the text front end
//! (`filmcraft-tts-text`: normalizer, CMUdict + letter-to-sound, chunks of at most 400 phonemes
//! and pause markers), each chunk through Kokoro-82M, pauses become exact silence, and the vocal
//! pitch is applied afterwards with the pitch shifter from `filmcraft-audio-dsp` (Kokoro has no
//! pitch control). Pace is Kokoro's speed. The model, the dictionary and voice packs are loaded
//! once per package directory and shared.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use filmcraft_audio_dsp::AudioEffect;
use filmcraft_tts_text::{Chunk, Lang, Lexicon, Overrides};

use crate::catalog;
use crate::kokoro::{KOKORO_RATE, Kokoro, VoicePack};
use crate::{Audio, MAX_SECONDS, Params, Pronunciations, TtsError, UnknownWord, Voice, VoiceInfo, check_text};

/// Silence between sentences, ms.
const SENTENCE_GAP_MS: u32 = 120;

struct Loaded {
    dir: PathBuf,
    model: Arc<Kokoro>,
    lexicon: Arc<Lexicon>,
    packs: HashMap<&'static str, Arc<VoicePack>>,
}

fn cache() -> &'static Mutex<Option<Loaded>> {
    static C: OnceLock<Mutex<Option<Loaded>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// A neural voice ready to speak.
pub struct NeuralVoice {
    info: VoiceInfo,
    model: Arc<Kokoro>,
    lexicon: Arc<Lexicon>,
    pack: Arc<VoicePack>,
}

/// The loaded package for `dir`, loading it first when another (or none) is held.
fn loaded_for<'a>(guard: &'a mut Option<Loaded>, dir: &Path) -> Result<&'a mut Loaded, TtsError> {
    if guard.as_ref().is_none_or(|l| l.dir != dir) {
        let model = Arc::new(Kokoro::load(dir)?);
        let bytes = std::fs::read(dir.join("cmudict.dict")).map_err(|e| TtsError::Model(format!("cmudict.dict: {e}")))?;
        let lexicon = Arc::new(Lexicon::parse_cmudict(&bytes).map_err(|e| TtsError::Model(format!("cmudict.dict: {e}")))?);
        *guard = Some(Loaded { dir: dir.to_path_buf(), model, lexicon, packs: HashMap::new() });
    }
    guard.as_mut().ok_or_else(|| TtsError::Model("voice package not loaded".into()))
}

/// The dictionary of the package in `dir`, without loading the voice model (which takes seconds):
/// the one the voices already hold when there is one, else read once and kept.
fn lexicon_only(dir: &Path) -> Result<Arc<Lexicon>, TtsError> {
    static ALONE: OnceLock<Mutex<Option<(PathBuf, Arc<Lexicon>)>>> = OnceLock::new();
    if let Some(l) = cache().lock().unwrap_or_else(PoisonError::into_inner).as_ref().filter(|l| l.dir == dir) {
        return Ok(l.lexicon.clone());
    }
    let mut alone = ALONE.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((d, l)) = alone.as_ref()
        && d == dir
    {
        return Ok(l.clone());
    }
    let bytes = std::fs::read(dir.join("cmudict.dict")).map_err(|e| TtsError::Model(format!("cmudict.dict: {e}")))?;
    let lexicon = Arc::new(Lexicon::parse_cmudict(&bytes).map_err(|e| TtsError::Model(format!("cmudict.dict: {e}")))?);
    *alone = Some((dir.to_path_buf(), lexicon.clone()));
    Ok(lexicon)
}

/// The words of `text` the voices would guess at; see [`crate::check_words`].
pub fn check_words(text: &str, models_dir: &Path, pronounce: &Pronunciations) -> Result<Vec<UnknownWord>, TtsError> {
    crate::check_text(text)?;
    if !catalog::installed(models_dir) {
        return Err(TtsError::NotInstalled);
    }
    let lexicon = lexicon_only(&catalog::package_dir(models_dir))?;
    let found = filmcraft_tts_text::unknown_words(text, Lang::EnUs, &lexicon, &overrides_of(pronounce)).map_err(|e| TtsError::Invalid(e.to_string()))?;
    Ok(found.into_iter().map(|u| UnknownWord { word: u.word, count: u.count }).collect())
}

fn overrides_of(p: &Pronunciations) -> Overrides {
    Overrides(p.0.clone())
}

/// Load (or reuse) the package in `models_dir` and the voice `id`.
pub fn load(id: &str, models_dir: &Path) -> Result<NeuralVoice, TtsError> {
    let v = catalog::find(id).ok_or_else(|| TtsError::UnknownVoice(id.to_string()))?;
    if !catalog::installed(models_dir) {
        return Err(TtsError::NotInstalled);
    }
    let dir = catalog::package_dir(models_dir);
    let mut guard = cache().lock().unwrap_or_else(PoisonError::into_inner);
    let loaded = loaded_for(&mut guard, &dir)?;
    let pack = match loaded.packs.get(v.file) {
        Some(p) => p.clone(),
        None => {
            let p = Arc::new(VoicePack::load(&dir.join(v.file))?);
            loaded.packs.insert(v.file, p.clone());
            p
        }
    };
    Ok(NeuralVoice { info: VoiceInfo { installed: true, ..v.info }, model: loaded.model.clone(), lexicon: loaded.lexicon.clone(), pack })
}

impl Voice for NeuralVoice {
    fn info(&self) -> VoiceInfo {
        self.info
    }

    fn synthesize(&self, text: &str, params: &Params) -> Result<Audio, TtsError> {
        self.synthesize_with(text, params, &Pronunciations::default())
    }

    fn synthesize_with(&self, text: &str, params: &Params, pronounce: &Pronunciations) -> Result<Audio, TtsError> {
        check_text(text)?;
        params.check()?;
        let chunks = filmcraft_tts_text::phonemize(text, Lang::EnUs, &self.lexicon, &overrides_of(pronounce)).map_err(|e| TtsError::Invalid(e.to_string()))?;
        let rate = KOKORO_RATE;
        let max = (MAX_SECONDS * f64::from(rate)) as usize;
        let mut out: Vec<f32> = Vec::new();
        let mut spoke = false;
        for (i, c) in chunks.iter().enumerate() {
            match c {
                Chunk::Pause { millis } => {
                    let n = (u64::from(*millis) * u64::from(rate) / 1000) as usize;
                    out.resize(out.len().saturating_add(n), 0.0);
                }
                Chunk::Phonemes(p) => {
                    if self.model.ids(p).is_empty() {
                        continue;
                    }
                    if spoke {
                        out.resize(out.len().saturating_add((SENTENCE_GAP_MS * rate / 1000) as usize), 0.0);
                    }
                    let s = self.model.speak(p, &self.pack, params.pace, i as u64 + 1)?;
                    out.extend_from_slice(&s);
                    spoke = true;
                }
            }
            if out.len() > max {
                return Err(TtsError::AudioTooLong);
            }
        }
        if !spoke {
            return Err(TtsError::Empty);
        }
        if params.semitones.abs() > 1e-6 {
            out = pitch_shift(out, params.semitones, rate);
        }
        // headroom: never clip
        let peak = out.iter().fold(0f32, |m, x| m.max(x.abs()));
        if peak > 0.98 {
            let g = 0.98 / peak;
            out.iter_mut().for_each(|x| *x *= g);
        }
        Ok(Audio { samples: out, sample_rate: rate })
    }
}

/// Shift pitch by `semitones` keeping the length (latency compensated).
fn pitch_shift(mut x: Vec<f32>, semitones: f64, rate: u32) -> Vec<f32> {
    let mut ps = filmcraft_audio_dsp::effects::PitchShifter::new(rate as f32, 1);
    ps.set_param("semitones", semitones as f32);
    ps.reset();
    let lat = ps.latency();
    let n = x.len();
    x.resize(n + lat, 0.0);
    for block in x.chunks_mut(4096) {
        ps.process(&mut [block]);
    }
    x.drain(..lat.min(x.len()));
    x.truncate(n);
    x
}
