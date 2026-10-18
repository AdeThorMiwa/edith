//! Tests of `timeline.placeCutList` (fork roadmap F16.3): a run of shots in one call and one undo step, checked
//! before anything changes, with no cut inside a spoken word.

use filmcraft_project::{ClipId, ItemKind, SpokenWord};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::Session;

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("filmcraft-cutlist-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d.to_string_lossy().into_owned()
}

fn media(s: &Session, name: &str) -> u64 {
    s.project.items.values().find(|i| i.name.contains(name) && matches!(i.kind, ItemKind::Media(_))).map(|i| i.id.0).expect(name)
}

/// The demo's footage in a fresh, empty sequence.
fn edit() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Cut", "audio": 2, "video": 2})).unwrap();
    s
}

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

fn v1(s: &Session) -> Vec<filmcraft_project::TrackItem> {
    s.active_sequence().unwrap().video_tracks[0].items.clone()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

fn place(s: &mut Session, entries: Value) -> crate::Result<Value> {
    s.execute("timeline.placeCutList", json!({"entries": entries}))
}

#[test]
fn a_run_of_shots_is_placed_with_its_reasons_in_one_undo_step() {
    let mut s = edit();
    let (ocean, city, dunes) = (media(&s, "Ocean_Sunset"), media(&s, "City_Night"), media(&s, "Desert_Dunes"));
    let undo = s.history.undo.len();
    let r = place(
        &mut s,
        json!([
            {"start": 0, "end": 4, "item": ocean, "reason": "Opens on the water the narration names."},
            {"start": 4, "end": "0:09", "item": city, "sourceIn": 2, "reason": "The city, as the second sentence says."},
            {"start": 9, "end": 12.5, "item": dunes}
        ]),
    )
    .unwrap();
    assert_eq!((r["placed"].clone(), r["wordsChecked"].clone()), (json!(3), json!(false)));
    assert!(close(r["seconds"].as_f64().unwrap(), 12.5));
    let clips = v1(&s);
    assert_eq!(clips.len(), 3);
    for (c, (a, b)) in clips.iter().zip([(0.0, 4.0), (4.0, 9.0), (9.0, 12.5)]) {
        assert!(close(secs(c.start), a) && close(secs(c.end()), b), "{} {}", secs(c.start), secs(c.end()));
    }
    assert!(close(secs(clips[1].source_in), 2.0), "it starts two seconds in");
    assert_eq!(clips[0].markers[0].comment, "Opens on the water the narration names.");
    assert_eq!(clips[0].markers[0].name, "Why this shot");
    assert!(clips[2].markers.is_empty(), "no reason, no note");
    // picture only, and one undo step puts everything back
    assert!(s.active_sequence().unwrap().audio_tracks.iter().all(|t| t.items.is_empty()));
    assert_eq!(s.history.undo.len(), undo + 1, "the whole list is one step: {:?}", s.history.undo.iter().map(|u| u.0.clone()).collect::<Vec<_>>());
    assert!(s.history.undo.last().unwrap().0.starts_with("Place Cut List (3 shots)"));
    s.undo().unwrap();
    assert!(v1(&s).is_empty());
    s.redo().unwrap();
    assert_eq!(v1(&s).len(), 3);
}

#[test]
fn speed_and_a_camera_move_are_part_of_the_shot() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    place(&mut s, json!([{"start": 0, "end": 6, "item": ocean, "speed": 0.5, "move": "pushIn", "amount": 60}, {"start": 6, "end": 9, "item": ocean, "sourceIn": 5, "move": "sway", "seed": 3}])).unwrap();
    let clips = v1(&s);
    assert!((clips[0].speed - 0.5).abs() < 1e-9 && (clips[1].speed - 1.0).abs() < 1e-9);
    assert!(clips[0].effects.iter().any(|e| e.params.iter().any(|(_, p)| p.keyframes.len() >= 2)), "a push in has keyframes");
    assert!(!clips[1].effects.is_empty(), "so does a sway");
}

#[test]
fn a_bad_entry_refuses_the_whole_list_and_says_which_and_why() {
    let mut s = edit();
    let (ocean, city) = (media(&s, "Ocean_Sunset"), media(&s, "City_Night"));
    let score = media(&s, "Ambient_Score");
    let seq = s.project.items.values().find(|i| matches!(i.kind, ItemKind::Sequence(_))).map(|i| i.id.0).unwrap();
    let good = json!({"start": 0, "end": 3, "item": ocean});
    let cases: Vec<(Value, &str)> = vec![
        (json!([]), "needs 1 to 2000"),
        (json!("shots"), "must be a list"),
        (json!([good.clone(), 5]), "entry 2: is not an object"),
        (json!([good.clone(), {"start": 3, "item": city}]), "entry 2: `end` is required"),
        (json!([good.clone(), {"end": 5, "item": city}]), "entry 2: `start` is required"),
        (json!([good.clone(), {"start": 5, "end": 5, "item": city}]), "less than one frame"),
        (json!([good.clone(), {"start": 6, "end": 5, "item": city}]), "less than one frame"),
        (json!([good.clone(), {"start": 2, "end": 6, "item": city}]), "entries 1 and 2 overlap"),
        (json!([good.clone(), {"start": 3, "end": 6, "item": 999_999}]), "there is no item 999999"),
        (json!([good.clone(), {"start": 3, "end": 6, "item": score}]), "not a picture or footage"),
        (json!([good.clone(), {"start": 3, "end": 6, "item": seq}]), "not a picture or footage"),
        (json!([good.clone(), {"start": 3, "end": 6}]), "`item` must be"),
        (json!([good.clone(), {"start": 3, "end": 6, "item": "x"}]), "`item` must be"),
        (json!([{"start": 0, "end": 60, "item": ocean}]), "left from"),
        (json!([{"start": 0, "end": 3, "item": ocean, "sourceIn": 17}]), "left from"),
        (json!([{"start": 0, "end": 3, "item": ocean, "sourceIn": -1}]), "sourceIn"),
        (json!([{"start": 0, "end": 3, "item": ocean, "speed": 0}]), "`speed` must be"),
        (json!([{"start": 0, "end": 3, "item": ocean, "speed": 9}]), "`speed` must be"),
        (json!([{"start": 0, "end": 3, "item": ocean, "move": "spin"}]), "`move` must be one of"),
        (json!([{"start": 0, "end": 3, "item": ocean, "move": "pushIn", "amount": 400}]), "`amount` must be"),
        (json!([{"start": 0, "end": 3, "item": ocean, "reason": "x".repeat(301)}]), "longer than 300"),
        (json!([{"start": 0, "end": 3, "item": ocean, "reason": 5}]), "`reason` must be text"),
        (json!([{"start": "soon", "end": 3, "item": ocean}]), "start"),
        (json!([{"start": -2, "end": 3, "item": ocean}]), "start"),
        (json!([{"start": 0, "end": 1e30, "item": ocean}]), "end"),
    ];
    let before = (s.revision, s.history.undo.len(), serde_json::to_string(&*s.project).unwrap().len());
    for (entries, want) in cases {
        let e = place(&mut s, entries.clone()).expect_err(&entries.to_string()).to_string();
        assert!(e.contains(want), "{want:?} not in {e:?} for {entries}");
        assert_eq!((s.revision, s.history.undo.len(), serde_json::to_string(&*s.project).unwrap().len()), before, "nothing changed for {entries}");
    }
    let many: Vec<Value> = (0..2001).map(|k| json!({"start": k, "end": k + 1, "item": ocean})).collect();
    assert!(place(&mut s, json!(many)).is_err());
    assert!(s.execute("timeline.placeCutList", json!({})).is_err());
    assert!(s.execute("timeline.placeCutList", json!({"entries": [good.clone()], "track": "A1"})).is_err(), "an audio track");
    assert!(s.execute("timeline.placeCutList", json!({"entries": [good.clone()], "track": "V9"})).is_err(), "no such track");
    s.execute("timeline.setTrack", json!({"track": "V1", "locked": true})).unwrap();
    assert!(s.execute("timeline.placeCutList", json!({"entries": [good.clone()], "track": "V1"})).is_err(), "V1 is locked and was named");
    // without a track, the first one that is not locked is used
    let on_v2 = place(&mut s, json!([good.clone()])).unwrap();
    assert!(s.active_sequence().unwrap().video_tracks[1].items.len() == 1 && s.active_sequence().unwrap().video_tracks[0].items.is_empty(), "{on_v2}");
    assert_eq!(on_v2["placed"], 1);
}

/// A narration at `at` seconds whose words are replaced by exact ones: "One two." then "Three four.".
fn narrated(s: &mut Session, at: f64) -> (u64, u64) {
    let r = s.execute("tts.create", json!({"text": "One two. Three four.", "seconds": at, "dir": tmp("words")})).unwrap();
    let (item, clip) = (r["item"].as_u64().unwrap(), r["clip"].as_u64().unwrap());
    let w = |t: &str, a: f64, b: f64, n: u32| SpokenWord {
        text: t.into(),
        start: Tick((a * TICKS_PER_SECOND as f64) as i64),
        end: Tick((b * TICKS_PER_SECOND as f64) as i64),
        sentence: n,
        exact: true,
    };
    s.edit("test", |pr, _| {
        pr.narrations.get_mut(&filmcraft_project::ItemId(item)).unwrap().words =
            vec![w("One", 0.1, 0.4, 0), w("two", 0.5, 0.9, 0), w("Three", 1.2, 1.5, 1), w("four", 1.6, 1.9, 1)];
        Ok(())
    })
    .unwrap();
    (item, clip)
}

#[test]
fn no_cut_may_fall_inside_a_spoken_word() {
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    narrated(&mut s, 0.0);
    let before = (s.revision, v1(&s).len());
    // 0.75 s is inside "two" (0.5 to 0.9)
    let e = place(&mut s, json!([{"start": 0, "end": 0.75, "item": ocean}, {"start": 0.75, "end": 3, "item": ocean}])).unwrap_err().to_string();
    assert!(e.contains("nothing was placed") && e.contains("2 cuts would split a spoken word"), "{e}");
    assert!(
        e.contains("entry 1 ends at 0.75 s, inside the word “two” (0.50–0.90 s)")
            && e.contains("move it to 0.50 s (the word's start), 0.90 s (its end), or a pause"),
        "{e}"
    );
    assert!(e.contains("entry 2 starts at 0.75 s"), "every offending cut is named at once: {e}");
    assert_eq!((s.revision, v1(&s).len()), before, "nothing changed");
    // in the pause between the sentences (0.9 to 1.2), at a word's end, and at a word's start: all fine
    let ok =
        place(&mut s, json!([{"start": 0, "end": 1.0, "item": ocean}, {"start": 1.0, "end": 1.2, "item": ocean}, {"start": 1.2, "end": 3, "item": ocean}]))
            .unwrap();
    assert_eq!((ok["placed"].clone(), ok["wordsChecked"].clone()), (json!(3), json!(true)));
    // a cut a frame inside a word is within the tolerance (the grid is a frame)
    let near = place(&mut s, json!([{"start": 5, "end": 6, "item": ocean}])).unwrap();
    assert_eq!(near["placed"], 1, "away from the speech nothing is in the way");
}

#[test]
fn the_assistant_can_place_a_cut_list_inside_a_version_and_the_person_sees_it_as_one_step() {
    use crate::assistant_edit::run_edit;
    let mut s = edit();
    let ocean = media(&s, "Ocean_Sunset");
    s.execute("assistant.settings", json!({"mode": "edit"})).unwrap();
    let begun = run_edit(&mut s, "begin_version", &json!({"title": "First idea"})).unwrap();
    assert!(!begun.is_error, "{}", begun.content);
    let out = run_edit(
        &mut s,
        "edit",
        &json!({"command": "timeline.placeCutList", "params": {"entries": [{"start": 0, "end": 3, "item": ocean, "reason": "the water"}, {"start": 3, "end": 6, "item": ocean, "sourceIn": 6, "reason": "the same light, later"}]}}),
    )
    .unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(v1(&s).len(), 2);
    let info = s.version_info().unwrap();
    assert_eq!(info.steps.len(), 1, "{:?}", info.steps.iter().map(|x| x.label.clone()).collect::<Vec<_>>());
    assert!(info.steps[0].label.contains("Place Cut List (2 shots)"));
    // discarding takes it all back
    s.execute("version.discard", json!({})).unwrap();
    assert!(v1(&s).is_empty());
    let _ = ClipId(0);
}
