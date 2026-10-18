//! Does the natural voice's own word timing agree with an independent listener? (fork roadmap F16.2)
//!
//! Speaks a script with a natural voice, has Whisper transcribe the result, matches the words in order
//! and compares each word's start. Needs both downloaded models and both features, and skips otherwise:
//!
//! ```sh
//! FILMCRAFT_MODELS_DIR="$HOME/Library/Application Support/FilmCraft/models" \
//!   cargo test --release -p filmcraft-engine --features neural-voices,whisper --test word_times_vs_whisper -- --nocapture
//! ```
#![cfg(all(feature = "neural-voices", feature = "whisper"))]

use filmcraft_speech::Options;
use filmcraft_tts::{Params, voice_in};

const SCRIPT: &str = "The harbour was quiet that morning. Nobody saw the small boat leave, and nobody asked why. \
    By noon the fog had lifted, and the water lay flat and bright under a pale sky. \
    [pause 1s] Along the quay, the fishermen mended their nets and talked about the weather. \
    One of them looked up, shaded his eyes, and said that the tide had never come in so early. \
    The others laughed, because he said the same thing every year. \
    Still, as the afternoon wore on, a strange silence settled over the town. \
    Even the gulls seemed to be waiting for something.";

/// 24 kHz to 16 kHz: linear up by two, average three, keep every third (Whisper is not fussy).
fn to_16k(x: &[f32]) -> Vec<f32> {
    let up: Vec<f32> = x.iter().flat_map(|w| [*w, *w]).collect();
    up.chunks(3).filter(|c| c.len() == 3).map(|c| (c[0] + c[1] + c[2]) / 3.0).collect()
}

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Pairs of indices (ours, theirs) of the longest run of equal words, in order.
fn match_words(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

#[test]
fn the_voices_word_starts_agree_with_whispers_to_a_fraction_of_a_second() {
    let Some(models) = std::env::var_os("FILMCRAFT_MODELS_DIR").map(std::path::PathBuf::from) else { return };
    let voice = voice_in("kokoro-heart", Some(&models)).expect("the natural voice package");
    let audio = voice.synthesize(SCRIPT, &Params::default()).expect("speech");
    let whisper = filmcraft_speech::load(&models, "whisper-base").expect("whisper-base");
    let heard =
        whisper.transcribe(&to_16k(&audio.samples), &Options { language: Some("en".into()), ..Default::default() }, &mut |_, _| true).expect("transcription");

    let rate = f64::from(audio.sample_rate);
    let ours: Vec<String> = audio.words.iter().map(|w| norm(&w.text)).collect();
    let theirs: Vec<String> = heard.words.iter().map(|w| norm(&w.text)).collect();
    let pairs = match_words(&ours, &theirs);
    assert!(
        pairs.len() * 10 >= ours.len() * 9,
        "Whisper heard what was said: {} of {} words matched\nours:   {ours:?}\ntheirs: {theirs:?}",
        pairs.len(),
        ours.len()
    );

    let mut signed: Vec<f64> = Vec::new();
    let mut starts: Vec<f64> = Vec::new();
    let mut ends: Vec<f64> = Vec::new();
    for (i, j) in &pairs {
        let (w, h) = (&audio.words[*i], &heard.words[*j]);
        signed.push(w.start as f64 / rate - h.start.0 as f64 / 254_016_000_000.0);
        starts.push(signed.last().copied().unwrap_or(0.0).abs());
        ends.push((w.end as f64 / rate - h.end.0 as f64 / 254_016_000_000.0).abs());
    }
    let pct = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(f64::total_cmp);
        v.get(((v.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(f64::NAN)
    };
    // Whisper times a word from where its sound is strongest, which can sit a little off the start: a steady
    // offset says that, and what is left after taking it out says how well the two agree on the rest
    let offset = pct(&mut signed.clone(), 0.5);
    let mut spread: Vec<f64> = signed.iter().map(|d| (d - offset).abs()).collect();
    let (s50, s90) = (pct(&mut spread, 0.5), pct(&mut spread, 0.9));
    let (m50, m90, mx) = (pct(&mut starts, 0.5), pct(&mut starts, 0.9), pct(&mut starts, 1.0));
    let (e50, e90) = (pct(&mut ends, 0.5), pct(&mut ends, 0.9));
    println!(
        "{} words, {} matched, {:.1} s of speech\nstart differences: median {:.0} ms, 90th percentile {:.0} ms, worst {:.0} ms\nsteady offset (ours minus Whisper's): {:.0} ms; around it, median {:.0} ms, 90th percentile {:.0} ms\nend differences:   median {:.0} ms, 90th percentile {:.0} ms",
        ours.len(),
        pairs.len(),
        audio.samples.len() as f64 / rate,
        m50 * 1000.0,
        m90 * 1000.0,
        mx * 1000.0,
        offset * 1000.0,
        s50 * 1000.0,
        s90 * 1000.0,
        e50 * 1000.0,
        e90 * 1000.0
    );
    // The truth that does not depend on Whisper: after a pause the audio is silent until the word begins, so
    // the first moment of real sound is where the word starts. Compare it with the time the voice recorded.
    let win = (rate * 0.010) as usize;
    let rms = |from: usize, to: usize| -> f32 {
        let part = audio.samples.get(from.min(audio.samples.len())..to.min(audio.samples.len())).unwrap_or(&[]);
        (part.iter().map(|x| x * x).sum::<f32>() / part.len().max(1) as f32).sqrt()
    };
    let mut onset: Vec<f64> = Vec::new();
    for (k, w) in audio.words.iter().enumerate().skip(1) {
        let prev_end = audio.words[k - 1].end;
        if (w.start.saturating_sub(prev_end)) as f64 / rate < 0.10 {
            continue;
        }
        let peak = (w.start..w.end).step_by(win.max(1)).map(|a| rms(a, a + win)).fold(0.0f32, f32::max);
        let from = prev_end.saturating_sub((rate * 0.05) as usize);
        let heard = (from..w.end).step_by(win.max(1)).find(|a| rms(*a, *a + win) > peak * 0.15);
        if let Some(a) = heard {
            onset.push((w.start as f64 - a as f64) / rate);
        }
    }
    let mut abs_onset: Vec<f64> = onset.iter().map(|d| d.abs()).collect();
    let (o50, o90) = (pct(&mut abs_onset, 0.5), pct(&mut abs_onset, 0.9));
    let signed_onset = pct(&mut onset.clone(), 0.5);
    println!(
        "{} words that follow a pause: the recorded start against where the sound begins in the audio: median {:.0} ms, 90th percentile {:.0} ms (signed median {:.0} ms; negative means the voice recorded it earlier than the sound)",
        onset.len(),
        o50 * 1000.0,
        o90 * 1000.0,
        signed_onset * 1000.0
    );
    assert!(onset.len() >= 5, "enough words follow a pause to check: {}", onset.len());
    assert!(o50 < 0.08 && o90 < 0.20, "the voice's recorded starts sit on the sound: median {o50:.3} s, 90th percentile {o90:.3} s");
    // and Whisper, once its steady habit is allowed for, tells the same story
    assert!(s50 < 0.10 && s90 < 0.20, "around Whisper's offset: median {s50:.3} s, 90th percentile {s90:.3} s");
    assert!(offset.abs() < 0.25, "Whisper's steady offset {offset:.3} s");
}
