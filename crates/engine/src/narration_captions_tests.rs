//! Tests of `transcript.fromNarration` and `captions.fromNarration` (fork roadmap F30): the voice's word times become
//! a transcript and captions, in one undo step, without touching a transcript that is already there.

use filmcraft_project::SpokenWord;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::json;

use crate::Session;

fn tmp(name: &str) -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("edith-narrcap-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d.to_string_lossy().into_owned()
}

fn edit() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Cap", "audio": 2, "video": 1})).unwrap();
    s
}

/// A narration whose words are given as (text, start, end), sentences numbered by `.`.
fn narrated(s: &mut Session, words: &[(&str, f64, f64)]) -> filmcraft_project::ItemId {
    let r = s.execute("tts.create", json!({"text": "One two. Three four.", "seconds": 0.0, "dir": tmp("words")})).unwrap();
    let item = filmcraft_project::ItemId(r["item"].as_u64().unwrap());
    let spoken: Vec<SpokenWord> = words
        .iter()
        .map(|(t, a, b)| SpokenWord {
            text: (*t).into(),
            start: Tick((a * TICKS_PER_SECOND as f64) as i64),
            end: Tick((b * TICKS_PER_SECOND as f64) as i64),
            sentence: 0,
            exact: true,
        })
        .collect();
    s.edit("test", |pr, _| {
        pr.narrations.get_mut(&item).unwrap().words = spoken;
        Ok(())
    })
    .unwrap();
    item
}

const SHORT: [(&str, f64, f64); 4] = [("One", 0.1, 0.4), ("two", 0.5, 0.9), ("Three", 1.2, 1.5), ("four", 1.6, 1.9)];

#[test]
fn the_voices_words_become_the_narrations_transcript_and_a_transcript_already_there_is_kept() {
    let mut s = edit();
    let item = narrated(&mut s, &SHORT);
    assert!(!s.project.transcripts.contains_key(&item));
    let r = s.execute("transcript.fromNarration", json!({})).unwrap();
    assert_eq!(r["written"], json!([item.0]), "{r}");
    let t = s.project.transcripts.get(&item).unwrap();
    assert_eq!((t.source.as_str(), t.language.as_str(), t.text().as_str()), ("narration", "en", "One two Three four"));
    // the sequence's transcript has them too, at their times on the timeline
    let words = crate::transcript::sequence_words(&s);
    assert_eq!(words.len(), 4);
    // a second ask writes nothing, and `overwrite` writes it again
    let again = s.execute("transcript.fromNarration", json!({})).unwrap();
    assert_eq!((again["written"].clone(), again["alreadyHadOne"].clone()), (json!([]), json!(1)));
    let forced = s.execute("transcript.fromNarration", json!({"overwrite": true})).unwrap();
    assert_eq!(forced["written"], json!([item.0]));
    // a call that wrote something is one undo step
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.project.transcripts.contains_key(&item), "the call that wrote nothing left no step, so two undos take both writes back");
}

#[test]
fn captions_are_made_from_the_narration_in_one_undo_step_and_fast_ones_are_said() {
    let mut s = edit();
    let item = narrated(&mut s, &SHORT);
    let r = s.execute("captions.fromNarration", json!({})).unwrap();
    assert_eq!(r["captions"], 1, "a pause of 0.3 s does not start a new caption: {r}");
    assert_eq!(r["fast"], 0);
    let track = &s.active_sequence().unwrap().caption_tracks[0];
    assert_eq!(track.name, "Narration");
    assert_eq!(track.captions[0].text.replace('\n', " "), "One two Three four");
    // the transcript was made on the way, and one undo takes captions and transcript together
    assert!(s.project.transcripts.contains_key(&item));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().caption_tracks.is_empty());
    assert!(!s.project.transcripts.contains_key(&item));
    // a limit on the line length and a name
    let r = s.execute("captions.fromNarration", json!({"maxChars": 10, "lines": 1, "name": "Subtitles"})).unwrap();
    assert!(r["captions"].as_u64().unwrap() >= 2, "{r}");
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].name, "Subtitles");
}

#[test]
fn a_caption_that_goes_by_too_fast_to_read_is_counted() {
    let mut s = edit();
    // thirty characters in a second and a quarter, with a long pause after so it is its own caption
    narrated(&mut s, &[("Extraordinarily", 0.0, 0.5), ("complicated", 0.5, 0.9), ("circumstances", 0.9, 1.25)]);
    let r = s.execute("captions.fromNarration", json!({"minSeconds": 0.5})).unwrap();
    assert_eq!(r["fast"], 1, "{r}");
    assert!(r["notes"][0].as_str().unwrap().contains("faster than about 21 characters a second"), "{r}");
}

#[test]
fn without_a_narration_made_in_the_app_there_is_nothing_to_do_and_a_narration_without_word_times_is_named() {
    let mut s = edit();
    assert!(s.execute("captions.fromNarration", json!({})).unwrap_err().to_string().contains("no narration"));
    assert!(s.execute("transcript.fromNarration", json!({})).unwrap_err().to_string().contains("no narration"));
    let item = narrated(&mut s, &SHORT);
    s.edit("test", |pr, _| {
        pr.narrations.get_mut(&item).unwrap().words.clear();
        Ok(())
    })
    .unwrap();
    let r = s.execute("transcript.fromNarration", json!({})).unwrap();
    assert_eq!((r["written"].clone(), r["skippedNoWords"].as_array().unwrap().len()), (json!([]), 1), "{r}");
    // captions need something to group
    assert!(s.execute("captions.fromNarration", json!({})).is_err());
}
