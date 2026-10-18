//! Word times from the natural voice (fork roadmap F16.2). They need the downloaded voice package: set
//! `FILMCRAFT_KOKORO_DIR` to the `kokoro-82m` folder (the one holding `config.json`); they skip otherwise.
#![cfg(feature = "kokoro")]

use filmcraft_tts::{Params, Word, voice_in};

/// None when the voice package is not set up (the test skips); otherwise what the voice made of `text`.
fn try_speak(text: &str, p: Params) -> Option<Result<filmcraft_tts::Audio, filmcraft_tts::TtsError>> {
    let package = std::path::PathBuf::from(std::env::var_os("FILMCRAFT_KOKORO_DIR")?);
    let models = package.parent()?;
    // the variable is set, so a voice that cannot load is a failure, not a skip
    let voice = voice_in("kokoro-heart", Some(models)).unwrap_or_else(|e| panic!("the voice package in {package:?} will not load: {e}"));
    Some(voice.synthesize(text, &p))
}

fn speak(text: &str, p: Params) -> Option<filmcraft_tts::Audio> {
    try_speak(text, p).map(|r| r.unwrap_or_else(|e| panic!("{text:?}: {e}")))
}

fn check_shape(words: &[Word], samples: usize) {
    let mut at = 0;
    for w in words {
        assert!(w.start >= at, "words are in order and do not overlap: {w:?} after {at}");
        assert!(w.start < w.end && w.end <= samples, "inside the audio: {w:?} of {samples}");
        assert!(w.exact);
        at = w.end;
    }
}

#[test]
fn every_word_is_timed_in_order_and_inside_the_audio() {
    let Some(a) = speak("The harbour was quiet. Nobody saw the boat leave, and nobody asked why.", Params::default()) else { return };
    let said: Vec<&str> = a.words.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(said.len(), 13, "{said:?}");
    check_shape(&a.words, a.samples.len());
    assert_eq!(a.words.iter().map(|w| w.sentence).collect::<Vec<_>>(), [0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    // the speech is covered: the first word starts near the start, the last ends near the end
    let rate = f64::from(a.sample_rate);
    assert!(a.words[0].start as f64 / rate < 0.5 && (a.samples.len() - a.words[12].end) as f64 / rate < 0.7, "{:?}", (&a.words[0], &a.words[12]));
    // the gap between the sentences is the voice's breath, not nothing and not seconds
    let gap = (a.words[4].start - a.words[3].end) as f64 / rate;
    assert!((0.05..1.0).contains(&gap), "{gap}");
}

#[test]
fn pauses_in_the_script_are_gaps_between_words_and_numbers_are_spoken() {
    let Some(a) = speak("In 1984 it began. [pause 2s] Then it ended.", Params::default()) else { return };
    let said: Vec<String> = a.words.iter().map(|w| w.text.to_lowercase()).collect();
    assert!(said.iter().any(|w| w.contains("nineteen")), "words are as spoken, not as written: {said:?}");
    check_shape(&a.words, a.samples.len());
    let before = a.words.iter().position(|w| w.text.eq_ignore_ascii_case("began")).expect("began");
    let gap = (a.words[before + 1].start - a.words[before].end) as f64 / f64::from(a.sample_rate);
    assert!((2.0..3.0).contains(&gap), "two seconds of pause: {gap}");
}

#[test]
fn pace_pitch_and_directions_move_the_words_with_the_audio() {
    let text = "One small step for a person. [pace 0.8] One giant leap for everyone.";
    let (Some(plain), Some(fast), Some(low)) =
        (speak(text, Params::default()), speak(text, Params { pace: 1.5, ..Params::default() }), speak(text, Params { semitones: -3.0, ..Params::default() }))
    else {
        return;
    };
    for a in [&plain, &fast, &low] {
        check_shape(&a.words, a.samples.len());
        assert_eq!(a.words.len(), 11);
    }
    let end = |a: &filmcraft_tts::Audio| a.words.last().map_or(0, |w| w.end) as f64 / f64::from(a.sample_rate);
    assert!(end(&fast) < end(&plain) * 0.8, "faster speech ends sooner: {} vs {}", end(&fast), end(&plain));
    // a pitch shift does not change the length, so the words still land inside the audio
    assert!((end(&low) - end(&plain)).abs() < 0.3);
}

#[test]
fn a_take_changes_the_times_and_the_same_take_repeats_them() {
    let text = "Slowly, the light changed over the water.";
    let (Some(a), Some(b), Some(c)) =
        (speak(text, Params { take: 2, ..Params::default() }), speak(text, Params { take: 2, ..Params::default() }), speak(text, Params::default()))
    else {
        return;
    };
    assert_eq!(a.words, b.words, "deterministic");
    assert_ne!(a.words, c.words, "another take is another delivery");
}

#[test]
fn hostile_scripts_give_words_inside_the_audio_or_an_error() {
    for t in ["...", "a", "[pause 1s] x", "Mr. Smith vs. Dr. Jones, Inc. 3.5% 12:30 a.m. e.g. U.S.A.", "😀 hello 😀", &format!("{} end.", "word ".repeat(60))]
    {
        // an error ("nothing to say") is fine; words, when there are any, must still be sound
        if let Some(Ok(a)) = try_speak(t, Params::default()) {
            check_shape(&a.words, a.samples.len());
        }
    }
}
