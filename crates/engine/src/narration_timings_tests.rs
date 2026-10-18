//! Tests of `narration_timings` and of the word times kept with a narration (fork roadmap F16.2).

use filmcraft_project::SpokenWord;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::json;

use crate::Session;

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("filmcraft-words-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d.to_string_lossy().into_owned()
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn timings(s: &Session, input: serde_json::Value) -> edith_ai::ToolOutput {
    s.run_assistant_tool("narration_timings", &input)
}

fn sec(x: f64) -> Tick {
    Tick((x * TICKS_PER_SECOND as f64).round() as i64)
}

/// A narration made at `at` seconds with the basic voice (about 1.9 s of speech), its words replaced by exact
/// ones at known times: "One two." then "Three four.", with 0.7 s of breath between the sentences.
fn exact_narration(s: &mut Session, name: &str, at: f64) -> (u64, u64) {
    let r = s.execute("tts.create", json!({"text": "One two. Three four.", "seconds": at, "dir": tmp(name)})).unwrap();
    let (item, clip) = (r["item"].as_u64().unwrap(), r["clip"].as_u64().unwrap());
    let w = |t: &str, a: f64, b: f64, n: u32| SpokenWord { text: t.into(), start: sec(a), end: sec(b), sentence: n, exact: true };
    s.edit("test", |pr, _| {
        pr.narrations.get_mut(&filmcraft_project::ItemId(item)).unwrap().words =
            vec![w("One", 0.1, 0.3, 0), w("two", 0.4, 0.7, 0), w("Three", 1.1, 1.4, 1), w("four", 1.5, 1.8, 1)];
        Ok(())
    })
    .unwrap();
    (item, clip)
}

#[test]
fn a_new_narration_keeps_when_every_word_is_spoken() {
    let mut s = demo();
    let r = s.execute("tts.create", json!({"text": "Welcome to the harbour. It was quiet! [pause 1s] Nobody came.", "time": 0, "dir": tmp("kept")})).unwrap();
    let item = filmcraft_project::ItemId(r["item"].as_u64().unwrap());
    let n = &s.project.narrations[&item];
    let said: Vec<&str> = n.words.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(said, ["Welcome", "to", "the", "harbour", "It", "was", "quiet", "Nobody", "came"]);
    assert_eq!(n.words.iter().map(|w| w.sentence).collect::<Vec<_>>(), [0, 0, 0, 0, 1, 1, 1, 2, 2]);
    assert!(n.words.iter().all(|w| !w.exact), "the basic voice only estimates inside a stretch");
    let mut at = Tick::ZERO;
    for w in &n.words {
        assert!(w.start >= at && w.start <= w.end && w.end <= n.speech_duration, "{w:?} in {:?}", n.speech_duration);
        at = w.end;
    }
    n.check().unwrap();
    // it is saved with the project and comes back
    let bytes = filmcraft_format::encode(&s.project, false);
    let back = filmcraft_format::decode(&bytes).unwrap();
    assert_eq!(back.project.narrations[&item].words, n.words);
}

#[test]
fn editing_the_narration_makes_its_times_again() {
    let mut s = demo();
    let r = s.execute("tts.create", json!({"text": "Short.", "time": 0, "dir": tmp("edit")})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    let e = s.execute("tts.edit", json!({"clip": clip, "text": "A rather longer sentence now.", "dir": tmp("edit")})).unwrap();
    let item = filmcraft_project::ItemId(e["item"].as_u64().unwrap());
    assert_eq!(s.project.narrations[&item].words.len(), 5);
    // and undo brings back the old narration with its own times
    s.undo().unwrap();
    let old = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap().1.item;
    assert_eq!(s.project.narrations[&old].words.len(), 1);
}

#[test]
fn the_times_are_on_the_timeline_with_sentences_and_pauses() {
    let mut s = demo();
    let (_, clip) = exact_narration(&mut s, "tl", 10.0);
    let out = timings(&s, json!({"clip": clip}));
    assert!(!out.is_error, "{}", out.content);
    for want in [
        "4 words in 2 sentences",
        "exact: the voice itself timed every word",
        "[voice-exact]",
        "S1: One@10.10-10.30 two@10.40-10.70",
        "S2: ‹pause 0.40› Three@11.10-11.40 four@11.50-11.80",
        "never inside one",
    ] {
        assert!(out.content.contains(want), "{want:?} not in\n{}", out.content);
    }
    // with a single narration, the clip can be left out
    let alone = timings(&s, json!({}));
    assert!(alone.content.contains("S2:"), "{}", alone.content);
}

#[test]
fn trimming_and_speeding_the_clip_move_the_words_the_way_the_audio_moves() {
    let mut s = demo();
    let (_, clip) = exact_narration(&mut s, "trim", 0.0);
    let id = filmcraft_project::ClipId(clip);
    // take the first 0.9 s off the front and play the next second at twice the speed, from 0.0 s
    s.edit("trim", |pr, _| {
        let seq_id = pr.sequences().next().map(|q| q.id).unwrap();
        let seq = pr.sequence_mut(seq_id).unwrap();
        let (_, it) = seq.find_item_mut(id).unwrap();
        it.start = sec(20.0);
        it.source_in = sec(0.9);
        it.duration = sec(0.5);
        it.speed = 2.0;
        Ok(())
    })
    .unwrap();
    let out = timings(&s, json!({"clip": clip}));
    assert!(!out.is_error, "{}", out.content);
    // One and two are before the new start: gone. Three at file 1.1-1.4 is (1.1-0.9)/2 = 0.10 s to 0.25 s into the
    // clip, four at 1.5-1.8 is 0.30-0.45 s
    assert!(!out.content.contains("One@") && !out.content.contains("two@"), "{}", out.content);
    assert!(out.content.contains("Three@20.10-20.25") && out.content.contains("four@20.30-20.45"), "{}", out.content);
    // a window narrows it
    let late = timings(&s, json!({"clip": clip, "from": 20.27}));
    assert!(late.content.contains("four@") && !late.content.contains("Three@"), "{}", late.content);
    let early = timings(&s, json!({"clip": clip, "to": "0:20"}));
    assert!(early.is_error || !early.content.contains("Three@"), "to 0:20 is before both: {}", early.content);
    let early = timings(&s, json!({"clip": clip, "to": 20.27}));
    assert!(early.content.contains("Three@") && !early.content.contains("four@"), "{}", early.content);
    // a half-written time is refused, with the reason
    assert!(timings(&s, json!({"clip": clip, "to": "0:20.2"})).is_error);
}

#[test]
fn a_recorded_voice_gets_its_times_from_the_transcript_and_says_so() {
    let mut s = demo();
    let (item, clip, from_media) = {
        let q = s.active_sequence().unwrap();
        let it = q.audio_tracks.iter().flat_map(|t| t.items.iter()).next().unwrap();
        (it.item.0, it.id.0, it.source_in.seconds())
    };
    // the words sit inside the part of the recording the clip plays
    let t = |text: &str, a: f64, b: f64| json!({"text": text, "start": sec(from_media + a).0, "end": sec(from_media + b).0});
    s.execute(
        "transcript.set",
        json!({"item": item, "transcript": {"language": "en", "words": [t("We", 0.0, 0.2), t("left", 0.3, 0.6), t("early.", 0.7, 1.1), t("Nobody", 2.0, 2.4), t("noticed.", 2.5, 3.0)]}}),
    )
    .unwrap();
    let out = timings(&s, json!({"clip": clip}));
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("from the transcript") && out.content.contains("[transcript]"), "{}", out.content);
    assert!(out.content.contains("S1: We@") && out.content.contains("S2: ‹pause 0.90› Nobody@"), "sentences end at the full stops: {}", out.content);
}

#[test]
fn a_narration_from_before_times_were_kept_gets_a_flagged_estimate() {
    let mut s = demo();
    let (item, clip) = exact_narration(&mut s, "old", 0.0);
    s.edit("old", |pr, _| {
        pr.narrations.get_mut(&filmcraft_project::ItemId(item)).unwrap().words.clear();
        Ok(())
    })
    .unwrap();
    let out = timings(&s, json!({"clip": clip}));
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("rough estimate") && out.content.contains("[estimated]"), "{}", out.content);
    assert!(out.content.contains("4 words"), "{}", out.content);
}

#[test]
fn nothing_to_time_several_clips_and_bad_input_are_explained() {
    let mut s = demo();
    assert!(timings(&s, json!({})).content.contains("no narration or transcribed speech"));
    let (_, a) = exact_narration(&mut s, "a", 0.0);
    let (_, b) = exact_narration(&mut s, "b", 60.0);
    let many = timings(&s, json!({}));
    assert!(many.is_error && many.content.contains("2 clips have speech to time") && many.content.contains(&format!("clip {a}")), "{}", many.content);
    assert!(timings(&s, json!({"clip": b})).content.contains("S1:"));
    for bad in [
        json!({"clip": 999_999}),
        json!({"clip": "x"}),
        json!({"clip": a, "from": "banana"}),
        json!({"clip": a, "to": -1}),
        json!({"clip": a, "page": 99}),
        json!(null),
        json!([1]),
        json!("x"),
    ] {
        let r = timings(&s, bad.clone());
        assert!(r.is_error || r.content.contains("S1:"), "{bad}: {}", r.content);
    }
    // a clip of something else (the demo's video) has nothing to time
    let video = s.active_sequence().unwrap().video_tracks.iter().flat_map(|t| t.items.iter()).next().unwrap().id.0;
    assert!(timings(&s, json!({"clip": video})).is_error);
    assert!(timings(&Session::default(), json!({})).is_error);
}

#[test]
fn a_long_narration_is_paged_and_every_word_appears_once() {
    let mut s = demo();
    let text = (1..=400).map(|i| format!("word{i}.")).collect::<Vec<_>>().join(" ");
    let r = s.execute("tts.create", json!({"text": text, "seconds": 0, "dir": tmp("long")})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    let first = timings(&s, json!({"clip": clip}));
    assert!(first.content.contains("Page 1 of 3") && first.content.contains("call again with page 2"), "{}", first.content);
    let all: String = (1..=3).map(|p| timings(&s, json!({"clip": clip, "page": p})).content).collect();
    for i in [1, 150, 151, 300, 301, 400] {
        assert_eq!(all.matches(&format!("word{i}@")).count(), 1, "word{i}");
    }
    assert!(timings(&s, json!({"clip": clip, "page": 4})).is_error);
}

#[test]
fn the_tool_is_read_only_and_in_the_list() {
    let t = crate::assistant_tools::tools().into_iter().find(|t| t.spec.name == "narration_timings").expect("the tool");
    assert!(t.read_only);
}
