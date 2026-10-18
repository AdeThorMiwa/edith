//! Tests of `clip.audioLead` (fork roadmap F16.3): a J cut and an L cut on shots with their own sound, one undo
//! step, no overlaps left on the audio track, and refusals with the reason.

use filmcraft_project::{ClipId, ItemKind, TrackItem};
use filmcraft_time::TICKS_PER_SECOND;
use serde_json::{Value, json};

use crate::Session;

fn media(s: &Session, name: &str) -> u64 {
    s.project.items.values().find(|i| i.name.contains(name) && matches!(i.kind, ItemKind::Media(_))).map(|i| i.id.0).expect(name)
}

fn secs(t: filmcraft_time::Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

/// Two shots with their sound, back to back: Ocean 2–6 s of its file at 0–4 s, City 3–8 s of its file at 4–9 s.
fn two_shots() -> (Session, (u64, u64), (u64, u64)) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Cut", "audio": 2, "video": 2})).unwrap();
    let (ocean, city) = (media(&s, "Ocean_Sunset"), media(&s, "City_Night"));
    let t = |x: f64| (x * TICKS_PER_SECOND as f64) as i64;
    let place = |s: &mut Session, item: u64, at: f64, sin: f64, dur: f64| {
        s.execute("timeline.place", json!({"item": item, "track": "V1", "audioTrack": "A1", "time": t(at), "sourceIn": t(sin), "duration": t(dur)})).unwrap();
    };
    place(&mut s, ocean, 0.0, 2.0, 4.0);
    place(&mut s, city, 4.0, 3.0, 5.0);
    let q = s.active_sequence().unwrap();
    let v: Vec<u64> = q.video_tracks[0].items.iter().map(|i| i.id.0).collect();
    let a: Vec<u64> = q.audio_tracks[0].items.iter().map(|i| i.id.0).collect();
    assert_eq!((v.len(), a.len()), (2, 2), "each shot has its picture and its sound");
    (s, (v[0], v[1]), (a[0], a[1]))
}

fn clip(s: &Session, id: u64) -> TrackItem {
    s.active_sequence().unwrap().find_item(ClipId(id)).unwrap().1.clone()
}

#[test]
fn a_j_cut_starts_the_next_shots_sound_early_and_takes_the_time_off_the_one_before() {
    let (mut s, (_, v2), (a1, a2)) = two_shots();
    let undo = s.history.undo.len();
    let (before_a1, before_a2) = (clip(&s, a1), clip(&s, a2));
    let r = s.execute("clip.audioLead", json!({"clip": v2, "frames": 12})).unwrap();
    assert_eq!((r["clip"].clone(), r["edge"].clone(), r["neighbourTrimmed"].clone()), (json!(a2), json!("in"), json!(a1)));
    let (a1n, a2n) = (clip(&s, a1), clip(&s, a2));
    // the sound starts half a second early and shows half a second more of its file; the picture has not moved
    assert!(
        close(secs(a2n.start), secs(before_a2.start) - 0.5) && close(secs(a2n.source_in), secs(before_a2.source_in) - 0.5),
        "{} {}",
        secs(a2n.start),
        secs(a2n.source_in)
    );
    assert!(close(secs(a2n.end()), secs(before_a2.end())), "its end stays");
    assert!(close(secs(clip(&s, v2).start), 4.0));
    // the sound before gives way, so the track has no overlap
    assert!(close(secs(a1n.end()), secs(before_a1.end()) - 0.5) && a1n.end() <= a2n.start, "{} vs {}", secs(a1n.end()), secs(a2n.start));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    s.undo().unwrap();
    assert_eq!((clip(&s, a1).end(), clip(&s, a2).start, clip(&s, a2).source_in), (before_a1.end(), before_a2.start, before_a2.source_in));
}

#[test]
fn an_l_cut_carries_a_shots_sound_over_the_next_picture() {
    let (mut s, (v1, _), (a1, a2)) = two_shots();
    let (before_a1, before_a2) = (clip(&s, a1), clip(&s, a2));
    // through the audio clip's own id, to show either works
    let r = s.execute("clip.audioLead", json!({"clip": a1, "frames": 24, "edge": "out"})).unwrap();
    assert_eq!((r["edge"].clone(), r["neighbourTrimmed"].clone()), (json!("out"), json!(a2)));
    let (a1n, a2n) = (clip(&s, a1), clip(&s, a2));
    assert!(close(secs(a1n.end()), secs(before_a1.end()) + 1.0), "{}", secs(a1n.end()));
    assert!(
        close(secs(a2n.start), secs(before_a2.start) + 1.0) && close(secs(a2n.source_in), secs(before_a2.source_in) + 1.0),
        "the next sound starts later, from further in"
    );
    assert!(a1n.end() <= a2n.start);
    assert!(close(secs(clip(&s, v1).end()), 4.0), "the picture's cut did not move");
    s.undo().unwrap();
    assert_eq!(clip(&s, a1).end(), before_a1.end());
}

#[test]
fn it_refuses_when_there_is_no_sound_to_use_nowhere_to_go_or_nothing_to_lead() {
    let (mut s, (v1, v2), (a1, a2)) = two_shots();
    let snapshot = |s: &Session| (s.revision, s.history.undo.len(), serde_json::to_string(&*s.project).unwrap().len());
    let before = snapshot(&s);
    let cases: Vec<(Value, &str)> = vec![
        (json!({"clip": v1, "frames": 12}), "would start before the beginning"),
        (json!({"clip": v2, "frames": 100}), "only 3.00 s of sound before"),
        (json!({"clip": v2, "frames": 120, "edge": "in"}), "only 3.00 s of sound before"),
        (json!({"clip": v2, "frames": 0}), "`frames` must be"),
        (json!({"clip": v2, "frames": 241}), "`frames` must be"),
        (json!({"clip": v2, "frames": -3}), "`frames` must be"),
        (json!({"clip": v2, "frames": 1.5}), "`frames` must be"),
        (json!({"clip": v2, "frames": 5, "edge": "sideways"}), "`edge` must be"),
        (json!({"clip": 999_999, "frames": 5}), "there is no clip"),
        (json!({"frames": 5}), "`clip` must be"),
        (json!({"clip": "x", "frames": 5}), "`clip` must be"),
    ];
    for (p, want) in cases {
        let e = s.execute("clip.audioLead", p.clone()).expect_err(&p.to_string()).to_string();
        assert!(e.contains(want), "{want:?} not in {e:?} for {p}");
        assert_eq!(snapshot(&s), before, "{p} changed something");
    }
    // a lead longer than the sound before it would swallow it
    s.execute("clip.audioLead", json!({"clip": v2, "frames": 24})).unwrap();
    let e = s.execute("clip.audioLead", json!({"clip": v2, "frames": 24 * 5})).unwrap_err().to_string();
    assert!(e.contains("only 2.00 s of sound before") || e.contains("swallowed"), "{e}");
    // a locked audio track
    s.execute("timeline.setTrack", json!({"track": "A1", "locked": true})).unwrap();
    assert!(s.execute("clip.audioLead", json!({"clip": a2, "frames": 5})).unwrap_err().to_string().contains("locked"));
    let _ = a1;
}

#[test]
fn a_shot_from_a_cut_list_has_no_sound_of_its_own_to_lead() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Cut", "audio": 2, "video": 2})).unwrap();
    let ocean = media(&s, "Ocean_Sunset");
    let r = s
        .execute("timeline.placeCutList", json!({"entries": [{"start": 0, "end": 3, "item": ocean}, {"start": 3, "end": 6, "item": ocean, "sourceIn": 5}]}))
        .unwrap();
    let clip = r["clips"][1].as_u64().unwrap();
    let e = s.execute("clip.audioLead", json!({"clip": clip, "frames": 6})).unwrap_err().to_string();
    assert!(e.contains("no sound of its own"), "{e}");
}

#[test]
fn the_assistant_may_use_it_inside_a_version() {
    assert!(crate::assistant_edit::may_run("clip.audioLead").is_ok());
    assert!(crate::assistant_edit::may_run("timeline.placeCutList").is_ok());
}
