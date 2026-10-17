//! Tests of the audio measures on synthetic audio with known properties, and on hostile input.

use std::f64::consts::PI;

use crate::audio::{AudioKind, analyze_audio};
use crate::{AnalysisError, AudioStats};

const SR: u32 = 48_000;

/// A repeatable stream of noise.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn signed(&mut self) -> f64 {
        self.next() * 2.0 - 1.0
    }
}

fn render(seconds: f64, mut f: impl FnMut(f64) -> f64) -> Vec<f32> {
    (0..(seconds * f64::from(SR)) as usize).map(|i| f(i as f64 / f64::from(SR)) as f32).collect()
}

fn sine(hz: f64, amp: f64, seconds: f64) -> Vec<f32> {
    render(seconds, |t| amp * (2.0 * PI * hz * t).sin())
}

fn measure(samples: &[f32]) -> AudioStats {
    analyze_audio(SR, 1, samples).unwrap()
}

#[test]
fn a_sine_of_known_level_has_the_loudness_the_standard_gives() {
    // BS.1770: a full-scale 997 Hz sine on one channel is -3.01 LUFS; 0.1 is 20 dB lower
    let tone: Vec<f32> = render(6.0, |t| 0.1 * (2.0 * PI * 997.0 * t).sin());
    let s = measure(&tone);
    assert!((s.integrated_lufs.unwrap() + 23.01).abs() < 0.2, "{:?}", s.integrated_lufs);
    assert!((s.sample_peak_dbfs.unwrap() + 20.0).abs() < 0.1, "{:?}", s.sample_peak_dbfs);
    assert!((s.true_peak_dbtp.unwrap() + 20.0).abs() < 0.4, "{:?}", s.true_peak_dbtp);
    assert!(s.loudness_range_lu.unwrap() < 0.5, "a steady tone has no range: {:?}", s.loudness_range_lu);
    assert!((s.duration_seconds - 6.0).abs() < 1e-9);
    assert_eq!(s.silence_share, 0.0);
    // the same on two channels is 3 LU louder
    let stereo: Vec<f32> = tone.iter().flat_map(|v| [*v, *v]).collect();
    let two = analyze_audio(SR, 2, &stereo).unwrap();
    assert!((two.integrated_lufs.unwrap() - s.integrated_lufs.unwrap() - 3.01).abs() < 0.1);
}

#[test]
fn a_programme_that_gets_quieter_in_steps_has_a_loudness_range() {
    let mut v = Vec::new();
    for amp in [0.1, 0.03, 0.01] {
        v.extend(sine(997.0, amp, 12.0));
    }
    let s = measure(&v);
    assert!(s.loudness_range_lu.unwrap() > 8.0, "{:?}", s.loudness_range_lu);
}

#[test]
fn silence_has_no_loudness_and_a_half_silent_clip_is_half_silent() {
    let s = measure(&vec![0.0; SR as usize * 3]);
    assert_eq!((s.silence_share, s.kind), (1.0, AudioKind::Silence));
    assert_eq!((s.integrated_lufs, s.loudness_range_lu, s.true_peak_dbtp, s.tempo), (None, None, None, None));
    assert_eq!(s.spectral.centroid_hz, 0.0);
    let mut v = sine(440.0, 0.2, 2.0);
    v.extend(vec![0.0; SR as usize * 2]);
    let h = measure(&v);
    assert!((h.silence_share - 0.5).abs() < 0.03, "{}", h.silence_share);
    // very quiet is silent too: -70 dBFS RMS
    let hiss = measure(&render(2.0, |t| 0.0003 * (2.0 * PI * 300.0 * t).sin()));
    assert!(hiss.silence_share > 0.95, "{}", hiss.silence_share);
}

#[test]
fn the_spectral_balance_follows_where_the_energy_is() {
    let low = measure(&sine(100.0, 0.3, 3.0)).spectral;
    let mid = measure(&sine(1000.0, 0.3, 3.0)).spectral;
    let high = measure(&sine(8000.0, 0.3, 3.0)).spectral;
    assert!(low.low > 0.98 && (low.centroid_hz - 100.0).abs() < 40.0, "{low:?}");
    assert!(mid.mid > 0.98 && (mid.centroid_hz - 1000.0).abs() < 40.0, "{mid:?}");
    assert!(high.high > 0.98 && (high.centroid_hz - 8000.0).abs() < 60.0, "{high:?}");
    for b in [low, mid, high] {
        assert!((b.low + b.mid + b.high - 1.0).abs() < 1e-4);
    }
}

/// Speech-like: a buzzing voice in bursts of syllables, with pauses between words.
fn speechlike(seconds: f64) -> Vec<f32> {
    let mut n = Noise(42);
    let mut shape = vec![0f64; (seconds * 100.0) as usize + 1];
    let (mut t, mut syllables_left) = (0.2, 0);
    while t < seconds {
        if syllables_left == 0 {
            syllables_left = 2 + (n.next() * 4.0) as usize;
            t += 0.25 + n.next() * 0.4;
        }
        let len = 0.15 + n.next() * 0.08;
        let mut x = 0.0;
        while x < len {
            let i = ((t + x) * 100.0) as usize;
            if let Some(s) = shape.get_mut(i) {
                *s = (PI * x / len).sin().powi(2);
            }
            x += 0.01;
        }
        t += len + 0.03;
        syllables_left -= 1;
    }
    render(seconds, |t| {
        let env = shape.get((t * 100.0) as usize).copied().unwrap_or(0.0);
        let voice: f64 = (1..=24).map(|h| (2.0 * PI * 120.0 * f64::from(h) * t).sin() / f64::from(h).sqrt()).sum();
        0.12 * env * voice / 4.0
    })
}

/// Music-like: sustained chords, a bass, a kick every half second and soft hats.
fn musiclike(seconds: f64) -> Vec<f32> {
    let mut n = Noise(8);
    render(seconds, |t| {
        let chord = [[220.0, 277.2, 329.6], [196.0, 246.9, 293.7], [174.6, 220.0, 261.6], [196.0, 246.9, 293.7]][((t / 2.0) as usize) % 4];
        let pad: f64 = chord.iter().map(|f| (2.0 * PI * f * t).sin() + 0.3 * (4.0 * PI * f * t).sin()).sum::<f64>() * 0.05;
        let beat = t % 0.5;
        let kick = (-beat * 22.0).exp() * (2.0 * PI * (55.0 + 90.0 * (-beat * 40.0).exp()) * beat).sin() * 0.5;
        let hat = if (t % 0.25) < 0.03 { n.signed() * 0.05 * (1.0 - (t % 0.25) / 0.03) } else { 0.0 };
        pad + kick + hat
    })
}

/// Ambience: steady low-passed noise that drifts slowly.
fn ambience(seconds: f64) -> Vec<f32> {
    let mut n = Noise(77);
    let mut lp = 0.0;
    render(seconds, |t| {
        lp += 0.2 * (n.signed() - lp);
        0.08 * lp * (1.0 + 0.3 * (2.0 * PI * 0.1 * t).sin())
    })
}

#[test]
fn speech_music_and_ambience_are_told_apart() {
    let (speech, music, amb) = (measure(&speechlike(24.0)), measure(&musiclike(24.0)), measure(&ambience(24.0)));
    assert_eq!(speech.kind, AudioKind::Speech, "{:?}", speech.scores);
    assert_eq!(music.kind, AudioKind::Music, "{:?}", music.scores);
    assert_eq!(amb.kind, AudioKind::Ambience, "{:?}", amb.scores);
    for s in [speech, music, amb] {
        let sc = s.scores;
        assert!([sc.speech, sc.music, sc.ambience].iter().all(|v| (0.0..=1.0).contains(v)));
    }
}

fn clicks(bpm: f64, offset: f64, seconds: f64) -> Vec<f32> {
    let mut n = Noise(3);
    let period = 60.0 / bpm;
    render(seconds, |t| {
        let since = (t - offset).rem_euclid(period);
        if t >= offset && since < 0.02 { n.signed() * 0.6 * (1.0 - since / 0.02) } else { 0.0 }
    })
}

#[test]
fn a_steady_pulse_gives_its_tempo_and_where_the_beats_fall() {
    for (bpm, offset) in [(120.0, 0.13), (90.0, 0.4), (150.0, 0.0), (75.0, 0.2), (100.0, 0.31)] {
        let s = measure(&clicks(bpm, offset, 16.0));
        let t = s.tempo.unwrap_or_else(|| panic!("no tempo for {bpm}: {:?}", s.scores));
        assert!((t.bpm - bpm).abs() < 1.5, "{bpm}: got {}", t.bpm);
        assert!(t.confidence > 0.3, "{bpm}: {}", t.confidence);
        let first = t.beats[0];
        let period = 60.0 / bpm;
        let off = (first - offset).rem_euclid(period);
        assert!(off.min(period - off) < 0.04, "{bpm}: first beat {first}, true offset {offset}");
        assert!(t.beats.windows(2).all(|w| w[1] > w[0]));
        assert!(t.beats.len() as f64 > 16.0 / period - 3.0, "{bpm}: {} beats", t.beats.len());
    }
    let music = measure(&musiclike(20.0)).tempo.unwrap();
    assert!((music.bpm - 120.0).abs() < 2.0, "{}", music.bpm);
}

#[test]
fn audio_with_no_pulse_has_no_beat_grid() {
    assert!(measure(&ambience(20.0)).tempo.is_none());
    assert!(measure(&sine(300.0, 0.2, 20.0)).tempo.is_none());
    assert!(measure(&clicks(120.0, 0.0, 2.0)).tempo.is_none(), "two seconds is too short to call a tempo");
}

#[test]
fn hostile_formats_and_samples_are_refused_or_survived() {
    let tone = sine(440.0, 0.1, 1.0);
    for (sr, ch) in [(0, 1), (7_999, 1), (384_001, 1), (u32::MAX, 2), (48_000, 0), (48_000, 17), (48_000, u16::MAX)] {
        assert_eq!(analyze_audio(sr, ch, &tone), Err(AnalysisError::BadAudioFormat { sample_rate: sr, channels: ch }));
    }
    assert_eq!(analyze_audio(SR, 1, &[]), Err(AnalysisError::Empty));
    assert_eq!(analyze_audio(SR, 2, &[0.5]), Err(AnalysisError::Empty), "less than one frame");
    // NaN, infinity and absurd values count as silence or are limited; the numbers stay finite
    let mut bad = tone.clone();
    for (i, v) in bad.iter_mut().enumerate().take(20_000) {
        *v = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30][i % 5];
    }
    let s = measure(&bad);
    assert!(s.silence_share.is_finite() && s.spectral.low.is_finite() && s.scores.speech.is_finite());
    assert!(s.integrated_lufs.is_none_or(f64::is_finite));
    let all_nan = measure(&vec![f32::NAN; 48_000]);
    assert_eq!(all_nan.kind, AudioKind::Silence);
    // tiny, odd and wide inputs
    for (sr, ch, n) in [(48_000, 1, 1), (48_000, 1, 100), (8_000, 1, 17), (384_000, 1, 5000), (48_000, 16, 16 * 3000), (44_100, 3, 3 * 50_001 + 2)] {
        let mut noise = Noise(9);
        let v: Vec<f32> = (0..n).map(|_| noise.signed() as f32 * 0.3).collect();
        let s = analyze_audio(sr, ch, &v).unwrap_or_else(|e| panic!("{sr} {ch} {n}: {e}"));
        assert!(s.duration_seconds > 0.0 && s.silence_share.is_finite());
        assert!([s.spectral.low, s.spectral.mid, s.spectral.high, s.spectral.centroid_hz].iter().all(|v| v.is_finite()));
    }
}

#[test]
fn the_same_audio_always_gives_the_same_numbers_and_they_round_trip_as_json() {
    let v = musiclike(8.0);
    let a = measure(&v);
    assert_eq!(a, measure(&v));
    let back: AudioStats = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
    assert_eq!(a, back);
    assert!(serde_json::to_value(&a).unwrap().get("silenceShare").is_some());
}

#[test]
fn the_mix_meter_agrees_with_measuring_the_whole_buffer_however_it_is_fed() {
    use crate::audio::{Mix, MixMeter};
    let mut v = sine(997.0, 0.1, 6.0);
    v.extend(vec![0.0; SR as usize * 2]);
    v.extend(sine(200.0, 0.4, 5.0));
    let whole = analyze_audio(SR, 1, &v).unwrap();
    let reference = Mix::from(&whole);
    for chunk in [v.len(), 48_000, 4_800, 777, 1] {
        let mut m = MixMeter::new(SR, 1).unwrap();
        for part in v.chunks(chunk.max(1)) {
            m.push(part);
        }
        let got = m.finish();
        assert!((got.duration_seconds - reference.duration_seconds).abs() < 1e-9, "{chunk}");
        assert!((got.integrated_lufs.unwrap() - reference.integrated_lufs.unwrap()).abs() < 1e-6, "{chunk}: {got:?} vs {reference:?}");
        assert!((got.loudness_range_lu.unwrap() - reference.loudness_range_lu.unwrap()).abs() < 1e-6, "{chunk}");
        assert!((got.true_peak_dbtp.unwrap() - reference.true_peak_dbtp.unwrap()).abs() < 1e-6, "{chunk}");
        // windows are counted from the start of the stream, however the chunks fall
        assert!((got.silence_share - reference.silence_share).abs() < 1e-6, "{chunk}: {} vs {}", got.silence_share, reference.silence_share);
    }
    // two channels, fed in pieces that cut a frame in half: the stray half frame is ignored
    let stereo: Vec<f32> = v.iter().flat_map(|s| [*s, *s]).collect();
    let mut m = MixMeter::new(SR, 2).unwrap();
    for part in stereo.chunks(10_001) {
        m.push(part);
    }
    let got = m.finish();
    assert!((got.duration_seconds - whole.duration_seconds).abs() < 1.0, "{}", got.duration_seconds);
}

#[test]
fn the_mix_meter_measures_an_hour_without_holding_it_and_survives_hostile_input() {
    use crate::audio::MixMeter;
    // an hour at 8 kHz, one second at a time: nothing is kept
    let second = sine(440.0, 0.05, 1.0);
    let quiet = vec![0.0f32; 8000];
    let mut m = MixMeter::new(8000, 1).unwrap();
    let tone: Vec<f32> = second.iter().step_by(6).copied().collect();
    for i in 0..3600 {
        m.push(if i % 4 == 3 { &quiet } else { &tone });
    }
    let r = m.finish();
    assert!((r.duration_seconds - 3600.0).abs() < 1.0 || r.duration_seconds > 3000.0, "{}", r.duration_seconds);
    assert!((r.silence_share - 0.25).abs() < 0.02, "{}", r.silence_share);
    assert!(r.integrated_lufs.is_some_and(f64::is_finite));
    // formats are checked, and nothing makes it panic
    assert!(MixMeter::new(0, 1).is_err() && MixMeter::new(48_000, 0).is_err() && MixMeter::new(48_000, 99).is_err() && MixMeter::new(u32::MAX, 2).is_err());
    let mut h = MixMeter::new(48_000, 2).unwrap();
    h.push(&[]);
    h.push(&[f32::NAN, f32::INFINITY, -f32::INFINITY, 1e30, -1e30, 0.5, 0.5]);
    let r = h.finish();
    assert!(r.silence_share.is_finite() && r.duration_seconds.is_finite());
    assert!(r.integrated_lufs.is_none_or(f64::is_finite));
    // less than one window is one window; nothing at all is no silence
    let mut short = MixMeter::new(48_000, 1).unwrap();
    short.push(&vec![0.0; 100]);
    assert_eq!(short.finish().silence_share, 1.0);
    assert_eq!(MixMeter::new(48_000, 1).unwrap().finish().silence_share, 0.0);
}
