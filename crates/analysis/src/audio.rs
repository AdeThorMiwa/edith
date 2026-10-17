//! Measures of a stretch of audio ([`analyze_audio`] → [`AudioStats`]).
//!
//! The input is interleaved `f32` samples, as the mixer produces them. Samples that are not finite
//! count as silence and everything is limited to ±16 before it is measured, so no input poisons a
//! filter. Loudness is the BS.1770 / EBU R128 meter of `filmcraft-audio-dsp` (K-weighting, gates,
//! loudness range, 4× oversampled true peak): this module does not reimplement it.
//!
//! | measure | what it is |
//! |---|---|
//! | `integrated_lufs`, `loudness_range_lu`, `true_peak_dbtp`, `sample_peak_dbfs` | EBU R128; `None` when the audio is too quiet or too short to have one |
//! | `silence_share` | the share of 50 ms windows whose level is below −60 dBFS |
//! | `spectral` | the share of the energy in a low (to 250 Hz), mid (to 4 kHz) and high band, and the energy-weighted centre (centroid) in Hz |
//! | `kind` | a rough guess, speech, music, ambience or silence, with the score of each |
//! | `tempo` | a constant-tempo beat grid when the audio has a steady pulse: tempo in BPM and where the beats fall |
//!
//! The guess and the beat grid are heuristics on purpose small enough to explain: speech has pauses
//! and a 3–6 Hz rhythm of syllables, music has a steady pulse and no pauses, ambience is noise-like
//! with neither. They are a starting point for the coach, not a classifier to trust on its own.

use filmcraft_audio_dsp::fft::Fft;
use filmcraft_audio_dsp::loudness::LoudnessMeter;
use serde::{Deserialize, Serialize};

use crate::{AnalysisError, Result};

/// Bumped whenever a number in [`AudioStats`] could change for the same audio.
pub const AUDIO_VERSION: u32 = 1;
pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 384_000;
pub const MAX_CHANNELS: u16 = 16;
/// Loudness and silence cover everything given; the pause and rhythm measures look at the first
/// hour, and the beat grid at the first ten minutes.
const ENVELOPE_SECONDS: usize = 3600;
const BEAT_SECONDS: usize = 600;
/// The most spectrum frames averaged; they are spread evenly over the audio.
const SPECTRUM_FRAMES: usize = 4000;
const SPECTRUM_N: usize = 2048;
const FLUX_N: usize = 1024;
const FLUX_HOP: usize = 512;
const SILENCE_DB: f64 = -60.0;
const MAX_BEATS: usize = 20_000;

/// A validated stretch of audio: interleaved samples.
#[derive(Clone, Copy, Debug)]
pub struct AudioBuffer<'a> {
    sample_rate: u32,
    channels: usize,
    samples: &'a [f32],
}

impl<'a> AudioBuffer<'a> {
    /// Checks the format; a trailing part of a frame is ignored.
    pub fn new(sample_rate: u32, channels: u16, samples: &'a [f32]) -> Result<Self> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) || channels == 0 || channels > MAX_CHANNELS {
            return Err(AnalysisError::BadAudioFormat { sample_rate, channels });
        }
        let channels = usize::from(channels);
        let frames = samples.len() / channels;
        if frames == 0 {
            return Err(AnalysisError::Empty);
        }
        Ok(AudioBuffer { sample_rate, channels, samples: samples.get(..frames * channels).unwrap_or(samples) })
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    pub fn duration_seconds(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate)
    }

    fn clean(s: f32) -> f32 {
        if s.is_finite() { s.clamp(-16.0, 16.0) } else { 0.0 }
    }

    /// The mean of the channels at `frame`, 0 past the end.
    fn mono(&self, frame: usize) -> f32 {
        let Some(s) = self.samples.get(frame * self.channels..(frame + 1) * self.channels) else { return 0.0 };
        s.iter().map(|v| Self::clean(*v)).sum::<f32>() / self.channels as f32
    }

    /// The mean square over all channels of frames `from..to`.
    fn mean_square(&self, from: usize, to: usize) -> f64 {
        let to = to.min(self.frames());
        let Some(s) = self.samples.get(from * self.channels..to * self.channels) else { return 0.0 };
        if s.is_empty() {
            return 0.0;
        }
        s.iter().map(|v| f64::from(Self::clean(*v)).powi(2)).sum::<f64>() / s.len() as f64
    }
}

/// The share of the energy in three bands, and the energy-weighted centre.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpectralBalance {
    /// 20 to 250 Hz.
    pub low: f32,
    /// 250 Hz to 4 kHz.
    pub mid: f32,
    /// 4 kHz up to 20 kHz (or the top of the audio).
    pub high: f32,
    /// Hz; 0 for silence.
    pub centroid_hz: f32,
}

/// How much each guess fits, 0..1 (they are not shares: they need not add up to 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KindScores {
    pub speech: f32,
    pub music: f32,
    pub ambience: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioKind {
    Silence,
    Speech,
    Music,
    Ambience,
}

/// A constant-tempo beat grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tempo {
    pub bpm: f64,
    /// 0..1: how strongly the audio repeats at this tempo.
    pub confidence: f32,
    /// Seconds from the start of the audio, at most 20,000 of them.
    pub beats: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStats {
    pub duration_seconds: f64,
    pub integrated_lufs: Option<f64>,
    pub loudness_range_lu: Option<f64>,
    pub true_peak_dbtp: Option<f64>,
    pub sample_peak_dbfs: Option<f64>,
    pub silence_share: f32,
    pub spectral: SpectralBalance,
    pub kind: AudioKind,
    pub scores: KindScores,
    pub tempo: Option<Tempo>,
}

fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

fn db(power: f64) -> f64 {
    10.0 * power.max(1e-20).log10()
}

/// Measures audio given as interleaved `f32`; see the module docs for what each number is.
pub fn analyze_audio(sample_rate: u32, channels: u16, samples: &[f32]) -> Result<AudioStats> {
    let buf = AudioBuffer::new(sample_rate, channels, samples)?;
    Ok(analyze(&buf))
}

fn loudness(buf: &AudioBuffer) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    let mut meter = LoudnessMeter::new(f64::from(buf.sample_rate), buf.channels);
    let chunk = usize::try_from(buf.sample_rate).unwrap_or(48_000) * buf.channels;
    let mut clean = Vec::with_capacity(chunk);
    for part in buf.samples.chunks(chunk.max(buf.channels)) {
        clean.clear();
        clean.extend(part.iter().map(|v| AudioBuffer::clean(*v)));
        meter.process_interleaved(&clean);
    }
    let s = meter.summary();
    // the range is only meaningful when there was enough programme for the meter to gate
    let range = finite(s.loudness_range_lu).filter(|_| finite(s.integrated_lufs).is_some());
    (finite(s.integrated_lufs), range, finite(s.true_peak_dbtp), finite(s.sample_peak_dbfs))
}

/// The share of 50 ms windows quieter than −60 dBFS.
fn silence_share(buf: &AudioBuffer) -> f32 {
    let win = (buf.sample_rate as usize / 20).max(1);
    let frames = buf.frames();
    if frames < win {
        return if db(buf.mean_square(0, frames)) < SILENCE_DB { 1.0 } else { 0.0 };
    }
    let n = frames / win;
    let quiet = (0..n).filter(|i| db(buf.mean_square(i * win, (i + 1) * win)) < SILENCE_DB).count();
    quiet as f32 / n as f32
}

/// The mean square in every 10 ms of the first hour.
fn envelope(buf: &AudioBuffer) -> Vec<f32> {
    let hop = (buf.sample_rate as usize / 100).max(1);
    let n = (buf.frames() / hop).min(ENVELOPE_SECONDS * 100);
    (0..n).map(|i| buf.mean_square(i * hop, (i + 1) * hop) as f32).collect()
}

fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos() as f32).collect()
}

struct Spectrum {
    balance: SpectralBalance,
    /// Mean spectral flatness (1 is white noise, 0 a pure tone) of the frames that are not silent.
    flatness: f32,
}

fn spectrum(buf: &AudioBuffer) -> Spectrum {
    let fft = Fft::new(SPECTRUM_N);
    let window = hann(SPECTRUM_N);
    let frames = buf.frames();
    let count = (frames / SPECTRUM_N).clamp(1, SPECTRUM_FRAMES);
    let stride = (frames.saturating_sub(SPECTRUM_N) / count).max(SPECTRUM_N);
    let bin_hz = f64::from(buf.sample_rate) / SPECTRUM_N as f64;
    let (mut re, mut im) = (vec![0f32; SPECTRUM_N], vec![0f32; SPECTRUM_N]);
    let mut mean_power = vec![0f64; SPECTRUM_N / 2];
    let (mut used, mut flat_sum) = (0usize, 0f64);
    for k in 0..count {
        let start = k * stride;
        if start >= frames {
            break;
        }
        for (i, (r, w)) in re.iter_mut().zip(&window).enumerate() {
            *r = buf.mono(start + i) * w;
        }
        im.iter_mut().for_each(|v| *v = 0.0);
        let energy = re.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / SPECTRUM_N as f64;
        if db(energy) < SILENCE_DB {
            continue;
        }
        fft.forward(&mut re, &mut im);
        let power: Vec<f64> = re.iter().zip(&im).take(SPECTRUM_N / 2).map(|(a, b)| f64::from(*a).powi(2) + f64::from(*b).powi(2)).collect();
        // flatness over 100 Hz to 8 kHz
        let (lo, hi) = ((100.0 / bin_hz) as usize, ((8000.0 / bin_hz) as usize).min(power.len()));
        if let Some(band) = power.get(lo..hi).filter(|b| b.len() > 4) {
            let log_mean = band.iter().map(|p| p.max(1e-20).ln()).sum::<f64>() / band.len() as f64;
            let mean = band.iter().sum::<f64>() / band.len() as f64;
            if mean > 0.0 {
                flat_sum += (log_mean.exp() / mean).clamp(0.0, 1.0);
            }
        }
        for (m, p) in mean_power.iter_mut().zip(&power) {
            *m += p;
        }
        used += 1;
    }
    if used == 0 {
        return Spectrum { balance: SpectralBalance::default(), flatness: 0.0 };
    }
    let top = (20_000.0 / bin_hz).min(mean_power.len() as f64) as usize;
    let band = |from_hz: f64, to_hz: f64| -> f64 {
        let (a, b) = ((from_hz / bin_hz).ceil() as usize, ((to_hz / bin_hz) as usize).min(top));
        mean_power.get(a..b.max(a)).map_or(0.0, |s| s.iter().sum())
    };
    let (low, mid, high) = (band(20.0, 250.0), band(250.0, 4000.0), band(4000.0, 20_000.0));
    let total = low + mid + high;
    let weighted: f64 = mean_power.iter().take(top).enumerate().map(|(i, p)| i as f64 * bin_hz * p).sum();
    let all: f64 = mean_power.iter().take(top).sum();
    let balance = if total > 0.0 {
        SpectralBalance {
            low: (low / total) as f32,
            mid: (mid / total) as f32,
            high: (high / total) as f32,
            centroid_hz: if all > 0.0 { (weighted / all) as f32 } else { 0.0 },
        }
    } else {
        SpectralBalance::default()
    };
    Spectrum { balance, flatness: (flat_sum / used as f64) as f32 }
}

/// The share of 10 ms frames whose level is under a twentieth of the mean (the pauses).
fn pause_ratio(env: &[f32]) -> f32 {
    let mean = env.iter().map(|v| f64::from(*v)).sum::<f64>() / env.len().max(1) as f64;
    if mean <= 0.0 {
        return 0.0;
    }
    env.iter().filter(|v| f64::from(**v) < mean * 0.05).count() as f32 / env.len() as f32
}

/// How much of the envelope's fluctuation lies at 3–6 Hz (syllables) rather than 0.5–20 Hz overall,
/// averaged over up to 24 segments of 10.24 s.
fn syllable_share(env: &[f32]) -> f32 {
    const N: usize = 1024;
    let amp: Vec<f32> = env.iter().map(|v| v.max(0.0).sqrt()).collect();
    if amp.len() < 200 {
        return 0.0;
    }
    let fft = Fft::new(N);
    let window = hann(N);
    let segments = (amp.len() / N).clamp(1, 24);
    let stride = (amp.len().saturating_sub(N) / segments).max(1);
    let (mut band, mut total) = (0f64, 0f64);
    let (mut re, mut im) = (vec![0f32; N], vec![0f32; N]);
    for k in 0..segments {
        let start = (k * stride).min(amp.len().saturating_sub(1));
        let part = amp.get(start..(start + N).min(amp.len())).unwrap_or(&[]);
        let mean = part.iter().sum::<f32>() / part.len().max(1) as f32;
        for (i, (r, w)) in re.iter_mut().zip(&window).enumerate() {
            *r = part.get(i).map_or(0.0, |v| v - mean) * w;
        }
        im.iter_mut().for_each(|v| *v = 0.0);
        fft.forward(&mut re, &mut im);
        // 100 Hz envelope rate: bin width 100/1024 Hz
        let hz = |f: f64| (f * N as f64 / 100.0) as usize;
        let power = |i: usize| f64::from(re.get(i).copied().unwrap_or(0.0)).powi(2) + f64::from(im.get(i).copied().unwrap_or(0.0)).powi(2);
        band += (hz(3.0)..hz(6.0)).map(power).sum::<f64>();
        total += (hz(0.5)..hz(20.0)).map(power).sum::<f64>();
    }
    if total > 0.0 { (band / total) as f32 } else { 0.0 }
}

fn clamp01(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// The onset strength of every 512 samples (half-wave rectified change of the log spectrum) of the
/// first ten minutes, with the slow average taken out.
fn onset_envelope(buf: &AudioBuffer) -> Vec<f32> {
    let fft = Fft::new(FLUX_N);
    let window = hann(FLUX_N);
    let limit = buf.frames().min(BEAT_SECONDS * buf.sample_rate as usize);
    let n = limit.saturating_sub(FLUX_N) / FLUX_HOP;
    let (mut re, mut im) = (vec![0f32; FLUX_N], vec![0f32; FLUX_N]);
    let mut prev = vec![0f32; FLUX_N / 2];
    let mut flux = Vec::with_capacity(n);
    for k in 0..n {
        for (i, (r, w)) in re.iter_mut().zip(&window).enumerate() {
            *r = buf.mono(k * FLUX_HOP + i) * w;
        }
        im.iter_mut().for_each(|v| *v = 0.0);
        fft.forward(&mut re, &mut im);
        let mut sum = 0f32;
        for (p, (a, b)) in prev.iter_mut().zip(re.iter().zip(&im)).take(FLUX_N / 2) {
            let mag = (1.0 + 100.0 * (a * a + b * b).sqrt() / FLUX_N as f32).ln();
            sum += (mag - *p).max(0.0);
            *p = mag;
        }
        flux.push(sum);
    }
    // take out the local average (about half a second)
    let w = (buf.sample_rate as usize / FLUX_HOP / 2).max(2);
    let mut out = Vec::with_capacity(flux.len());
    for i in 0..flux.len() {
        let (lo, hi) = (i.saturating_sub(w), (i + w + 1).min(flux.len()));
        let local = flux.get(lo..hi).map_or(0.0, |s| s.iter().sum::<f32>() / s.len().max(1) as f32);
        out.push((flux.get(i).copied().unwrap_or(0.0) - local).max(0.0));
    }
    out
}

/// A constant-tempo grid from the autocorrelation of the onset strength, 60 to 200 BPM, with a mild
/// preference for tempi near 120 where octaves tie.
fn tempo(buf: &AudioBuffer, onset: &[f32]) -> Option<Tempo> {
    // a little smoothing, so a pulse that falls between two frames still lines up with itself
    let smooth: Vec<f32> = (0..onset.len())
        .map(|i| {
            0.25 * onset.get(i.wrapping_sub(1)).copied().unwrap_or(0.0)
                + 0.5 * onset.get(i).copied().unwrap_or(0.0)
                + 0.25 * onset.get(i + 1).copied().unwrap_or(0.0)
        })
        .collect();
    let onset = smooth.as_slice();
    let rate = f64::from(buf.sample_rate) / FLUX_HOP as f64;
    if onset.len() < (rate * 4.0) as usize {
        return None;
    }
    let energy: f64 = onset.iter().map(|v| f64::from(*v).powi(2)).sum();
    if energy <= 1e-9 {
        return None;
    }
    let (min_lag, max_lag) = ((rate * 60.0 / 200.0).floor().max(2.0) as usize, (rate * 60.0 / 60.0).ceil() as usize);
    let corr = |lag: usize| -> f64 {
        let n = onset.len().saturating_sub(lag);
        if n == 0 {
            return 0.0;
        }
        let s: f64 = onset.iter().zip(onset.iter().skip(lag)).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
        // the overlap shrinks with the lag: make lags comparable
        s / energy * onset.len() as f64 / n as f64
    };
    let lags: Vec<(usize, f64)> = (min_lag..=max_lag).map(|l| (l, corr(l))).collect();
    let mean = lags.iter().map(|(_, c)| *c).sum::<f64>() / lags.len().max(1) as f64;
    let weight = |lag: usize| {
        let bpm = 60.0 * rate / lag as f64;
        let octaves = (bpm / 120.0).log2();
        (-0.5 * (octaves / 0.9).powi(2)).exp()
    };
    let (best_lag, best) = lags.iter().copied().max_by(|a, b| (a.1 * weight(a.0)).total_cmp(&(b.1 * weight(b.0))))?;
    // walk uphill to the top of the peak, then refine between lags
    let at = |l: usize| lags.get(l.wrapping_sub(min_lag)).map_or(0.0, |x| x.1);
    // a pulse repeats at twice its period too: when the faster tempo's own peak is nearly as strong,
    // that is the pulse (a click track at 150 is not 75)
    let mut best_lag = best_lag;
    while best_lag / 2 >= min_lag {
        let half = (best_lag / 2).saturating_sub(1).max(min_lag)..=best_lag / 2 + 1;
        let Some((l, c)) = half.map(|l| (l, at(l))).max_by(|a, b| a.1.total_cmp(&b.1)) else { break };
        if c >= 0.7 * best {
            best_lag = l;
        } else {
            break;
        }
    }
    let mut lag = best_lag;
    while lag > min_lag && at(lag - 1) > at(lag) {
        lag -= 1;
    }
    while lag < max_lag && at(lag + 1) > at(lag) {
        lag += 1;
    }
    let (a, b, c) = (at(lag.saturating_sub(1)), at(lag), at(lag + 1));
    let denom = a - 2.0 * b + c;
    let shift = if denom.abs() > 1e-12 { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) } else { 0.0 };
    let period = lag as f64 + shift;
    let confidence = ((at(lag) - mean) / (at(lag).abs() + 1e-9)).clamp(0.0, 1.0) as f32 * at(lag).clamp(0.0, 1.0) as f32;
    if period <= 1.0 {
        return None;
    }
    // the phase: where, within one period, the onsets pile up
    let steps = period.ceil() as usize;
    let score = |phase: f64| -> f64 {
        let mut s = 0.0;
        let mut t = phase;
        while (t as usize) < onset.len() {
            s += f64::from(onset.get(t.round() as usize).copied().unwrap_or(0.0));
            t += period;
        }
        s
    };
    let phase = (0..steps).map(|i| i as f64).max_by(|a, b| score(*a).total_cmp(&score(*b)))?;
    let beats: Vec<f64> =
        (0..MAX_BEATS).map(|k| (phase + k as f64 * period) / rate).take_while(|t| *t <= buf.duration_seconds().min(BEAT_SECONDS as f64)).collect();
    Some(Tempo { bpm: 60.0 * rate / period, confidence, beats })
}

fn kind(silence: f32, pause: f32, syllables: f32, flatness: f32, tempo_confidence: f32) -> (AudioKind, KindScores) {
    if silence >= 0.98 {
        return (AudioKind::Silence, KindScores::default());
    }
    let sat = |v: f32, full: f32| clamp01(v / full);
    // speech: pauses between words and a 3-6 Hz rhythm of syllables
    let speech = 0.55 * sat(pause, 0.15) + 0.45 * sat(syllables - 0.10, 0.25);
    // music: a steady pulse, no pauses, tonal rather than noise-like
    let music = 0.5 * sat(tempo_confidence, 0.5) + 0.25 * (1.0 - sat(pause, 0.05)) + 0.25 * (1.0 - sat(flatness, 0.5));
    // ambience: noise-like, with neither pauses nor pulse nor syllables
    let ambience =
        0.45 * sat(flatness, 0.3) + 0.2 * (1.0 - sat(pause, 0.05)) + 0.2 * (1.0 - sat(tempo_confidence, 0.4)) + 0.15 * (1.0 - sat(syllables - 0.10, 0.25));
    let scores = KindScores { speech: clamp01(speech), music: clamp01(music), ambience: clamp01(ambience) };
    let best = if scores.speech >= scores.music && scores.speech >= scores.ambience {
        AudioKind::Speech
    } else if scores.music >= scores.ambience {
        AudioKind::Music
    } else {
        AudioKind::Ambience
    };
    (best, scores)
}

fn analyze(buf: &AudioBuffer) -> AudioStats {
    let (integrated, range, true_peak, sample_peak) = loudness(buf);
    let silence = silence_share(buf);
    let spec = spectrum(buf);
    let env = envelope(buf);
    let onset = onset_envelope(buf);
    let tempo = tempo(buf, &onset);
    let confidence = tempo.as_ref().map_or(0.0, |t| t.confidence);
    let (kind, scores) = kind(silence, pause_ratio(&env), syllable_share(&env), spec.flatness, confidence);
    AudioStats {
        duration_seconds: buf.duration_seconds(),
        integrated_lufs: integrated,
        loudness_range_lu: range,
        true_peak_dbtp: true_peak,
        sample_peak_dbfs: sample_peak,
        silence_share: silence,
        spectral: spec.balance,
        kind,
        scores,
        tempo: tempo.filter(|t| t.confidence >= 0.15),
    }
}
